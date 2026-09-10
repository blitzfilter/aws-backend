use crate::ports::{FxRateSnapshotReadError, FxRateSnapshotReader};
use application::error::static_error;
use fxrate_core::{FxRateId, FxRateSnapshot};
use indexmap::IndexMap;
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::info;

const DEFAULT_MAX_ENTRIES: usize = 512;
const DEFAULT_LATEST_SELECTION_TTL: Duration = Duration::from_secs(30);

/// Search-only policy for bounded FX input reuse.
///
/// Exact IDs reuse immutable snapshots until FIFO capacity eviction. Latest selections
/// are reusable only for the fixed monotonic TTL and compatible valuation instants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FxSearchCacheConfig {
    enabled: bool,
    max_entries: usize,
    latest_selection_ttl: Duration,
}

impl FxSearchCacheConfig {
    pub fn new(
        enabled: bool,
        max_entries: usize,
        latest_selection_ttl: Duration,
    ) -> Result<Self, FxSearchCacheConfigError> {
        if max_entries == 0 {
            return Err(FxSearchCacheConfigError::ZeroMaxEntries);
        }

        Ok(Self {
            enabled,
            max_entries,
            latest_selection_ttl,
        })
    }

    pub fn public_search_defaults(enabled: bool) -> Self {
        Self {
            enabled,
            max_entries: DEFAULT_MAX_ENTRIES,
            latest_selection_ttl: DEFAULT_LATEST_SELECTION_TTL,
        }
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub const fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub const fn latest_selection_ttl(&self) -> Duration {
        self.latest_selection_ttl
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FxSearchCacheConfigError {
    #[error("FX search cache maximum entries must be greater than zero")]
    ZeroMaxEntries,
}

struct FxCacheState {
    by_id: IndexMap<FxRateId, Arc<FxRateSnapshot>>,
    latest: Option<LatestSelection>,
}

impl FxCacheState {
    fn new() -> Self {
        Self {
            by_id: IndexMap::new(),
            latest: None,
        }
    }
}

struct LatestSelection {
    snapshot: Arc<FxRateSnapshot>,
    selected_for: OffsetDateTime,
    fresh_until: Instant,
}

/// Bounded, process-local decorator for public ProductListing search FX reads.
///
/// The underlying reader remains authoritative. Disabled mode bypasses all cache
/// state and coordination. This decorator must not be used for administrative,
/// invariant-critical, or financial-write reads.
pub struct CachedFxRateSnapshotReader<R> {
    inner: Arc<R>,
    config: FxSearchCacheConfig,
    state: Arc<Mutex<FxCacheState>>,
    fill_gate: Arc<Mutex<()>>,
}

impl<R> Clone for CachedFxRateSnapshotReader<R> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            config: self.config.clone(),
            state: Arc::clone(&self.state),
            fill_gate: Arc::clone(&self.fill_gate),
        }
    }
}

impl<R> CachedFxRateSnapshotReader<R> {
    pub fn new(inner: R, config: FxSearchCacheConfig) -> Self {
        Self {
            inner: Arc::new(inner),
            config,
            state: Arc::new(Mutex::new(FxCacheState::new())),
            fill_gate: Arc::new(Mutex::new(())),
        }
    }

    async fn cached_by_id(&self, id: FxRateId) -> Option<FxRateSnapshot> {
        self.state
            .lock()
            .await
            .by_id
            .get(&id)
            .map(|snapshot| snapshot.as_ref().clone())
    }

    async fn admit_by_id(&self, snapshot: FxRateSnapshot) {
        let mut state = self.state.lock().await;
        admit_snapshot(&mut state.by_id, snapshot, self.config.max_entries);
    }

    async fn cached_latest_at_or_before(&self, at: OffsetDateTime) -> Option<FxRateSnapshot> {
        let state = self.state.lock().await;
        let now = Instant::now();
        state.latest.as_ref().and_then(|latest| {
            latest_is_usable(latest, at, now, self.config.latest_selection_ttl)
                .then(|| latest.snapshot.as_ref().clone())
        })
    }

    async fn admit_latest(
        &self,
        snapshot: FxRateSnapshot,
        selected_for: OffsetDateTime,
        fresh_until: Instant,
    ) {
        let mut state = self.state.lock().await;
        admit_snapshot(&mut state.by_id, snapshot.clone(), self.config.max_entries);

        if Instant::now() >= fresh_until {
            return;
        }

        if state
            .latest
            .as_ref()
            .is_some_and(|latest| latest.selected_for > selected_for)
        {
            return;
        }

        state.latest = Some(LatestSelection {
            snapshot: Arc::new(snapshot),
            selected_for,
            fresh_until,
        });
    }

    async fn seed_by_id(&self, snapshot: FxRateSnapshot) {
        self.admit_by_id(snapshot).await;
    }
}

#[async_trait::async_trait]
impl<R> FxRateSnapshotReader for CachedFxRateSnapshotReader<R>
where
    R: FxRateSnapshotReader,
{
    async fn find_by_id(
        &self,
        id: FxRateId,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
        if !self.config.enabled {
            let backend_started = Instant::now();
            let result = self.inner.find_by_id(id).await;
            emit_fx_cache_metric(
                "fx_by_id",
                "bypassed",
                Duration::ZERO,
                backend_started.elapsed(),
            );
            return result;
        }

        if let Some(snapshot) = self.cached_by_id(id).await {
            emit_fx_cache_metric("fx_by_id", "hit", Duration::ZERO, Duration::ZERO);
            return Ok(Some(snapshot));
        }

        let gate_started = Instant::now();
        let _fill_guard = self.fill_gate.lock().await;
        let fill_gate_wait = gate_started.elapsed();
        if let Some(snapshot) = self.cached_by_id(id).await {
            emit_fx_cache_metric("fx_by_id", "coalesced_hit", fill_gate_wait, Duration::ZERO);
            return Ok(Some(snapshot));
        }

        let backend_started = Instant::now();
        let result = self.inner.find_by_id(id).await;
        let backend_duration = backend_started.elapsed();
        let snapshot = match result {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => {
                emit_fx_cache_metric("fx_by_id", "missing", fill_gate_wait, backend_duration);
                return Ok(None);
            }
            Err(error) => {
                emit_fx_cache_metric("fx_by_id", "load_error", fill_gate_wait, backend_duration);
                return Err(error);
            }
        };
        if let Err(error) = validate_exact_snapshot(&snapshot, id) {
            emit_fx_cache_metric("fx_by_id", "load_error", fill_gate_wait, backend_duration);
            return Err(error);
        }
        self.admit_by_id(snapshot.clone()).await;
        emit_fx_cache_metric("fx_by_id", "filled", fill_gate_wait, backend_duration);
        Ok(Some(snapshot))
    }

    async fn find_latest_at_or_before(
        &self,
        at: OffsetDateTime,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
        if !self.config.enabled {
            let backend_started = Instant::now();
            let result = self.inner.find_latest_at_or_before(at).await;
            emit_fx_cache_metric(
                "fx_latest",
                "bypassed",
                Duration::ZERO,
                backend_started.elapsed(),
            );
            return result;
        }

        if self.config.latest_selection_ttl.is_zero() {
            let backend_started = Instant::now();
            let result = self.load_latest_without_selection(at).await;
            emit_fx_cache_metric(
                "fx_latest",
                "bypassed",
                Duration::ZERO,
                backend_started.elapsed(),
            );
            return result;
        }

        let lookup_started = Instant::now();
        if let Some(snapshot) = self.cached_latest_at_or_before(at).await {
            emit_fx_cache_metric("fx_latest", "hit", Duration::ZERO, Duration::ZERO);
            return Ok(Some(snapshot));
        }

        let gate_started = Instant::now();
        let _fill_guard = self.fill_gate.lock().await;
        let fill_gate_wait = gate_started.elapsed();
        if let Some(snapshot) = self.cached_latest_at_or_before(at).await {
            emit_fx_cache_metric("fx_latest", "coalesced_hit", fill_gate_wait, Duration::ZERO);
            return Ok(Some(snapshot));
        }

        let backend_started = Instant::now();
        let result = self.inner.find_latest_at_or_before(at).await;
        let backend_duration = backend_started.elapsed();
        let snapshot = match result {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => {
                emit_fx_cache_metric("fx_latest", "missing", fill_gate_wait, backend_duration);
                return Ok(None);
            }
            Err(error) => {
                emit_fx_cache_metric("fx_latest", "load_error", fill_gate_wait, backend_duration);
                return Err(error);
            }
        };
        if let Err(error) = validate_latest_snapshot(&snapshot, at) {
            emit_fx_cache_metric("fx_latest", "load_error", fill_gate_wait, backend_duration);
            return Err(error);
        }

        let fresh_until = lookup_started + self.config.latest_selection_ttl;
        self.admit_latest(snapshot.clone(), at, fresh_until).await;
        emit_fx_cache_metric("fx_latest", "filled", fill_gate_wait, backend_duration);
        Ok(Some(snapshot))
    }
}

impl<R> CachedFxRateSnapshotReader<R>
where
    R: FxRateSnapshotReader,
{
    async fn load_latest_without_selection(
        &self,
        at: OffsetDateTime,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
        let snapshot = self.inner.find_latest_at_or_before(at).await?;
        let Some(snapshot) = snapshot else {
            return Ok(None);
        };
        validate_latest_snapshot(&snapshot, at)?;
        self.seed_by_id(snapshot.clone()).await;
        Ok(Some(snapshot))
    }
}

fn emit_fx_cache_metric(
    component: &'static str,
    outcome: &'static str,
    fill_gate_wait: Duration,
    backend_duration: Duration,
) {
    info!(
        metric = "product_listing_search_cache",
        component,
        outcome,
        fill_gate_wait_ms = fill_gate_wait.as_millis(),
        backend_duration_ms = backend_duration.as_millis(),
        "product listing search cache operation"
    );
}

fn admit_snapshot(
    snapshots: &mut IndexMap<FxRateId, Arc<FxRateSnapshot>>,
    snapshot: FxRateSnapshot,
    capacity: usize,
) {
    if !snapshots.contains_key(&snapshot.id()) {
        while snapshots.len() >= capacity {
            let _ = snapshots.shift_remove_index(0);
        }
    }
    snapshots.insert(snapshot.id(), Arc::new(snapshot));
}

fn latest_is_usable(
    latest: &LatestSelection,
    requested_at: OffsetDateTime,
    now: Instant,
    ttl: Duration,
) -> bool {
    now < latest.fresh_until
        && latest.snapshot.captured_at() <= requested_at
        && latest.selected_for <= requested_at
        && selection_age_is_less_than_ttl(requested_at, latest.selected_for, ttl)
}

fn selection_age_is_less_than_ttl(
    requested_at: OffsetDateTime,
    selected_for: OffsetDateTime,
    ttl: Duration,
) -> bool {
    let age = requested_at - selected_for;
    if age.is_negative() {
        return false;
    }

    u128::try_from(age.whole_nanoseconds()).is_ok_and(|age| age < ttl.as_nanos())
}

fn validate_exact_snapshot(
    snapshot: &FxRateSnapshot,
    requested_id: FxRateId,
) -> Result<(), FxRateSnapshotReadError> {
    if snapshot.id() != requested_id {
        return Err(FxRateSnapshotReadError::InvalidPersistedSnapshot {
            source: static_error("FX snapshot reader returned a different snapshot ID"),
        });
    }
    Ok(())
}

fn validate_latest_snapshot(
    snapshot: &FxRateSnapshot,
    requested_at: OffsetDateTime,
) -> Result<(), FxRateSnapshotReadError> {
    if snapshot.captured_at() > requested_at {
        return Err(FxRateSnapshotReadError::InvalidPersistedSnapshot {
            source: static_error(
                "FX snapshot reader returned a snapshot after the requested cutoff",
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use fxrate_core::{FX_RATE_SCALE, FxRateGeneration, FxRateQuote, FxRateSource};
    use money::Currency;
    use std::collections::VecDeque;
    use std::future::{Future, poll_fn};
    use std::pin::Pin;
    use std::sync::{Mutex as StdMutex, MutexGuard};
    use std::task::Poll;
    use strum::IntoEnumIterator;
    use tokio::sync::Notify;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Lookup {
        ById(FxRateId),
        Latest(OffsetDateTime),
    }

    enum Reply {
        Value(Option<FxRateSnapshot>),
        ReadFailure,
        Block {
            entered: Arc<Notify>,
            release: Arc<Notify>,
            value: Option<FxRateSnapshot>,
        },
    }

    #[derive(Default)]
    struct FakeState {
        by_id: VecDeque<Reply>,
        latest: VecDeque<Reply>,
        calls: Vec<Lookup>,
    }

    #[derive(Clone, Default)]
    struct FakeReader(Arc<StdMutex<FakeState>>);

    impl FakeReader {
        fn push_by_id(&self, reply: Reply) {
            lock(&self.0).by_id.push_back(reply);
        }

        fn push_latest(&self, reply: Reply) {
            lock(&self.0).latest.push_back(reply);
        }

        fn calls(&self) -> Vec<Lookup> {
            lock(&self.0).calls.clone()
        }

        async fn respond(reply: Reply) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
            match reply {
                Reply::Value(value) => Ok(value),
                Reply::ReadFailure => Err(FxRateSnapshotReadError::ReadFailed {
                    source: static_error("reader unavailable"),
                }),
                Reply::Block {
                    entered,
                    release,
                    value,
                } => {
                    entered.notify_one();
                    release.notified().await;
                    Ok(value)
                }
            }
        }
    }

    #[async_trait]
    impl FxRateSnapshotReader for FakeReader {
        async fn find_by_id(
            &self,
            id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
            let reply = {
                let mut state = lock(&self.0);
                state.calls.push(Lookup::ById(id));
                state.by_id.pop_front().unwrap_or(Reply::Value(None))
            };
            Self::respond(reply).await
        }

        async fn find_latest_at_or_before(
            &self,
            at: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
            let reply = {
                let mut state = lock(&self.0);
                state.calls.push(Lookup::Latest(at));
                state.latest.pop_front().unwrap_or(Reply::Value(None))
            };
            Self::respond(reply).await
        }
    }

    fn lock(state: &StdMutex<FakeState>) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    async fn assert_pending_once<F: Future + ?Sized>(mut future: Pin<&mut F>) {
        poll_fn(|cx| {
            assert!(
                future.as_mut().poll(cx).is_pending(),
                "future completed before the controlled dependency was released"
            );
            Poll::Ready(())
        })
        .await;
    }

    fn snapshot(
        id: FxRateId,
        captured_at: OffsetDateTime,
    ) -> Result<FxRateSnapshot, Box<dyn std::error::Error>> {
        let quotes = Currency::iter().map(|currency| {
            FxRateQuote::new(
                currency,
                if currency == Currency::Eur {
                    FX_RATE_SCALE
                } else {
                    FX_RATE_SCALE * 2
                },
            )
        });
        Ok(FxRateSnapshot::rehydrate(
            id,
            FxRateGeneration::try_from(1)?,
            captured_at,
            FxRateSource::FxRatesApi,
            quotes,
        )?)
    }

    fn cache(
        reader: FakeReader,
    ) -> Result<CachedFxRateSnapshotReader<FakeReader>, FxSearchCacheConfigError> {
        Ok(CachedFxRateSnapshotReader::new(
            reader,
            FxSearchCacheConfig::new(true, 2, Duration::from_secs(30))?,
        ))
    }

    #[tokio::test]
    async fn should_reuse_exact_snapshot_after_a_successful_load()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let id = FxRateId::new();
        let value = snapshot(id, OffsetDateTime::UNIX_EPOCH)?;
        reader.push_by_id(Reply::Value(Some(value.clone())));
        let cache = cache(reader.clone())?;

        assert_eq!(Some(value.clone()), cache.find_by_id(id).await?);
        assert_eq!(Some(value), cache.find_by_id(id).await?);
        assert_eq!(vec![Lookup::ById(id)], reader.calls());
        Ok(())
    }

    #[tokio::test]
    async fn should_collapse_concurrent_exact_loads() -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let id = FxRateId::new();
        let value = snapshot(id, OffsetDateTime::UNIX_EPOCH)?;
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        reader.push_by_id(Reply::Block {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            value: Some(value.clone()),
        });
        let cache = cache(reader.clone())?;

        let mut first = Box::pin(cache.find_by_id(id));
        assert_pending_once(first.as_mut()).await;
        entered.notified().await;
        let mut second = Box::pin(cache.find_by_id(id));
        assert_pending_once(second.as_mut()).await;
        assert_eq!(vec![Lookup::ById(id)], reader.calls());
        release.notify_one();

        let (first, second) = tokio::join!(first, second);
        assert_eq!(Some(value.clone()), first?);
        assert_eq!(Some(value), second?);
        assert_eq!(vec![Lookup::ById(id)], reader.calls());
        Ok(())
    }

    #[tokio::test]
    async fn should_collapse_concurrent_compatible_latest_loads()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let at = OffsetDateTime::UNIX_EPOCH;
        let value = snapshot(FxRateId::new(), at)?;
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        reader.push_latest(Reply::Block {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            value: Some(value.clone()),
        });
        let cache = cache(reader.clone())?;

        let mut first = Box::pin(cache.find_latest_at_or_before(at));
        assert_pending_once(first.as_mut()).await;
        entered.notified().await;
        let mut second = Box::pin(cache.find_latest_at_or_before(at));
        assert_pending_once(second.as_mut()).await;
        assert_eq!(vec![Lookup::Latest(at)], reader.calls());
        release.notify_one();

        let (first, second) = tokio::join!(first, second);
        assert_eq!(Some(value.clone()), first?);
        assert_eq!(Some(value), second?);
        assert_eq!(vec![Lookup::Latest(at)], reader.calls());
        Ok(())
    }

    #[tokio::test]
    async fn should_retry_exact_misses_and_reader_failures()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let id = FxRateId::new();
        let value = snapshot(id, OffsetDateTime::UNIX_EPOCH)?;
        reader.push_by_id(Reply::Value(None));
        reader.push_by_id(Reply::ReadFailure);
        reader.push_by_id(Reply::Value(Some(value.clone())));
        let cache = cache(reader.clone())?;

        assert_eq!(None, cache.find_by_id(id).await?);
        assert!(matches!(
            cache.find_by_id(id).await,
            Err(FxRateSnapshotReadError::ReadFailed { .. })
        ));
        assert_eq!(Some(value), cache.find_by_id(id).await?);
        assert_eq!(3, reader.calls().len());
        Ok(())
    }

    #[tokio::test]
    async fn should_evict_exact_snapshot_at_capacity() -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let first = FxRateId::new();
        let second = FxRateId::new();
        let third = FxRateId::new();
        reader.push_by_id(Reply::Value(Some(snapshot(
            first,
            OffsetDateTime::UNIX_EPOCH,
        )?)));
        reader.push_by_id(Reply::Value(Some(snapshot(
            second,
            OffsetDateTime::UNIX_EPOCH,
        )?)));
        reader.push_by_id(Reply::Value(Some(snapshot(
            third,
            OffsetDateTime::UNIX_EPOCH,
        )?)));
        reader.push_by_id(Reply::Value(Some(snapshot(
            first,
            OffsetDateTime::UNIX_EPOCH,
        )?)));
        let cache = cache(reader.clone())?;

        let _ = cache.find_by_id(first).await?;
        let _ = cache.find_by_id(second).await?;
        let _ = cache.find_by_id(third).await?;
        let _ = cache.find_by_id(first).await?;

        assert_eq!(
            vec![
                Lookup::ById(first),
                Lookup::ById(second),
                Lookup::ById(third),
                Lookup::ById(first)
            ],
            reader.calls()
        );
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_reuse_latest_without_extending_its_deadline()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let initial_at = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(100);
        let second_at = initial_at + time::Duration::seconds(1);
        let third_at = initial_at + time::Duration::seconds(2);
        let old = snapshot(FxRateId::new(), initial_at - time::Duration::seconds(1))?;
        let new = snapshot(FxRateId::new(), third_at)?;
        reader.push_latest(Reply::Value(Some(old.clone())));
        reader.push_latest(Reply::Value(Some(new.clone())));
        let cache = cache(reader.clone())?;

        assert_eq!(
            Some(old.clone()),
            cache.find_latest_at_or_before(initial_at).await?
        );
        tokio::time::advance(Duration::from_secs(29)).await;
        assert_eq!(Some(old), cache.find_latest_at_or_before(second_at).await?);
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(Some(new), cache.find_latest_at_or_before(third_at).await?);
        assert_eq!(
            vec![Lookup::Latest(initial_at), Lookup::Latest(third_at)],
            reader.calls()
        );
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_recheck_latest_fx_expiry_after_waiting_for_the_state_lock()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let at = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(100);
        let old = snapshot(FxRateId::new(), at - time::Duration::seconds(1))?;
        let new = snapshot(FxRateId::new(), at)?;
        reader.push_latest(Reply::Value(Some(new.clone())));
        let cache = cache(reader.clone())?;
        let mut state = cache.state.lock().await;
        state.latest = Some(LatestSelection {
            snapshot: Arc::new(old),
            selected_for: at,
            fresh_until: Instant::now() + Duration::from_secs(1),
        });

        let mut request = Box::pin(cache.find_latest_at_or_before(at));
        assert_pending_once(request.as_mut()).await;
        tokio::time::advance(Duration::from_secs(1)).await;
        drop(state);

        assert_eq!(Some(new), request.await?);
        assert_eq!(vec![Lookup::Latest(at)], reader.calls());
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_propagate_latest_fx_load_error_after_waiting_for_the_state_lock()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let at = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(100);
        let old = snapshot(FxRateId::new(), at - time::Duration::seconds(1))?;
        reader.push_latest(Reply::ReadFailure);
        let cache = cache(reader.clone())?;
        let mut state = cache.state.lock().await;
        state.latest = Some(LatestSelection {
            snapshot: Arc::new(old),
            selected_for: at,
            fresh_until: Instant::now() + Duration::from_secs(1),
        });

        let mut request = Box::pin(cache.find_latest_at_or_before(at));
        assert_pending_once(request.as_mut()).await;
        tokio::time::advance(Duration::from_secs(1)).await;
        drop(state);

        assert!(matches!(
            request.await,
            Err(FxRateSnapshotReadError::ReadFailed { .. })
        ));
        assert_eq!(vec![Lookup::Latest(at)], reader.calls());
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_keep_cursor_snapshot_pinned_after_latest_refresh()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let initial_at = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(100);
        let refreshed_at = initial_at + time::Duration::seconds(30);
        let original = snapshot(FxRateId::new(), initial_at)?;
        let original_id = original.id();
        let refreshed = snapshot(FxRateId::new(), refreshed_at)?;
        reader.push_latest(Reply::Value(Some(original.clone())));
        reader.push_latest(Reply::Value(Some(refreshed.clone())));
        let cache = cache(reader.clone())?;

        assert_eq!(
            Some(original.clone()),
            cache.find_latest_at_or_before(initial_at).await?
        );
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(
            Some(refreshed),
            cache.find_latest_at_or_before(refreshed_at).await?
        );
        assert_eq!(Some(original), cache.find_by_id(original_id).await?);
        assert_eq!(
            vec![Lookup::Latest(initial_at), Lookup::Latest(refreshed_at)],
            reader.calls()
        );
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_bypass_latest_selection_for_earlier_or_far_later_cutoffs()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let selected_for = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(100);
        let earlier = selected_for - time::Duration::seconds(1);
        let far_later = selected_for + time::Duration::seconds(31);
        let selection = snapshot(FxRateId::new(), selected_for - time::Duration::seconds(10))?;
        let earlier_value = snapshot(FxRateId::new(), earlier)?;
        let later_value = snapshot(FxRateId::new(), far_later)?;
        reader.push_latest(Reply::Value(Some(selection.clone())));
        reader.push_latest(Reply::Value(Some(earlier_value.clone())));
        reader.push_latest(Reply::Value(Some(later_value.clone())));
        let cache = cache(reader.clone())?;

        assert_eq!(
            Some(selection),
            cache.find_latest_at_or_before(selected_for).await?
        );
        assert_eq!(
            Some(earlier_value),
            cache.find_latest_at_or_before(earlier).await?
        );
        assert_eq!(
            Some(later_value),
            cache.find_latest_at_or_before(far_later).await?
        );
        assert_eq!(
            vec![
                Lookup::Latest(selected_for),
                Lookup::Latest(earlier),
                Lookup::Latest(far_later)
            ],
            reader.calls()
        );
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_not_retain_latest_selection_when_the_fill_exceeds_its_ttl()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let at = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(100);
        let first = snapshot(FxRateId::new(), at)?;
        let second = snapshot(FxRateId::new(), at)?;
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        reader.push_latest(Reply::Block {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            value: Some(first.clone()),
        });
        reader.push_latest(Reply::Value(Some(second.clone())));
        let cache = cache(reader.clone())?;

        let load = tokio::spawn({
            let cache = cache.clone();
            async move { cache.find_latest_at_or_before(at).await }
        });
        entered.notified().await;
        tokio::time::advance(Duration::from_secs(30)).await;
        release.notify_one();
        assert_eq!(Some(first), load.await??);
        assert_eq!(Some(second), cache.find_latest_at_or_before(at).await?);
        assert_eq!(vec![Lookup::Latest(at), Lookup::Latest(at)], reader.calls());
        Ok(())
    }

    #[tokio::test]
    async fn should_query_every_latest_selection_when_latest_ttl_is_zero()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let at = OffsetDateTime::UNIX_EPOCH;
        let value = snapshot(FxRateId::new(), at)?;
        let id = value.id();
        reader.push_latest(Reply::Value(Some(value.clone())));
        reader.push_latest(Reply::Value(Some(value.clone())));
        let cache = CachedFxRateSnapshotReader::new(
            reader.clone(),
            FxSearchCacheConfig::new(true, 2, Duration::ZERO)?,
        );

        assert_eq!(
            Some(value.clone()),
            cache.find_latest_at_or_before(at).await?
        );
        assert_eq!(
            Some(value.clone()),
            cache.find_latest_at_or_before(at).await?
        );
        assert_eq!(Some(value), cache.find_by_id(id).await?);
        assert_eq!(vec![Lookup::Latest(at), Lookup::Latest(at)], reader.calls());
        Ok(())
    }

    #[tokio::test]
    async fn should_not_admit_incompatible_reader_results() -> Result<(), Box<dyn std::error::Error>>
    {
        let reader = FakeReader::default();
        let requested = FxRateId::new();
        let other = FxRateId::new();
        reader.push_by_id(Reply::Value(Some(snapshot(
            other,
            OffsetDateTime::UNIX_EPOCH,
        )?)));
        reader.push_by_id(Reply::Value(Some(snapshot(
            requested,
            OffsetDateTime::UNIX_EPOCH,
        )?)));
        let at = OffsetDateTime::UNIX_EPOCH;
        reader.push_latest(Reply::Value(Some(snapshot(
            FxRateId::new(),
            at + time::Duration::seconds(1),
        )?)));
        let cache = cache(reader.clone())?;

        assert!(matches!(
            cache.find_by_id(requested).await,
            Err(FxRateSnapshotReadError::InvalidPersistedSnapshot { .. })
        ));
        assert_eq!(
            Some(snapshot(requested, OffsetDateTime::UNIX_EPOCH)?),
            cache.find_by_id(requested).await?
        );
        assert!(matches!(
            cache.find_latest_at_or_before(at).await,
            Err(FxRateSnapshotReadError::InvalidPersistedSnapshot { .. })
        ));
        assert_eq!(3, reader.calls().len());
        Ok(())
    }

    #[tokio::test]
    async fn should_release_the_fill_gate_when_the_caller_is_cancelled()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let id = FxRateId::new();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        reader.push_by_id(Reply::Block {
            entered: Arc::clone(&entered),
            release,
            value: Some(snapshot(id, OffsetDateTime::UNIX_EPOCH)?),
        });
        let expected = snapshot(id, OffsetDateTime::UNIX_EPOCH)?;
        reader.push_by_id(Reply::Value(Some(expected.clone())));
        let cache = cache(reader.clone())?;

        let load = tokio::spawn({
            let cache = cache.clone();
            async move { cache.find_by_id(id).await }
        });
        entered.notified().await;
        load.abort();
        let _ = load.await;

        assert_eq!(Some(expected), cache.find_by_id(id).await?);
        assert_eq!(2, reader.calls().len());
        Ok(())
    }

    #[tokio::test]
    async fn should_bypass_state_and_latest_selection_when_disabled()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let id = FxRateId::new();
        let at = OffsetDateTime::UNIX_EPOCH;
        let exact = snapshot(id, at)?;
        let latest = snapshot(FxRateId::new(), at)?;
        reader.push_by_id(Reply::Value(Some(exact.clone())));
        reader.push_by_id(Reply::Value(Some(exact.clone())));
        reader.push_latest(Reply::Value(Some(latest.clone())));
        reader.push_latest(Reply::Value(Some(latest.clone())));
        let cache = CachedFxRateSnapshotReader::new(
            reader.clone(),
            FxSearchCacheConfig::public_search_defaults(false),
        );

        let _state_guard = cache.state.lock().await;
        let _fill_guard = cache.fill_gate.lock().await;
        assert_eq!(Some(exact.clone()), cache.find_by_id(id).await?);
        assert_eq!(
            Some(latest.clone()),
            cache.find_latest_at_or_before(at).await?
        );
        drop(_fill_guard);
        drop(_state_guard);
        assert_eq!(Some(exact), cache.find_by_id(id).await?);
        assert_eq!(Some(latest), cache.find_latest_at_or_before(at).await?);
        assert_eq!(4, reader.calls().len());
        Ok(())
    }

    #[tokio::test]
    async fn should_keep_cache_instances_independent() -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let id = FxRateId::new();
        let value = snapshot(id, OffsetDateTime::UNIX_EPOCH)?;
        reader.push_by_id(Reply::Value(Some(value.clone())));
        reader.push_by_id(Reply::Value(Some(value.clone())));
        let first = cache(reader.clone())?;
        let second = cache(reader.clone())?;

        assert_eq!(Some(value.clone()), first.find_by_id(id).await?);
        assert_eq!(Some(value), second.find_by_id(id).await?);
        assert_eq!(2, reader.calls().len());
        Ok(())
    }
}
