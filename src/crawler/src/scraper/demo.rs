//! Demo binary — showcases end-to-end usage of [`ScraperService`].
//!
//! Uses a fixed local Postgres database (`crawler_demo_scraper`). Demo startup bootstraps
//! Docker and migrations only with explicit `STAGE=local`, `ephemeral`, or `test`.
//! Bundled Docker needs `POSTGRES_SSL_MODE=disable`. No production bootstrap.
//!
//! # What it does
//!
//! 1. Initialises structured info logging.
//! 2. Connects to local Postgres and applies pending migrations.
//! 3. Wires up all real service implementations:
//!    - [`ProductListingNormalizationServiceImpl`]
//!    - [`ProductListingSchemaServiceImpl`]
//!    - [`ScraperServiceImpl`] (backed by a real [`reqwest::Client`])
//! 4. Iterates over the placeholder targets below and writes results to JSON.
//!
//! # Configuration
//!
//! | Env var          | Purpose                              | Default                         |
//! |------------------|--------------------------------------|--------------------|
//! | `STAGE` | Required non-real stage | no default |
//! | `POSTGRES_SSL_MODE` | Explicit shared PostgreSQL TLS mode | no default |
//! | `VERTEX_AI_PROJECT_ID` | Google Cloud project for Vertex AI | *(required)* |
//! | `VERTEX_AI_LOCATION` | Vertex AI location | *(required)* |
//! | `GOOGLE_APPLICATION_CREDENTIALS` | Optional local Application Default Credentials file | unset |
//! | `VERTEX_AI_MODEL` | Schema generation/repair model | `gemini-3.1-pro-preview` |
//! | `CRAWLER_VERTEX_AI_CHEAP_MODEL` | Default low-risk crawler LLM model | `gemini-3.1-flash-lite` |
//! | `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS` | Max in-flight crawler LLM calls | `1` |
//! | `CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS` | Minimum delay between LLM request starts | `2000` |
//! | `LOG_LEVEL`      | Log level for `init_logging`         | `info`             |
//!
//! # Running
//!
//! ```powershell
//! $env:STAGE="local"
//! $env:POSTGRES_SSL_MODE="disable"
//! gcloud auth application-default login
//! $env:VERTEX_AI_PROJECT_ID="my-project"
//! $env:VERTEX_AI_LOCATION="europe-west3"
//! cargo run --bin demo-scraper -p crawler
//! ```

use listing_source_core::ListingSourceId;
use std::fs::File;
use std::io::BufWriter;
use std::sync::Arc;

use crawler::llm_runtime::{CrawlerLlmGovernor, CrawlerLlmRateLimitConfig};
use crawler::local_db::{
    DEMO_SCRAPER_DB_NAME, LocalDatabaseError, LocalDevelopmentConfig, bootstrap_local_database,
    migrate_local_database, parse_postgres_environment,
};
use crawler::logging::HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE;
use crawler::scraper::candidate_service::ScraperCandidateServiceImpl;
use crawler::scraper::css_selector::product_schema_repository::ListingSourceProductSchemaRepositoryImpl;
use crawler::scraper::css_selector::product_schema_service::ProductListingSchemaServiceImpl;
use crawler::scraper::css_selector::removed_page_schema_repository::RemovedPageSchemaRepositoryImpl;
use crawler::scraper::normalization::product_normalization_service::ProductListingNormalizationServiceImpl;
use crawler::scraper::scraper_service::{
    DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE, ReqwestHtmlFetcher, ScraperService,
    ScraperServiceImpl,
};
use crawler::vertex_ai::{CrawlerVertexAiConfig, CrawlerVertexAiModels};

use sqlx::PgPool;
use tracing::{Instrument, error, info};
use url::Url;

// ---------------------------------------------------------------------------
// Pool sizing
// ---------------------------------------------------------------------------

/// Scraper demo pool size: 5 connections cover all repository queries with room to spare.
const DEMO_POOL_MAX_CONNECTIONS: u32 = 5;

// ---------------------------------------------------------------------------
// Scrape targets — fill in your own ListingSource IDs and URLs below
// ---------------------------------------------------------------------------

struct ScrapeTarget {
    listing_source_id: ListingSourceId,
    url: &'static str,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), LocalDatabaseError> {
    dotenvy::dotenv().ok();
    let local = parse_postgres_environment(
        |key| std::env::var(key),
        |get| LocalDevelopmentConfig::from_lookup("crawler-demo-scraper", get),
    )?;

    let targets: &[ScrapeTarget] = &[
        ScrapeTarget {
            listing_source_id: "ls_01h455vb4pex5ty7enb1p677vg".try_into().unwrap(),
            url: "https://www.antiquitaeten-tuebingen.de/weichholzschrank-mit-orig-bemalung-salzburg-um-1800-art-7001/",
        },
        ScrapeTarget {
            listing_source_id: "ls_01h455vb4pex5ty7enb1p677vg".try_into().unwrap(),
            url: "https://www.antiquitaeten-tuebingen.de/bildnis-in-oel-der-gattin-von-samuel-de-la-roche1947-art-g1475/",
        },
        ScrapeTarget {
            listing_source_id: "ls_01h455vb4pex5ty7enb1p677vh".try_into().unwrap(),
            url: "https://20thcenturymilitaria.com/shop.php?code=51609",
        },
        ScrapeTarget {
            listing_source_id: "ls_01h455vb4pex5ty7enb1p677vh".try_into().unwrap(),
            url: "https://20thcenturymilitaria.com/shop.php?code=52012",
        },
        ScrapeTarget {
            listing_source_id: "ls_01h455vb4pex5ty7enb1p677vh".try_into().unwrap(),
            url: "https://20thcenturymilitaria.com/shop.php?code=52014",
        },
        ScrapeTarget {
            listing_source_id: "ls_01h455vb4pex5ty7enb1p677vg".try_into().unwrap(),
            url: "https://www.antiquitaeten-tuebingen.de/https-www-antiquitaeten-tuebingen-de-gemaelde-artnr-g-58-oelgemaelde-landschaftsmalerei-mitte-19-jh/",
        },
        ScrapeTarget {
            listing_source_id: "ls_01h455vb4pex5ty7enb1p677vj".try_into().unwrap(),
            url: "https://nostalgie-palast.de/couchtisch-uebersee-mit-glasplatte-113-m-x-053-m/",
        },
        ScrapeTarget {
            listing_source_id: "ls_01h455vb4pex5ty7enb1p677vk".try_into().unwrap(),
            url: "https://www.lot-tissimo.com/de-de/auction-catalogues/chiswick-auctions/catalogue-id-srchis11168/lot-61a5b754-6fc7-435b-80b3-b3fa0141c94e",
        },
    ];

    unsafe { std::env::set_var("LOG_LEVEL", "info") };
    init_logging();

    async {
        let pool = match connect_and_migrate(&local).await {
            Ok(pool) => pool,
            Err(error) => {
                error!(%error, "Failed to prepare demo PostgreSQL");
                return;
            }
        };
        let service = build_scraper_service(pool);

        let mut products: Vec<serde_json::Value> = vec![];
        for target in targets {
            let listing_source_id = target.listing_source_id;
            let url = match Url::parse(target.url) {
                Ok(u) => u,
                Err(e) => {
                    error!(
                        url = target.url,
                        error = %e,
                        "Invalid URL — skipping"
                    );
                    continue;
                }
            };

            let scrape_span = tracing::info_span!(
                "scraper_demo_target",
                listing_source_id = %listing_source_id,
                url = %url
            );
            match service
                .scrape(&listing_source_id, &url, None, None, None, None)
                .instrument(scrape_span)
                .await
            {
                Ok(Some(scraped)) => {
                    info!(
                        raw_input_sha256 = ?scraped.raw_input_sha256,
                        "Scrape succeeded; writing raw normalization input display"
                    );
                    products.push(serde_json::json!({
                        "action": scraped.raw_input.operation().as_str(),
                        "payloadFormat": scraped.raw_input.payload_format().as_str(),
                        "sourcePayload": scraped.raw_input.source_payload().value(),
                        "rawValues": scraped.raw_input.raw_values().value(),
                        "normalizationContext": scraped.raw_input.normalization_context().value(),
                    }));
                }
                Ok(None) => {
                    info!("Hash matched, skipped scraping");
                }
                Err(e) => {
                    error!(error = %e, "Scrape failed");
                }
            }
        }

        info!("Scraping complete, writing output to 'scraper_output.json'…");
        let file = File::create("scraper_output.json").unwrap();
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, &products).unwrap();
        info!("Output written to 'scraper_output.json'.");
    }
    .instrument(tracing::info_span!(
        "crawler_scraper_demo",
        entrypoint = "demo-scraper",
        database = DEMO_SCRAPER_DB_NAME,
        target_count = targets.len()
    ))
    .await;
    Ok(())
}

fn init_logging() {
    let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let filter = tracing_subscriber::EnvFilter::new(format!(
        "{log_level},{HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE}"
    ));

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

// ---------------------------------------------------------------------------
// Postgres helpers
// ---------------------------------------------------------------------------

/// Connects to local Postgres, applies pending migrations, and
/// returns a `'static` reference to a [`PgPool`].
///
/// The pool is intentionally leaked: the repositories hold `&'static PgPool`
/// references and must outlive the service, which lives until end of `main`.
#[tracing::instrument(skip_all)]
async fn connect_and_migrate(
    local: &LocalDevelopmentConfig,
) -> Result<&'static PgPool, LocalDatabaseError> {
    let database = local.pool_config(DEMO_SCRAPER_DB_NAME, DEMO_POOL_MAX_CONNECTIONS)?;
    bootstrap_local_database(local, DEMO_SCRAPER_DB_NAME).await?;
    let pool = database.connect().await?;

    info!(
        max_connections = DEMO_POOL_MAX_CONNECTIONS,
        "Connected to Postgres"
    );

    migrate_local_database(local, &pool).await?;

    info!("Database migrations applied successfully");

    // Leak the pool so it obtains a `'static` lifetime that can be shared
    // across all repository impls without fighting the borrow checker.
    Ok(Box::leak(Box::new(pool)))
}

// ---------------------------------------------------------------------------
// Service wiring
// ---------------------------------------------------------------------------

/// Constructs and returns a fully wired [`ScraperServiceImpl`].
///
/// Each LLM-backed service receives its own concrete model. The service implementations
/// remain generic over the provider-neutral `LargeLanguageModel` capability.
#[tracing::instrument(skip(pool))]
fn build_scraper_service(pool: &'static PgPool) -> ScraperServiceImpl {
    let vertex_ai_config = CrawlerVertexAiConfig::from_env()
        .expect("VERTEX_AI_PROJECT_ID and VERTEX_AI_LOCATION must be set");
    let vertex_ai_models = CrawlerVertexAiModels::from_env();

    info!(
        llm_provider = "vertex_ai",
        schema_model = %vertex_ai_models.product_schema,

        "Crawler scraper demo Vertex AI configuration resolved"
    );
    let llm_governor = Arc::new(CrawlerLlmGovernor::new(
        CrawlerLlmRateLimitConfig::from_env(),
    ));

    let create_schema_llm = vertex_ai_config
        .create_model(vertex_ai_models.product_schema.clone())
        .expect("failed to initialize Vertex AI model for schema generation");
    let single_schema_llm = vertex_ai_config
        .create_model(vertex_ai_models.product_schema.clone())
        .expect("failed to initialize Vertex AI model for fresh schema generation");
    // Pure deterministic normalization service.
    let normalization_svc = ProductListingNormalizationServiceImpl::new();

    // Schema service (DB-backed + initial/fresh LLM generation).
    let schema_repo = Box::new(ListingSourceProductSchemaRepositoryImpl::new(pool));
    let removed_page_schema_repo = Box::new(RemovedPageSchemaRepositoryImpl::new(pool));
    let schema_svc = ProductListingSchemaServiceImpl::new(
        create_schema_llm,
        single_schema_llm,
        schema_repo,
        Some(Arc::clone(&llm_governor)),
    );

    // HTTP fetcher using spider.
    let fetcher = Box::new(ReqwestHtmlFetcher::new());

    let candidate_service = Arc::new(ScraperCandidateServiceImpl::new(pool.clone()));

    ScraperServiceImpl::new_with_schema_seed_pages(
        fetcher,
        Box::new(schema_svc),
        Box::new(normalization_svc),
        candidate_service,
        3,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    )
    .with_removed_page_schema_repository(removed_page_schema_repo)
}
