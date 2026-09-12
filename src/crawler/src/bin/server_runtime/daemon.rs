//! Concrete daemon composition restored from server.rs at
//! c7a46b9b434eb0dc02c26a4e91edc863cae34896, with validated startup inputs.
//!
//! The legacy review/run_loop task lifecycle is retained, not a graceful-shutdown proof.
//! Signal handling, operational failure propagation, drain deadlines, and process fencing
//! remain iteration04e2 work. This module is never called by help or check-config.
//!
//! Optional CloudWatch export still creates the group/stream and publishes log batches;
//! it requires logs:CreateLogGroup, logs:CreateLogStream, and logs:PutLogEvents.

use super::config::ServerConfig;
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_cloudwatchlogs::Client as CloudWatchLogsClient;
use aws_sdk_cloudwatchlogs::error::SdkError;
use aws_sdk_cloudwatchlogs::operation::create_log_group::CreateLogGroupError;
use aws_sdk_cloudwatchlogs::operation::create_log_stream::CreateLogStreamError;
use crawler::llm_runtime::CrawlerLlmGovernor;
use crawler::local_db::{
    CrawlerSchemaError,
    crawler_domain_configuration_repository::CrawlerDomainConfigurationRepositoryImpl,
    verify_crawler_schema,
};
use crawler::logging::{
    CloudWatchBootstrapClient, CloudWatchBootstrapError, CloudWatchLoggingConfig,
    ensure_cloudwatch_log_destination,
};
use crawler::review::repository::CrawlerReviewRepository;
use crawler::review::server::ReviewServer;
use crawler::scraper::candidate_service::ScraperCandidateServiceImpl;
use crawler::scraper::css_selector::product_schema_repository::ListingSourceProductSchemaRepositoryImpl;
use crawler::scraper::css_selector::product_schema_service::ProductListingSchemaServiceImpl;
use crawler::scraper::css_selector::removed_page_schema_repository::RemovedPageSchemaRepositoryImpl;
use crawler::scraper::normalization::product_normalization_service::ProductListingNormalizationServiceImpl;
use crawler::scraper::scraper_service::{ReqwestHtmlFetcher, ScraperServiceImpl};
use crawler::service::crawler_domain_configuration::CrawlerDomainAdministrationHandler;
use crawler::service::cron::CrawlerCronJob;
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
use google_cloud_auth::credentials::Builder as GoogleCredentialsBuilder;
use large_language_model::{VertexAiConfig, VertexAiGemini};
use listing_source_postgres::SqlxListingSourceReaders;
use listing_source_service::ports::WebCrawlSourceReader;
use platform_postgres::{
    PostgresConnectError, PostgresSchemaError, SqlxUnitOfWork, verify_business_schema,
};
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingRawCaptureWriterFactory,
};
use product_listing_service::use_cases::CaptureProductListingRawObservationHandler;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tracing::{Instrument, info, warn};
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
            .map_err(|_| {
                ListingSourceSyncError::FetchError(
                    "authoritative ListingSource read failed (details redacted)".into(),
                )
            })?
            .into_iter()
            .map(|source| {
                Ok(RegisteredListingSource {
                    listing_source_id: source.listing_source_id,
                    listing_source_name: source.listing_source_name,
                    listing_source_slug: source.listing_source_slug,
                    crawl_enabled: source.web_crawl_enabled,
                    fallback_currency: source.fallback_currency,
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
            Err(_) => Err(CloudWatchBootstrapError::Other(
                "CloudWatch create log group failed (details redacted)".into(),
            )),
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
            Err(_) => Err(CloudWatchBootstrapError::Other(
                "CloudWatch create log stream failed (details redacted)".into(),
            )),
        }
    }
}

fn map_create_log_group_error(error: &CreateLogGroupError) -> CloudWatchBootstrapError {
    if error.is_resource_already_exists_exception() {
        CloudWatchBootstrapError::AlreadyExists
    } else {
        CloudWatchBootstrapError::Other(
            "CloudWatch create log group rejected (details redacted)".into(),
        )
    }
}

fn map_create_log_stream_error(error: &CreateLogStreamError) -> CloudWatchBootstrapError {
    if error.is_resource_already_exists_exception() {
        CloudWatchBootstrapError::AlreadyExists
    } else {
        CloudWatchBootstrapError::Other(
            "CloudWatch create log stream rejected (details redacted)".into(),
        )
    }
}

// tracing-cloudwatch 0.4.1 prints SDK export errors with Debug to stderr, and its
// trait hides the log/error types needed for a client wrapper. Redact only the
// export client's completed failures, AFTER SDK retry classification. Clear the
// raw response too: SDK ResponseError includes its body/headers in Debug.
#[derive(Debug)]
struct RedactCloudWatchExportErrors;

impl aws_sdk_cloudwatchlogs::config::Intercept for RedactCloudWatchExportErrors {
    fn name(&self) -> &'static str {
        "RedactCloudWatchExportErrors"
    }

    fn modify_before_completion(
        &self,
        context: &mut aws_sdk_cloudwatchlogs::config::interceptors::FinalizerInterceptorContextMut<
            '_,
        >,
        _components: &aws_sdk_cloudwatchlogs::config::RuntimeComponents,
        _config: &mut aws_sdk_cloudwatchlogs::config::ConfigBag,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        if context
            .output_or_error()
            .is_some_and(|result| result.is_err())
        {
            if let Some(response) = context.response_mut() {
                *response = aws_sdk_cloudwatchlogs::config::http::HttpResponse::new(
                    response.status(),
                    "".into(),
                );
            }
            return Err(
                std::io::Error::other("CloudWatch log export failed (details redacted)").into(),
            );
        }
        Ok(())
    }
}

fn init_crawler_logging(
    filter: EnvFilter,
    cloudwatch_config: Option<&CloudWatchLoggingConfig>,
    cloudwatch_client: Option<CloudWatchLogsClient>,
) -> Result<Option<tracing_cloudwatch::CloudWatchWorkerGuard>, DaemonError> {
    let stdout_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(true)
        .with_span_list(true)
        .with_ansi(false);

    if let (Some(config), Some(client)) = (cloudwatch_config, cloudwatch_client) {
        let export_client = CloudWatchLogsClient::from_conf(
            client
                .config()
                .to_builder()
                .interceptor(RedactCloudWatchExportErrors)
                .build(),
        );
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
                export_client,
                tracing_cloudwatch::ExportConfig::default()
                    .with_batch_size(50usize)
                    .with_interval(Duration::from_secs(1))
                    .with_log_group_name(config.log_group_name.clone())
                    .with_log_stream_name(config.log_stream_name.clone()),
            );

        tracing_subscriber::registry()
            .with(filter)
            .with(stdout_layer)
            .with(cloudwatch_layer)
            .try_init()
            .map_err(|error| DaemonError::Logging(RedactedDaemonCause::new(error)))?;
        Ok(Some(cloudwatch_guard))
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(stdout_layer)
            .try_init()
            .map_err(|error| DaemonError::Logging(RedactedDaemonCause::new(error)))?;
        Ok(None)
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum DaemonError {
    #[error(transparent)]
    DatabaseConnect(#[from] PostgresConnectError),
    #[error(transparent)]
    CrawlerSchema(#[from] CrawlerSchemaError),
    #[error(transparent)]
    BusinessSchema(#[from] PostgresSchemaError),
    #[error("failed to initialize CloudWatch log destination")]
    CloudWatchBootstrap(#[source] RedactedDaemonCause),
    #[error("failed to initialize crawler logging")]
    Logging(#[source] RedactedDaemonCause),
    #[error("failed to initialize Google application default credentials")]
    Credentials(#[source] RedactedDaemonCause),
    #[error("failed to initialize Vertex AI client")]
    VertexClient(#[source] RedactedDaemonCause),
    #[error("crawler review server failed")]
    ReviewServer(#[source] RedactedDaemonCause),
    #[error("crawler review task failed")]
    ReviewTask(#[source] RedactedDaemonCause),
    #[error("crawler cron task failed")]
    CronTask(#[source] RedactedDaemonCause),
    #[error("crawler review server stopped unexpectedly; graceful shutdown not verified")]
    ReviewStopped,
    #[error("crawler cron loop stopped unexpectedly; graceful shutdown not verified")]
    CronStopped,
}

// Retain original technical causes privately; even recursive source formatting stays safe.
pub(super) struct RedactedDaemonCause {
    _original: Box<dyn Error + Send + Sync>,
}

impl RedactedDaemonCause {
    fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self {
            _original: Box::new(error),
        }
    }
}

impl fmt::Display for RedactedDaemonCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("daemon dependency error (details redacted)")
    }
}

impl fmt::Debug for RedactedDaemonCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Error for RedactedDaemonCause {}

fn create_model(config: VertexAiConfig) -> Result<VertexAiGemini, DaemonError> {
    // Same ADC builder/scope and Vertex constructor as crawler::vertex_ai, but consume
    // the validated snapshot instead of rereading environment after startup side effects.
    let credentials = GoogleCredentialsBuilder::default()
        .with_scopes(["https://www.googleapis.com/auth/cloud-platform"])
        .build_access_token_credentials()
        .map_err(|error| DaemonError::Credentials(RedactedDaemonCause::new(error)))?;
    VertexAiGemini::new(config, credentials)
        .map_err(|error| DaemonError::VertexClient(RedactedDaemonCause::new(error)))
}

pub(super) async fn run(validated: ServerConfig) -> Result<(), DaemonError> {
    let ServerConfig {
        commit_sha,
        databases,
        cron: config,
        spider_max_size_bytes,
        review: review_config,
        review_required,
        url_pattern_review_required,
        vertex_ai_models,
        product_schema_model,
        url_classification_model,
        llm_rate_limit: llm_rate_limit_config,
        cloudwatch: cloudwatch_logging,
        log_filter,
    } = validated;
    let cloudwatch_client = if cloudwatch_logging.is_some() {
        let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
            .load()
            .await;
        Some(CloudWatchLogsClient::new(&aws_config))
    } else {
        None
    };

    if let (Some(config), Some(client)) = (cloudwatch_logging.as_ref(), cloudwatch_client.as_ref())
    {
        let bootstrap_client = AwsSdkCloudWatchBootstrapClient {
            client: client.clone(),
        };
        ensure_cloudwatch_log_destination(&bootstrap_client, config)
            .await
            .map_err(|error| DaemonError::CloudWatchBootstrap(RedactedDaemonCause::new(error)))?;
    }

    let _cloudwatch_guard = init_crawler_logging(
        log_filter,
        cloudwatch_logging.as_ref(),
        cloudwatch_client.clone(),
    )?;

    async move {
        info!(%commit_sha, "Starting Crawler Server");
        warn!("Legacy daemon lifecycle active; signal/drain/deadline integration remains incomplete until iteration04e2");

        if let Some(config) = cloudwatch_logging.as_ref() {
            info!(
                log_group = %config.log_group_name,
                log_stream = %config.log_stream_name,
                "CloudWatch log export enabled"
            );
        }

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

        let pool = databases.crawler.connect().await?;

        info!(
            max_connections = config.effective_db_max_connections(),
            "Connected to crawler-local Postgres"
        );

        verify_crawler_schema(&pool).await?;
        info!("Crawler-local schema and migration history verified (read-only)");

        let business_db_max_connections = config.effective_business_db_max_connections();
        let business_pool = databases.business.connect().await?;
        info!(
            max_connections = business_db_max_connections,
            raw_capture_max_concurrency = config.effective_push_max_concurrency(),
            "Connected to authoritative business Postgres"
        );

        verify_business_schema(&business_pool).await?;
        info!("Business schema and migration history verified (read-only)");

        let review_repo = CrawlerReviewRepository::new(pool.clone());

        // 4. Wire scraper + spider dependencies. Provider and model choices stay here;
        // crawler services depend only on the generic LargeLanguageModel capability.

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

        let create_schema_llm = create_model(product_schema_model.clone())?;
        let single_schema_llm = create_model(product_schema_model)?;

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

        let classification_llm = create_model(url_classification_model)?;
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
        let mut website_config = config.spider_website_config();
        website_config.max_response_body_bytes = spider_max_size_bytes;
        let website_spider = Box::new(SpiderImpl::new(website_config));

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
        // Preserve the baseline execution path. 04e2 must replace this legacy task
        // boundary with retained run_until futures, signals, drain deadlines and fencing.
        let review_handle = tokio::spawn(async move { review_server.run().await });
        let cron_handle = tokio::spawn(async move {
            cron_job.run_loop().await;
        });

        tokio::select! {
            result = review_handle => {
                result.map_err(|error| DaemonError::ReviewTask(RedactedDaemonCause::new(error)))?
                    .map_err(|error| DaemonError::ReviewServer(RedactedDaemonCause::new(error)))?;
                Err(DaemonError::ReviewStopped)
            }
            result = cron_handle => {
                result.map_err(|error| DaemonError::CronTask(RedactedDaemonCause::new(error)))?;
                Err(DaemonError::CronStopped)
            }
        }
    }
    .instrument(tracing::info_span!("crawler_startup"))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_redact_provider_logging_and_task_errors_through_startup_source_chains() {
        const CANARY: &str = "private-provider-body-or-credential-path";
        let constructors: [fn(RedactedDaemonCause) -> DaemonError; 7] = [
            DaemonError::CloudWatchBootstrap,
            DaemonError::Logging,
            DaemonError::Credentials,
            DaemonError::VertexClient,
            DaemonError::ReviewServer,
            DaemonError::ReviewTask,
            DaemonError::CronTask,
        ];
        for constructor in constructors {
            let error = crate::StartupError::Daemon(constructor(RedactedDaemonCause::new(
                std::io::Error::other(CANARY),
            )));
            let mut node: Option<&dyn Error> = Some(&error);
            let mut depth = 0;
            while let Some(error) = node {
                assert!(!format!("{error} {error:?} {error:#?}").contains(CANARY));
                node = error.source();
                depth += 1;
            }
            assert!(depth >= 2);
        }
    }
}
