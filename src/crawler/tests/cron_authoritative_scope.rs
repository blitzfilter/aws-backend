use async_trait::async_trait;
use crawler::scraper::candidate_service::ScraperCandidateService;
use crawler::service::cron::{CrawlerCronConfig, CrawlerCronJob};
use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
    ListingSourceRegistrationService, ListingSourceRegistrationSource, ListingSourceSyncError,
    RegisteredListingSource,
};
use crawler::service::raw_capture::{
    ProductListingRawCaptureItem, ProductListingRawCaptureOutcome, ProductListingRawCaptureService,
};
use crawler::spider::advisory_lock::LocalLockManager;
use crawler::spider::candidate_service::{SpiderCandidateService, SpiderCandidateServiceImpl};
use crawler::spider::service::{SpiderRunResult, SpiderService, SpiderServiceError};
use crawler::{CrawlerDomainId, scraper};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use serial_test::serial;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use test_api::*;
use url::Url;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

struct AbsentAuthoritativeSource;

#[async_trait]
impl ListingSourceRegistrationSource for AbsentAuthoritativeSource {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError> {
        Ok(vec![])
    }
}

struct NoWorkSpider {
    started: Arc<AtomicBool>,
}

#[async_trait]
impl SpiderService for NoWorkSpider {
    async fn run(
        &self,
        _: &ListingSourceId,
        _: &CrawlerDomainId,
        _: &str,
        _: usize,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        self.started.store(true, Ordering::SeqCst);
        unreachable!("disabled crawler source must not start spider work")
    }
}

struct NoWorkScraper {
    started: Arc<AtomicBool>,
}

#[async_trait]
impl scraper::scraper_service::ScraperService for NoWorkScraper {
    async fn scrape(
        &self,
        _: &ListingSourceId,
        _: &Url,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&[u8]>,
    ) -> Result<
        Option<scraper::scraper_service::ScrapedProduct>,
        scraper::scraper_service::ScraperError,
    > {
        self.started.store(true, Ordering::SeqCst);
        unreachable!("disabled crawler source must not start scraper work")
    }
}

struct NoWorkRawCapture {
    started: Arc<AtomicBool>,
}

#[async_trait]
impl ProductListingRawCaptureService for NoWorkRawCapture {
    async fn capture(
        &self,
        _: Vec<ProductListingRawCaptureItem>,
    ) -> Vec<ProductListingRawCaptureOutcome> {
        self.started.store(true, Ordering::SeqCst);
        unreachable!("disabled crawler source must not start raw capture work")
    }
}

fn registered_listing_source(listing_source_id: ListingSourceId) -> RegisteredListingSource {
    RegisteredListingSource {
        listing_source_id,
        listing_source_name: ListingSourceName::try_from("Crawler admission source")
            .unwrap_or_else(|error| panic!("valid test ListingSource name: {error}")),
        listing_source_slug: ListingSourceSlugId::raw("crawler-admission-source")
            .unwrap_or_else(|error| panic!("valid test ListingSource slug: {error}")),
        crawl_enabled: true,
        fallback_currency: None,
    }
}

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_disable_absent_source_before_spider_and_scraper_select_work() {
    let pool = get_postgres_client().await;
    let listing_source_id = ListingSourceId::new();
    let registration_repository = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    registration_repository
        .apply_snapshot(&[registered_listing_source(listing_source_id)])
        .await
        .unwrap_or_else(|error| panic!("seed enabled crawler ListingSource: {error}"));
    let domain_id = CrawlerDomainId::new();
    sqlx::query(
        "INSERT INTO listing_source_domains \
         (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, 'crawler-admission.example', 'crawler-admission.example')",
    )
    .bind(domain_id.as_uuid())
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("seed crawler domain: {error}"));
    sqlx::query(
        "INSERT INTO listing_source_urls \
         (listing_source_id, domain_id, url, url_class) \
         VALUES ($1, $2, 'https://crawler-admission.example/products/1', 'product')",
    )
    .bind(listing_source_id.as_uuid())
    .bind(domain_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("seed crawler product URL: {error}"));

    let spider_candidates = SpiderCandidateServiceImpl::new(pool.clone());
    let scraper_candidates =
        scraper::candidate_service::ScraperCandidateServiceImpl::new(pool.clone());
    assert_eq!(
        1,
        spider_candidates
            .get_candidates(1, &[])
            .await
            .unwrap_or_else(|error| panic!("select seeded spider candidate: {error}"))
            .len()
    );
    assert_eq!(
        1,
        scraper_candidates
            .get_candidates(1, 1, &[])
            .await
            .unwrap_or_else(|error| panic!("select seeded scraper candidate: {error}"))
            .len()
    );

    let registration = ListingSourceRegistrationService::new(
        Box::new(AbsentAuthoritativeSource),
        Box::new(ListingSourceRegistrationRepositoryImpl::new(pool.clone())),
    );
    let spider_started = Arc::new(AtomicBool::new(false));
    let scraper_started = Arc::new(AtomicBool::new(false));
    let raw_capture_started = Arc::new(AtomicBool::new(false));
    let job = CrawlerCronJob::new(
        CrawlerCronConfig {
            spider_interval: Duration::from_millis(1),
            scraper_interval: Duration::from_millis(1),
            spider_concurrency: 1,
            scraper_concurrency: 1,
            ..CrawlerCronConfig::default()
        },
        Arc::new(LocalLockManager::new()),
        Box::new(spider_candidates),
        Box::new(NoWorkSpider {
            started: Arc::clone(&spider_started),
        }),
        Box::new(scraper_candidates),
        Box::new(NoWorkScraper {
            started: Arc::clone(&scraper_started),
        }),
        registration,
        Box::new(NoWorkRawCapture {
            started: Arc::clone(&raw_capture_started),
        }),
    );
    let scheduler = tokio::spawn(job.run_loop());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let enabled = loop {
        let enabled = sqlx::query_scalar::<_, bool>(
            "SELECT crawl_enabled FROM listing_sources WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("read refreshed crawler ListingSource: {error}"));
        if !enabled {
            break enabled;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "crawler scheduler did not apply the authoritative empty scope"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    scheduler.abort();
    let _ = scheduler.await;

    assert!(!enabled);
    assert!(!spider_started.load(Ordering::SeqCst));
    assert!(!scraper_started.load(Ordering::SeqCst));
    assert!(!raw_capture_started.load(Ordering::SeqCst));
    assert!(
        SpiderCandidateServiceImpl::new(pool.clone())
            .get_candidates(1, &[])
            .await
            .unwrap_or_else(|error| panic!("select disabled spider candidate: {error}"))
            .is_empty()
    );
    assert!(
        scraper::candidate_service::ScraperCandidateServiceImpl::new(pool)
            .get_candidates(1, 1, &[])
            .await
            .unwrap_or_else(|error| panic!("select disabled scraper candidate: {error}"))
            .is_empty()
    );
}
