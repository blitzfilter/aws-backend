pub mod cdc;
mod http;
pub mod jobs;
pub mod notification_delivery;
mod operations;
pub mod product_content_assessment;
pub mod product_embedding;
pub mod product_listing_opensearch;
pub mod product_listing_raw_normalization;
pub mod product_translation;
pub mod queue;

pub mod search_filter_match_notifications;
pub mod search_filter_percolator;
pub mod search_filter_projection;
pub mod watchlist_notifications;
mod wire;

use platform_postgres::{PostgresPoolConfig, PostgresPoolConfigError};
use std::future::Future;
use std::net::{AddrParseError, SocketAddr};
use std::num::ParseIntError;
use std::time::Duration;
#[cfg(test)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
#[cfg(test)]
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::info;

use crate::cdc::{
    CdcFanout, CdcIngestError, WorkerQueue, WorkerQueueReceivers, WorkerQueueRegistry,
};

pub const WORKER_HEALTH_BIND_ADDR_ENV: &str = "AURA_HISTORIA_WORKER_HEALTH_BIND_ADDR";
pub const WORKER_DRAIN_TIMEOUT_SECONDS_ENV: &str = "AURA_HISTORIA_WORKER_DRAIN_TIMEOUT_SECONDS";
pub const WORKER_STOP_TIMEOUT_SECONDS_ENV: &str = "AURA_HISTORIA_WORKER_STOP_TIMEOUT_SECONDS";
/// HTTP drain allowance shared with the binary's whole-runtime shutdown supervisor.
pub const WORKER_HTTP_DRAIN_TIMEOUT: Duration = http::CONNECTION_TIMEOUT;
pub const WORKER_SCOPE_ENV: &str = "AURA_HISTORIA_WORKER_SCOPE";
pub const WORKER_STAGE_ENV: &str = "STAGE";
pub const OPENSEARCH_ENDPOINT_URL_ENV: &str = "OPENSEARCH_ENDPOINT_URL";
pub const OPENSEARCH_USERNAME_ENV: &str = "OPENSEARCH_USERNAME";
pub const OPENSEARCH_PASSWORD_ENV: &str = "OPENSEARCH_PASSWORD";
pub const VERTEX_AI_PROJECT_ID_ENV: &str = "VERTEX_AI_PROJECT_ID";
pub const VERTEX_AI_LOCATION_ENV: &str = "VERTEX_AI_LOCATION";
pub const VERTEX_AI_MODEL_ENV: &str = "VERTEX_AI_MODEL";
pub const S3_BUCKET_NAME_TEMPLATES_ENV: &str = "S3_BUCKET_NAME_TEMPLATES";
pub const NOTIFICATION_EMAIL_FROM_ENV: &str = "NOTIFICATION_EMAIL_FROM";
pub const NOTIFICATION_EMAIL_REPLY_TO_ENV: &str = "NOTIFICATION_EMAIL_REPLY_TO";
pub const COMMIT_SHA_ENV: &str = "COMMIT_SHA";

const DEFAULT_WORKER_HEALTH_BIND_ADDR: &str = "0.0.0.0:8081";
const DEFAULT_WORKER_DRAIN_TIMEOUT_SECONDS: u64 = operations::DEFAULT_DRAIN_SECONDS;
const DEFAULT_LOCAL_WORKER_SCOPE: &str = "search-filter-projection";

pub const SEQUIN_CDC_PATH: &str = "/cdc/sequin";

pub trait WorkerJob: Send + 'static {}

impl<T> WorkerJob for T where T: Send + 'static {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum WorkerScope {
    SearchFilterProjection,
    SearchFilterPercolator,
    SearchFilterMatchNotification,
    WatchlistNotification,
    ProductListingContentAssessment,
    ProductListingTranslation,
    ProductListingEmbedding,
    ProductListingOpenSearch,
    ProductListingRawNormalization,
    NotificationDelivery,
}

impl WorkerScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SearchFilterProjection => "search-filter-projection",
            Self::SearchFilterPercolator => "search-filter-percolator",
            Self::SearchFilterMatchNotification => "search-filter-match-notification",
            Self::WatchlistNotification => "watchlist-notification",
            Self::ProductListingContentAssessment => "product-content-assessment",
            Self::ProductListingTranslation => "product-translation",
            Self::ProductListingEmbedding => "product-embedding",
            Self::ProductListingOpenSearch => "product-listing-opensearch",
            Self::ProductListingRawNormalization => "product-listing-normalization",
            Self::NotificationDelivery => "notification-delivery",
        }
    }

    pub(crate) fn from_getter<F>(mut get: F) -> Result<Self, WorkerStartupConfigError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let stage = get(WORKER_STAGE_ENV);
        let value = match get(WORKER_SCOPE_ENV) {
            Some(value) => value,
            None if is_local_development_stage(stage.as_deref()) => {
                DEFAULT_LOCAL_WORKER_SCOPE.to_owned()
            }
            None => {
                return Err(WorkerStartupConfigError::MissingEnv {
                    name: WORKER_SCOPE_ENV,
                });
            }
        };

        use strum::IntoEnumIterator;
        Self::iter()
            .find(|scope| scope.as_str() == value)
            .ok_or(WorkerStartupConfigError::InvalidScope { value })
    }

    pub(crate) const fn consumer_queue(self) -> WorkerQueue {
        match self {
            Self::SearchFilterProjection => WorkerQueue::SearchFilterOpenSearch,
            Self::SearchFilterPercolator => WorkerQueue::SearchFilterPercolator,
            Self::SearchFilterMatchNotification => WorkerQueue::SearchFilterMatchNotification,
            Self::WatchlistNotification => WorkerQueue::WatchlistNotification,
            Self::ProductListingContentAssessment => WorkerQueue::ProductListingContentAssessment,
            Self::ProductListingTranslation => WorkerQueue::ProductListingTranslate,
            Self::ProductListingEmbedding => WorkerQueue::ProductListingEmbed,
            Self::ProductListingOpenSearch => WorkerQueue::ProductListingOpenSearch,
            Self::ProductListingRawNormalization => WorkerQueue::ProductListingRawNormalization,
            Self::NotificationDelivery => WorkerQueue::NotificationDelivery,
        }
    }
}

fn is_local_development_stage(stage: Option<&str>) -> bool {
    matches!(stage, Some("ephemeral" | "local" | "test"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerConfig {
    health_bind_addr: SocketAddr,
    drain_timeout: Duration,
    stop_timeout: Duration,
    operational: Option<operations::OperationalConfig>,
}

impl WorkerConfig {
    pub fn from_env() -> Result<Self, WorkerConfigError> {
        Self::from_getter(|name| std::env::var(name).ok())
    }

    pub fn from_getter<F>(mut get: F) -> Result<Self, WorkerConfigError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let raw_health_bind_addr = get(WORKER_HEALTH_BIND_ADDR_ENV)
            .unwrap_or_else(|| DEFAULT_WORKER_HEALTH_BIND_ADDR.to_owned());
        let health_bind_addr = raw_health_bind_addr.parse().map_err(|source| {
            WorkerConfigError::InvalidHealthBindAddr {
                value: raw_health_bind_addr,
                source,
            }
        })?;
        let drain_timeout = match get(WORKER_DRAIN_TIMEOUT_SECONDS_ENV) {
            Some(value) => {
                let seconds = value
                    .parse()
                    .map_err(|source| WorkerConfigError::InvalidDrainTimeout { value, source })?;
                if seconds == 0 {
                    return Err(WorkerConfigError::ZeroDrainTimeout);
                }
                Duration::from_secs(seconds)
            }
            None => Duration::from_secs(DEFAULT_WORKER_DRAIN_TIMEOUT_SECONDS),
        };

        let stop_timeout = match get(WORKER_STOP_TIMEOUT_SECONDS_ENV) {
            Some(value) => {
                let seconds = value
                    .parse::<u64>()
                    .map_err(|_| WorkerConfigError::InvalidStopTimeout)?;
                if seconds == 0 {
                    return Err(WorkerConfigError::InvalidStopTimeout);
                }
                Duration::from_secs(seconds)
            }
            None => Duration::from_secs(operations::DEFAULT_STOP_SECONDS),
        };
        let maximum = Duration::from_secs(operations::MAX_SHUTDOWN_SECONDS);
        if drain_timeout > maximum || stop_timeout > maximum {
            return Err(WorkerConfigError::ShutdownBudgetTooLarge);
        }
        Ok(Self {
            health_bind_addr,
            drain_timeout,
            stop_timeout,
            operational: None,
        })
    }

    pub const fn health_bind_addr(&self) -> SocketAddr {
        self.health_bind_addr
    }

    pub const fn drain_timeout(&self) -> Duration {
        self.drain_timeout
    }

    /// Declared external stop budget; the process cannot configure its own supervisor.
    pub const fn stop_timeout(&self) -> Duration {
        self.stop_timeout
    }
}

#[derive(thiserror::Error, Debug)]
pub enum WorkerConfigError {
    #[error("invalid {env_name}: {value}", env_name = WORKER_HEALTH_BIND_ADDR_ENV)]
    InvalidHealthBindAddr {
        value: String,
        source: AddrParseError,
    },
    #[error("invalid {env_name}: {value}", env_name = WORKER_DRAIN_TIMEOUT_SECONDS_ENV)]
    InvalidDrainTimeout {
        value: String,
        source: ParseIntError,
    },
    #[error("{env_name} must be greater than zero", env_name = WORKER_DRAIN_TIMEOUT_SECONDS_ENV)]
    ZeroDrainTimeout,
    #[error("worker external stop timeout must be positive whole seconds")]
    InvalidStopTimeout,
    #[error("worker drain/stop timeouts must not exceed {max}s", max = operations::MAX_SHUTDOWN_SECONDS)]
    ShutdownBudgetTooLarge,
}

pub struct WorkerOpenSearchConfig {
    endpoint: url::Url,
    basic_auth: Option<(String, String)>,
}

impl WorkerOpenSearchConfig {
    pub fn endpoint(&self) -> &url::Url {
        &self.endpoint
    }

    pub fn basic_auth(&self) -> Option<(&str, &str)> {
        self.basic_auth
            .as_ref()
            .map(|(username, password)| (username.as_str(), password.as_str()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerVertexAiConfig {
    project_id: String,
    location: String,
    model: Option<String>,
}

impl WorkerVertexAiConfig {
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn location(&self) -> &str {
        &self.location
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
}

pub struct WorkerEmailDeliveryConfig {
    template_bucket: String,
    from_email_address: String,
    reply_to_email_address: String,
    stage: String,
    commit_sha: String,
}

impl WorkerEmailDeliveryConfig {
    pub fn template_bucket(&self) -> &str {
        &self.template_bucket
    }
    pub fn from_email_address(&self) -> &str {
        &self.from_email_address
    }
    pub fn reply_to_email_address(&self) -> &str {
        &self.reply_to_email_address
    }
    pub fn stage(&self) -> &str {
        &self.stage
    }
    pub fn commit_sha(&self) -> &str {
        &self.commit_sha
    }
}

pub struct WorkerNotificationDeliveryConfig {
    email: Option<WorkerEmailDeliveryConfig>,
}

impl WorkerNotificationDeliveryConfig {
    pub fn email(&self) -> Option<&WorkerEmailDeliveryConfig> {
        self.email.as_ref()
    }
}

pub struct WorkerStartupConfig {
    worker: WorkerConfig,
    scope: WorkerScope,
    queue: queue::SqsQueueConfig,
    postgres: PostgresPoolConfig,
    opensearch: Option<WorkerOpenSearchConfig>,
    vertex_ai: Option<WorkerVertexAiConfig>,
    notification_delivery: Option<WorkerNotificationDeliveryConfig>,
}

impl WorkerStartupConfig {
    pub fn from_env() -> Result<Self, WorkerStartupConfigError> {
        Self::from_getter(|name| match std::env::var(name) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            // Preserve presence so malformed optional inputs cannot fall back to defaults.
            Err(std::env::VarError::NotUnicode(_)) => Some(String::new()),
        })
    }

    pub(crate) fn from_getter<F>(mut get: F) -> Result<Self, WorkerStartupConfigError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let stage = get(WORKER_STAGE_ENV);
        let scope = WorkerScope::from_getter(&mut get)?;
        let mut worker = WorkerConfig::from_getter(&mut get)?;
        let queue = queue::SqsQueueConfig::from_getter(scope, &mut get)?;
        let postgres = postgres_config(&mut get)?;
        let identity = operations::OperationalIdentity::parse(
            scope,
            stage.as_deref(),
            get(COMMIT_SHA_ENV),
            &worker,
        )?;
        let commit_sha = identity.source_sha.clone();
        worker.operational = Some(operations::OperationalConfig {
            identity,
            drain: worker.drain_timeout(),
            stop: worker.stop_timeout(),
            execution: queue.execution_budget(),
        });
        let (opensearch, vertex_ai, notification_delivery) = match scope {
            WorkerScope::SearchFilterProjection | WorkerScope::ProductListingOpenSearch => (
                Some(opensearch_config(&mut get, stage.as_deref())?),
                None,
                None,
            ),
            WorkerScope::SearchFilterPercolator => (
                Some(opensearch_config(&mut get, stage.as_deref())?),
                Some(WorkerVertexAiConfig {
                    project_id: required_env(&mut get, VERTEX_AI_PROJECT_ID_ENV)?,
                    location: required_env(&mut get, VERTEX_AI_LOCATION_ENV)?,
                    model: Some(required_env(&mut get, VERTEX_AI_MODEL_ENV)?),
                }),
                None,
            ),
            WorkerScope::ProductListingTranslation => (
                None,
                Some(WorkerVertexAiConfig {
                    project_id: required_env(&mut get, VERTEX_AI_PROJECT_ID_ENV)?,
                    location: required_env(&mut get, VERTEX_AI_LOCATION_ENV)?,
                    model: Some(required_env(&mut get, VERTEX_AI_MODEL_ENV)?),
                }),
                None,
            ),
            WorkerScope::ProductListingEmbedding => (
                None,
                Some(WorkerVertexAiConfig {
                    project_id: required_env(&mut get, VERTEX_AI_PROJECT_ID_ENV)?,
                    location: required_env(&mut get, VERTEX_AI_LOCATION_ENV)?,
                    model: None,
                }),
                None,
            ),
            WorkerScope::NotificationDelivery => (
                None,
                None,
                Some(WorkerNotificationDeliveryConfig {
                    email: Some(WorkerEmailDeliveryConfig {
                        template_bucket: required_env(&mut get, S3_BUCKET_NAME_TEMPLATES_ENV)?,
                        from_email_address: required_env(&mut get, NOTIFICATION_EMAIL_FROM_ENV)?,
                        reply_to_email_address: required_env(
                            &mut get,
                            NOTIFICATION_EMAIL_REPLY_TO_ENV,
                        )?,
                        stage: stage.ok_or(WorkerStartupConfigError::MissingEnv {
                            name: WORKER_STAGE_ENV,
                        })?,
                        // EMAIL asset prefixes still require a release SHA, even in local tests.
                        commit_sha: commit_sha.ok_or(WorkerStartupConfigError::MissingEnv {
                            name: COMMIT_SHA_ENV,
                        })?,
                    }),
                }),
            ),
            WorkerScope::SearchFilterMatchNotification
            | WorkerScope::WatchlistNotification
            | WorkerScope::ProductListingContentAssessment
            | WorkerScope::ProductListingRawNormalization => (None, None, None),
        };

        Ok(Self {
            worker,
            scope,
            queue,
            postgres,
            opensearch,
            vertex_ai,
            notification_delivery,
        })
    }

    pub fn queue(&self) -> &queue::SqsQueueConfig {
        &self.queue
    }

    pub const fn worker(&self) -> &WorkerConfig {
        &self.worker
    }

    pub const fn scope(&self) -> WorkerScope {
        self.scope
    }

    pub const fn postgres(&self) -> &PostgresPoolConfig {
        &self.postgres
    }

    pub fn opensearch(&self) -> Option<&WorkerOpenSearchConfig> {
        self.opensearch.as_ref()
    }

    pub fn vertex_ai(&self) -> Option<&WorkerVertexAiConfig> {
        self.vertex_ai.as_ref()
    }

    pub fn notification_delivery(&self) -> Option<&WorkerNotificationDeliveryConfig> {
        self.notification_delivery.as_ref()
    }
}

fn postgres_config<F>(get: &mut F) -> Result<PostgresPoolConfig, WorkerPostgresConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    Ok(PostgresPoolConfig::from_lookup(
        "aura-historia-worker",
        get,
    )?)
}

#[cfg(test)]
mod postgres_config_tests;

fn opensearch_config<F>(
    get: &mut F,
    stage: Option<&str>,
) -> Result<WorkerOpenSearchConfig, WorkerStartupConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    let endpoint = required_env(get, OPENSEARCH_ENDPOINT_URL_ENV)?;
    let endpoint = url::Url::parse(&endpoint)
        .map_err(|source| WorkerStartupConfigError::InvalidOpenSearchEndpoint { source })?;
    if endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || !(endpoint.scheme() == "https"
            || (is_local_development_stage(stage) && endpoint.scheme() == "http"))
    {
        return Err(WorkerStartupConfigError::UnsupportedOpenSearchEndpoint);
    }
    let basic_auth = if is_local_development_stage(stage) {
        None
    } else {
        Some((
            required_env(get, OPENSEARCH_USERNAME_ENV)?,
            required_env(get, OPENSEARCH_PASSWORD_ENV)?,
        ))
    };

    Ok(WorkerOpenSearchConfig {
        endpoint,
        basic_auth,
    })
}

fn required_env<F>(get: &mut F, name: &'static str) -> Result<String, WorkerStartupConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    match get(name) {
        Some(value) if !value.is_empty() => Ok(value),
        Some(_) | None => Err(WorkerStartupConfigError::MissingEnv { name }),
    }
}

#[derive(thiserror::Error, Debug)]
pub enum WorkerPostgresConfigError {
    #[error("invalid PostgreSQL configuration")]
    Config(#[from] PostgresPoolConfigError),
}

#[derive(thiserror::Error, Debug)]
pub enum WorkerStartupConfigError {
    #[error(transparent)]
    Queue(#[from] queue::QueueError),
    #[error(transparent)]
    Worker(#[from] WorkerConfigError),
    #[error(transparent)]
    Postgres(#[from] WorkerPostgresConfigError),
    #[error("missing required environment variable {name}")]
    MissingEnv { name: &'static str },
    #[error("invalid worker scope {value}")]
    InvalidScope { value: String },
    #[error("worker deployment needs drain >=270s, external stop >=300s and >=drain+30s")]
    UnsafeDeploymentBudgets,
    #[error("COMMIT_SHA must be a canonical non-placeholder 40-character lowercase release SHA")]
    InvalidReleaseSha,
    #[error("unsupported OpenSearch endpoint configuration")]
    UnsupportedOpenSearchEndpoint,
    #[error("invalid OpenSearch endpoint URL")]
    InvalidOpenSearchEndpoint { source: url::ParseError },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueConfig {
    capacity: usize,
}

impl QueueConfig {
    pub const fn new(capacity: usize) -> Self {
        Self { capacity }
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }
}

#[derive(Debug, Clone)]
pub struct InMemoryQueueSender<T> {
    sender: mpsc::Sender<T>,
}

#[derive(Debug)]
pub struct InMemoryQueueReceiver<T> {
    receiver: mpsc::Receiver<T>,
}

pub fn in_memory_queue<T>(
    config: QueueConfig,
) -> Result<(InMemoryQueueSender<T>, InMemoryQueueReceiver<T>), QueueConfigError>
where
    T: WorkerJob,
{
    if config.capacity() == 0 {
        return Err(QueueConfigError::InvalidCapacity);
    }

    let (sender, receiver) = mpsc::channel(config.capacity());
    Ok((
        InMemoryQueueSender { sender },
        InMemoryQueueReceiver { receiver },
    ))
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum QueueConfigError {
    #[error("queue capacity must be greater than zero")]
    InvalidCapacity,
    #[error("scoped worker consumer queue is not registered: {queue:?}")]
    MissingScopedConsumer { queue: WorkerQueue },
}

impl<T> InMemoryQueueSender<T>
where
    T: WorkerJob,
{
    pub async fn enqueue(&self, job: T) -> Result<(), mpsc::error::SendError<T>> {
        self.sender.send(job).await
    }

    pub fn try_enqueue(&self, job: T) -> Result<(), mpsc::error::TrySendError<T>> {
        self.sender.try_send(job)
    }
}

impl<T> InMemoryQueueReceiver<T>
where
    T: WorkerJob,
{
    pub async fn recv(&mut self) -> Option<T> {
        self.receiver.recv().await
    }
}

#[derive(Debug, Clone)]
pub struct WorkerRuntime {
    cdc_fanout: CdcFanout,
    control: queue::RuntimeControl,
    operational: Option<operations::OperationalConfig>,
}

impl WorkerRuntime {
    pub fn new(cdc_fanout: CdcFanout) -> Self {
        Self {
            cdc_fanout,
            control: queue::RuntimeControl::new(true),
            operational: None,
        }
    }

    pub fn with_all_queues(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let (registry, receivers) = WorkerQueueRegistry::with_all_queues(config)?;
        Ok((Self::new(CdcFanout::new(registry)), receivers))
    }

    pub fn with_watchlist_notification_queue(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let (sender, receiver) = in_memory_queue(config)?;
        let registry =
            WorkerQueueRegistry::new().with_queue(WorkerQueue::WatchlistNotification, sender);
        let mut receivers = WorkerQueueReceivers::new();
        receivers.insert(WorkerQueue::WatchlistNotification, receiver);
        Ok((
            Self::new(CdcFanout::watchlist_notification(registry)),
            receivers,
        ))
    }

    pub fn with_search_filter_percolator_queue(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let (sender, receiver) = in_memory_queue(config)?;
        let registry =
            WorkerQueueRegistry::new().with_queue(WorkerQueue::SearchFilterPercolator, sender);
        let mut receivers = WorkerQueueReceivers::new();
        receivers.insert(WorkerQueue::SearchFilterPercolator, receiver);
        Ok((
            Self::new(CdcFanout::search_filter_percolator(registry)),
            receivers,
        ))
    }

    pub fn with_search_filter_match_notification_queue(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let (sender, receiver) = in_memory_queue(config)?;
        let registry = WorkerQueueRegistry::new()
            .with_queue(WorkerQueue::SearchFilterMatchNotification, sender);
        let mut receivers = WorkerQueueReceivers::new();
        receivers.insert(WorkerQueue::SearchFilterMatchNotification, receiver);
        Ok((
            Self::new(CdcFanout::search_filter_match_notification(registry)),
            receivers,
        ))
    }

    pub fn with_product_listing_opensearch_queue(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let (sender, receiver) = in_memory_queue(config)?;
        let registry =
            WorkerQueueRegistry::new().with_queue(WorkerQueue::ProductListingOpenSearch, sender);
        let mut receivers = WorkerQueueReceivers::new();
        receivers.insert(WorkerQueue::ProductListingOpenSearch, receiver);
        Ok((
            Self::new(CdcFanout::product_listing_opensearch(registry)),
            receivers,
        ))
    }

    pub fn with_product_content_assessment_queue(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let (sender, receiver) = in_memory_queue(config)?;
        let registry = WorkerQueueRegistry::new()
            .with_queue(WorkerQueue::ProductListingContentAssessment, sender);
        let mut receivers = WorkerQueueReceivers::new();
        receivers.insert(WorkerQueue::ProductListingContentAssessment, receiver);
        Ok((
            Self::new(CdcFanout::product_content_assessment(registry)),
            receivers,
        ))
    }

    pub fn with_product_embedding_queue(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let (sender, receiver) = in_memory_queue(config)?;
        let registry =
            WorkerQueueRegistry::new().with_queue(WorkerQueue::ProductListingEmbed, sender);
        let mut receivers = WorkerQueueReceivers::new();
        receivers.insert(WorkerQueue::ProductListingEmbed, receiver);
        Ok((Self::new(CdcFanout::product_embedding(registry)), receivers))
    }

    pub fn with_product_translation_queue(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let (sender, receiver) = in_memory_queue(config)?;
        let registry =
            WorkerQueueRegistry::new().with_queue(WorkerQueue::ProductListingTranslate, sender);
        let mut receivers = WorkerQueueReceivers::new();
        receivers.insert(WorkerQueue::ProductListingTranslate, receiver);
        Ok((
            Self::new(CdcFanout::product_translation(registry)),
            receivers,
        ))
    }

    pub fn with_product_listing_raw_normalization_queue(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let (sender, receiver) = in_memory_queue(config)?;
        let registry = WorkerQueueRegistry::new()
            .with_queue(WorkerQueue::ProductListingRawNormalization, sender);
        let mut receivers = WorkerQueueReceivers::new();
        receivers.insert(WorkerQueue::ProductListingRawNormalization, receiver);
        Ok((
            Self::new(CdcFanout::product_listing_raw_normalization(registry)),
            receivers,
        ))
    }

    pub fn with_notification_delivery_queue(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let (sender, receiver) = in_memory_queue(config)?;
        let registry =
            WorkerQueueRegistry::new().with_queue(WorkerQueue::NotificationDelivery, sender);
        let mut receivers = WorkerQueueReceivers::new();
        receivers.insert(WorkerQueue::NotificationDelivery, receiver);
        Ok((
            Self::new(CdcFanout::notification_delivery(registry)),
            receivers,
        ))
    }

    pub fn with_search_filter_projection_queue(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let (sender, receiver) = in_memory_queue(config)?;
        let registry =
            WorkerQueueRegistry::new().with_queue(WorkerQueue::SearchFilterOpenSearch, sender);
        let mut receivers = WorkerQueueReceivers::new();
        receivers.insert(WorkerQueue::SearchFilterOpenSearch, receiver);
        Ok((
            Self::new(CdcFanout::search_filter_projection(registry)),
            receivers,
        ))
    }

    pub fn empty() -> Self {
        Self::new(CdcFanout::new(WorkerQueueRegistry::new()))
    }

    pub fn shutdown(&self) {
        self.control.shutdown();
    }

    /// Join cancelled receive, handler and reconciliation children after joining the consumer.
    /// The process supervisor must bound parent and child cancellation under one deadline.
    pub async fn join_cancelled_tasks(&self) {
        self.control.join_cancelled_tasks().await;
    }

    pub(crate) fn admitting(&self) -> bool {
        self.cdc_fanout.has_destinations() && !self.control.stopping()
    }

    pub async fn ingest_cdc_json(&self, body: &str) -> Result<usize, CdcIngestError> {
        if self.control.stopping() {
            return Err(cdc::CdcFanoutError::PublicationFailed.into());
        }
        self.cdc_fanout.ingest_json(body).await
    }
}

pub struct WorkerRuntimeComposition {
    runtime: WorkerRuntime,
    receiver: queue::WorkerQueueReceiver,
}

impl WorkerRuntimeComposition {
    /// Normal production composition, also usable by embedded runtimes with their AWS client.
    pub async fn with_sqs(
        client: aws_sdk_sqs::Client,
        config: queue::SqsQueueConfig,
    ) -> Result<Self, queue::QueueError> {
        Ok(Self::from_sqs_queue(
            queue::SqsQueue::new(client, config).await?,
        ))
    }

    pub fn from_sqs_queue(queue: queue::SqsQueue) -> Self {
        let scope = queue.config().scope();
        let control = queue::RuntimeControl::new(false);
        let receiver = queue::WorkerQueueReceiver::sqs(queue.clone(), control.clone());
        let registry = WorkerQueueRegistry::new().with_sqs_queue(queue);
        let runtime = WorkerRuntime {
            cdc_fanout: CdcFanout::for_scope(scope, registry),
            control,
            operational: None,
        };
        Self { runtime, receiver }
    }

    /// Explicit legacy in-memory composition. Never selected by production startup.
    pub fn build(scope: WorkerScope, config: QueueConfig) -> Result<Self, QueueConfigError> {
        let (runtime, mut receivers) = match scope {
            WorkerScope::SearchFilterProjection => {
                WorkerRuntime::with_search_filter_projection_queue(config)?
            }
            WorkerScope::SearchFilterPercolator => {
                WorkerRuntime::with_search_filter_percolator_queue(config)?
            }
            WorkerScope::SearchFilterMatchNotification => {
                WorkerRuntime::with_search_filter_match_notification_queue(config)?
            }
            WorkerScope::WatchlistNotification => {
                WorkerRuntime::with_watchlist_notification_queue(config)?
            }
            WorkerScope::ProductListingContentAssessment => {
                WorkerRuntime::with_product_content_assessment_queue(config)?
            }
            WorkerScope::ProductListingTranslation => {
                WorkerRuntime::with_product_translation_queue(config)?
            }
            WorkerScope::ProductListingEmbedding => {
                WorkerRuntime::with_product_embedding_queue(config)?
            }
            WorkerScope::ProductListingOpenSearch => {
                WorkerRuntime::with_product_listing_opensearch_queue(config)?
            }
            WorkerScope::ProductListingRawNormalization => {
                WorkerRuntime::with_product_listing_raw_normalization_queue(config)?
            }
            WorkerScope::NotificationDelivery => {
                WorkerRuntime::with_notification_delivery_queue(config)?
            }
        };
        let consumer_queue = scope.consumer_queue();
        let receiver =
            receivers
                .take(consumer_queue)
                .ok_or(QueueConfigError::MissingScopedConsumer {
                    queue: consumer_queue,
                })?;

        Ok(Self {
            runtime,
            receiver: receiver.into(),
        })
    }

    pub fn into_parts(self) -> (WorkerRuntime, queue::WorkerQueueReceiver) {
        (self.runtime, self.receiver)
    }
}

impl Default for WorkerRuntime {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status_code: u16,
    pub body: &'static str,
}

pub fn route(method: &str, path: &str) -> HttpResponse {
    match (method, path) {
        ("GET", "/health") => HttpResponse {
            status_code: 200,
            body: "ok\n",
        },
        ("GET", "/ready") => HttpResponse {
            status_code: 200,
            body: "ready\n",
        },
        _ => HttpResponse {
            status_code: 404,
            body: "not found\n",
        },
    }
}

pub async fn run_until_shutdown<S>(config: WorkerConfig, shutdown: S) -> Result<(), WorkerRunError>
where
    S: Future<Output = ()> + Send + 'static,
{
    run_until_shutdown_with_runtime(config, WorkerRuntime::default(), shutdown).await
}

pub async fn run_until_shutdown_with_runtime<S>(
    config: WorkerConfig,
    runtime: WorkerRuntime,
    shutdown: S,
) -> Result<(), WorkerRunError>
where
    S: Future<Output = ()> + Send + 'static,
{
    let mut runtime = runtime;
    runtime.operational = config.operational.clone();
    let listener = TcpListener::bind(config.health_bind_addr())
        .await
        .map_err(WorkerRunError::Bind)?;
    serve_with_runtime(listener, runtime, shutdown).await
}

pub async fn serve<S>(listener: TcpListener, shutdown: S) -> Result<(), WorkerRunError>
where
    S: Future<Output = ()> + Send + 'static,
{
    serve_with_runtime(listener, WorkerRuntime::default(), shutdown).await
}

pub async fn serve_with_runtime<S>(
    listener: TcpListener,
    runtime: WorkerRuntime,
    shutdown: S,
) -> Result<(), WorkerRunError>
where
    S: Future<Output = ()> + Send + 'static,
{
    let local_addr = listener.local_addr().map_err(WorkerRunError::LocalAddr)?;
    info!(bind_addr = %local_addr, "aura-historia-worker private health and CDC server listening");
    http::serve(listener, runtime, shutdown).await
}

#[derive(thiserror::Error, Debug)]
pub enum WorkerRunError {
    #[error("failed to bind worker health listener")]
    Bind(#[source] std::io::Error),
    #[error("failed to read worker health listener local address")]
    LocalAddr(#[source] std::io::Error),
    #[error("failed to accept worker health connection")]
    Accept(#[source] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::SocketAddr;

    use super::*;
    use crate::cdc::{CdcFanout, DomainJob, WorkerQueue, WorkerQueueRegistry};
    use rstest::rstest;
    use tokio::sync::oneshot;

    fn env(values: &[(&'static str, &str)]) -> HashMap<&'static str, String> {
        values
            .iter()
            .map(|(key, value)| (*key, (*value).to_owned()))
            .collect()
    }

    fn production_worker_env(scope: &str) -> HashMap<&'static str, String> {
        env(&[
            (WORKER_STAGE_ENV, "prod"),
            (COMMIT_SHA_ENV, "d5bd9ca854e713b0c587528f02037211b2020fd4"),
            (WORKER_SCOPE_ENV, scope),
            (queue::AWS_REGION_ENV, "eu-central-1"),
            (
                queue::WORKER_QUEUE_URL_ENV,
                &format!(
                    "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-{scope}-prod"
                ),
            ),
            ("POSTGRES_HOST", "postgres"),
            ("POSTGRES_DATABASE", "aura_historia"),
            ("POSTGRES_USERNAME", "worker"),
            ("POSTGRES_PASSWORD", "not-a-real-secret"),
            ("POSTGRES_SSL_MODE", "verify-full"),
            (
                "POSTGRES_SSL_ROOT_CERT",
                concat!(env!("CARGO_MANIFEST_DIR"), "/src/postgres-test-ca.crt"),
            ),
        ])
    }

    fn add_scope_dependencies(values: &mut HashMap<&'static str, String>, scope: WorkerScope) {
        match scope {
            WorkerScope::SearchFilterProjection
            | WorkerScope::SearchFilterPercolator
            | WorkerScope::ProductListingOpenSearch => {
                values.insert(
                    OPENSEARCH_ENDPOINT_URL_ENV,
                    "https://opensearch:9200".to_owned(),
                );
                values.insert(OPENSEARCH_USERNAME_ENV, "worker".to_owned());
                values.insert(OPENSEARCH_PASSWORD_ENV, "not-a-real-secret".to_owned());
            }
            WorkerScope::SearchFilterMatchNotification
            | WorkerScope::WatchlistNotification
            | WorkerScope::ProductListingContentAssessment
            | WorkerScope::ProductListingRawNormalization => {}
            WorkerScope::NotificationDelivery => {
                values.insert(S3_BUCKET_NAME_TEMPLATES_ENV, "templates".to_owned());
                values.insert(
                    NOTIFICATION_EMAIL_FROM_ENV,
                    "no-reply@example.test".to_owned(),
                );
                values.insert(
                    NOTIFICATION_EMAIL_REPLY_TO_ENV,
                    "contact@example.test".to_owned(),
                );
                values.insert(
                    COMMIT_SHA_ENV,
                    "d5bd9ca854e713b0c587528f02037211b2020fd4".to_owned(),
                );
            }
            WorkerScope::ProductListingTranslation => {}
            WorkerScope::ProductListingEmbedding => {}
        }
        if matches!(
            scope,
            WorkerScope::SearchFilterPercolator
                | WorkerScope::ProductListingTranslation
                | WorkerScope::ProductListingEmbedding
        ) {
            values.insert(VERTEX_AI_PROJECT_ID_ENV, "aura-historia-dev".to_owned());
            values.insert(VERTEX_AI_LOCATION_ENV, "europe-west3".to_owned());
            if !matches!(scope, WorkerScope::ProductListingEmbedding) {
                values.insert(VERTEX_AI_MODEL_ENV, "gemini-3.1-flash-lite".to_owned());
            }
        }
    }

    #[test]
    fn should_use_default_health_bind_addr_when_env_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[]);

        let config = WorkerConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!(
            "0.0.0.0:8081".parse::<SocketAddr>()?,
            config.health_bind_addr()
        );
        assert_eq!(
            Duration::from_secs(DEFAULT_WORKER_DRAIN_TIMEOUT_SECONDS),
            config.drain_timeout()
        );
        Ok(())
    }

    #[test]
    fn should_read_health_bind_addr_from_env() -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[(WORKER_HEALTH_BIND_ADDR_ENV, "127.0.0.1:9001")]);

        let config = WorkerConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!(
            "127.0.0.1:9001".parse::<SocketAddr>()?,
            config.health_bind_addr()
        );
        Ok(())
    }

    #[test]
    fn should_read_drain_timeout_from_env() -> Result<(), Box<dyn std::error::Error>> {
        let values = env(&[(WORKER_DRAIN_TIMEOUT_SECONDS_ENV, "321")]);

        let config = WorkerConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!(Duration::from_secs(321), config.drain_timeout());
        Ok(())
    }

    #[rstest]
    #[case("not-a-number", false)]
    #[case("0", true)]
    fn should_reject_invalid_drain_timeout(#[case] value: &str, #[case] zero: bool) {
        let values = env(&[(WORKER_DRAIN_TIMEOUT_SECONDS_ENV, value)]);

        let config = WorkerConfig::from_getter(|name| values.get(name).cloned());

        if zero {
            assert!(matches!(config, Err(WorkerConfigError::ZeroDrainTimeout)));
        } else {
            assert!(matches!(
                config,
                Err(WorkerConfigError::InvalidDrainTimeout { .. })
            ));
        }
    }

    #[test]
    fn should_fail_when_health_bind_addr_is_invalid() {
        let values = env(&[(WORKER_HEALTH_BIND_ADDR_ENV, "not-an-addr")]);

        let config = WorkerConfig::from_getter(|name| values.get(name).cloned());

        assert!(matches!(
            config,
            Err(WorkerConfigError::InvalidHealthBindAddr { .. })
        ));
    }

    #[test]
    fn should_require_worker_scope_in_production() {
        let values = env(&[(WORKER_STAGE_ENV, "prod")]);

        let config = WorkerStartupConfig::from_getter(|name| values.get(name).cloned());

        assert!(matches!(
            config,
            Err(WorkerStartupConfigError::MissingEnv {
                name: WORKER_SCOPE_ENV
            })
        ));
    }

    #[test]
    fn should_include_invalid_worker_scope_in_error() {
        let values = production_worker_env("wrong-worker");

        let config = WorkerStartupConfig::from_getter(|name| values.get(name).cloned());

        assert!(matches!(
            config,
            Err(WorkerStartupConfigError::InvalidScope { value }) if value == "wrong-worker"
        ));
    }

    #[rstest]
    #[case("ephemeral")]
    #[case("local")]
    #[case("test")]
    fn should_default_scope_only_in_local_development(#[case] stage: &str) {
        let values = env(&[(WORKER_STAGE_ENV, stage)]);

        let scope = WorkerScope::from_getter(|name| values.get(name).cloned());

        assert!(matches!(scope, Ok(WorkerScope::SearchFilterProjection)));
    }

    #[rstest]
    #[case(
        WorkerScope::SearchFilterProjection,
        "search-filter-projection",
        WorkerQueue::SearchFilterOpenSearch
    )]
    #[case(
        WorkerScope::SearchFilterPercolator,
        "search-filter-percolator",
        WorkerQueue::SearchFilterPercolator
    )]
    #[case(
        WorkerScope::SearchFilterMatchNotification,
        "search-filter-match-notification",
        WorkerQueue::SearchFilterMatchNotification
    )]
    #[case(
        WorkerScope::WatchlistNotification,
        "watchlist-notification",
        WorkerQueue::WatchlistNotification
    )]
    #[case(
        WorkerScope::ProductListingContentAssessment,
        "product-content-assessment",
        WorkerQueue::ProductListingContentAssessment
    )]
    #[case(
        WorkerScope::ProductListingTranslation,
        "product-translation",
        WorkerQueue::ProductListingTranslate
    )]
    #[case(
        WorkerScope::ProductListingEmbedding,
        "product-embedding",
        WorkerQueue::ProductListingEmbed
    )]
    #[case(
        WorkerScope::ProductListingOpenSearch,
        "product-listing-opensearch",
        WorkerQueue::ProductListingOpenSearch
    )]
    #[case(
        WorkerScope::ProductListingRawNormalization,
        "product-listing-normalization",
        WorkerQueue::ProductListingRawNormalization
    )]
    #[case(
        WorkerScope::NotificationDelivery,
        "notification-delivery",
        WorkerQueue::NotificationDelivery
    )]
    fn should_validate_only_dependencies_for_selected_scope(
        #[case] scope: WorkerScope,
        #[case] scope_name: &str,
        #[case] consumer_queue: WorkerQueue,
    ) -> Result<(), WorkerStartupConfigError> {
        let mut values = production_worker_env(scope_name);
        add_scope_dependencies(&mut values, scope);

        let config = WorkerStartupConfig::from_getter(|name| values.get(name).cloned())?;

        assert_eq!(scope, config.scope());
        assert_eq!(consumer_queue, scope.consumer_queue());
        assert_eq!(
            matches!(
                scope,
                WorkerScope::SearchFilterProjection
                    | WorkerScope::SearchFilterPercolator
                    | WorkerScope::ProductListingOpenSearch
            ),
            config.opensearch().is_some()
        );
        assert_eq!(
            matches!(
                scope,
                WorkerScope::SearchFilterPercolator
                    | WorkerScope::ProductListingTranslation
                    | WorkerScope::ProductListingEmbedding
            ),
            config.vertex_ai().is_some()
        );
        Ok(())
    }

    #[rstest]
    #[case(VERTEX_AI_PROJECT_ID_ENV)]
    #[case(VERTEX_AI_LOCATION_ENV)]
    #[case(VERTEX_AI_MODEL_ENV)]
    fn should_require_vertex_configuration_for_production_percolator(
        #[case] missing_env: &'static str,
    ) {
        let mut values = production_worker_env("search-filter-percolator");
        add_scope_dependencies(&mut values, WorkerScope::SearchFilterPercolator);
        values.remove(missing_env);

        let config = WorkerStartupConfig::from_getter(|name| values.get(name).cloned());

        assert!(matches!(
            config,
            Err(WorkerStartupConfigError::MissingEnv { name }) if name == missing_env
        ));
    }

    fn expected_schema_v2_job(scope: WorkerScope) -> serde_json::Value {
        let (job_type, payload, idempotency_key, ordering_key) = match scope {
            WorkerScope::SearchFilterProjection => (
                "SEARCH_FILTER_CHANGED",
                serde_json::json!({
                    "user_id": "usr_01j0000000e008000000000001",
                    "user_search_filter_id": "sf_01j0000000e008000000000002",
                    "version": 1,
                    "operation": "INSERT",
                }),
                "search-filter:sf_01j0000000e008000000000002:1:insert",
                "search-filter:sf_01j0000000e008000000000002",
            ),
            WorkerScope::SearchFilterMatchNotification => (
                "SEARCH_FILTER_MATCH_CREATED",
                serde_json::json!({
                    "user_id": "usr_01j0000000e008000000000001",
                    "user_search_filter_id": "sf_01j0000000e008000000000002",
                    "product_listing_id": "pl_01j0000000e008000000000003",
                    "origin_event_id": "evt_01j0000000e008000000000004",
                }),
                "search-filter-match:usr_01j0000000e008000000000001:sf_01j0000000e008000000000002:pl_01j0000000e008000000000003:evt_01j0000000e008000000000004",
                "user:usr_01j0000000e008000000000001",
            ),
            WorkerScope::ProductListingRawNormalization => (
                "PRODUCT_LISTING_RAW_REVISION",
                serde_json::json!({
                    "product_listing_raw_stream_id": "prs_01j0000000e008000000000001",
                    "product_listing_raw_revision_id": "prr_01j0000000e008000000000002",
                    "revision": 1,
                }),
                "product-listing-raw-revision:prr_01j0000000e008000000000002",
                "product-listing-raw-stream:prs_01j0000000e008000000000001",
            ),
            WorkerScope::NotificationDelivery => (
                "NOTIFICATION_DELIVERY_CREATED",
                serde_json::json!({
                    "notification_delivery_id": "nd_01j0000000e008000000000006",
                }),
                "notification-delivery:nd_01j0000000e008000000000006",
                "notification-delivery:nd_01j0000000e008000000000006",
            ),
            WorkerScope::SearchFilterPercolator
            | WorkerScope::WatchlistNotification
            | WorkerScope::ProductListingContentAssessment
            | WorkerScope::ProductListingTranslation
            | WorkerScope::ProductListingEmbedding
            | WorkerScope::ProductListingOpenSearch => (
                "PRODUCT_LISTING_EVENT",
                serde_json::json!({
                    "event_id": "evt_01j0000000e008000000000004",
                    "product_listing_id": "pl_01j0000000e008000000000003",
                }),
                "product-event:evt_01j0000000e008000000000004",
                "product:pl_01j0000000e008000000000003",
            ),
        };
        serde_json::json!({
            "schema_version": 2,
            "scope": scope.as_str(),
            "job_type": job_type,
            "idempotency_key": idempotency_key,
            "ordering_key": ordering_key,
            "payload": payload,
        })
    }

    #[rstest]
    #[case(
        WorkerScope::SearchFilterProjection,
        WorkerQueue::SearchFilterOpenSearch,
        r#"{"changes":[{"table":"search_filters","operation":"insert","record":{"user_id":"01900000-0000-7000-8000-000000000001","user_search_filter_id":"01900000-0000-7000-8000-000000000002","version":1}}]}"#
    )]
    #[case(
        WorkerScope::SearchFilterPercolator,
        WorkerQueue::SearchFilterPercolator,
        r#"{"changes":[{"table":"product_listing_events","operation":"insert","record":{"event_id":"01900000-0000-7000-8000-000000000004","product_listing_id":"01900000-0000-7000-8000-000000000003","event_type":"PRODUCT_LISTING_DISCOVERED","event_group":"DOMAIN","event_type_schema_version":1,"payload":{"listingSourceId":"01900000-0000-7000-8000-000000000001","sourceListingId":"fixture-source-id","title":null,"description":null,"pricing":{"price":null,"priceEstimateMin":null,"priceEstimateMax":null},"availability":null,"url":"https://example.test/product","imageCount":0,"auction":{"start":null,"end":null}}}}]}"#
    )]
    #[case(
        WorkerScope::SearchFilterMatchNotification,
        WorkerQueue::SearchFilterMatchNotification,
        r#"{"changes":[{"table":"search_filter_matches","operation":"insert","record":{"user_id":"01900000-0000-7000-8000-000000000001","user_search_filter_id":"01900000-0000-7000-8000-000000000002","product_listing_id":"01900000-0000-7000-8000-000000000003","origin_event_id":"01900000-0000-7000-8000-000000000004"}}]}"#
    )]
    #[case(
        WorkerScope::WatchlistNotification,
        WorkerQueue::WatchlistNotification,
        r#"{"changes":[{"table":"product_listing_events","operation":"insert","record":{"event_id":"01900000-0000-7000-8000-000000000004","product_listing_id":"01900000-0000-7000-8000-000000000003","event_type":"PRODUCT_LISTING_CHANGED","event_group":"DOMAIN","event_type_schema_version":1,"payload":{"availability":{"previous":null,"current":"AVAILABLE"}}}}]}"#
    )]
    #[case(
        WorkerScope::ProductListingContentAssessment,
        WorkerQueue::ProductListingContentAssessment,
        r#"{"changes":[{"table":"product_listing_events","operation":"insert","record":{"event_id":"01900000-0000-7000-8000-000000000004","product_listing_id":"01900000-0000-7000-8000-000000000003","event_type":"PRODUCT_LISTING_DISCOVERED","event_group":"DOMAIN","event_type_schema_version":1,"payload":{"listingSourceId":"01900000-0000-7000-8000-000000000001","sourceListingId":"fixture-source-id","title":null,"description":null,"pricing":{"price":null,"priceEstimateMin":null,"priceEstimateMax":null},"availability":null,"url":"https://example.test/product","imageCount":0,"auction":{"start":null,"end":null}}}}]}"#
    )]
    #[case(
        WorkerScope::ProductListingTranslation,
        WorkerQueue::ProductListingTranslate,
        r#"{"changes":[{"table":"product_listing_events","operation":"insert","record":{"event_id":"01900000-0000-7000-8000-000000000004","product_listing_id":"01900000-0000-7000-8000-000000000003","event_type":"PRODUCT_LISTING_DISCOVERED","event_group":"DOMAIN","event_type_schema_version":1,"payload":{"listingSourceId":"01900000-0000-7000-8000-000000000001","sourceListingId":"fixture-source-id","title":null,"description":null,"pricing":{"price":null,"priceEstimateMin":null,"priceEstimateMax":null},"availability":null,"url":"https://example.test/product","imageCount":0,"auction":{"start":null,"end":null}}}}]}"#
    )]
    #[case(
        WorkerScope::ProductListingEmbedding,
        WorkerQueue::ProductListingEmbed,
        r#"{"changes":[{"table":"product_listing_events","operation":"insert","record":{"event_id":"01900000-0000-7000-8000-000000000004","product_listing_id":"01900000-0000-7000-8000-000000000003","event_type":"PRODUCT_LISTING_DISCOVERED","event_group":"DOMAIN","event_type_schema_version":1,"payload":{"listingSourceId":"01900000-0000-7000-8000-000000000001","sourceListingId":"fixture-source-id","title":null,"description":null,"pricing":{"price":null,"priceEstimateMin":null,"priceEstimateMax":null},"availability":null,"url":"https://example.test/product","imageCount":0,"auction":{"start":null,"end":null}}}}]}"#
    )]
    #[case(
        WorkerScope::ProductListingEmbedding,
        WorkerQueue::ProductListingEmbed,
        r#"{"changes":[{"table":"product_listing_events","operation":"insert","record":{"event_id":"01900000-0000-7000-8000-000000000004","product_listing_id":"01900000-0000-7000-8000-000000000003","event_type":"PRODUCT_LISTING_CHANGED","event_group":"DOMAIN","event_type_schema_version":1,"payload":{"images":{"previousCount":1,"currentCount":2}}}}]}"#
    )]
    #[case(
        WorkerScope::ProductListingOpenSearch,
        WorkerQueue::ProductListingOpenSearch,
        r#"{"changes":[{"table":"product_listing_events","operation":"insert","record":{"event_id":"01900000-0000-7000-8000-000000000004","product_listing_id":"01900000-0000-7000-8000-000000000003","event_type":"PRODUCT_LISTING_DISCOVERED","event_group":"DOMAIN","event_type_schema_version":1,"payload":{"listingSourceId":"01900000-0000-7000-8000-000000000001","sourceListingId":"fixture-source-id","title":null,"description":null,"pricing":{"price":null,"priceEstimateMin":null,"priceEstimateMax":null},"availability":null,"url":"https://example.test/product","imageCount":0,"auction":{"start":null,"end":null}}}}]}"#
    )]
    #[case(
        WorkerScope::ProductListingRawNormalization,
        WorkerQueue::ProductListingRawNormalization,
        r#"{"changes":[{"table":"product_listing_raw_revisions","operation":"insert","record":{"product_listing_raw_stream_id":"01900000-0000-7000-8000-000000000001","product_listing_raw_revision_id":"01900000-0000-7000-8000-000000000002","revision":1}}]}"#
    )]
    #[case(
        WorkerScope::NotificationDelivery,
        WorkerQueue::NotificationDelivery,
        r#"{"changes":[{"table":"notification_deliveries","operation":"insert","record":{"notification_delivery_id":"01900000-0000-7000-8000-000000000006","notification_id":"01900000-0000-7000-8000-000000000005","channel":"EMAIL","status":"PENDING"}}]}"#
    )]
    #[tokio::test]
    async fn should_build_one_intended_cdc_route_and_consumer_for_scope(
        #[case] scope: WorkerScope,
        #[case] consumer_queue: WorkerQueue,
        #[case] cdc_json: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let composition = WorkerRuntimeComposition::build(scope, QueueConfig::new(1))?;

        let (runtime, mut receiver) = composition.into_parts();
        assert_eq!(1, runtime.ingest_cdc_json(cdc_json).await?);
        let _guard = receiver.start(scope).ok_or("consumer scope mismatch")?;
        let delivery = receiver.recv().await.ok_or("scoped consumer stopped")?;
        let (sent, received) = oneshot::channel();
        receiver
            .process(delivery, move |job| async move {
                let _closed = sent.send(job);
                queue::JobOutcome::Complete("test_completed")
            })
            .await;
        let job = received.await?;
        assert_eq!(consumer_queue, job.target_queue);
        assert_eq!(
            expected_schema_v2_job(scope),
            serde_json::from_str::<serde_json::Value>(&crate::wire::encode(&job)?)?
        );

        Ok(())
    }

    #[tokio::test]
    async fn should_not_enqueue_embedded_event_for_product_translation_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let composition = WorkerRuntimeComposition::build(
            WorkerScope::ProductListingTranslation,
            QueueConfig::new(1),
        )?;
        let (runtime, _receiver) = composition.into_parts();
        let cdc_json = r#"{"changes":[{"table":"product_listing_events","operation":"insert","record":{"event_id":"01900000-0000-7000-8000-000000000004","product_listing_id":"01900000-0000-7000-8000-000000000003","event_type":"ENRICHMENT_EMBEDDED","event_group":"ENRICHMENT","event_type_schema_version":1,"payload":{"sourceEventId":"01900000-0000-7000-8000-000000000005"}}}]}"#;

        assert_eq!(0, runtime.ingest_cdc_json(cdc_json).await?);
        Ok(())
    }

    #[rstest]
    #[case("GET", "/health", 200, "ok\n")]
    #[case("GET", "/ready", 200, "ready\n")]
    #[case("POST", "/health", 404, "not found\n")]
    fn should_route_health_endpoints(
        #[case] method: &str,
        #[case] path: &str,
        #[case] status_code: u16,
        #[case] body: &'static str,
    ) {
        let response = route(method, path);

        assert_eq!(status_code, response.status_code);
        assert_eq!(body, response.body);
    }

    #[tokio::test]
    async fn should_enqueue_and_receive_jobs() -> Result<(), Box<dyn std::error::Error>> {
        let (sender, mut receiver) = in_memory_queue::<String>(QueueConfig::new(2))?;

        sender.enqueue("product:1".to_owned()).await?;

        assert_eq!(Some("product:1".to_owned()), receiver.recv().await);
        Ok(())
    }

    #[tokio::test]
    async fn should_apply_backpressure_when_queue_is_full() -> Result<(), Box<dyn std::error::Error>>
    {
        let (sender, _receiver) = in_memory_queue::<String>(QueueConfig::new(1))?;

        let first_result = sender.try_enqueue("product:1".to_owned());
        let second_result = sender.try_enqueue("product:2".to_owned());

        assert!(first_result.is_ok());
        assert!(matches!(
            second_result,
            Err(mpsc::error::TrySendError::Full(_))
        ));
        Ok(())
    }

    #[test]
    fn should_reject_zero_queue_capacity() {
        let queue = in_memory_queue::<String>(QueueConfig::new(0));

        assert!(matches!(queue, Err(QueueConfigError::InvalidCapacity)));
    }

    #[tokio::test]
    async fn should_serve_health_endpoint_until_shutdown() -> Result<(), Box<dyn std::error::Error>>
    {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve(listener, async move {
            let _ = shutdown_rx.await;
        }));

        let response = request(addr, "GET /health HTTP/1.1\r\nhost: localhost\r\n\r\n").await?;
        let _send_result = shutdown_tx.send(());
        server.await??;

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.ends_with("ok\n"));
        Ok(())
    }

    #[tokio::test]
    async fn should_accept_sequin_cdc_after_fanout() -> Result<(), Box<dyn std::error::Error>> {
        let (product_sender, mut product_receiver) =
            in_memory_queue::<DomainJob>(QueueConfig::new(8))?;
        let (percolator_sender, mut percolator_receiver) =
            in_memory_queue::<DomainJob>(QueueConfig::new(8))?;
        let (embed_sender, mut embed_receiver) = in_memory_queue::<DomainJob>(QueueConfig::new(8))?;
        let (assessment_sender, mut assessment_receiver) =
            in_memory_queue::<DomainJob>(QueueConfig::new(8))?;
        let (translation_sender, mut translation_receiver) =
            in_memory_queue::<DomainJob>(QueueConfig::new(8))?;
        let runtime = WorkerRuntime::new(CdcFanout::new(
            WorkerQueueRegistry::new()
                .with_queue(WorkerQueue::ProductListingOpenSearch, product_sender)
                .with_queue(WorkerQueue::SearchFilterPercolator, percolator_sender)
                .with_queue(WorkerQueue::ProductListingEmbed, embed_sender)
                .with_queue(
                    WorkerQueue::ProductListingContentAssessment,
                    assessment_sender,
                )
                .with_queue(WorkerQueue::ProductListingTranslate, translation_sender),
        ));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));
        let body = r#"{
            "changes": [
                {
                    "table": "product_listing_events",
                    "operation": "insert",
                    "record": {
                        "event_id": "01900000-0000-7000-8000-000000000004",
                        "product_listing_id": "01900000-0000-7000-8000-000000000003",
                        "event_type": "PRODUCT_LISTING_DISCOVERED",
                        "event_group": "DOMAIN",
                        "event_type_schema_version": 1,
                        "payload": {
                            "listingSourceId": "01900000-0000-7000-8000-000000000001",
                            "sourceListingId": "fixture-source-id",
                            "title": null,
                            "description": null,
                            "pricing": {
                                "price": null,
                                "priceEstimateMin": null,
                                "priceEstimateMax": null
                            },
                            "availability": null,
                            "url": "https://example.test/product",
                            "imageCount": 0,
                            "auction": {"start": null, "end": null}
                        }
                    }
                }
            ]
        }"#;
        let request_text = format!(
            "POST {SEQUIN_CDC_PATH} HTTP/1.1\r\nhost: localhost\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );

        let response = request(addr, &request_text).await?;
        let _send_result = shutdown_tx.send(());
        server.await??;

        assert!(response.starts_with("HTTP/1.1 202 Accepted"));
        assert!(product_receiver.recv().await.is_some());
        assert!(percolator_receiver.recv().await.is_some());
        assert!(embed_receiver.recv().await.is_some());
        assert!(assessment_receiver.recv().await.is_some());
        assert!(translation_receiver.recv().await.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_sequin_cdc_before_ack_when_product_event_ids_are_invalid_or_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, mut receiver) = in_memory_queue::<DomainJob>(QueueConfig::new(1))?;
        let runtime = WorkerRuntime::new(CdcFanout::search_filter_percolator(
            WorkerQueueRegistry::new().with_queue(WorkerQueue::SearchFilterPercolator, sender),
        ));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));
        for body in [
            r#"{
                "changes": [
                    {
                        "table": "product_listing_events",
                        "operation": "insert",
                        "record": {
                            "event_id": "not-a-uuid",
                            "product_listing_id": "01900000-0000-7000-8000-000000000003",
                            "event_type": "PRODUCT_LISTING_DISCOVERED",
                            "event_group": "DOMAIN",
                            "event_type_schema_version": 1
                        }
                    }
                ]
            }"#,
            r#"{
                "changes": [
                    {
                        "table": "product_listing_events",
                        "operation": "insert",
                        "record": {
                            "event_id": "01900000-0000-7000-8000-000000000004",
                            "event_type": "PRODUCT_LISTING_DISCOVERED",
                            "event_group": "DOMAIN",
                            "event_type_schema_version": 1
                        }
                    }
                ]
            }"#,
            r#"{
                "changes": [
                    {
                        "table": "product_listing_events",
                        "operation": "insert",
                        "record": {
                            "event_id": "01900000-0000-7000-8000-000000000004",
                            "product_listing_id": "01900000-0000-7000-8000-000000000003",
                            "event_type": "PRODUCT_LISTING_DISCOVERED",
                            "event_group": "DOMAIN",
                            "event_type_schema_version": 1,
                            "payload": {
                                "listingSourceId": "01900000-0000-7000-8000-000000000001",
                                "sourceListingId": "fixture-source-id",
                                "title": null,
                                "description": null,
                                "pricing": {
                                    "price": null,
                                    "priceEstimateMin": null,
                                    "priceEstimateMax": null
                                },
                                "availability": null,
                                "url": "https://example.test/product",
                                "imageCount": 0,
                                "auction": {"start": null, "end": null},
                                "unexpected": true
                            }
                        }
                    }
                ]
            }"#,
        ] {
            let request_text = format!(
                "POST {SEQUIN_CDC_PATH} HTTP/1.1\r\nhost: localhost\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let response = request(addr, &request_text).await?;
            assert!(response.starts_with("HTTP/1.1 503 Service Unavailable"));
        }
        let _send_result = shutdown_tx.send(());
        server.await??;

        assert!(receiver.recv().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_sequin_cdc_json() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve(listener, async move {
            let _ = shutdown_rx.await;
        }));
        let request_text = format!(
            "POST {SEQUIN_CDC_PATH} HTTP/1.1\r\nhost: localhost\r\ncontent-length: 8\r\n\r\nnot-json"
        );

        let response = request(addr, &request_text).await?;
        let _send_result = shutdown_tx.send(());
        server.await??;

        assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
        Ok(())
    }

    async fn request(
        addr: SocketAddr,
        request_text: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut stream = TcpStream::connect(addr).await?;
        stream.write_all(request_text.as_bytes()).await?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await?;
        Ok(response)
    }
}
