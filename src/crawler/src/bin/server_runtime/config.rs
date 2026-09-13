use crawler::llm_runtime::CrawlerLlmRateLimitConfig;
use crawler::local_db::ServerDatabaseConfig;
use crawler::logging::{CloudWatchLoggingConfig, HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE};
use crawler::review::server::ReviewServerConfig;
use crawler::scraper::scraper_service::DEFAULT_SCHEMA_SEED_PAGES;
use crawler::service::cron::CrawlerCronConfig;
use crawler::vertex_ai::CrawlerVertexAiModels;
use large_language_model::VertexAiConfig;
use platform_postgres::PostgresPoolConfigError;
use std::collections::BTreeMap;
use std::env::VarError;
use std::ffi::OsString;
use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Daemon,
    CheckConfig,
    Help,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(super) enum ArgumentError {
    #[error("arguments must be Unicode; use server --help")]
    NonUnicode,
    #[error("expected no arguments, exactly --check-config, or exactly --help")]
    Invalid,
}

impl Mode {
    pub(super) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, ArgumentError> {
        let Some(first) = args.next() else {
            return Ok(Self::Daemon);
        };
        let first = first.to_str().ok_or(ArgumentError::NonUnicode)?;
        if let Some(extra) = args.next() {
            return Err(if extra.to_str().is_none() {
                ArgumentError::NonUnicode
            } else {
                ArgumentError::Invalid
            });
        }
        match first {
            "--check-config" => Ok(Self::CheckConfig),
            "--help" => Ok(Self::Help),
            _ => Err(ArgumentError::Invalid),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ConfigError {
    #[error("missing required configuration: {0}")]
    Missing(&'static str),
    #[error("empty configuration: {0}")]
    Empty(&'static str),
    #[error("non-Unicode configuration: {0}")]
    NonUnicode(&'static str),
    #[error("malformed configuration: {key}; {requirement}")]
    Malformed {
        key: &'static str,
        requirement: &'static str,
    },
    #[error("invalid PostgreSQL configuration: {0}")]
    Database(#[from] PostgresPoolConfigError),
}

fn malformed(key: &'static str, requirement: &'static str) -> ConfigError {
    ConfigError::Malformed { key, requirement }
}

// Snapshot even optional/overridden inputs: invalid Unicode never turns into a default.
const INPUTS: &[&str] = &[
    "LOCAL_DB_URL",
    "BUSINESS_DATABASE_URL",
    "STAGE",
    "POSTGRES_SSL_MODE",
    "POSTGRES_SSL_ROOT_CERT",
    "PGSSLCERT",
    "PGSSLKEY",
    "PGSSLROOTCERT",
    "PGOPTIONS",
    "SPIDER_MAX_SIZE_BYTES",
    "VERTEX_AI_PROJECT_ID",
    "VERTEX_AI_LOCATION",
    "VERTEX_AI_MODEL",
    "CRAWLER_VERTEX_AI_CHEAP_MODEL",
    "CRAWLER_VERTEX_AI_URL_CLASSIFICATION_MODEL",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "CRAWLER_LLM_MAX_CONCURRENT_REQUESTS",
    "CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS",
    "CRAWLER_SHUTDOWN_GRACE_SECONDS",
    "CRAWLER_STOP_TIMEOUT_SECONDS",
    "CRAWLER_STARTUP_TIMEOUT_SECONDS",
    "CRAWLER_OPERATIONS_BIND_ADDR",
    "CRAWLER_REVIEW_BIND_ADDR",
    "CRAWLER_REVIEW_AUTH_TOKEN",
    "CRAWLER_REVIEW_REQUIRED",
    "CRAWLER_REVIEW_URL_PATTERN_REQUIRED",
    "CRAWLER_CLOUDWATCH_LOG_GROUP",
    "CRAWLER_CLOUDWATCH_LOG_STREAM",
    "HOSTNAME",
    "COMPUTERNAME",
    "LOG_LEVEL",
];

struct Environment(BTreeMap<&'static str, String>);

impl Environment {
    fn read(
        mut get: impl FnMut(&'static str) -> Result<String, VarError>,
    ) -> Result<Self, ConfigError> {
        let mut values = BTreeMap::new();
        for &key in INPUTS {
            match get(key) {
                Ok(value) => {
                    values.insert(key, value);
                }
                Err(VarError::NotPresent) => {}
                Err(VarError::NotUnicode(_)) => return Err(ConfigError::NonUnicode(key)),
            }
        }
        Ok(Self(values))
    }

    fn optional(&self, key: &'static str) -> Result<Option<&str>, ConfigError> {
        match self.0.get(key) {
            Some(value) if value.trim().is_empty() => Err(ConfigError::Empty(key)),
            value => Ok(value.map(String::as_str)),
        }
    }

    fn required(&self, key: &'static str) -> Result<&str, ConfigError> {
        self.optional(key)?.ok_or(ConfigError::Missing(key))
    }

    fn number(
        &self,
        key: &'static str,
        default: u64,
        min: u64,
        max: u64,
    ) -> Result<u64, ConfigError> {
        self.optional(key)?
            // Match the existing usize/u64 parsers' optional leading plus sign.
            .map(|value| bounded_number(key, value.strip_prefix('+').unwrap_or(value), min, max))
            .unwrap_or(Ok(default))
    }

    fn boolean(&self, key: &'static str) -> Result<bool, ConfigError> {
        match self.optional(key)? {
            None | Some("false" | "FALSE" | "0" | "no" | "NO") => Ok(false),
            Some("true" | "TRUE" | "1" | "yes" | "YES") => Ok(true),
            Some(_) => Err(malformed(
                key,
                "expected true/false, TRUE/FALSE, 1/0, yes/no or YES/NO",
            )),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct LifecycleConfig {
    pub(super) shutdown_grace: Duration,
    pub(super) stop_timeout: Duration,
    pub(super) startup_timeout: Duration,
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            shutdown_grace: Duration::from_secs(300),
            stop_timeout: Duration::from_secs(330),
            startup_timeout: Duration::from_secs(60),
        }
    }
}

impl LifecycleConfig {
    fn from_environment(env: &Environment) -> Result<Self, ConfigError> {
        let seconds = |key, default, min| {
            env.optional(key)?
                .map(|value| bounded_number(key, value, min, 3600))
                .unwrap_or(Ok(default))
                .map(Duration::from_secs)
        };
        let minimum_grace = if matches!(env.required("STAGE")?, "local" | "ephemeral" | "test") {
            1
        } else {
            300
        };
        let shutdown_grace = seconds("CRAWLER_SHUTDOWN_GRACE_SECONDS", 300, minimum_grace)?;
        let stop_timeout = seconds("CRAWLER_STOP_TIMEOUT_SECONDS", 330, 1)?;
        if stop_timeout < shutdown_grace + Duration::from_secs(30) {
            return Err(malformed(
                "CRAWLER_STOP_TIMEOUT_SECONDS",
                "must be at least shutdown grace plus 30 seconds",
            ));
        }
        Ok(Self {
            shutdown_grace,
            stop_timeout,
            startup_timeout: seconds("CRAWLER_STARTUP_TIMEOUT_SECONDS", 60, 1)?,
        })
    }
}

pub(super) struct ServerConfig {
    pub(super) commit_sha: String,
    pub(super) lifecycle: LifecycleConfig,
    pub(super) operations_bind_addr: SocketAddr,
    pub(super) databases: ServerDatabaseConfig,
    pub(super) cron: CrawlerCronConfig,
    pub(super) spider_max_size_bytes: usize,
    pub(super) review: ReviewServerConfig,
    pub(super) review_required: bool,
    pub(super) url_pattern_review_required: bool,
    pub(super) vertex_ai_models: CrawlerVertexAiModels,
    pub(super) product_schema_model: VertexAiConfig,
    pub(super) url_classification_model: VertexAiConfig,
    pub(super) llm_rate_limit: CrawlerLlmRateLimitConfig,
    pub(super) cloudwatch: Option<CloudWatchLoggingConfig>,
    pub(super) log_filter: EnvFilter,
}

impl fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ServerConfig { inputs: <redacted> }")
    }
}

impl ServerConfig {
    pub(super) fn from_lookup(
        build_commit_sha: Option<&str>,
        get: impl FnMut(&'static str) -> Result<String, VarError>,
    ) -> Result<Self, ConfigError> {
        Self::from_lookup_with_lifecycle(build_commit_sha, get, |_| {})
    }

    pub(super) fn from_lookup_with_lifecycle(
        build_commit_sha: Option<&str>,
        get: impl FnMut(&'static str) -> Result<String, VarError>,
        configure_lifecycle: impl FnOnce(LifecycleConfig),
    ) -> Result<Self, ConfigError> {
        let commit_sha = validate_commit_sha(build_commit_sha)?.to_owned();
        let env = Environment::read(get)?;
        let lifecycle = LifecycleConfig::from_environment(&env)?;
        // The daemon must apply these validated budgets before shared TLS validation can
        // read a CA file synchronously. Preflight supplies a no-op and retains its own guard.
        configure_lifecycle(lifecycle);
        // Keep existing crawl, capture, and review defaults; no pacing/budget relaxation.
        let cron = CrawlerCronConfig {
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
        for key in [
            "LOCAL_DB_URL",
            "BUSINESS_DATABASE_URL",
            "STAGE",
            "POSTGRES_SSL_MODE",
        ] {
            env.required(key)?;
        }
        env.optional("POSTGRES_SSL_ROOT_CERT")?;
        let databases = ServerDatabaseConfig::from_lookup(
            cron.effective_db_max_connections(),
            cron.effective_business_db_max_connections(),
            |key| env.0.get(key).cloned(),
        )?;
        let spider_max_size_bytes = bounded_number(
            "SPIDER_MAX_SIZE_BYTES",
            env.required("SPIDER_MAX_SIZE_BYTES")?,
            1024 * 1024,
            8 * 1024 * 1024,
        )? as usize;
        let project = identifier(
            "VERTEX_AI_PROJECT_ID",
            env.required("VERTEX_AI_PROJECT_ID")?,
            false,
        )?;
        let location = identifier(
            "VERTEX_AI_LOCATION",
            env.required("VERTEX_AI_LOCATION")?,
            false,
        )?;
        let schema_model = identifier(
            "VERTEX_AI_MODEL",
            env.optional("VERTEX_AI_MODEL")?
                .unwrap_or("gemini-3.1-pro-preview"),
            true,
        )?;
        let cheap_model = identifier(
            "CRAWLER_VERTEX_AI_CHEAP_MODEL",
            env.optional("CRAWLER_VERTEX_AI_CHEAP_MODEL")?
                .unwrap_or("gemini-3.1-flash-lite"),
            true,
        )?;
        let classification_model = identifier(
            "CRAWLER_VERTEX_AI_URL_CLASSIFICATION_MODEL",
            env.optional("CRAWLER_VERTEX_AI_URL_CLASSIFICATION_MODEL")?
                .unwrap_or(cheap_model),
            true,
        )?;
        // A path is configuration only. Do not open it or invoke ADC discovery in preflight.
        if let Some(path) = env.optional("GOOGLE_APPLICATION_CREDENTIALS")?
            && (path.len() > 4096 || path.chars().any(char::is_control))
        {
            return Err(malformed(
                "GOOGLE_APPLICATION_CREDENTIALS",
                "expected a nonempty path without control characters",
            ));
        }
        let defaults = CrawlerLlmRateLimitConfig::default();
        let llm_rate_limit = CrawlerLlmRateLimitConfig {
            // Preserve positive usize inputs except those that panic Semaphore::new.
            max_concurrent_requests: env.number(
                "CRAWLER_LLM_MAX_CONCURRENT_REQUESTS",
                defaults.max_concurrent_requests as u64,
                1,
                tokio::sync::Semaphore::MAX_PERMITS as u64,
            )? as usize,
            min_request_interval: Duration::from_millis(env.number(
                "CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS",
                defaults.min_request_interval.as_millis() as u64,
                1,
                u64::MAX,
            )?),
        };
        let bind_addr: SocketAddr = env
            .optional("CRAWLER_REVIEW_BIND_ADDR")?
            .unwrap_or("127.0.0.1:7878")
            .parse()
            .map_err(|_| malformed("CRAWLER_REVIEW_BIND_ADDR", "expected an IP socket address"))?;
        if bind_addr.port() == 0 {
            return Err(malformed(
                "CRAWLER_REVIEW_BIND_ADDR",
                "port must be nonzero",
            ));
        }
        let auth_token = env
            .optional("CRAWLER_REVIEW_AUTH_TOKEN")?
            .map(str::to_owned);
        if auth_token
            .as_ref()
            .is_some_and(|token| token.len() > 4096 || !token.bytes().all(|b| b.is_ascii_graphic()))
        {
            return Err(malformed(
                "CRAWLER_REVIEW_AUTH_TOKEN",
                "expected at most 4096 visible ASCII bytes without whitespace",
            ));
        }
        let review = ReviewServerConfig {
            bind_addr,
            auth_token,
        }
        .validate()
        .map_err(|_| {
            ConfigError::Missing("CRAWLER_REVIEW_AUTH_TOKEN (required for non-loopback review)")
        })?;
        let operations_bind_addr: SocketAddr = env
            .optional("CRAWLER_OPERATIONS_BIND_ADDR")?
            .unwrap_or("127.0.0.1:9083")
            .parse()
            .map_err(|_| {
                malformed(
                    "CRAWLER_OPERATIONS_BIND_ADDR",
                    "expected an IP socket address",
                )
            })?;
        if !operations_bind_addr.ip().is_loopback()
            || operations_bind_addr.port() == 0
            || operations_bind_addr.port() == bind_addr.port()
        {
            return Err(malformed(
                "CRAWLER_OPERATIONS_BIND_ADDR",
                "must be loopback, nonzero, and use a different port from review",
            ));
        }
        let review_required = env.boolean("CRAWLER_REVIEW_REQUIRED")?;
        let url_pattern_review_required = env.boolean("CRAWLER_REVIEW_URL_PATTERN_REQUIRED")?;
        let cloudwatch = cloudwatch_config(&env)?;
        let log_level = env.optional("LOG_LEVEL")?.unwrap_or("info");
        if log_level.len() > 4096 {
            return Err(malformed("LOG_LEVEL", "filter exceeds 4096 bytes"));
        }
        let log_filter = EnvFilter::builder().with_regex(false).parse(format!(
            "{log_level},spider=warn,sqlx::postgres::notice=warn,{HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE}"
        )).map_err(|_| malformed("LOG_LEVEL", "expected a valid tracing filter"))?;
        Ok(Self {
            commit_sha,
            lifecycle,
            operations_bind_addr,
            databases,
            cron,
            spider_max_size_bytes,
            review,
            review_required,
            url_pattern_review_required,
            vertex_ai_models: CrawlerVertexAiModels {
                product_schema: schema_model.to_owned(),
                url_classification: classification_model.to_owned(),
            },
            product_schema_model: VertexAiConfig::new(project, location, schema_model),
            url_classification_model: VertexAiConfig::new(project, location, classification_model),
            llm_rate_limit,
            cloudwatch,
            log_filter,
        })
    }
}

fn validate_commit_sha(value: Option<&str>) -> Result<&str, ConfigError> {
    let value = value.ok_or(ConfigError::Missing("build-time COMMIT_SHA"))?;
    if value.trim().is_empty() {
        return Err(ConfigError::Empty("build-time COMMIT_SHA"));
    }
    if value.len() != 40
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(malformed(
            "build-time COMMIT_SHA",
            "expected exactly 40 lowercase hexadecimal characters",
        ));
    }
    Ok(value)
}

fn bounded_number(key: &'static str, value: &str, min: u64, max: u64) -> Result<u64, ConfigError> {
    let invalid = || {
        malformed(
            key,
            "expected an unsigned decimal integer within the documented bounds",
        )
    };
    if !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let number = value.parse::<u64>().map_err(|_| invalid())?;
    if !(min..=max).contains(&number) {
        return Err(invalid());
    }
    Ok(number)
}

fn identifier<'a>(key: &'static str, value: &'a str, model: bool) -> Result<&'a str, ConfigError> {
    if value.len() > 128
        || !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b) || (model && b == b'@'))
    {
        return Err(malformed(
            key,
            "expected a bounded Vertex identifier, not a resource path or URL",
        ));
    }
    Ok(value)
}

fn cloudwatch_config(env: &Environment) -> Result<Option<CloudWatchLoggingConfig>, ConfigError> {
    let group = env.optional("CRAWLER_CLOUDWATCH_LOG_GROUP")?.map(str::trim);
    let stream = env
        .optional("CRAWLER_CLOUDWATCH_LOG_STREAM")?
        .map(str::trim);
    let hostname = env.optional("HOSTNAME")?;
    let computername = env.optional("COMPUTERNAME")?;
    if let Some(group) = group
        && (group.len() > 512
            || group.starts_with("aws/")
            || !group
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-/#".contains(&b)))
    {
        return Err(malformed(
            "CRAWLER_CLOUDWATCH_LOG_GROUP",
            "expected a valid CloudWatch log group name",
        ));
    }
    for (key, value) in [
        ("CRAWLER_CLOUDWATCH_LOG_STREAM", stream),
        ("HOSTNAME", hostname),
        ("COMPUTERNAME", computername),
    ] {
        if let Some(value) = value
            && (value.chars().count() > 512 || value.contains([':', '*']))
        {
            return Err(malformed(
                key,
                "expected a valid CloudWatch stream/host name",
            ));
        }
    }
    Ok(group.map(|group| CloudWatchLoggingConfig {
        log_group_name: group.to_owned(),
        log_stream_name: stream
            .or(hostname)
            .or(computername)
            .unwrap_or("unknown-host")
            .to_owned(),
    }))
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
