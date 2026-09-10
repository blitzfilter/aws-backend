use crate::ports::{
    ListingSourceSummaryReadError, ListingSourceSummaryReader, ListingSourceSummaryWithReferral,
};
use application::error::static_error;
use indexmap::{IndexMap, IndexSet};
use listing_source_core::{ListingSourceId, ReferralConfiguration};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

const DEFAULT_MAX_ENTRIES: usize = 4_096;
const DEFAULT_MAX_ACCOUNTED_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_TTL: Duration = Duration::from_secs(60);
const MAX_ENTRY_ACCOUNTED_BYTES: usize = 16 * 1024;
const FIXED_ENTRY_ACCOUNTED_BYTES: usize = 128;

/// Search-only policy for bounded ListingSource presentation reuse.
///
/// The byte limit accounts for semantic strings and a fixed per-entry allowance. It bounds
/// cache-owned payload accounting, not allocator RSS or request-local clones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSearchCacheConfig {
    enabled: bool,
    max_entries: usize,
    max_accounted_bytes: usize,
    ttl: Duration,
}

impl SourceSearchCacheConfig {
    pub fn new(
        enabled: bool,
        max_entries: usize,
        max_accounted_bytes: usize,
        ttl: Duration,
    ) -> Result<Self, SourceSearchCacheConfigError> {
        if max_entries == 0 {
            return Err(SourceSearchCacheConfigError::ZeroMaxEntries);
        }
        if max_accounted_bytes == 0 {
            return Err(SourceSearchCacheConfigError::ZeroMaxAccountedBytes);
        }
        if ttl.is_zero() {
            return Err(SourceSearchCacheConfigError::ZeroTtl);
        }

        Ok(Self {
            enabled,
            max_entries,
            max_accounted_bytes,
            ttl,
        })
    }

    pub fn public_search_defaults(enabled: bool) -> Self {
        Self {
            enabled,
            max_entries: DEFAULT_MAX_ENTRIES,
            max_accounted_bytes: DEFAULT_MAX_ACCOUNTED_BYTES,
            ttl: DEFAULT_TTL,
        }
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SourceSearchCacheConfigError {
    #[error("product listing source search cache maximum entries must be greater than zero")]
    ZeroMaxEntries,
    #[error(
        "product listing source search cache maximum accounted bytes must be greater than zero"
    )]
    ZeroMaxAccountedBytes,
    #[error("product listing source search cache TTL must be greater than zero")]
    ZeroTtl,
}

struct CachedSource {
    value: Arc<ListingSourceSummaryWithReferral>,
    fresh_until: Instant,
    accounted_bytes: usize,
}

struct SourceCacheState {
    entries: IndexMap<ListingSourceId, CachedSource>,
    accounted_bytes: usize,
}

impl SourceCacheState {
    fn new() -> Self {
        Self {
            entries: IndexMap::new(),
            accounted_bytes: 0,
        }
    }
}

/// Bounded, process-local decorator for public ProductListing search source summaries.
///
/// It caches complete source/referral summaries by `ListingSourceId`. It must be wired only
/// into public search; administrative, detail, similar-listing, worker, and write flows keep
/// direct fresh readers. Misses are loaded as one batch under a coarse fill gate. Missing and
/// failed values are never cached, and a hit never extends its fixed monotonic deadline.
pub struct CachedListingSourceSummaryReader<R> {
    inner: Arc<R>,
    config: SourceSearchCacheConfig,
    state: Arc<Mutex<SourceCacheState>>,
    fill_gate: Arc<Mutex<()>>,
}

impl<R> Clone for CachedListingSourceSummaryReader<R> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            config: self.config.clone(),
            state: Arc::clone(&self.state),
            fill_gate: Arc::clone(&self.fill_gate),
        }
    }
}

impl<R> CachedListingSourceSummaryReader<R> {
    pub fn new(inner: R, config: SourceSearchCacheConfig) -> Self {
        Self {
            inner: Arc::new(inner),
            config,
            state: Arc::new(Mutex::new(SourceCacheState::new())),
            fill_gate: Arc::new(Mutex::new(())),
        }
    }

    async fn live_entries(
        &self,
        ids: &[ListingSourceId],
        now: Instant,
    ) -> HashMap<ListingSourceId, ListingSourceSummaryWithReferral> {
        let mut state = self.state.lock().await;
        let expired = ids
            .iter()
            .copied()
            .filter(|id| {
                state
                    .entries
                    .get(id)
                    .is_some_and(|entry| entry.fresh_until <= now)
            })
            .collect::<Vec<_>>();
        for id in expired {
            remove_entry(&mut state, id);
        }

        ids.iter()
            .filter_map(|id| {
                state
                    .entries
                    .get(id)
                    .map(|entry| (*id, entry.value.as_ref().clone()))
            })
            .collect()
    }

    async fn admit_all(
        &self,
        values: &HashMap<ListingSourceId, ListingSourceSummaryWithReferral>,
        loaded_at: Instant,
    ) {
        let fresh_until = loaded_at + self.config.ttl;
        let mut state = self.state.lock().await;
        remove_expired_entries(&mut state, Instant::now());
        if Instant::now() >= fresh_until {
            return;
        }

        for (id, value) in values {
            admit_entry(
                &mut state,
                *id,
                value.clone(),
                fresh_until,
                self.config.max_entries,
                self.config.max_accounted_bytes,
            );
        }
    }
}

#[async_trait::async_trait]
impl<R> ListingSourceSummaryReader for CachedListingSourceSummaryReader<R>
where
    R: ListingSourceSummaryReader,
{
    async fn find_summaries(
        &self,
        listing_source_ids: &[ListingSourceId],
    ) -> Result<
        HashMap<ListingSourceId, ListingSourceSummaryWithReferral>,
        ListingSourceSummaryReadError,
    > {
        let ids = listing_source_ids
            .iter()
            .copied()
            .collect::<IndexSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(HashMap::new());
        }

        if !self.config.enabled {
            return self.inner.find_summaries(&ids).await;
        }

        let hits = self.live_entries(&ids, Instant::now()).await;
        let misses = missing_ids(&ids, &hits);
        if misses.is_empty() {
            return Ok(hits);
        }

        let _fill_guard = self.fill_gate.lock().await;
        let hits = self.live_entries(&ids, Instant::now()).await;
        let misses = missing_ids(&ids, &hits);
        if misses.is_empty() {
            return Ok(hits);
        }

        let load_started = Instant::now();
        let fetched = self.inner.find_summaries(&misses).await?;
        validate_returned_summaries(&fetched)?;
        let fetched_requested = fetched
            .into_iter()
            .filter(|(id, _)| misses.contains(id))
            .collect::<HashMap<_, _>>();

        let mut resolved = hits;
        resolved.extend(fetched_requested.clone());
        self.admit_all(&fetched_requested, load_started).await;
        Ok(resolved)
    }
}

fn missing_ids(
    ids: &[ListingSourceId],
    hits: &HashMap<ListingSourceId, ListingSourceSummaryWithReferral>,
) -> Vec<ListingSourceId> {
    ids.iter()
        .copied()
        .filter(|id| !hits.contains_key(id))
        .collect()
}

fn validate_returned_summaries(
    values: &HashMap<ListingSourceId, ListingSourceSummaryWithReferral>,
) -> Result<(), ListingSourceSummaryReadError> {
    if values
        .iter()
        .any(|(id, value)| value.summary.listing_source_id != *id)
    {
        return Err(ListingSourceSummaryReadError::InvalidReadModel {
            source: static_error(
                "listing source reader returned a summary for a different source ID",
            ),
        });
    }
    Ok(())
}

fn remove_entry(state: &mut SourceCacheState, id: ListingSourceId) {
    if let Some(entry) = state.entries.shift_remove(&id) {
        state.accounted_bytes = state.accounted_bytes.saturating_sub(entry.accounted_bytes);
    }
}

fn remove_expired_entries(state: &mut SourceCacheState, now: Instant) {
    state.entries.retain(|_, entry| entry.fresh_until > now);
    state.accounted_bytes = state.entries.values().fold(0usize, |total, entry| {
        total.saturating_add(entry.accounted_bytes)
    });
}

fn admit_entry(
    state: &mut SourceCacheState,
    id: ListingSourceId,
    value: ListingSourceSummaryWithReferral,
    fresh_until: Instant,
    max_entries: usize,
    max_accounted_bytes: usize,
) {
    let accounted_bytes = accounted_bytes(&value);
    if accounted_bytes > MAX_ENTRY_ACCOUNTED_BYTES || accounted_bytes > max_accounted_bytes {
        return;
    }

    remove_entry(state, id);
    while state.entries.len() >= max_entries
        || state.accounted_bytes.saturating_add(accounted_bytes) > max_accounted_bytes
    {
        let Some((_, evicted)) = state.entries.shift_remove_index(0) else {
            break;
        };
        state.accounted_bytes = state
            .accounted_bytes
            .saturating_sub(evicted.accounted_bytes);
    }

    state.accounted_bytes = state.accounted_bytes.saturating_add(accounted_bytes);
    state.entries.insert(
        id,
        CachedSource {
            value: Arc::new(value),
            fresh_until,
            accounted_bytes,
        },
    );
}

fn accounted_bytes(value: &ListingSourceSummaryWithReferral) -> usize {
    let referral_bytes = match value.referral_configuration.as_ref() {
        Some(ReferralConfiguration::Partnerize { camref }) => camref.as_ref().len(),
        None => 0,
    };

    FIXED_ENTRY_ACCOUNTED_BYTES
        .saturating_add(value.summary.name.as_ref().len())
        .saturating_add(value.summary.slug_id.as_ref().len())
        .saturating_add(referral_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::static_error;
    use async_trait::async_trait;
    use listing_source_core::{
        ListingSourceName, ListingSourceSlugId, PartnerizeCamref, ReferralConfiguration,
    };
    use std::collections::VecDeque;
    use std::sync::{Mutex as StdMutex, MutexGuard};
    use tokio::sync::Notify;

    enum Reply {
        Value(HashMap<ListingSourceId, ListingSourceSummaryWithReferral>),
        Error,
        Block {
            entered: Arc<Notify>,
            release: Arc<Notify>,
            value: HashMap<ListingSourceId, ListingSourceSummaryWithReferral>,
        },
    }

    #[derive(Default)]
    struct FakeState {
        replies: VecDeque<Reply>,
        requests: Vec<Vec<ListingSourceId>>,
    }

    #[derive(Clone, Default)]
    struct FakeReader(Arc<StdMutex<FakeState>>);

    impl FakeReader {
        fn push(&self, reply: Reply) {
            lock(&self.0).replies.push_back(reply);
        }

        fn requests(&self) -> Vec<Vec<ListingSourceId>> {
            lock(&self.0).requests.clone()
        }
    }

    #[async_trait]
    impl ListingSourceSummaryReader for FakeReader {
        async fn find_summaries(
            &self,
            listing_source_ids: &[ListingSourceId],
        ) -> Result<
            HashMap<ListingSourceId, ListingSourceSummaryWithReferral>,
            ListingSourceSummaryReadError,
        > {
            let reply = {
                let mut state = lock(&self.0);
                state.requests.push(listing_source_ids.to_vec());
                state
                    .replies
                    .pop_front()
                    .unwrap_or(Reply::Value(HashMap::new()))
            };
            match reply {
                Reply::Value(value) => Ok(value),
                Reply::Error => Err(ListingSourceSummaryReadError::QueryFailed {
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

    fn lock(state: &StdMutex<FakeState>) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn summary(
        id: ListingSourceId,
        referral_configuration: Option<ReferralConfiguration>,
    ) -> Result<ListingSourceSummaryWithReferral, Box<dyn std::error::Error>> {
        Ok(ListingSourceSummaryWithReferral {
            summary: crate::ports::ListingSourceSummary {
                listing_source_id: id,
                name: ListingSourceName::try_from("Source")?,
                slug_id: ListingSourceSlugId::raw("source")?,
            },
            referral_configuration,
        })
    }

    fn summaries(
        values: impl IntoIterator<Item = ListingSourceSummaryWithReferral>,
    ) -> HashMap<ListingSourceId, ListingSourceSummaryWithReferral> {
        values
            .into_iter()
            .map(|value| (value.summary.listing_source_id, value))
            .collect()
    }

    fn cache(
        reader: FakeReader,
    ) -> Result<CachedListingSourceSummaryReader<FakeReader>, SourceSearchCacheConfigError> {
        Ok(CachedListingSourceSummaryReader::new(
            reader,
            SourceSearchCacheConfig::new(true, 2, 1_024, Duration::from_secs(5))?,
        ))
    }

    #[tokio::test]
    async fn should_not_load_for_empty_input() -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let cache = cache(reader.clone())?;

        assert!(cache.find_summaries(&[]).await?.is_empty());
        assert!(reader.requests().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_deduplicate_cold_batch_loads() -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let first = ListingSourceId::new();
        let second = ListingSourceId::new();
        let values = summaries([summary(first, None)?, summary(second, None)?]);
        reader.push(Reply::Value(values.clone()));
        let cache = cache(reader.clone())?;

        assert_eq!(values, cache.find_summaries(&[first, second, first]).await?);
        assert_eq!(vec![vec![first, second]], reader.requests());
        Ok(())
    }

    #[tokio::test]
    async fn should_reuse_live_entries_and_load_only_batch_misses()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let first = ListingSourceId::new();
        let second = ListingSourceId::new();
        let first_value = summary(first, None)?;
        let second_value = summary(second, None)?;
        reader.push(Reply::Value(summaries([first_value.clone()])));
        reader.push(Reply::Value(summaries([second_value.clone()])));
        let cache = cache(reader.clone())?;

        assert_eq!(
            summaries([first_value.clone()]),
            cache.find_summaries(&[first]).await?
        );
        assert_eq!(
            summaries([first_value, second_value]),
            cache.find_summaries(&[first, second]).await?
        );
        assert_eq!(vec![vec![first], vec![second]], reader.requests());
        Ok(())
    }

    #[tokio::test]
    async fn should_cache_a_valid_source_without_referral_configuration()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let id = ListingSourceId::new();
        let value = summary(id, None)?;
        reader.push(Reply::Value(summaries([value.clone()])));
        let cache = cache(reader.clone())?;

        assert_eq!(
            summaries([value.clone()]),
            cache.find_summaries(&[id]).await?
        );
        assert_eq!(summaries([value]), cache.find_summaries(&[id]).await?);
        assert_eq!(vec![vec![id]], reader.requests());
        Ok(())
    }

    #[tokio::test]
    async fn should_not_cache_missing_or_failed_batch_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let id = ListingSourceId::new();
        reader.push(Reply::Value(HashMap::new()));
        reader.push(Reply::Error);
        let cache = cache(reader.clone())?;

        assert!(cache.find_summaries(&[id]).await?.is_empty());
        assert!(matches!(
            cache.find_summaries(&[id]).await,
            Err(ListingSourceSummaryReadError::QueryFailed { .. })
        ));
        assert_eq!(vec![vec![id], vec![id]], reader.requests());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_mismatched_source_identity_without_admission()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let requested = ListingSourceId::new();
        let returned = ListingSourceId::new();
        let value = summary(returned, None)?;
        reader.push(Reply::Value(HashMap::from([(requested, value)])));
        reader.push(Reply::Value(summaries([summary(requested, None)?])));
        let cache = cache(reader.clone())?;

        assert!(matches!(
            cache.find_summaries(&[requested]).await,
            Err(ListingSourceSummaryReadError::InvalidReadModel { .. })
        ));
        assert_eq!(1, cache.find_summaries(&[requested]).await?.len());
        assert_eq!(vec![vec![requested], vec![requested]], reader.requests());
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_expire_at_the_fixed_deadline_without_extending_hot_hits()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let id = ListingSourceId::new();
        let old = summary(id, None)?;
        let new = summary(
            id,
            Some(ReferralConfiguration::Partnerize {
                camref: PartnerizeCamref::try_from("newCampaign")?,
            }),
        )?;
        reader.push(Reply::Value(summaries([old.clone()])));
        reader.push(Reply::Value(summaries([new.clone()])));
        let cache = cache(reader.clone())?;

        assert_eq!(summaries([old.clone()]), cache.find_summaries(&[id]).await?);
        tokio::time::advance(Duration::from_secs(4)).await;
        assert_eq!(summaries([old]), cache.find_summaries(&[id]).await?);
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(summaries([new]), cache.find_summaries(&[id]).await?);
        assert_eq!(vec![vec![id], vec![id]], reader.requests());
        Ok(())
    }

    #[tokio::test]
    async fn should_collapse_concurrent_identical_cold_batches()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let id = ListingSourceId::new();
        let value = summaries([summary(id, None)?]);
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        reader.push(Reply::Block {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            value: value.clone(),
        });
        let cache = cache(reader.clone())?;

        let first = tokio::spawn({
            let cache = cache.clone();
            async move { cache.find_summaries(&[id]).await }
        });
        entered.notified().await;
        let second = tokio::spawn({
            let cache = cache.clone();
            async move { cache.find_summaries(&[id]).await }
        });
        release.notify_one();

        assert_eq!(value, first.await??);
        assert_eq!(value, second.await??);
        assert_eq!(vec![vec![id]], reader.requests());
        Ok(())
    }

    #[tokio::test]
    async fn should_release_fill_gate_when_request_is_canceled()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let id = ListingSourceId::new();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        reader.push(Reply::Block {
            entered: Arc::clone(&entered),
            release,
            value: HashMap::new(),
        });
        let value = summaries([summary(id, None)?]);
        reader.push(Reply::Value(value.clone()));
        let cache = cache(reader.clone())?;

        let first = tokio::spawn({
            let cache = cache.clone();
            async move { cache.find_summaries(&[id]).await }
        });
        entered.notified().await;
        first.abort();
        assert!(first.await.is_err());

        assert_eq!(value, cache.find_summaries(&[id]).await?);
        assert_eq!(vec![vec![id], vec![id]], reader.requests());
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_not_admit_a_fill_that_finishes_after_its_ttl()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let id = ListingSourceId::new();
        let value = summaries([summary(id, None)?]);
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        reader.push(Reply::Block {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            value: value.clone(),
        });
        reader.push(Reply::Value(value.clone()));
        let cache = CachedListingSourceSummaryReader::new(
            reader.clone(),
            SourceSearchCacheConfig::new(true, 2, 1_024, Duration::from_secs(1))?,
        );

        let first = tokio::spawn({
            let cache = cache.clone();
            async move { cache.find_summaries(&[id]).await }
        });
        entered.notified().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        release.notify_one();
        assert_eq!(value, first.await??);
        assert_eq!(value, cache.find_summaries(&[id]).await?);
        assert_eq!(vec![vec![id], vec![id]], reader.requests());
        Ok(())
    }

    #[tokio::test]
    async fn should_bound_entry_and_accounted_payload_admission()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let first = ListingSourceId::new();
        let second = ListingSourceId::new();
        let third = ListingSourceId::new();
        let first_value = summary(first, None)?;
        let second_value = summary(second, None)?;
        let third_value = summary(third, None)?;
        reader.push(Reply::Value(summaries([first_value.clone()])));
        reader.push(Reply::Value(summaries([second_value.clone()])));
        reader.push(Reply::Value(summaries([third_value.clone()])));
        reader.push(Reply::Value(summaries([first_value.clone()])));
        let cache = cache(reader.clone())?;

        let _ = cache.find_summaries(&[first]).await?;
        let _ = cache.find_summaries(&[second]).await?;
        let _ = cache.find_summaries(&[third]).await?;
        let _ = cache.find_summaries(&[first]).await?;
        let state = cache.state.lock().await;
        assert!(state.entries.len() <= 2);
        assert!(state.accounted_bytes <= 1_024);
        drop(state);
        assert_eq!(
            vec![vec![first], vec![second], vec![third], vec![first]],
            reader.requests()
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_return_an_oversized_source_without_retaining_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let reader = FakeReader::default();
        let id = ListingSourceId::new();
        let mut value = summary(id, None)?;
        value.summary.slug_id = ListingSourceSlugId::raw("a".repeat(MAX_ENTRY_ACCOUNTED_BYTES))?;
        let values = summaries([value]);
        reader.push(Reply::Value(values.clone()));
        reader.push(Reply::Value(values.clone()));
        let cache = cache(reader.clone())?;

        assert_eq!(values, cache.find_summaries(&[id]).await?);
        assert_eq!(values, cache.find_summaries(&[id]).await?);
        assert_eq!(vec![vec![id], vec![id]], reader.requests());
        Ok(())
    }

    #[tokio::test]
    async fn should_bypass_state_and_gate_when_disabled() -> Result<(), Box<dyn std::error::Error>>
    {
        let reader = FakeReader::default();
        let id = ListingSourceId::new();
        let value = summaries([summary(id, None)?]);
        reader.push(Reply::Value(value.clone()));
        reader.push(Reply::Value(value.clone()));
        let cache = CachedListingSourceSummaryReader::new(
            reader.clone(),
            SourceSearchCacheConfig::new(false, 2, 1_024, Duration::from_secs(5))?,
        );

        assert_eq!(value, cache.find_summaries(&[id, id]).await?);
        assert_eq!(value, cache.find_summaries(&[id]).await?);
        assert_eq!(vec![vec![id], vec![id]], reader.requests());
        Ok(())
    }
}
