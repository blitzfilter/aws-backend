use super::config::CrawlerCronConfig;
use super::metrics::PerfCounter;
use crate::scraper::candidate_service::ScraperCandidateService;
use crate::scraper::scraper_service::ScraperService;
use crate::service::listing_source_registration::ListingSourceRegistrationService;
use crate::service::raw_capture::ProductListingRawCaptureService;
use crate::spider::advisory_lock::LocalLockManager;
use crate::spider::candidate_service::SpiderCandidateService;
use crate::spider::service::SpiderService;
#[cfg(test)]
use listing_source_core::ListingSourceId;
use std::sync::Arc;
use tracing::{info, warn};

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

    #[tracing::instrument(name = "crawler_run_loop", skip(self))]
    pub async fn run_loop(self) {
        info!("Starting crawler cron job loop");

        self.run_listing_source_sync_once().await;

        let spider_job = self.clone();
        let sync_job = self.clone();
        let scraper_job = self;

        let sync_handle = tokio::spawn(async move {
            sync_job.listing_source_sync_loop().await;
        });

        let spider_handle = tokio::spawn(async move {
            spider_job.spider_loop().await;
        });

        let scraper_handle = tokio::spawn(async move {
            scraper_job.scraper_loop().await;
        });

        let _ = tokio::join!(spider_handle, scraper_handle, sync_handle);
    }

    #[tracing::instrument(name = "crawler_spider_loop", skip(self))]
    async fn spider_loop(&self) {
        loop {
            self.run_spider_once().await;
            tokio::time::sleep(self.config.spider_interval).await;
        }
    }

    #[tracing::instrument(name = "crawler_scraper_loop", skip(self))]
    async fn scraper_loop(&self) {
        loop {
            self.run_scraper_once().await;
            tokio::time::sleep(self.config.scraper_interval).await;
        }
    }

    #[tracing::instrument(name = "crawler_listing_source_sync_loop", skip(self))]
    async fn listing_source_sync_loop(&self) {
        loop {
            tokio::time::sleep(self.config.listing_source_sync_interval).await;
            self.run_listing_source_sync_once().await;
        }
    }

    #[tracing::instrument(name = "crawler_run_listing_source_sync_once", skip(self))]
    async fn run_listing_source_sync_once(&self) {
        match self.listing_source_registration.sync().await {
            Ok(_) => {}
            Err(e) => warn!(error = %e, "ListingSource sync failed"),
        }
    }
}

#[cfg(test)]
mod tests {
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

        job.run_listing_source_sync_once().await;
    }
}
