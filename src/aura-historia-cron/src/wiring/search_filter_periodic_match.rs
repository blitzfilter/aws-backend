use crate::scheduled_job::CronJob;
use chrono::Utc;
use cron_tab::Cron;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use google_cloud_auth::credentials::Builder as GoogleCredentialsBuilder;
use large_language_model::{VertexAiConfig, VertexAiGemini};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use platform_postgres::{
    PostgresConnectError, PostgresPoolConfig, PostgresPoolConfigError, SqlxUnitOfWork,
};
use product_listing_opensearch::OpenSearchProductListingSearchReader;
use product_listing_postgres::{
    SqlxProductListingCurrentEventGuardFactory,
    SqlxProductListingSearchFilterMatchSourceReaderFactory,
};
use search_filter_postgres::{
    SqlxExistingSearchFilterMatchReader, SqlxPeriodicSearchFilterCandidateReader,
    SqlxPeriodicSearchFilterMatchingRunLock, SqlxPeriodicSearchFilterProgressFactory,
    SqlxSearchFilterMatchWriterFactory,
};
use search_filter_service::use_cases::{
    PeriodicSearchFilterMatchingPolicy, RunPeriodicSearchFilterMatchingHandler,
    RunPeriodicSearchFilterMatchingUseCase,
};
use std::{
    num::{NonZeroU64, NonZeroUsize},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const MAX_HYBRID_SCAN_LIMIT: usize = 100;

pub async fn build_from_env() -> Result<(Arc<dyn CronJob>, String, Duration), WiringError> {
    let config = PeriodicMatchConfig::from_env()?;
    let pool = config
        .postgres
        .connect()
        .await
        .map_err(WiringError::Postgres)?;
    let client = opensearch_client(&config)?;
    let credentials = GoogleCredentialsBuilder::default()
        .with_scopes([GOOGLE_CLOUD_PLATFORM_SCOPE])
        .build_access_token_credentials()
        .map_err(|error| WiringError::VertexCredentials {
            detail: error.to_string(),
        })?;
    let evaluator = VertexAiGemini::new(
        VertexAiConfig::new(
            config.vertex_project_id,
            config.vertex_location,
            config.vertex_model,
        ),
        credentials,
    )
    .map_err(WiringError::VertexClient)?;
    let handler: Arc<dyn RunPeriodicSearchFilterMatchingUseCase> = Arc::new(
        RunPeriodicSearchFilterMatchingHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxPeriodicSearchFilterMatchingRunLock::new(config.postgres),
            SqlxPeriodicSearchFilterCandidateReader::new(pool.clone()),
            SqlxFxRateSnapshotRepositoryFactory,
            OpenSearchProductListingSearchReader::new(client),
            SqlxExistingSearchFilterMatchReader::new(pool),
            SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
            evaluator,
            SqlxProductListingCurrentEventGuardFactory::new(),
            SqlxSearchFilterMatchWriterFactory,
            SqlxPeriodicSearchFilterProgressFactory,
            config.policy,
        )
        .map_err(WiringError::Handler)?,
    );
    Ok((
        Arc::new(crate::jobs::SearchFilterPeriodicMatchJob::new(handler)),
        config.schedule,
        config.max_run_duration,
    ))
}

struct PeriodicMatchConfig {
    postgres: PostgresPoolConfig,
    endpoint: url::Url,
    auth: Option<(String, String)>,
    vertex_project_id: String,
    vertex_location: String,
    vertex_model: String,
    schedule: String,
    max_run_duration: Duration,
    policy: PeriodicSearchFilterMatchingPolicy,
}
impl PeriodicMatchConfig {
    fn from_env() -> Result<Self, WiringError> {
        let stage = std::env::var("STAGE")
            .ok()
            .map(|value| value.trim().to_owned());
        let filter_page_size = nonzero("PERIODIC_MATCH_FILTER_PAGE_SIZE", 100)?;
        let hybrid_scan_limit = nonzero("PERIODIC_MATCH_HYBRID_SCAN_LIMIT", 100)?;
        let evaluation_limit = nonzero("PERIODIC_MATCH_EVALUATION_LIMIT", 50)?;
        let llm_concurrency = nonzero("PERIODIC_MATCH_LLM_CONCURRENCY", 8)?;
        let max_attempts = nonzero("PERIODIC_MATCH_MAX_ATTEMPTS", 3)?;
        if hybrid_scan_limit.get() > MAX_HYBRID_SCAN_LIMIT
            || evaluation_limit > hybrid_scan_limit
            || max_attempts.get() > 10
        {
            return Err(WiringError::InvalidPolicy);
        }
        let endpoint_raw = required("OPENSEARCH_ENDPOINT_URL")?;
        let endpoint = url::Url::parse(&endpoint_raw).map_err(WiringError::OpenSearchUrl)?;
        let auth = if matches!(stage.as_deref(), Some("local" | "test" | "ephemeral")) {
            None
        } else {
            Some((
                required("OPENSEARCH_USERNAME")?,
                required("OPENSEARCH_PASSWORD")?,
            ))
        };
        let postgres = postgres_config(&mut |name| match std::env::var(name) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            // Preserve presence so malformed optional inputs cannot fall back to defaults.
            Err(std::env::VarError::NotUnicode(_)) => Some(String::new()),
        })?;
        let schedule = optional("SEARCH_FILTER_PERIODIC_MATCH_CRON", "0 0 15 * * * *");
        validate_schedule(&schedule)?;
        Ok(Self {
            postgres,
            endpoint,
            auth,
            vertex_project_id: required("VERTEX_AI_PROJECT_ID")?,
            vertex_location: required("VERTEX_AI_LOCATION")?,
            vertex_model: required("VERTEX_AI_MODEL")?,
            schedule,
            max_run_duration: positive_duration("PERIODIC_MATCH_MAX_RUN_SECONDS", 7200)?,
            policy: PeriodicSearchFilterMatchingPolicy {
                filter_page_size,
                hybrid_scan_limit,
                evaluation_limit,
                llm_concurrency,
                max_attempts,
                projection_lag: periodic_duration(
                    "PERIODIC_MATCH_PROJECTION_LAG_SECONDS",
                    number::<u64>("PERIODIC_MATCH_PROJECTION_LAG_SECONDS", 900)?,
                )?,
                replay_overlap: periodic_duration(
                    "PERIODIC_MATCH_REPLAY_OVERLAP_SECONDS",
                    number::<u64>("PERIODIC_MATCH_REPLAY_OVERLAP_SECONDS", 7200)?,
                )?,
            },
        })
    }
}
fn postgres_config(
    get: &mut impl FnMut(&'static str) -> Option<String>,
) -> Result<PostgresPoolConfig, WiringError> {
    PostgresPoolConfig::from_lookup("aura-historia-cron", get).map_err(WiringError::PostgresConfig)
}

#[cfg(test)]
#[path = "postgres_config_tests.rs"]
mod postgres_config_tests;

fn required(name: &'static str) -> Result<String, WiringError> {
    std::env::var(name)
        .ok()
        .and_then(trimmed_non_empty)
        .ok_or(WiringError::MissingEnv { name })
}

fn optional(name: &'static str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .and_then(trimmed_non_empty)
        .unwrap_or_else(|| default.to_owned())
}

fn trimmed_non_empty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn number<T>(name: &'static str, default: T) -> Result<T, WiringError>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    std::env::var(name)
        .ok()
        .map(|value| parse_number(name, &value))
        .unwrap_or(Ok(default))
}

fn parse_number<T>(name: &'static str, value: &str) -> Result<T, WiringError>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value
        .trim()
        .parse()
        .map_err(|source| WiringError::InvalidNumber {
            name,
            source: Box::new(source),
        })
}
fn nonzero(name: &'static str, default: usize) -> Result<NonZeroUsize, WiringError> {
    NonZeroUsize::new(number(name, default)?).ok_or(WiringError::InvalidPolicy)
}

fn periodic_duration(name: &'static str, seconds: u64) -> Result<time::Duration, WiringError> {
    let seconds = i64::try_from(seconds).map_err(|source| WiringError::InvalidNumber {
        name,
        source: Box::new(source),
    })?;
    Ok(time::Duration::seconds(seconds))
}

fn positive_duration(name: &'static str, default: u64) -> Result<Duration, WiringError> {
    let seconds = NonZeroU64::new(number(name, default)?).ok_or(WiringError::InvalidPolicy)?;
    Ok(Duration::from_secs(seconds.get()))
}

fn validate_schedule(schedule: &str) -> Result<(), WiringError> {
    let mut cron = Cron::new(Utc);
    cron.add_fn(schedule, || {})
        .map(|_| ())
        .map_err(|error| WiringError::InvalidSchedule {
            value: schedule.to_owned(),
            detail: error.to_string(),
        })
}
fn opensearch_client(config: &PeriodicMatchConfig) -> Result<OpenSearch, WiringError> {
    let pool = SingleNodeConnectionPool::new(config.endpoint.clone());
    let builder = TransportBuilder::new(pool);
    let builder = match &config.auth {
        Some((username, password)) => {
            builder.auth(Credentials::Basic(username.to_owned(), password.to_owned()))
        }
        None => builder,
    };
    Ok(OpenSearch::new(builder.build().map_err(|error| {
        WiringError::OpenSearch {
            detail: error.to_string(),
        }
    })?))
}
#[derive(Debug, thiserror::Error)]
pub enum WiringError {
    #[error("missing required environment variable {name}")]
    MissingEnv { name: &'static str },
    #[error("invalid numeric environment variable {name}")]
    InvalidNumber {
        name: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("invalid periodic matching policy")]
    InvalidPolicy,
    #[error("invalid SEARCH_FILTER_PERIODIC_MATCH_CRON {value}: {detail}")]
    InvalidSchedule { value: String, detail: String },
    #[error("invalid PostgreSQL configuration")]
    PostgresConfig(#[source] PostgresPoolConfigError),
    #[error("invalid OpenSearch endpoint")]
    OpenSearchUrl(#[source] url::ParseError),
    #[error("failed to connect to PostgreSQL")]
    Postgres(#[source] PostgresConnectError),
    #[error("failed to configure OpenSearch: {detail}")]
    OpenSearch { detail: String },
    #[error("failed to initialize Vertex AI credentials: {detail}")]
    VertexCredentials { detail: String },
    #[error("failed to build Vertex AI client")]
    VertexClient(#[source] reqwest::Error),
    #[error("failed to build periodic matching handler")]
    Handler(#[source] search_filter_service::use_cases::RunPeriodicSearchFilterMatchingError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_zero_periodic_match_max_run_seconds() {
        let result = positive_duration("PERIODIC_MATCH_MAX_RUN_SECONDS", 0);
        assert!(matches!(result, Err(WiringError::InvalidPolicy)));
    }

    #[test]
    fn should_reject_invalid_periodic_match_cron() {
        let result = validate_schedule("invalid");
        assert!(matches!(result, Err(WiringError::InvalidSchedule { .. })));
    }

    #[test]
    fn should_accept_valid_seven_field_periodic_match_cron() {
        assert!(validate_schedule("0 0 15 * * * *").is_ok());
    }

    #[test]
    fn should_trim_string_and_numeric_wiring_inputs() {
        assert_eq!(
            trimmed_non_empty("  value  ".to_owned()),
            Some("value".to_owned())
        );
        assert!(matches!(
            parse_number::<u16>("PERIODIC_MATCH_MAX_ATTEMPTS", " 3 "),
            Ok(3)
        ));
    }

    #[test]
    fn should_reject_numeric_values_outside_the_target_type() {
        assert!(matches!(
            parse_number::<u16>("PERIODIC_MATCH_MAX_ATTEMPTS", "65536"),
            Err(WiringError::InvalidNumber { .. })
        ));
        assert!(matches!(
            parse_number::<u64>("PERIODIC_MATCH_PROJECTION_LAG_SECONDS", "-1"),
            Err(WiringError::InvalidNumber { .. })
        ));
        assert!(matches!(
            periodic_duration("PERIODIC_MATCH_PROJECTION_LAG_SECONDS", u64::MAX),
            Err(WiringError::InvalidNumber { .. })
        ));
    }
}
