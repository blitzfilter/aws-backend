use super::job::{CrawlerCronJob, CrawlerRunError, stop_requested, wait_for_stop};
#[cfg(test)]
use crate::CrawlerDomainId;
use crate::network::policy::{NetworkErrorKind, durable_retry_cooldown_for};
use crate::spider::advisory_lock::DomainLock;
use crate::spider::candidate_service::SpiderCandidate;
use crate::spider::classification::url_classification_service::UrlClassificationError;
use crate::spider::classification::url_pattern_service::UrlPatternServiceError;
use crate::spider::service::{SpiderService, SpiderServiceError};
use futures::FutureExt;
#[cfg(test)]
use listing_source_core::ListingSourceId;
use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tracing::{Instrument, info, warn};

const CRAWL_RETRY_COOLDOWN: Duration = Duration::from_secs(6 * 60 * 60);
const TRANSIENT_CRAWL_LONG_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);
const RECOVERABLE_CRAWL_LONG_COOLDOWN: Duration = Duration::from_secs(3 * 24 * 60 * 60);
const DURABLE_BLOCK_CRAWL_LONG_COOLDOWN: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const LONG_COOLDOWN_FAILURE_COUNT: i32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrawlCooldownProfile {
    Transient,
    Recoverable,
    DurableBlock,
}

impl CrawlCooldownProfile {
    fn long_cooldown(self) -> Duration {
        match self {
            CrawlCooldownProfile::Transient => TRANSIENT_CRAWL_LONG_COOLDOWN,
            CrawlCooldownProfile::Recoverable => RECOVERABLE_CRAWL_LONG_COOLDOWN,
            CrawlCooldownProfile::DurableBlock => DURABLE_BLOCK_CRAWL_LONG_COOLDOWN,
        }
    }

    fn cooldown_for_failure_count(self, failure_count: i32) -> Duration {
        match failure_count >= LONG_COOLDOWN_FAILURE_COUNT {
            true => self.long_cooldown(),
            false => CRAWL_RETRY_COOLDOWN,
        }
    }
}

fn next_failure_count(candidate: &SpiderCandidate, error_kind: &str) -> i32 {
    if candidate.last_crawl_error_kind.as_deref() == Some(error_kind) {
        candidate.crawl_failure_count.max(0).saturating_add(1)
    } else {
        1
    }
}

fn crawl_cooldown_profile(error_kind: &str) -> Option<CrawlCooldownProfile> {
    match error_kind {
        "EmptyCrawl"
        | "TinyCrawl"
        | "RateLimited"
        | "CloudflareChallenge"
        | "BotProtection"
        | "ConnectError"
        | "ServerError" => Some(CrawlCooldownProfile::Transient),
        "InsufficientInferenceSample" | "TlsError" | "RedirectProblem" | "InvalidUrl" => {
            Some(CrawlCooldownProfile::Recoverable)
        }
        "AccessDenied" | "JavascriptRequired" => Some(CrawlCooldownProfile::DurableBlock),
        _ => None,
    }
}

fn cooldown_for_spider_failure(error_kind: &str, failure_count: i32) -> Duration {
    crawl_cooldown_profile(error_kind)
        .map(|profile| profile.cooldown_for_failure_count(failure_count))
        .unwrap_or_else(|| durable_retry_cooldown_for(NetworkErrorKind::Unknown))
}

fn error_kind_for_spider_error(error: &SpiderServiceError) -> &'static str {
    match error {
        SpiderServiceError::UrlPattern(UrlPatternServiceError::PendingReview { .. }) => {
            "PendingUrlPatternReview"
        }
        SpiderServiceError::TinyCrawl { .. } => "TinyCrawl",
        SpiderServiceError::EmptyCrawl { .. } => "EmptyCrawl",
        SpiderServiceError::InsufficientInferenceSample { .. } => "InsufficientInferenceSample",
        SpiderServiceError::DiagnosticCrawlFailure { kind, .. } => kind.as_str(),
        _ => "spider_run_error",
    }
}

/// Expected, durably cooled-down site failures do not poison future passes.
/// Unknown service/task errors and failed metadata writes always reject the pass.
#[must_use]
#[derive(Debug)]
pub(super) struct SpiderPassOutcome {
    result: Result<(), CrawlerRunError>,
    admission_stopped: bool,
}

impl SpiderPassOutcome {
    pub(super) fn admission_stopped(&self) -> bool {
        self.admission_stopped
    }

    pub(super) fn into_result(self) -> Result<(), CrawlerRunError> {
        self.result
    }

    fn fail(&mut self, error: CrawlerRunError, stop_tx: &watch::Sender<bool>) {
        stop_tx.send_replace(true);
        warn!(%error, "Spider pass failed; stopping and joining workers");
        if self.result.is_ok() {
            self.result = Err(error);
        }
    }
}

enum SpiderSlotOutcome {
    Succeeded,
    Skipped,
    Stopped,
    SiteFailure,
}

fn expected_site_failure(error: &SpiderServiceError) -> bool {
    matches!(
        error,
        SpiderServiceError::EmptyCrawl { .. }
            | SpiderServiceError::TinyCrawl { .. }
            | SpiderServiceError::InsufficientInferenceSample { .. }
            | SpiderServiceError::EmptyClassificationSample { .. }
            | SpiderServiceError::DiagnosticCrawlFailure { .. }
            | SpiderServiceError::UrlPattern(UrlPatternServiceError::PendingReview { .. })
            // Classification owns generated responses. Direct UrlPattern::Regex instead
            // means an invalid persisted pattern and must remain operational.
            | SpiderServiceError::UrlPattern(UrlPatternServiceError::Classification(
                UrlClassificationError::Llm(_)
                    | UrlClassificationError::Regex(_)
                    | UrlClassificationError::NoProducts(_)
            ))
    )
}

#[allow(clippy::too_many_arguments)]
fn spawn_spider_candidate(
    join_set: &mut JoinSet<Result<SpiderSlotOutcome, CrawlerRunError>>,
    candidate: SpiderCandidate,
    spider_candidates: Arc<dyn crate::spider::candidate_service::SpiderCandidateService>,
    spider_service: Arc<dyn SpiderService>,
    lock_manager: Arc<crate::spider::advisory_lock::LocalLockManager>,
    threshold: usize,
    stop_tx: watch::Sender<bool>,
    stop: watch::Receiver<bool>,
) {
    let crawl_root_url = if candidate.listing_source_domain.starts_with("http") {
        candidate.listing_source_domain.clone()
    } else {
        format!("https://{}", candidate.listing_source_domain)
    };
    let span = tracing::info_span!(
        "spider_candidate",
        listing_source_id = %candidate.listing_source_id,
        domain_id = %candidate.domain_id
    );

    join_set.spawn(
        async move {
            if stop_requested(&stop) {
                return Ok(SpiderSlotOutcome::Stopped);
            }
            let Some(_lock) = DomainLock::try_acquire(&lock_manager, candidate.domain_id) else {
                warn!(
                    listing_source_id = %candidate.listing_source_id,
                    domain_id = %candidate.domain_id,
                    "Skipping domain - lock held by another worker"
                );
                return Ok(SpiderSlotOutcome::Skipped);
            };

            match spider_service
                .run_until(
                    &candidate.listing_source_id,
                    &candidate.domain_id,
                    &crawl_root_url,
                    threshold,
                    stop.clone(),
                )
                .await
            {
                Ok(_) => {
                    // Finish an admitted write; cancelling it could hide its result.
                    if spider_candidates
                        .reset_crawl_failure(&candidate.domain_id)
                        .await
                        .is_err()
                    {
                        stop_tx.send_replace(true);
                        return Err(CrawlerRunError::SpiderFailureResetFailed);
                    }
                    Ok(SpiderSlotOutcome::Succeeded)
                }
                Err(SpiderServiceError::Cancelled) if stop_requested(&stop) => {
                    Ok(SpiderSlotOutcome::Stopped)
                }
                Err(e) => {
                    let expected = expected_site_failure(&e);
                    if !expected {
                        stop_tx.send_replace(true);
                    }
                    let error_kind = error_kind_for_spider_error(&e);
                    let failure_count = next_failure_count(&candidate, error_kind);
                    let cooldown = cooldown_for_spider_failure(error_kind, failure_count);
                    let next_crawl_at = time::OffsetDateTime::now_utc()
                        + time::Duration::seconds(cooldown.as_secs() as i64);

                    if spider_candidates
                        .mark_crawl_failure(
                            &candidate.domain_id,
                            error_kind,
                            failure_count,
                            next_crawl_at,
                        )
                        .await
                        .is_err()
                    {
                        stop_tx.send_replace(true);
                        return Err(CrawlerRunError::SpiderFailureMarkFailed);
                    }
                    warn!(
                        error_kind,
                        failure_count,
                        cooldown_seconds = cooldown.as_secs(),
                        "Spider run failed"
                    );
                    if expected {
                        Ok(SpiderSlotOutcome::SiteFailure)
                    } else {
                        Err(CrawlerRunError::SpiderServiceFailed)
                    }
                }
            }
        }
        .instrument(span),
    );
}

impl CrawlerCronJob {
    #[cfg(test)]
    pub(super) async fn run_spider_once(&self) {
        let (keep_running, stop) = watch::channel(false);
        let outcome = self.run_spider_pass_until(stop, &keep_running).await;
        if let Err(error) = outcome.into_result() {
            warn!(%error, "Spider scheduler pass incomplete");
        }
        drop(keep_running);
    }

    /// Stop candidate selection, signal admitted services, then inspect every join.
    /// The receiver carries cancellation; the sender latches operational failure.
    /// The job forwards failure to sibling stop and runtime before service joins finish.
    /// Service cancellation owns discovery wrapper cleanup, not library process fencing.
    #[tracing::instrument(name = "crawler_run_spider_pass_until", skip_all)]
    pub(super) async fn run_spider_pass_until(
        &self,
        mut stop: watch::Receiver<bool>,
        stop_tx: &watch::Sender<bool>,
    ) -> SpiderPassOutcome {
        let mut outcome = SpiderPassOutcome {
            result: Ok(()),
            admission_stopped: stop_requested(&stop),
        };
        if outcome.admission_stopped {
            return outcome;
        }
        let spider_concurrency = self.config.spider_concurrency;
        if spider_concurrency == 0 {
            return outcome;
        }
        let pass_start = tokio::time::Instant::now();
        let mut excluded_domain_ids = HashSet::new();
        // Outside unwind isolation: a scheduler panic must not drop unjoined children.
        let mut workers = JoinSet::new();
        let mut total = 0usize;
        let mut succeeded = 0usize;
        let mut site_failed = 0usize;
        let mut skipped = 0usize;
        let mut record = |joined, outcome: &mut SpiderPassOutcome| match joined {
            Ok(Ok(SpiderSlotOutcome::Succeeded)) => succeeded += 1,
            Ok(Ok(SpiderSlotOutcome::SiteFailure)) => site_failed += 1,
            Ok(Ok(SpiderSlotOutcome::Skipped)) => skipped += 1,
            Ok(Ok(SpiderSlotOutcome::Stopped)) => outcome.admission_stopped = true,
            Ok(Err(error)) => outcome.fail(error, stop_tx),
            Err(_) => outcome.fail(CrawlerRunError::SpiderTaskFailed, stop_tx),
        };
        let admission = AssertUnwindSafe(async {
            match self.sync_until(&mut stop).await {
                Ok(true) => {}
                Ok(false) => return,
                Err(error) => {
                    outcome.fail(error, stop_tx);
                    return;
                }
            }
            'admission: loop {
                while workers.len() < spider_concurrency {
                    if stop_requested(&stop) {
                        break 'admission;
                    }
                    let limit = (spider_concurrency - workers.len()) as i64;
                    let excluded: Vec<_> = excluded_domain_ids.iter().copied().collect();
                    let lookup = async {
                        self.spider_candidates
                            .get_candidates(limit, &excluded)
                            .await
                    };
                    tokio::pin!(lookup);
                    let candidates = loop {
                        tokio::select! {
                            biased;
                            joined = workers.join_next(), if !workers.is_empty() => {
                                if let Some(joined) = joined {
                                    record(joined, &mut outcome);
                                    if outcome.result.is_err() || outcome.admission_stopped {
                                        break 'admission;
                                    }
                                }
                            }
                            _ = wait_for_stop(&mut stop) => break 'admission,
                            result = &mut lookup => break result,
                        }
                    };
                    let candidates = match candidates {
                        Ok(candidates) => candidates,
                        Err(_) => {
                            outcome.fail(CrawlerRunError::SpiderCandidateLookupFailed, stop_tx);
                            break 'admission;
                        }
                    };
                    let mut scheduled_any = false;
                    for candidate in candidates {
                        if stop_requested(&stop) {
                            break 'admission;
                        }
                        if workers.len() >= spider_concurrency {
                            break;
                        }
                        if !excluded_domain_ids.insert(candidate.domain_id) {
                            continue;
                        }
                        scheduled_any = true;
                        total += 1;
                        spawn_spider_candidate(
                            &mut workers,
                            candidate,
                            Arc::clone(&self.spider_candidates),
                            Arc::clone(&self.spider_service),
                            Arc::clone(&self.lock_manager),
                            self.config.spider_classify_threshold,
                            stop_tx.clone(),
                            stop.clone(),
                        );
                    }
                    if !scheduled_any {
                        break;
                    }
                }
                if workers.is_empty() || stop_requested(&stop) {
                    break;
                }
                tokio::select! {
                    biased;
                    joined = workers.join_next() => {
                        if let Some(joined) = joined {
                            record(joined, &mut outcome);
                            if outcome.result.is_err() || outcome.admission_stopped {
                                break;
                            }
                        }
                    }
                    _ = wait_for_stop(&mut stop) => break,
                }
            }
        })
        .catch_unwind()
        .await;
        if admission.is_err() {
            outcome.fail(CrawlerRunError::SpiderTaskFailed, stop_tx);
        }
        // Never abort a service that still owes discovery joins or a metadata write result.
        while let Some(joined) = workers.join_next().await {
            record(joined, &mut outcome);
        }
        outcome.admission_stopped |= stop_requested(&stop);
        let duration_ms = pass_start.elapsed().as_millis() as u64;
        info!(
            total,
            succeeded, site_failed, skipped, duration_ms, "Spider scheduler pass finished"
        );
        self.spider_perf.record(total as u64, duration_ms);
        outcome
    }
}

#[cfg(test)]
mod tests {
    mod cancellation {
        include!("spider_cancellation_tests.rs");
    }

    use super::*;
    use crate::scraper::candidate_service::MockScraperCandidateService;
    use crate::scraper::scraper_service::MockScraperService;
    use crate::service::cron::config::CrawlerCronConfig;
    use crate::service::cron::test_support::{noop_listing_source_registration, noop_raw_capture};
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationService, ListingSourceSyncError,
        MockListingSourceRegistrationRepository, MockListingSourceRegistrationSource,
    };
    use crate::spider::advisory_lock::LocalLockManager;
    use crate::spider::candidate_service::{MockSpiderCandidateService, SpiderCandidate};
    use crate::spider::discovery::website_spider::CrawlFailureKind;
    use crate::spider::service::{MockSpiderService, SpiderRunResult};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Notify;

    fn spider_candidate(domain_id: CrawlerDomainId) -> SpiderCandidate {
        SpiderCandidate {
            listing_source_id: ListingSourceId::new(),
            domain_id,
            listing_source_domain: "example.com".to_string(),
            crawl_failure_count: 0,
            last_crawl_error_kind: None,
        }
    }

    fn next_crawl_at_is_about(next_crawl_at: time::OffsetDateTime, cooldown: Duration) -> bool {
        let seconds = (next_crawl_at - time::OffsetDateTime::now_utc()).whole_seconds();
        let expected = cooldown.as_secs() as i64;
        seconds >= expected - 60 && seconds <= expected + 60
    }

    #[test]
    fn next_failure_count_increments_for_same_error_kind() {
        let mut candidate = spider_candidate(CrawlerDomainId::new());
        candidate.crawl_failure_count = 2;
        candidate.last_crawl_error_kind = Some("InsufficientInferenceSample".to_string());

        assert_eq!(
            next_failure_count(&candidate, "InsufficientInferenceSample"),
            3
        );
    }

    #[test]
    fn next_failure_count_resets_for_changed_error_kind() {
        let mut candidate = spider_candidate(CrawlerDomainId::new());
        candidate.crawl_failure_count = 2;
        candidate.last_crawl_error_kind = Some("TinyCrawl".to_string());

        assert_eq!(
            next_failure_count(&candidate, "InsufficientInferenceSample"),
            1
        );
    }

    #[test]
    fn cooldown_should_use_durable_retry_cooldown_for_first_two_grouped_failures() {
        for error_kind in [
            "EmptyCrawl",
            "TinyCrawl",
            "RateLimited",
            "CloudflareChallenge",
            "BotProtection",
            "ConnectError",
            "ServerError",
            "InsufficientInferenceSample",
            "TlsError",
            "RedirectProblem",
            "InvalidUrl",
            "AccessDenied",
            "JavascriptRequired",
        ] {
            assert_eq!(
                cooldown_for_spider_failure(error_kind, 1),
                CRAWL_RETRY_COOLDOWN
            );
            assert_eq!(
                cooldown_for_spider_failure(error_kind, 2),
                CRAWL_RETRY_COOLDOWN
            );
        }
    }

    #[test]
    fn cooldown_should_use_transient_long_cooldown_for_flaky_failures() {
        for error_kind in [
            "EmptyCrawl",
            "TinyCrawl",
            "RateLimited",
            "CloudflareChallenge",
            "BotProtection",
            "ConnectError",
            "ServerError",
        ] {
            assert_eq!(
                cooldown_for_spider_failure(error_kind, LONG_COOLDOWN_FAILURE_COUNT),
                TRANSIENT_CRAWL_LONG_COOLDOWN
            );
        }
    }

    #[test]
    fn cooldown_should_use_recoverable_long_cooldown_for_site_or_sample_failures() {
        for error_kind in [
            "InsufficientInferenceSample",
            "TlsError",
            "RedirectProblem",
            "InvalidUrl",
        ] {
            assert_eq!(
                cooldown_for_spider_failure(error_kind, LONG_COOLDOWN_FAILURE_COUNT),
                RECOVERABLE_CRAWL_LONG_COOLDOWN
            );
        }
    }

    #[test]
    fn cooldown_should_use_durable_block_long_cooldown_for_explicit_blocks() {
        for error_kind in ["AccessDenied", "JavascriptRequired"] {
            assert_eq!(
                cooldown_for_spider_failure(error_kind, LONG_COOLDOWN_FAILURE_COUNT),
                DURABLE_BLOCK_CRAWL_LONG_COOLDOWN
            );
        }
    }

    #[test]
    fn cooldown_should_keep_generic_fallback_for_unknown_failures() {
        assert_eq!(
            cooldown_for_spider_failure("spider_run_error", LONG_COOLDOWN_FAILURE_COUNT),
            durable_retry_cooldown_for(NetworkErrorKind::Unknown)
        );
    }

    #[tokio::test]
    async fn should_skip_spider_candidates_when_authoritative_scope_refresh_fails() {
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .once()
            .returning(|| {
                Box::pin(async {
                    Err(ListingSourceSyncError::FetchError(
                        "business unavailable".to_owned(),
                    ))
                })
            });
        let registration = ListingSourceRegistrationService::new(
            Box::new(source),
            Box::new(MockListingSourceRegistrationRepository::new()),
        );

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(MockSpiderCandidateService::new()),
            Box::new(MockSpiderService::new()),
            Box::new(MockScraperCandidateService::new()),
            Box::new(MockScraperService::new()),
            registration,
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_run_spider_candidates() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_domain_id)]) })
            });
        spider_candidates
            .expect_reset_crawl_failure()
            .withf(move |domain_id| *domain_id == expected_domain_id)
            .returning(|_| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run_until()
            .withf(move |_, domain_id, _, _, _| *domain_id == expected_domain_id)
            .returning(|_, _, _, _, _| {
                Box::pin(async {
                    Ok(SpiderRunResult {
                        total_links: 10,
                        product_urls_count: 5,
                        product_pattern: None,
                    })
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_refill_spider_slot_while_slow_crawl_is_running() {
        let slow_domain_id = CrawlerDomainId::new();
        let fast_domain_id = CrawlerDomainId::new();
        let refill_domain_id = CrawlerDomainId::new();

        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let excluded = excluded.to_vec();
                Box::pin(async move {
                    if excluded.is_empty() {
                        Ok(vec![
                            spider_candidate(slow_domain_id),
                            spider_candidate(fast_domain_id),
                        ])
                    } else if excluded.contains(&slow_domain_id)
                        && excluded.contains(&fast_domain_id)
                        && !excluded.contains(&refill_domain_id)
                    {
                        Ok(vec![spider_candidate(refill_domain_id)])
                    } else {
                        Ok(vec![])
                    }
                })
            });
        spider_candidates
            .expect_reset_crawl_failure()
            .times(3)
            .returning(|_| Box::pin(async { Ok(()) }));

        let slow_running = Arc::new(AtomicBool::new(false));
        let refill_started_while_slow_running = Arc::new(AtomicBool::new(false));
        let release_slow = Arc::new(Notify::new());
        let release_slow_for_mock = Arc::clone(&release_slow);
        let slow_running_for_mock = Arc::clone(&slow_running);
        let refill_started_for_mock = Arc::clone(&refill_started_while_slow_running);

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run_until()
            .times(3)
            .returning(move |_, domain_id, _, _, _| {
                let domain_id = *domain_id;
                let release_slow = Arc::clone(&release_slow_for_mock);
                let slow_running = Arc::clone(&slow_running_for_mock);
                let refill_started = Arc::clone(&refill_started_for_mock);
                Box::pin(async move {
                    if domain_id == slow_domain_id {
                        slow_running.store(true, Ordering::SeqCst);
                        release_slow.notified().await;
                        slow_running.store(false, Ordering::SeqCst);
                    } else if domain_id == fast_domain_id {
                        while !slow_running.load(Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(1)).await;
                        }
                    } else if domain_id == refill_domain_id {
                        if slow_running.load(Ordering::SeqCst) {
                            refill_started.store(true, Ordering::SeqCst);
                        }
                        release_slow.notify_one();
                    }
                    Ok(SpiderRunResult {
                        total_links: 10,
                        product_urls_count: 5,
                        product_pattern: None,
                    })
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));
        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig {
                spider_concurrency: 2,
                ..CrawlerCronConfig::default()
            },
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;

        assert!(refill_started_while_slow_running.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn should_mark_crawl_failure_when_spider_run_errors() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_domain_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "spider_run_error"
                    && *failure_count == 1
                    && next_crawl_at_is_about(
                        *next_crawl_at,
                        durable_retry_cooldown_for(NetworkErrorKind::Unknown),
                    )
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run_until()
            .returning(|_, _, _, _, _| {
                Box::pin(async {
                    Err(crate::spider::service::SpiderServiceError::Database(
                        sqlx::Error::RowNotFound,
                    ))
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_mark_tiny_crawl_failure_with_specific_error_kind() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_domain_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "TinyCrawl"
                    && *failure_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run_until()
            .returning(|_, _, _, _, _| {
                Box::pin(async move {
                    Err(crate::spider::service::SpiderServiceError::TinyCrawl {
                        crawl_root_url: "https://example.com/".to_string(),
                        total_links: 1,
                    })
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_mark_empty_crawl_failure_with_specific_error_kind() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_domain_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "EmptyCrawl"
                    && *failure_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run_until()
            .returning(|_, _, _, _, _| {
                Box::pin(async move {
                    Err(crate::spider::service::SpiderServiceError::EmptyCrawl {
                        crawl_root_url: "https://example.com/".to_string(),
                    })
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_mark_insufficient_inference_sample_failure_with_specific_error_kind() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_domain_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "InsufficientInferenceSample"
                    && *failure_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run_until()
            .returning(|_, _, _, _, _| {
                Box::pin(async move {
                    Err(
                        crate::spider::service::SpiderServiceError::InsufficientInferenceSample {
                            crawl_root_url: "https://example.com/".to_string(),
                            stage: "end_of_crawl",
                            sample_size: 16,
                            min_sample_size: 20,
                        },
                    )
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_use_long_cooldown_after_repeated_empty_crawl_failures() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    let mut candidate = spider_candidate(expected_domain_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind = Some("EmptyCrawl".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "EmptyCrawl"
                    && *failure_count == 3
                    && next_crawl_at_is_about(*next_crawl_at, TRANSIENT_CRAWL_LONG_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run_until()
            .returning(|_, _, _, _, _| {
                Box::pin(async move {
                    Err(crate::spider::service::SpiderServiceError::EmptyCrawl {
                        crawl_root_url: "https://example.com/".to_string(),
                    })
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_use_long_cooldown_after_repeated_insufficient_inference_sample_failures() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    let mut candidate = spider_candidate(expected_domain_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind =
                        Some("InsufficientInferenceSample".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "InsufficientInferenceSample"
                    && *failure_count == 3
                    && next_crawl_at_is_about(*next_crawl_at, RECOVERABLE_CRAWL_LONG_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run_until()
            .returning(|_, _, _, _, _| {
                Box::pin(async move {
                    Err(
                        crate::spider::service::SpiderServiceError::InsufficientInferenceSample {
                            crawl_root_url: "https://example.com/".to_string(),
                            stage: "end_of_crawl",
                            sample_size: 16,
                            min_sample_size: 20,
                        },
                    )
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    async fn assert_diagnostic_failure_is_persisted(
        kind: CrawlFailureKind,
        expected_error_kind: &'static str,
    ) {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_domain_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == expected_error_kind
                    && *failure_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run_until()
            .returning(move |_, _, _, _, _| {
                Box::pin(async move {
                    Err(
                        crate::spider::service::SpiderServiceError::DiagnosticCrawlFailure {
                            crawl_root_url: "https://example.com/".to_string(),
                            kind,
                            total_links: 1,
                            http_status: None,
                            final_url: None,
                            redirect_url: None,
                            diagnostic_reason: Some("test_diagnostic".to_string()),
                        },
                    )
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_persist_diagnostic_failure_kinds_with_small_crawl_cooldown() {
        for (kind, expected_error_kind) in [
            (CrawlFailureKind::EmptyCrawl, "EmptyCrawl"),
            (CrawlFailureKind::RateLimited, "RateLimited"),
            (CrawlFailureKind::AccessDenied, "AccessDenied"),
            (CrawlFailureKind::CloudflareChallenge, "CloudflareChallenge"),
            (CrawlFailureKind::BotProtection, "BotProtection"),
            (CrawlFailureKind::TlsError, "TlsError"),
            (CrawlFailureKind::ConnectError, "ConnectError"),
            (CrawlFailureKind::ServerError, "ServerError"),
            (CrawlFailureKind::RedirectProblem, "RedirectProblem"),
            (CrawlFailureKind::InvalidUrl, "InvalidUrl"),
            (CrawlFailureKind::JavascriptRequired, "JavascriptRequired"),
        ] {
            assert_diagnostic_failure_is_persisted(kind, expected_error_kind).await;
        }
    }

    #[tokio::test]
    async fn should_use_long_cooldown_after_repeated_diagnostic_failures() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    let mut candidate = spider_candidate(expected_domain_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind = Some("RateLimited".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "RateLimited"
                    && *failure_count == 3
                    && next_crawl_at_is_about(*next_crawl_at, TRANSIENT_CRAWL_LONG_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run_until()
            .returning(|_, _, _, _, _| {
                Box::pin(async move {
                    Err(
                        crate::spider::service::SpiderServiceError::DiagnosticCrawlFailure {
                            crawl_root_url: "https://example.com/".to_string(),
                            kind: CrawlFailureKind::RateLimited,
                            total_links: 1,
                            http_status: Some(429),
                            final_url: Some("https://example.com/".to_string()),
                            redirect_url: None,
                            diagnostic_reason: Some("canonical_non_success_status".to_string()),
                        },
                    )
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_skip_spider_candidate_when_domain_lock_is_already_held() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let locked_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(locked_domain_id)]) })
            });

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run_until().times(0);

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();
        let lock_manager = Arc::new(LocalLockManager::new());
        let _prelock = DomainLock::try_acquire(&lock_manager, locked_domain_id).unwrap();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::clone(&lock_manager),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }
}
