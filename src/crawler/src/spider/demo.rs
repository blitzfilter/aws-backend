//! Demo binary - showcases end-to-end usage of [`SpiderService`].
//!
//! Uses a hardcoded local Postgres database (`crawler_demo_spider`). Bootstrap with:
//!
//! ```powershell
//! # from src/crawler/
//! .\db-up.ps1
//! .\db-migrate.ps1
//! ```
//!
//! # Configuration
//!
//! | Env var          | Purpose                 | Default                         |
//! |------------------|-----------------------  |---------------------------------|
//! | `LOCAL_DB_URL`   | Hardcoded local DB URL | `.../crawler_demo_spider`      |
//! | `VERTEX_AI_PROJECT_ID` | Google Cloud project for Vertex AI | *(required)* |
//! | `VERTEX_AI_LOCATION` | Vertex AI location | *(required)* |
//! | `GOOGLE_APPLICATION_CREDENTIALS` | Optional local Application Default Credentials file | unset |
//! | `VERTEX_AI_MODEL` | Default model | `gemini-3.1-pro-preview` |
//! | `CRAWLER_VERTEX_AI_CHEAP_MODEL` | Default low-risk crawler LLM model | `gemini-3.1-flash-lite` |
//! | `CRAWLER_VERTEX_AI_URL_CLASSIFICATION_MODEL` | Optional URL classification model override | `CRAWLER_VERTEX_AI_CHEAP_MODEL` |
//! | `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS` | Max in-flight crawler LLM calls | `1` |
//! | `CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS` | Minimum delay between LLM request starts | `2000` |
//! | `LOG_LEVEL`      | Log level for this demo | `info`                          |
//!
//! # Running
//!
//! ```powershell
//! gcloud auth application-default login
//! $env:VERTEX_AI_PROJECT_ID="my-project"
//! $env:VERTEX_AI_LOCATION="europe-west3"
//! cargo run --bin demo-spider -p crawler -- https://www.christies.com/en
//! ```

use crawler::CrawlerDomainId;
use listing_source_core::ListingSourceId;
use std::env;
use std::fs::File;
use std::io::BufWriter;
use std::sync::Arc;

use crawler::llm_runtime::{CrawlerLlmGovernor, CrawlerLlmRateLimitConfig};
use crawler::local_db::{
    DEMO_SPIDER_DB_NAME, bootstrap_local_database,
    crawler_domain_configuration_repository::CrawlerDomainConfigurationRepositoryImpl,
    demo_spider_db_url,
};
use crawler::logging::HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE;
use crawler::service::crawler_domain_configuration::{
    CrawlerDomainConfigurationError, CrawlerDomainConfigurationRepository,
};
use crawler::spider::SpiderRunResult;
use crawler::spider::classification::url_classification_service::UrlClassificationServiceImpl;
use crawler::spider::classification::url_metadata_repository::UrlMetadataRepositoryImpl;
use crawler::spider::classification::url_pattern_repository::ListingSourceUrlPatternRepositoryImpl;
use crawler::spider::classification::url_pattern_service::UrlPatternServiceError;
use crawler::spider::classification::url_pattern_service::UrlPatternServiceImpl;
use crawler::spider::discovery::website_spider::SpiderDiscoveryError;
use crawler::spider::discovery::website_spider::SpiderImpl;
use crawler::spider::service::{SpiderService, SpiderServiceConfig, SpiderServiceImpl};
use crawler::vertex_ai::{CrawlerVertexAiConfig, CrawlerVertexAiModels};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use thiserror::Error;
use tracing::{Instrument, error, info};

#[derive(Debug, Error)]
enum DemoError {
    #[error("Demo error: {0}")]
    Demo(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Database(#[from] sqlx::Error),

    #[error(transparent)]
    Discovery(#[from] SpiderDiscoveryError),

    #[error(transparent)]
    UrlPattern(#[from] UrlPatternServiceError),

    #[error(transparent)]
    SpiderService(#[from] crawler::spider::SpiderServiceError),

    #[error(transparent)]
    DomainConfiguration(#[from] CrawlerDomainConfigurationError),
}

const DEFAULT_CRAWL_ROOT_URL: &str = "https://www.christies.com/en";
const DEFAULT_CLASSIFY_THRESHOLD: usize = 200;
/// Spider demo pool size: 1 advisory-lock connection + 4 query connections.
const DEMO_POOL_MAX_CONNECTIONS: u32 = 5;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    init_logging();

    let crawl_root_url = read_crawl_root_url();

    async {
        let vertex_ai_config = match CrawlerVertexAiConfig::from_env() {
            Ok(config) => config,
            Err(error) => {
                error!(%error, "Failed to load Vertex AI configuration");
                return;
            }
        };
        let vertex_ai_models = CrawlerVertexAiModels::from_env();

        let pool = match connect_and_migrate().await {
            Ok(p) => p,
            Err(error) => {
                error!(error = ?error, "Failed to connect to Postgres");
                return;
            }
        };

        let pattern_repository = build_pattern_repository(pool.clone());
        let url_repository = build_url_repository(pool.clone());

        info!(
            llm_provider = "vertex_ai",
            url_classification_model = %vertex_ai_models.url_classification,
            "Crawler spider demo Vertex AI configuration resolved"
        );
        let llm_governor = Arc::new(CrawlerLlmGovernor::new(
            CrawlerLlmRateLimitConfig::from_env(),
        ));
        let classification_llm =
            match vertex_ai_config.create_model(vertex_ai_models.url_classification.clone()) {
                Ok(model) => model,
                Err(error) => {
                    error!(%error, "Failed to initialize Vertex AI model for URL classification");
                    return;
                }
            };
        let classification_service = Box::new(UrlClassificationServiceImpl::new(
            classification_llm,
            Some(llm_governor),
        ));
        let pattern_service = Box::new(UrlPatternServiceImpl::new(
            pattern_repository.clone(),
            classification_service,
        ));

        let spider = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(SpiderImpl::default()),
            pattern_service,
            url_repository,
        );

        let listing_source_id: ListingSourceId = "ls_01h455vb4pex5ty7enb1p677vg"
            .parse()
            .unwrap_or_else(|error| panic!("invalid demo ListingSource TypeID: {error}"));
        let crawl_root_url_parsed = url::Url::parse(&crawl_root_url)
            .unwrap_or_else(|_| url::Url::parse("https://demo.invalid").unwrap());
        let demo_domain = crawl_root_url_parsed
            .host_str()
            .unwrap_or("demo.invalid")
            .to_string();

        let demo_domain_id =
            match insert_demo_listing_source_with_domain(&pool, &listing_source_id, &demo_domain)
                .await
            {
                Ok(id) => id,
                Err(error) => {
                    error!(error = ?error, "Failed to insert demo ListingSource rows into DB");
                    return;
                }
            };

        match spider
            .run(
                &listing_source_id,
                &demo_domain_id,
                &crawl_root_url,
                DEFAULT_CLASSIFY_THRESHOLD,
            )
            .await
        {
            Ok(result) => {
                info!(
                    linkCount = result.total_links,
                    productCount = result.product_urls_count,
                    "Spider run finished successfully"
                );
                if let Err(error) = write_output(&result) {
                    error!(error = ?error, "Failed to write demo output file");
                } else {
                    info!("Output written to 'spider_output.json'");
                }
            }
            Err(error) => {
                error!(error = ?error, "Spider run failed");
            }
        }
    }
    .instrument(tracing::info_span!(
        "crawler_spider_demo",
        entrypoint = "demo-spider",
        crawl_root_url = %crawl_root_url,
        classify_threshold = DEFAULT_CLASSIFY_THRESHOLD
    ))
    .await;
}

#[tracing::instrument]
fn read_crawl_root_url() -> String {
    let raw_url = env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_CRAWL_ROOT_URL.to_string());

    ensure_scheme(&raw_url)
}

#[tracing::instrument(skip(pool))]
fn build_pattern_repository(pool: PgPool) -> Arc<ListingSourceUrlPatternRepositoryImpl> {
    Arc::new(ListingSourceUrlPatternRepositoryImpl::new(pool))
}

#[tracing::instrument(skip(pool))]
fn build_url_repository(pool: PgPool) -> Arc<UrlMetadataRepositoryImpl> {
    Arc::new(UrlMetadataRepositoryImpl::new(pool))
}

/// Connects to local Postgres and applies pending migrations.
#[tracing::instrument]
async fn connect_and_migrate() -> Result<PgPool, DemoError> {
    bootstrap_local_database(DEMO_SPIDER_DB_NAME)
        .await
        .map_err(DemoError::Demo)?;
    let db_url = demo_spider_db_url();

    let pool = PgPoolOptions::new()
        .max_connections(DEMO_POOL_MAX_CONNECTIONS)
        .connect(&db_url)
        .await?;

    info!(
        max_connections = DEMO_POOL_MAX_CONNECTIONS,
        "Connected to Postgres"
    );

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|e| DemoError::Database(sqlx::Error::from(e)))?;

    info!("Database migrations applied successfully");

    Ok(pool)
}

/// Inserts a stable demo ListingSource and registers its crawler domain through the
/// same ownership boundary used by the production review API.
#[tracing::instrument(skip(pool), fields(listing_source_id = %listing_source_id, listing_source_domain = %listing_source_domain))]
async fn insert_demo_listing_source_with_domain(
    pool: &PgPool,
    listing_source_id: &ListingSourceId,
    listing_source_domain: &str,
) -> Result<CrawlerDomainId, DemoError> {
    let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();

    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated)
         VALUES ($1, 'Demo source', 'demo-source', TRUE, NOW(), NOW())
         ON CONFLICT (listing_source_id) DO NOTHING",
    )
    .bind(listing_source_id_uuid)
    .execute(pool)
    .await?;

    let domain = listing_source_core::Domain::try_from(listing_source_domain)
        .map_err(|error| DemoError::Demo(error.to_string()))?;
    let configuration = CrawlerDomainConfigurationRepositoryImpl::new(pool.clone());
    let domain_id = configuration
        .register(*listing_source_id, domain)
        .await?
        .domain_id;

    Ok(domain_id)
}

#[tracing::instrument(skip(url), fields(raw_url = %url))]
fn ensure_scheme(url: &str) -> String {
    let trimmed = url.trim();

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

fn init_logging() {
    let raw_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

    let filter = tracing_subscriber::EnvFilter::new(format!(
        "{raw_level},spider=warn,sqlx::postgres::notice=warn,{HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE}"
    ));

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .init();
}

#[tracing::instrument(skip(result), fields(total_links = result.total_links, product_urls_count = result.product_urls_count
))]
fn write_output(result: &SpiderRunResult) -> Result<(), std::io::Error> {
    let file = File::create("spider_output.json")?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, result)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(())
}
