use super::config::CrawlerCronConfig;
use super::metrics::PerfCounter;
use crate::scraper::candidate_service::ScraperCandidateService;
use crate::scraper::scraper_service::ScraperService;
use crate::service::listing_source_registration::ListingSourceRegistrationService;
use crate::service::raw_capture::ProductListingRawCaptureService;
use crate::spider::advisory_lock::LocalLockManager;
use crate::spider::candidate_service::SpiderCandidateService;
use crate::spider::service::SpiderService;
use futures::FutureExt;
#[cfg(test)]
use listing_source_core::ListingSourceId;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tracing::{info, warn};

/// Redacted scheduler outcomes: never carry provider bodies or panic payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CrawlerRunError {
    #[error("crawler authoritative ListingSource sync failed")]
    ListingSourceSyncFailed,
    #[error("crawler spider candidate lookup failed")]
    SpiderCandidateLookupFailed,
    #[error("crawler spider service failed")]
    SpiderServiceFailed,
    #[error("crawler spider failure metadata reset failed")]
    SpiderFailureResetFailed,
    #[error("crawler spider failure metadata write failed")]
    SpiderFailureMarkFailed,
    #[error("crawler spider task failed")]
    SpiderTaskFailed,
    #[error("crawler scraper pass incomplete")]
    ScraperPassIncomplete,
    #[error("crawler loop task failed")]
    LoopTaskFailed,
    #[error("crawler loop exited without a stop request")]
    UnexpectedLoopExit,
}

pub(super) fn stop_requested(stop: &watch::Receiver<bool>) -> bool {
    *stop.borrow() || stop.has_changed().is_err()
}

pub(super) async fn wait_for_stop(stop: &mut watch::Receiver<bool>) {
    loop {
        if stop_requested(stop) || stop.changed().await.is_err() {
            return;
        }
    }
}

#[derive(Clone)]
pub struct CrawlerCronJob {
    pub(super) config: CrawlerCronConfig,
    pub(super) lock_manager: Arc<LocalLockManager>,
    pub(super) spider_candidates: Arc<dyn SpiderCandidateService>,
    pub(super) spider_service: Arc<dyn SpiderService>,
    pub(super) scraper_candidates: Arc<dyn ScraperCandidateService>,
    pub(super) scraper_service: Arc<dyn ScraperService>,
    pub(super) listing_source_registration: Arc<ListingSourceRegistrationService>,
    pub(super) raw_capture: Arc<dyn ProductListingRawCaptureService>,
    pub(super) spider_perf: Arc<PerfCounter>,
    pub(super) scraper_perf: Arc<PerfCounter>,
}

impl CrawlerCronJob {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: CrawlerCronConfig,
        lock_manager: Arc<LocalLockManager>,
        spider_candidates: Box<dyn SpiderCandidateService>,
        spider_service: Box<dyn SpiderService>,
        scraper_candidates: Box<dyn ScraperCandidateService>,
        scraper_service: Box<dyn ScraperService>,
        listing_source_registration: ListingSourceRegistrationService,
        raw_capture: Box<dyn ProductListingRawCaptureService>,
    ) -> Self {
        Self {
            config,
            lock_manager,
            spider_candidates: spider_candidates.into(),
            spider_service: spider_service.into(),
            scraper_candidates: scraper_candidates.into(),
            scraper_service: scraper_service.into(),
            listing_source_registration: Arc::new(listing_source_registration),
            raw_capture: raw_capture.into(),
            spider_perf: Arc::new(PerfCounter::new(50, "spider")),
            scraper_perf: Arc::new(PerfCounter::new(500, "scraper")),
        }
    }

    /// Legacy demo boundary. Production must retain and inspect `run_until` instead.
    #[tracing::instrument(name = "crawler_run_loop", skip(self))]
    pub async fn run_loop(self) {
        let (keep_running, stop) = watch::channel(false);
        if let Err(error) = self.run_until(stop).await {
            warn!(%error, "Crawler cron job failed");
        }
        drop(keep_running);
    }

    /// True or sender loss latches stop. Every owned loop/pass is joined before return.
    /// Callers must not pulse true/false: watch can coalesce unobserved changes.
    /// The caller owns the absolute deadline, pools, runtime teardown and process fencing.
    /// Dropping/aborting this future is not a successful drain.
    #[tracing::instrument(name = "crawler_run_until", skip_all)]
    pub async fn run_until(self, stop: watch::Receiver<bool>) -> Result<(), CrawlerRunError> {
        self.run_until_with_signal(stop, None).await
    }

    /// Runtime observes this monotonic latch to start its drain deadline on an
    /// operational failure, before accepted capture work has finished draining.
    pub async fn run_until_with_failure(
        self,
        stop: watch::Receiver<bool>,
        failure_stop: watch::Sender<bool>,
    ) -> Result<(), CrawlerRunError> {
        self.run_until_with_signal(stop, Some(failure_stop)).await
    }

    async fn run_until_with_signal(
        self,
        mut stop: watch::Receiver<bool>,
        failure_stop: Option<watch::Sender<bool>>,
    ) -> Result<(), CrawlerRunError> {
        if stop_requested(&stop) {
            return Ok(());
        }
        let (stop_tx, run_stop) = watch::channel(false);
        let (failure_tx, mut failed) = watch::channel(false);
        let run = self.run_owned_loops(failure_tx.clone(), run_stop);
        tokio::pin!(run);
        let mut stopping = false;
        let mut failure_notified = false;
        // Cancellation is not failure. Keep observing failures even after caller stop:
        // a producer destructor or collector mark can fail while accepted work drains.
        loop {
            tokio::select! {
                biased;
                _ = wait_for_stop(&mut stop), if !stopping => {
                    stop_tx.send_replace(true);
                    stopping = true;
                }
                _ = wait_for_stop(&mut failed), if !failure_notified => {
                    stop_tx.send_replace(true);
                    if let Some(signal) = &failure_stop {
                        signal.send_replace(true);
                    }
                    failure_notified = true;
                }
                result = &mut run => {
                    if result.is_err() && let Some(signal) = &failure_stop {
                        signal.send_replace(true);
                    }
                    return result;
                },
            }
        }
    }

    async fn run_owned_loops(
        self,
        failure_tx: watch::Sender<bool>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<(), CrawlerRunError> {
        // No worker exists until the first authoritative snapshot has completed.
        match AssertUnwindSafe(self.sync_until(&mut stop))
            .catch_unwind()
            .await
        {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => return Ok(()),
            Ok(Err(error)) => return Err(error),
            Err(_) => return Err(CrawlerRunError::LoopTaskFailed),
        }
        if stop_requested(&stop) {
            return Ok(());
        }
        info!("Starting crawler cron job loops");
        let mut loops = JoinSet::new();
        let sync_job = self.clone();
        let sync_stop = stop.clone();
        loops.spawn(async move { sync_job.listing_source_sync_loop(sync_stop).await });
        let spider_job = self.clone();
        let spider_stop = stop.clone();
        let spider_stop_tx = failure_tx.clone();
        loops.spawn(async move { spider_job.spider_loop(spider_stop, spider_stop_tx).await });
        let scraper_stop = stop.clone();
        let scraper_stop_tx = failure_tx.clone();
        loops.spawn(async move { self.scraper_loop(scraper_stop, scraper_stop_tx).await });

        let mut failure = None;
        while let Some(joined) = loops.join_next().await {
            let result = match joined {
                Ok(Ok(())) if stop_requested(&stop) => Ok(()),
                Ok(Ok(())) => Err(CrawlerRunError::UnexpectedLoopExit),
                Ok(Err(error)) => Err(error),
                Err(_) => Err(CrawlerRunError::LoopTaskFailed),
            };
            if let Err(error) = result {
                failure_tx.send_replace(true);
                // Keep the first failure, but inspect every join, including drain failures.
                warn!(%error, "Crawler loop failed; stopping and joining siblings");
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    #[tracing::instrument(name = "crawler_spider_loop", skip_all)]
    async fn spider_loop(
        &self,
        mut stop: watch::Receiver<bool>,
        stop_tx: watch::Sender<bool>,
    ) -> Result<(), CrawlerRunError> {
        while !stop_requested(&stop) {
            let outcome = self.run_spider_pass_until(stop.clone(), &stop_tx).await;
            let stopped = outcome.admission_stopped();
            outcome.into_result()?;
            if stopped && !stop_requested(&stop) {
                return Err(CrawlerRunError::UnexpectedLoopExit);
            }
            tokio::select! {
                biased;
                _ = wait_for_stop(&mut stop) => break,
                _ = tokio::time::sleep(self.config.spider_interval) => {}
            }
        }
        Ok(())
    }

    #[tracing::instrument(name = "crawler_scraper_loop", skip_all)]
    async fn scraper_loop(
        &self,
        mut stop: watch::Receiver<bool>,
        stop_tx: watch::Sender<bool>,
    ) -> Result<(), CrawlerRunError> {
        while !stop_requested(&stop) {
            // Do not select/drop the pass on stop: its accepted collector must drain.
            let outcome = self
                .run_scraper_pass_until_with_failure(stop.clone(), stop_tx.clone())
                .await;
            if outcome.has_operational_failure() {
                return Err(CrawlerRunError::ScraperPassIncomplete);
            }
            if outcome.admission_stopped() && !stop_requested(&stop) {
                return Err(CrawlerRunError::UnexpectedLoopExit);
            }
            if !outcome.is_complete() {
                warn!(
                    outcome = "retryable_pass_failure",
                    "Scraper pass left site work for retry"
                );
            }
            tokio::select! {
                biased;
                _ = wait_for_stop(&mut stop) => break,
                _ = tokio::time::sleep(self.config.scraper_interval) => {}
            }
        }
        Ok(())
    }

    #[tracing::instrument(name = "crawler_listing_source_sync_loop", skip_all)]
    async fn listing_source_sync_loop(
        &self,
        mut stop: watch::Receiver<bool>,
    ) -> Result<(), CrawlerRunError> {
        loop {
            tokio::select! {
                biased;
                _ = wait_for_stop(&mut stop) => return Ok(()),
                _ = tokio::time::sleep(self.config.listing_source_sync_interval) => {}
            }
            if !self.sync_until(&mut stop).await? {
                return Ok(());
            }
        }
    }

    pub(super) async fn sync_until(
        &self,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<bool, CrawlerRunError> {
        if stop_requested(stop) {
            return Ok(false);
        }
        tokio::select! {
            biased;
            // A completed sync error must not become success because stop is also ready.
            result = self.run_listing_source_sync_once() => result.map(|()| !stop_requested(stop)),
            _ = wait_for_stop(stop) => Ok(false),
        }
    }

    #[tracing::instrument(name = "crawler_run_listing_source_sync_once", skip(self))]
    async fn run_listing_source_sync_once(&self) -> Result<(), CrawlerRunError> {
        self.listing_source_registration
            .sync()
            .await
            .map(|_| ())
            .map_err(|_| CrawlerRunError::ListingSourceSyncFailed)
    }

    pub(super) async fn admit_authoritative_scope_for_work(&self, work_kind: &'static str) -> bool {
        match self.run_listing_source_sync_once().await {
            Ok(()) => true,
            Err(error) => {
                warn!(
                    event = "crawler.work_skipped_stale_listing_source_scope",
                    work_kind,
                    error = %error,
                    outcome = "skipped",
                    "crawler work skipped because authoritative ListingSource scope could not be refreshed"
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    mod cancellation {
        include!("job_cancellation_tests.rs");
    }

    use super::*;
    use crate::scraper::candidate_service::MockScraperCandidateService;
    use crate::scraper::scraper_service::MockScraperService;
    use crate::service::cron::test_support::noop_raw_capture;
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationService, MockListingSourceRegistrationRepository,
        MockListingSourceRegistrationSource,
    };
    use crate::spider::advisory_lock::LocalLockManager;
    use crate::spider::candidate_service::MockSpiderCandidateService;
    use crate::spider::service::MockSpiderService;

    #[tokio::test]
    async fn should_run_listing_source_sync() {
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .returning(|| {
                Box::pin(async {
                    Ok(vec![
                        crate::service::listing_source_registration::RegisteredListingSource {
                            listing_source_id: ListingSourceId::new(),
                            listing_source_name: listing_source_core::ListingSourceName::try_from(
                                "Test source",
                            )
                            .unwrap_or_else(|error| {
                                panic!("invalid test listing source name: {error}")
                            }),
                            listing_source_slug: listing_source_core::ListingSourceSlugId::raw(
                                "test-source",
                            )
                            .unwrap_or_else(|error| {
                                panic!("valid test listing source slug: {error}")
                            }),
                            crawl_enabled: true,
                            fallback_currency: None,
                        },
                    ])
                })
            });

        let mut repository = MockListingSourceRegistrationRepository::new();
        repository
            .expect_apply_snapshot()
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Ok(crate::service::listing_source_registration::ListingSourceSnapshotResult::default())
                })
            });

        let listing_source_registration =
            ListingSourceRegistrationService::new(Box::new(source), Box::new(repository));

        let spider_candidates = MockSpiderCandidateService::new();
        let spider_service = MockSpiderService::new();
        let scraper_candidates = MockScraperCandidateService::new();
        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            listing_source_registration,
            noop_raw_capture(),
        );

        assert_eq!(job.run_listing_source_sync_once().await, Ok(()));
    }
}
