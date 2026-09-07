//! Production server binary for the crawler.
//!
//! Wires crawler-local Postgres, authoritative business Postgres, and the LLM, then starts the
//! [`CrawlerCronJob`] loop that continuously spiders ListingSource websites, scrapes product pages,
//! and captures changed raw ProductListing observations for asynchronous normalization.
//!
//! # Connection pool sizing
//!
//! `db_max_connections` defaults to
//! `spider_concurrency + scraper_concurrency + 10`.
//! Override it explicitly in [`CrawlerCronConfig`] if needed.
//!
//! # Required environment variables
//!
//! | Variable                        | Purpose                                                        |
//! |---------------------------------|----------------------------------------------------------------|
//! | `LOCAL_DB_URL`                  | Crawler-local Postgres URL (`crawler_server`)                  |
//! | `BUSINESS_DATABASE_URL`         | Required authoritative Postgres URL for listing_sources and products     |
//! | `VERTEX_AI_PROJECT_ID`          | Required Google Cloud project for Vertex AI                    |
//! | `VERTEX_AI_LOCATION`            | Required Vertex AI location                                    |
//! | `GOOGLE_APPLICATION_CREDENTIALS`| Optional local Application Default Credentials file             |
//! | `VERTEX_AI_MODEL`               | Schema generation/repair model (default: `gemini-3.1-pro-preview`) |
//! | `CRAWLER_VERTEX_AI_CHEAP_MODEL` | Default model for low-risk crawler LLM tasks                   |
//! | `CRAWLER_VERTEX_AI_URL_CLASSIFICATION_MODEL` | Optional URL classification model override       |
//! | `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS` | Max in-flight crawler LLM calls (default: `1`)          |
//! | `CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS` | Minimum delay between crawler LLM request starts (default: `2000`) |
//! | `CRAWLER_CLOUDWATCH_LOG_GROUP`  | Optional CloudWatch Logs group name for crawler server logs    |
//! | `CRAWLER_CLOUDWATCH_LOG_STREAM` | Optional CloudWatch Logs stream name; defaults to host name    |
//! | `SPIDER_MAX_SIZE_BYTES`          | Required Spider transport page-body ceiling; set to `8388608`  |
//!
//! # CloudWatch IAM permissions
//!
//! If `CRAWLER_CLOUDWATCH_LOG_GROUP` is set, the crawler runtime needs:
//!
//! - `logs:CreateLogGroup`
//! - `logs:CreateLogStream`
//! - `logs:PutLogEvents`

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_cloudwatchlogs::Client as CloudWatchLogsClient;
use aws_sdk_cloudwatchlogs::error::SdkError;
use aws_sdk_cloudwatchlogs::operation::create_log_group::CreateLogGroupError;
use aws_sdk_cloudwatchlogs::operation::create_log_stream::CreateLogStreamError;
use crawler::llm_runtime::{CrawlerLlmGovernor, CrawlerLlmRateLimitConfig};
use crawler::local_db::{
    SERVER_DB_NAME, bootstrap_local_database,
    crawler_domain_configuration_repository::CrawlerDomainConfigurationRepositoryImpl,
    server_db_url,
};
use crawler::logging::{
    CloudWatchBootstrapClient, CloudWatchBootstrapError, CloudWatchLoggingConfig,
    HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE, cloudwatch_logging_config,
    ensure_cloudwatch_log_destination,
};
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
use crawler::service::crawler_domain_configuration::CrawlerDomainAdministrationHandler;
use crawler::service::cron::{CrawlerCronConfig, CrawlerCronJob};
use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepositoryImpl, ListingSourceRegistrationService,
    ListingSourceRegistrationSource, ListingSourceSyncError, RegisteredListingSource,
};
use crawler::service::raw_capture::ProductListingRawCaptureServiceImpl;
use crawler::spider::advisory_lock::LocalLockManager;
use crawler::spider::candidate_service::SpiderCandidateServiceImpl;
use crawler::spider::classification::url_classification_service::UrlClassificationServiceImpl;
use crawler::spider::classification::url_metadata_repository::UrlMetadataRepositoryImpl;
use crawler::spider::classification::url_pattern_repository::ListingSourceUrlPatternRepositoryImpl;
use crawler::spider::classification::url_pattern_service::UrlPatternServiceImpl;
use crawler::spider::discovery::website_spider::SpiderImpl;
use crawler::spider::service::spider_service::{SpiderServiceConfig, SpiderServiceImpl};
use crawler::vertex_ai::{CrawlerVertexAiConfig, CrawlerVertexAiModels};
use listing_source_postgres::SqlxListingSourceReaders;
use listing_source_service::ports::WebCrawlSourceReader;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingRawCaptureWriterFactory,
};
use product_listing_service::use_cases::CaptureProductListingRawObservationHandler;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::time::Duration;
use tracing::{Instrument, info};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

// ---------------------------------------------------------------------------
// ListingSourceRegistrationSource backed only by WebCrawlSourceReader.
// ---------------------------------------------------------------------------

struct PostgresWebCrawlSource {
    sources: Box<dyn WebCrawlSourceReader>,
}

impl PostgresWebCrawlSource {
    fn new(sources: Box<dyn WebCrawlSourceReader>) -> Self {
        Self { sources }
    }
}

#[async_trait]
impl ListingSourceRegistrationSource for PostgresWebCrawlSource {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError> {
        self.sources
            .list_sources()
            .await
            .map_err(|error| ListingSourceSyncError::FetchError(error.to_string()))?
            .into_iter()
            .map(|source| {
                Ok(RegisteredListingSource {
                    listing_source_id: source.listing_source_id,
                    listing_source_name: source.listing_source_name,
                    listing_source_slug: source.listing_source_slug,
                    crawl_enabled: source.web_crawl_enabled,
                })
            })
            .collect()
    }
}

struct AwsSdkCloudWatchBootstrapClient {
    client: CloudWatchLogsClient,
}

#[async_trait]
impl CloudWatchBootstrapClient for AwsSdkCloudWatchBootstrapClient {
    async fn create_log_group(&self, log_group_name: &str) -> Result<(), CloudWatchBootstrapError> {
        match self
            .client
            .create_log_group()
            .log_group_name(log_group_name)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(err)) => Err(map_create_log_group_error(err.err())),
            Err(err) => Err(CloudWatchBootstrapError::Other(err.to_string())),
        }
    }

    async fn create_log_stream(
        &self,
        log_group_name: &str,
        log_stream_name: &str,
    ) -> Result<(), CloudWatchBootstrapError> {
        match self
            .client
            .create_log_stream()
            .log_group_name(log_group_name)
            .log_stream_name(log_stream_name)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(err)) => Err(map_create_log_stream_error(err.err())),
            Err(err) => Err(CloudWatchBootstrapError::Other(err.to_string())),
        }
    }
}

fn map_create_log_group_error(error: &CreateLogGroupError) -> CloudWatchBootstrapError {
    if error.is_resource_already_exists_exception() {
        CloudWatchBootstrapError::AlreadyExists
    } else {
        CloudWatchBootstrapError::Other(error.to_string())
    }
}

fn map_create_log_stream_error(error: &CreateLogStreamError) -> CloudWatchBootstrapError {
    if error.is_resource_already_exists_exception() {
        CloudWatchBootstrapError::AlreadyExists
    } else {
        CloudWatchBootstrapError::Other(error.to_string())
    }
}

fn build_log_filter() -> EnvFilter {
    let configured_log_level = std::env::var("LOG_LEVEL").ok();
    let raw_level = configured_log_level
        .as_deref()
        .unwrap_or("info")
        .to_string();
    let directives = format!(
        "{raw_level},spider=warn,sqlx::postgres::notice=warn,{HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE}"
    );
    EnvFilter::new(directives)
}

fn init_crawler_logging(
    cloudwatch_config: Option<&CloudWatchLoggingConfig>,
    cloudwatch_client: Option<CloudWatchLogsClient>,
) -> Option<tracing_cloudwatch::CloudWatchWorkerGuard> {
    let stdout_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(true)
        .with_span_list(true)
        .with_ansi(false);

    if let (Some(config), Some(client)) = (cloudwatch_config, cloudwatch_client) {
        let (cloudwatch_layer, cloudwatch_guard) = tracing_cloudwatch::layer()
            .with_fmt_layer(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_current_span(true)
                    .with_span_list(true)
                    .with_ansi(false),
            )
            .with_code_location(false)
            .with_target(false)
            .with_client(
                client,
                tracing_cloudwatch::ExportConfig::default()
                    .with_batch_size(50usize)
                    .with_interval(Duration::from_secs(1))
                    .with_log_group_name(config.log_group_name.clone())
                    .with_log_stream_name(config.log_stream_name.clone()),
            );

        tracing_subscriber::registry()
            .with(build_log_filter())
            .with(stdout_layer)
            .with(cloudwatch_layer)
            .init();
        Some(cloudwatch_guard)
    } else {
        tracing_subscriber::registry()
            .with(build_log_filter())
            .with(stdout_layer)
            .init();
        None
    }
}

fn crawler_review_required() -> bool {
    std::env::var("CRAWLER_REVIEW_REQUIRED")
        .map(|value| matches!(value.as_str(), "true" | "TRUE" | "1" | "yes" | "YES"))
        .unwrap_or(false)
}

fn crawler_review_url_pattern_required() -> bool {
    std::env::var("CRAWLER_REVIEW_URL_PATTERN_REQUIRED")
        .map(|value| matches!(value.as_str(), "true" | "TRUE" | "1" | "yes" | "YES"))
        .unwrap_or(false)
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let cloudwatch_logging = cloudwatch_logging_config()
        .expect("Failed to parse crawler CloudWatch logging configuration");
    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;
    let cloudwatch_client = cloudwatch_logging
        .as_ref()
        .map(|_| CloudWatchLogsClient::new(&aws_config));

    if let (Some(config), Some(client)) = (cloudwatch_logging.as_ref(), cloudwatch_client.as_ref())
    {
        let bootstrap_client = AwsSdkCloudWatchBootstrapClient {
            client: client.clone(),
        };
        ensure_cloudwatch_log_destination(&bootstrap_client, config)
            .await
            .expect("Failed to ensure CloudWatch log group and stream for crawler server");
    }

    let _cloudwatch_guard =
        init_crawler_logging(cloudwatch_logging.as_ref(), cloudwatch_client.clone());

    async move {
        info!("Starting Crawler Server");

        if let Some(config) = cloudwatch_logging.as_ref() {
            info!(
                log_group = %config.log_group_name,
                log_stream = %config.log_stream_name,
                "CloudWatch log export enabled"
            );
        }

        // 1. Build cron config (needed for pool sizing before everything else)
        let config = CrawlerCronConfig {
            spider_interval: Duration::from_hours(72),
            scraper_interval: Duration::from_mins(10),
            scraper_urls_per_domain: 100,
            spider_concurrency: 3,
            spider_site_concurrency_limit: 8,
            scraper_concurrency: 3,
            spider_classify_threshold: 400,
            scraper_schema_seed_pages: DEFAULT_SCHEMA_SEED_PAGES,
            push_batch_size: 1000,
            push_queue_capacity: 2000,
            push_max_batch_age: Duration::from_secs(5),
            push_max_concurrency: 4,
            business_db_max_connections: 8,
            ..Default::default()
        };

        config.validate_business_capacity();

        info!(
            spider_interval_s = config.spider_interval.as_secs(),
            scraper_interval_s = config.scraper_interval.as_secs(),
            spider_concurrency = config.spider_concurrency,
            spider_site_concurrency_limit = config.spider_site_concurrency_limit,
            scraper_concurrency = config.scraper_concurrency,
            scraper_schema_seed_pages = config.scraper_schema_seed_pages,
            scraper_domain_delay_ms = config.scraper_domain_delay.as_millis(),
            scraper_auto_throttle_target_concurrency =
                config.scraper_auto_throttle_target_concurrency,
            scraper_auto_throttle_max_delay_ms = config.scraper_auto_throttle_max_delay.as_millis(),
            scraper_auto_throttle_alpha = config.scraper_auto_throttle_alpha,
            scraper_max_llm_calls_per_listing_source =
                config.scraper_max_llm_calls_per_listing_source,
            push_batch_size = config.effective_push_batch_size(),
            push_queue_capacity = config.effective_push_queue_capacity(),
            push_max_batch_age_ms = config.effective_push_max_batch_age().as_millis(),
            push_max_concurrency = config.effective_push_max_concurrency(),
            business_db_max_connections = config.effective_business_db_max_connections(),
            "Crawler cron configuration loaded"
        );

        // 2. Connect to database — pool is sized to spider_concurrency + scraper_concurrency + 10
        //    to keep headroom for concurrent repository queries.
        bootstrap_local_database(SERVER_DB_NAME)
            .await
            .expect("Failed to bootstrap local Postgres database");
        let db_url = server_db_url();
        let pool = config
            .connect_pool(&db_url)
            .await
            .expect("Failed to connect to database");

        info!(
            max_connections = config.effective_db_max_connections(),
            "Connected to crawler-local Postgres"
        );

        // 3. Apply pending migrations — runs at startup so deploying a new binary
        //    is the only step required to update the production schema.
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("Failed to run database migrations");
        info!("Crawler-local database migrations applied successfully");

        let business_database_url = std::env::var("BUSINESS_DATABASE_URL")
            .expect("BUSINESS_DATABASE_URL environment variable must be set");
        let business_db_max_connections = config.effective_business_db_max_connections();
        let business_pool = PgPoolOptions::new()
            .max_connections(business_db_max_connections)
            .acquire_timeout(Duration::from_secs(30))
            .connect(&business_database_url)
            .await
            .expect("Failed to connect to authoritative business Postgres");
        info!(
            max_connections = business_db_max_connections,
            raw_capture_max_concurrency = config.effective_push_max_concurrency(),
            "Connected to authoritative business Postgres"
        );

        let review_required = crawler_review_required();
        let url_pattern_review_required = crawler_review_url_pattern_required();
        let review_config =
            ReviewServerConfig::from_env().expect("CRAWLER_REVIEW_BIND_ADDR must be host:port");
        let review_repo = CrawlerReviewRepository::new(pool.clone());

        // 4. Wire scraper + spider dependencies. Provider and model choices stay here;
        // crawler services depend only on the generic LargeLanguageModel capability.
        let vertex_ai_config = CrawlerVertexAiConfig::from_env()
            .expect("VERTEX_AI_PROJECT_ID and VERTEX_AI_LOCATION must be set");
        let vertex_ai_models = CrawlerVertexAiModels::from_env();
        let llm_rate_limit_config = CrawlerLlmRateLimitConfig::from_env();
        let llm_governor = Arc::new(CrawlerLlmGovernor::new(llm_rate_limit_config));

        info!(
            llm_provider = "vertex_ai",
            schema_model = %vertex_ai_models.product_schema,
            url_classification_model = %vertex_ai_models.url_classification,
            max_concurrent_requests = llm_rate_limit_config.max_concurrent_requests,
            min_request_interval_ms = llm_rate_limit_config.min_request_interval.as_millis(),
            "Crawler LLM governor configured"
        );

        let normalization_svc = ProductListingNormalizationServiceImpl::new();

        let create_schema_llm = vertex_ai_config
            .create_model(vertex_ai_models.product_schema.clone())
            .expect("failed to initialize Vertex AI model for schema generation");
        let single_schema_llm = vertex_ai_config
            .create_model(vertex_ai_models.product_schema.clone())
            .expect("failed to initialize Vertex AI model for fresh schema generation");

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

        let classification_llm = vertex_ai_config
            .create_model(vertex_ai_models.url_classification.clone())
            .expect("failed to initialize Vertex AI model for URL classification");
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

        let spider_config = SpiderServiceConfig {
            ..Default::default()
        };
        let website_spider = Box::new(SpiderImpl::new(config.spider_website_config()));

        let spider_svc = Box::new(SpiderServiceImpl::new(
            spider_config,
            website_spider,
            pattern_svc,
            url_metadata_repo.clone(),
        ));

        let spider_candidates = Box::new(SpiderCandidateServiceImpl::new(pool.clone()));

        // 5. Sync crawler scope from authoritative WebCrawl sources.
        let listing_source_source = Box::new(PostgresWebCrawlSource::new(Box::new(
            SqlxListingSourceReaders::new(business_pool.clone()),
        )));
        let listing_source_repo =
            Box::new(ListingSourceRegistrationRepositoryImpl::new(pool.clone()));
        let listing_source_registration =
            ListingSourceRegistrationService::new(listing_source_source, listing_source_repo);

        // 6. Capture changed crawler evidence through the operational raw-capture use case.
        let raw_capture = Box::new(ProductListingRawCaptureServiceImpl::new(
            Arc::new(CaptureProductListingRawObservationHandler::new(
                SqlxUnitOfWork::new(business_pool.clone()),
                SqlxProductListingRawCaptureWriterFactory::new(),
                SqlxPartnerProductListingAuthorizerFactory::new(),
            )),
            config.effective_push_max_concurrency(),
        ));

        let db_max_connections = config.effective_db_max_connections();
        let scraper_max_llm_calls_per_listing_source =
            config.scraper_max_llm_calls_per_listing_source;
        let push_batch_size = config.effective_push_batch_size();
        let push_queue_capacity = config.effective_push_queue_capacity();
        let push_max_batch_age_ms = config.effective_push_max_batch_age().as_millis();
        let push_max_concurrency = config.effective_push_max_concurrency();

        // 7. Build cron job
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

        // 8. Run forever
        info!(
            db_max_connections,
            scraper_max_llm_calls_per_listing_source,
            push_batch_size,
            push_queue_capacity,
            push_max_batch_age_ms,
            push_max_concurrency,
            business_db_max_connections,
            llm_provider = "vertex_ai",
            schema_model = %vertex_ai_models.product_schema,
            url_classification_model = %vertex_ai_models.url_classification,
            review_required,
            url_pattern_review_required,
            review_bind_addr = %review_config.bind_addr,
            "Crawler Server is fully initialized. Starting background tasks..."
        );
        let review_server = ReviewServer::new(
            review_repo,
            Arc::new(CrawlerDomainAdministrationHandler::new(Arc::new(
                CrawlerDomainConfigurationRepositoryImpl::new(pool.clone()),
            ))),
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
    .instrument(tracing::info_span!("crawler_startup"))
    .await;
}
