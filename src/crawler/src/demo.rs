//! Demo binary — runs the full crawler pipeline (spider + scraper) against a set of hardcoded
//! antique ListingSources without needing a running ListingSource service.
//!
//! On startup the demo automatically runs `docker compose up -d` (using the
//! `docker-compose.yml` inside the `crawler` crate) and waits for Postgres to
//! become ready before applying migrations. No manual setup required — just:
//!
//! ```powershell
//! gcloud auth application-default login
//! $env:VERTEX_AI_PROJECT_ID="my-project"
//! $env:VERTEX_AI_LOCATION="europe-west3"
//! cargo run -p crawler --bin demo
//! ```
//!
//! # Configuration
//!
//! | Env var          | Purpose                              | Default                                          |
//! |------------------|--------------------------------------|--------------------------------------------------|
//! | `VERTEX_AI_PROJECT_ID` | Google Cloud project for Vertex AI | *(required)* |
//! | `VERTEX_AI_LOCATION` | Vertex AI location | *(required)* |
//! | `GOOGLE_APPLICATION_CREDENTIALS` | Optional local Application Default Credentials file | unset |
//! | `VERTEX_AI_MODEL` | Schema generation/repair model | `gemini-3.1-pro-preview` |
//! | `CRAWLER_VERTEX_AI_CHEAP_MODEL` | Default low-risk crawler LLM model | `gemini-3.1-flash-lite` |
//! | `CRAWLER_VERTEX_AI_URL_CLASSIFICATION_MODEL` | Optional URL classification model override | `CRAWLER_VERTEX_AI_CHEAP_MODEL` |
//! | `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS` | Max in-flight crawler LLM calls | `1` |
//! | `CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS` | Minimum delay between LLM request starts | `2000` |
//! | `LOCAL_DB_URL`   | Hardcoded local DB URL                | `postgres://postgres:postgres@localhost:5432/crawler_demo` |
//! | `CRAWLER_REVIEW_REQUIRED` | Block generated patterns/schemas until approved | unset / `false`                       |
//! | `CRAWLER_REVIEW_URL_PATTERN_REQUIRED` | Block generated URL patterns until approved | unset / `false`            |
//! | `CRAWLER_REVIEW_BIND_ADDR` | Review UI bind address        | `127.0.0.1:7878`                                |
//! | `CRAWLER_REVIEW_AUTH_TOKEN` | Optional bearer token for the review UI/API | unset                               |
//! | `LOG_LEVEL`      | Global log level                     | `info`                                           |
//! | `CRAWLER_LOG_LEVEL` | Crawler-internal log level        | `info`                                           |
//!
//! Scraped products are written to `scraped_products.json` instead of calling the ProductListing upsert use case.

use listing_source_core::ListingSourceId;
use std::env;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use crawler::llm_runtime::{CrawlerLlmGovernor, CrawlerLlmRateLimitConfig};
use crawler::local_db::{
    DEMO_DB_NAME, bootstrap_local_database,
    crawler_domain_configuration_repository::CrawlerDomainConfigurationRepositoryImpl, demo_db_url,
};
use crawler::logging::HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE;
use crawler::review::repository::CrawlerReviewRepository;
use crawler::review::server::{ReviewServer, ReviewServerConfig};
use crawler::scraper::candidate_service::ScraperCandidateServiceImpl;
use crawler::scraper::css_selector::product_schema_repository::ListingSourceProductSchemaRepositoryImpl;
use crawler::scraper::css_selector::product_schema_service::ProductListingSchemaServiceImpl;
use crawler::scraper::css_selector::removed_page_schema_repository::RemovedPageSchemaRepositoryImpl;
use crawler::scraper::normalization::product_normalization_service::ProductListingNormalizationServiceImpl;
use crawler::scraper::scraper_service::{
    DEFAULT_SCHEMA_SEED_PAGES, ReqwestHtmlFetcher, ScraperServiceImpl,
};
use crawler::service::crawler_domain_configuration::{
    CrawlerDomainAdministrationHandler, CrawlerDomainConfigurationRepository,
};
use crawler::service::cron::{CrawlerCronConfig, CrawlerCronJob};
use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepositoryImpl, ListingSourceRegistrationService,
    ListingSourceRegistrationSource, ListingSourceSyncError, RegisteredListingSource,
};
use crawler::service::raw_capture::FileProductListingRawCaptureService;
use crawler::spider::advisory_lock::LocalLockManager;
use crawler::spider::candidate_service::SpiderCandidateServiceImpl;
use crawler::spider::classification::url_classification_service::UrlClassificationServiceImpl;
use crawler::spider::classification::url_metadata_repository::UrlMetadataRepositoryImpl;
use crawler::spider::classification::url_pattern_repository::ListingSourceUrlPatternRepositoryImpl;
use crawler::spider::classification::url_pattern_service::UrlPatternServiceImpl;
use crawler::spider::discovery::website_spider::SpiderImpl;
use crawler::spider::service::spider_service::{SpiderServiceConfig, SpiderServiceImpl};
use crawler::vertex_ai::{CrawlerVertexAiConfig, CrawlerVertexAiModels};

use tracing::{Instrument, error, info};

// ---------------------------------------------------------------------------
// Demo ListingSource source — returns hardcoded ListingSources (no OpenSearch needed)
// ---------------------------------------------------------------------------

struct DemoListingSourceSource {
    listing_sources: Vec<RegisteredListingSource>,
}

#[async_trait]
impl ListingSourceRegistrationSource for DemoListingSourceSource {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError> {
        Ok(self.listing_sources.clone())
    }
}

fn crawler_review_required() -> bool {
    env::var("CRAWLER_REVIEW_REQUIRED")
        .map(|value| matches!(value.as_str(), "true" | "TRUE" | "1" | "yes" | "YES"))
        .unwrap_or(false)
}

fn crawler_review_url_pattern_required() -> bool {
    env::var("CRAWLER_REVIEW_URL_PATTERN_REQUIRED")
        .map(|value| matches!(value.as_str(), "true" | "TRUE" | "1" | "yes" | "YES"))
        .unwrap_or(false)
}

fn demo_listing_sources() -> Vec<RegisteredListingSource> {
    [
        (1, "Hingstons Antiques", "hingstons-antiques"),
        (
            2,
            "Harrison Antique Furniture",
            "harrison-antique-furniture",
        ),
        (3, "Collinge Antiques", "collinge-antiques"),
    ]
    .into_iter()
    .map(
        |(index, listing_source_name, listing_source_slug)| RegisteredListingSource {
            listing_source_id: ListingSourceId::from(
                uuid::Uuid::parse_str(&format!("a1000000-0000-0000-0000-{index:012}"))
                    .unwrap_or_else(|error| panic!("invalid demo ListingSource ID: {error}")),
            ),
            listing_source_name: listing_source_core::ListingSourceName::try_from(
                listing_source_name,
            )
            .unwrap_or_else(|error| panic!("invalid demo ListingSource name: {error}")),
            listing_source_slug: listing_source_core::ListingSourceSlugId::raw(listing_source_slug)
                .unwrap_or_else(|error| panic!("invalid demo ListingSource slug: {error}")),
            crawl_enabled: true,
        },
    )
    .collect()
}

// ---------------------------------------------------------------------------
// CLI flag parsing
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    init_logging();

    async {
        let vertex_ai_config = match CrawlerVertexAiConfig::from_env() {
            Ok(config) => config,
            Err(error) => {
                error!(%error, "Failed to load Vertex AI configuration");
                return;
            }
        };
        let vertex_ai_models = CrawlerVertexAiModels::from_env();

        let config = CrawlerCronConfig {
            spider_interval: Duration::from_secs(120),
            scraper_interval: Duration::from_secs(30),
            scraper_urls_per_domain: 50,
            spider_concurrency: 100,
            spider_site_concurrency_limit: 8,
            scraper_concurrency: 10,
            spider_classify_threshold: 400,
            scraper_schema_seed_pages: DEFAULT_SCHEMA_SEED_PAGES,
            ..Default::default()
        };

        let db_url = demo_db_url();
        if let Err(error) = bootstrap_local_database(DEMO_DB_NAME).await {
            error!(error = ?error, "Failed to bootstrap local Postgres database");
            return;
        }

        info!("Waiting for Postgres to be ready…");
        let pool = match connect_with_retry(&config, &db_url).await {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "Failed to connect to Postgres after retries");
                return;
            }
        };

        if let Err(error) = sqlx::migrate!("./migrations").run(&pool).await {
            error!(error = ?error, "Failed to apply database migrations");
            return;
        }
        info!("Database migrations applied successfully");

        let review_required = crawler_review_required();
        let url_pattern_review_required = crawler_review_url_pattern_required();
        let review_config =
            ReviewServerConfig::from_env().expect("CRAWLER_REVIEW_BIND_ADDR must be host:port");
        let review_repo = CrawlerReviewRepository::new(pool.clone());

        info!(
            llm_provider = "vertex_ai",
            schema_model = %vertex_ai_models.product_schema,
            url_classification_model = %vertex_ai_models.url_classification,
            review_required,
            url_pattern_review_required,
            review_bind_addr = %review_config.bind_addr,
            "Wiring crawler dependencies..."
        );
        let llm_governor = Arc::new(CrawlerLlmGovernor::new(
            CrawlerLlmRateLimitConfig::from_env(),
        ));

        let normalization_svc = ProductListingNormalizationServiceImpl::new();

        let create_schema_llm =
            match vertex_ai_config.create_model(vertex_ai_models.product_schema.clone()) {
                Ok(model) => model,
                Err(error) => {
                    error!(%error, "Failed to initialize Vertex AI model for schema generation");
                    return;
                }
            };
        let single_schema_llm = match vertex_ai_config
            .create_model(vertex_ai_models.product_schema.clone())
        {
            Ok(model) => model,
            Err(error) => {
                error!(%error, "Failed to initialize Vertex AI model for fresh schema generation");
                return;
            }
        };

        let schema_repo = Box::new(ListingSourceProductSchemaRepositoryImpl::new(Box::leak(
            Box::new(pool.clone()),
        )));
        let schema_svc = ProductListingSchemaServiceImpl::new(
            create_schema_llm,
            single_schema_llm,
            schema_repo,
            Some(Arc::clone(&llm_governor)),
        );
        let removed_page_schema_repo = Box::new(RemovedPageSchemaRepositoryImpl::new(Box::leak(
            Box::new(pool.clone()),
        )));

        let scraper_candidates = Box::new(
            ScraperCandidateServiceImpl::new_with_max_llm_calls_per_listing_source(
                pool.clone(),
                config.scraper_max_llm_calls_per_listing_source,
            ),
        );

        let fetcher = Box::new(ReqwestHtmlFetcher::with_auto_throttle_config(
            config.scraper_auto_throttle_config(),
        ));
        let scraper_svc = Box::new(
            ScraperServiceImpl::new_with_schema_seed_pages(
                fetcher,
                Box::new(schema_svc),
                Box::new(normalization_svc),
                Arc::new(
                    ScraperCandidateServiceImpl::new_with_max_llm_calls_per_listing_source(
                        pool.clone(),
                        config.scraper_max_llm_calls_per_listing_source,
                    ),
                ),
                config.scraper_schema_seed_pages,
                config.scraper_max_llm_calls_per_listing_source,
            )
            .with_removed_page_schema_repository(removed_page_schema_repo)
            .with_review_gate(review_repo.clone(), review_required),
        );

        let url_metadata_repo = Arc::new(UrlMetadataRepositoryImpl::new(pool.clone()));
        let url_pattern_repo = Box::new(ListingSourceUrlPatternRepositoryImpl::new(pool.clone()));

        let classification_llm =
            match vertex_ai_config.create_model(vertex_ai_models.url_classification.clone()) {
                Ok(model) => model,
                Err(error) => {
                    error!(%error, "Failed to initialize Vertex AI model for URL classification");
                    return;
                }
            };
        let class_svc = Box::new(UrlClassificationServiceImpl::new(
            classification_llm,
            Some(Arc::clone(&llm_governor)),
        ));
        let pattern_svc = Box::new(UrlPatternServiceImpl::new_with_review(
            Arc::new(*url_pattern_repo),
            class_svc,
            review_repo.clone(),
            url_pattern_review_required,
        ));

        let spider_svc = Box::new(SpiderServiceImpl::new(
            SpiderServiceConfig {
                db_batch_size: 40,
                ..Default::default()
            },
            Box::new(SpiderImpl::new(config.spider_website_config())),
            pattern_svc,
            url_metadata_repo.clone(),
        ));
        let spider_candidates = Box::new(SpiderCandidateServiceImpl::new(pool.clone()));

        let listing_source_source = Box::new(DemoListingSourceSource {
            listing_sources: demo_listing_sources(),
        });
        let listing_source_repo =
            Box::new(ListingSourceRegistrationRepositoryImpl::new(pool.clone()));
        let listing_source_registration =
            ListingSourceRegistrationService::new(listing_source_source, listing_source_repo);
        if let Err(error) = listing_source_registration.sync().await {
            error!(error = ?error, "Failed to synchronize demo ListingSources");
            return;
        }
        let domain_configuration =
            Arc::new(CrawlerDomainConfigurationRepositoryImpl::new(pool.clone()));
        for (listing_source, domain) in demo_listing_sources().into_iter().zip([
            "hingstons-antiques.co.uk",
            "harrisonantiquefurniture.co.uk",
            "collingeantiques.com",
        ]) {
            if let Err(error) = domain_configuration
                .register(
                    listing_source.listing_source_id,
                    listing_source_core::Domain::try_from(domain)
                        .unwrap_or_else(|error| panic!("invalid demo crawler domain: {error}")),
                )
                .await
            {
                error!(error = ?error, domain, "Failed to configure demo crawler domain");
                return;
            }
        }
        let raw_capture = Box::new(FileProductListingRawCaptureService::new(
            "scraped_raw_observations.json",
        ));

        let cron_job = CrawlerCronJob::new(
            config,
            Arc::new(LocalLockManager::new()),
            spider_candidates,
            spider_svc,
            scraper_candidates,
            scraper_svc,
            listing_source_registration,
            raw_capture,
        );

        info!(
            listing_source_count = demo_listing_sources().len(),
            llm_provider = "vertex_ai",
            schema_model = %vertex_ai_models.product_schema,
            url_classification_model = %vertex_ai_models.url_classification,
            review_required,
            url_pattern_review_required,
            review_bind_addr = %review_config.bind_addr,
            "Crawler demo is fully initialized. Starting background tasks. Press Ctrl+C to stop."
        );
        let review_server = ReviewServer::new(
            review_repo,
            Arc::new(CrawlerDomainAdministrationHandler::new(
                domain_configuration,
            )),
            review_config,
        );
        let review_handle = tokio::spawn(async move {
            review_server
                .run()
                .await
                .expect("crawler review server failed")
        });
        let cron_handle = tokio::spawn(async move {
            cron_job.run_loop().await;
        });

        tokio::select! {
            result = review_handle => {
                result.expect("crawler review server task panicked");
            }
            result = cron_handle => {
                result.expect("crawler cron task panicked");
            }
        }
    }
    .instrument(tracing::info_span!(
        "crawler_demo",
        entrypoint = "demo",
        database = DEMO_DB_NAME
    ))
    .await;
}

// ---------------------------------------------------------------------------
// Database helpers
// ---------------------------------------------------------------------------

/// Runs `docker compose up -d` from the crawler crate directory.
///
/// `docker compose up -d` is idempotent:
/// - Container already running → no-op, returns immediately.
/// - Container exists but is stopped → restarts it.
/// - Container does not exist → creates and starts it.
///
/// The compose file path is baked in via `CARGO_MANIFEST_DIR` so this works
/// regardless of the working directory when `cargo run` is invoked.
/// Attempts to connect to Postgres, retrying with exponential back-off.
/// This handles the window between `docker compose up -d` returning and
/// Postgres actually accepting connections.
#[tracing::instrument(skip(config), fields(db_url = %db_url))]
async fn connect_with_retry(
    config: &CrawlerCronConfig,
    db_url: &str,
) -> Result<sqlx::PgPool, String> {
    let mut attempt = 0u32;
    let mut delay = Duration::from_millis(200);

    loop {
        attempt += 1;
        match config.connect_pool(db_url).await {
            Ok(pool) => {
                info!(
                    attempt,
                    max_connections = config.effective_db_max_connections(),
                    "Connected to Postgres"
                );
                return Ok(pool);
            }
            Err(e) if attempt < 30 => {
                info!(attempt, error = %e, "Postgres not ready yet, retrying…");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(3));
            }
            Err(e) => {
                return Err(format!(
                    "Could not connect to Postgres after {attempt} attempts: {e}"
                ));
            }
        }
    }
}

fn init_logging() {
    let raw_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let crawler_level = env::var("CRAWLER_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let filter = tracing_subscriber::EnvFilter::new(format!(
        "{raw_level},crawler={crawler_level},spider=warn,sqlx::postgres::notice=warn,{HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE}"
    ));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .init();
}
