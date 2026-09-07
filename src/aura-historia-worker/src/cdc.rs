use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use domain_primitives::event_id::EventId;
use localization::Language;
use product_listing_core::{
    description::Description, listing_availability::ListingAvailability,
    product_listing_id::ProductListingId, source_listing_id::SourceListingId, title::Title,
};
use product_listing_service::ports::{ProductListingRawRevisionId, ProductListingRawStreamId};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::sync::mpsc;
use tracing::{debug, warn};
use url::Url;
use uuid::Uuid;

use crate::{
    InMemoryQueueReceiver, InMemoryQueueSender, QueueConfig, QueueConfigError, in_memory_queue,
};

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct CdcBatch {
    #[serde(default, alias = "id", alias = "webhook_id")]
    pub delivery_id: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default, alias = "events", alias = "records")]
    pub changes: Vec<CdcChange>,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub struct CdcChange {
    #[serde(default, alias = "table_schema")]
    pub schema: Option<String>,
    #[serde(alias = "relation")]
    pub table: String,
    #[serde(
        alias = "op",
        alias = "action",
        deserialize_with = "deserialize_operation"
    )]
    pub operation: CdcOperation,
    #[serde(default, alias = "keys")]
    pub primary_key: BTreeMap<String, Value>,
    #[serde(default, alias = "new", alias = "new_record")]
    pub record: Option<Value>,
    #[serde(default, rename = "old", alias = "old_record", alias = "previous")]
    pub old_record: Option<Value>,
    #[serde(default, alias = "changed")]
    pub changed_columns: Vec<String>,
    #[serde(default)]
    pub commit_lsn: Option<String>,
    #[serde(default)]
    pub commit_timestamp: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdcOperation {
    Insert,
    Update,
    Delete,
}

impl WorkerQueue {
    pub const ALL: [Self; 11] = [
        Self::ProductListingOpenSearch,
        Self::ProductListingRawNormalization,
        Self::WatchlistNotification,
        Self::SearchFilterPercolator,
        Self::SearchFilterMatchNotification,
        Self::ProductListingContentAssessment,
        Self::ProductListingEmbed,
        Self::ProductListingTranslate,
        Self::SearchFilterOpenSearch,
        Self::UserTierEnforcement,
        Self::NotificationDelivery,
    ];
}

impl Display for CdcOperation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            CdcOperation::Insert => write!(formatter, "insert"),
            CdcOperation::Update => write!(formatter, "update"),
            CdcOperation::Delete => write!(formatter, "delete"),
        }
    }
}

impl FromStr for CdcOperation {
    type Err = CdcOperationParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "insert" | "create" => Ok(Self::Insert),
            "update" | "modify" => Ok(Self::Update),
            "delete" | "remove" => Ok(Self::Delete),
            _ => Err(CdcOperationParseError {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[error("unsupported CDC operation: {value}")]
pub struct CdcOperationParseError {
    value: String,
}

fn deserialize_operation<'de, D>(deserializer: D) -> Result<CdcOperation, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    CdcOperation::from_str(&value).map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainJob {
    pub target_queue: WorkerQueue,
    pub idempotency_key: IdempotencyKey,
    pub ordering_key: OrderingKey,
    pub payload: DomainJobPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerQueue {
    ProductListingOpenSearch,
    ProductListingRawNormalization,
    WatchlistNotification,
    SearchFilterPercolator,
    SearchFilterMatchNotification,
    ProductListingContentAssessment,
    ProductListingEmbed,
    ProductListingTranslate,
    SearchFilterOpenSearch,
    UserTierEnforcement,
    NotificationDelivery,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OrderingKey(String);

impl OrderingKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainJobPayload {
    ProductListingEvent(ProductListingEventJob),
    ProductListingRawRevision(ProductListingRawRevisionJob),
    SearchFilterChanged(SearchFilterChangedJob),
    SearchFilterMatchCreated(SearchFilterMatchCreatedJob),
    UserTierChanged(UserTierChangedJob),
    NotificationDeliveryCreated(NotificationDeliveryCreatedJob),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingEventJob {
    pub event_id: EventId,
    pub product_listing_id: ProductListingId,
}

/// Compact wake-up metadata. The normalizer rereads the immutable revision from PostgreSQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingRawRevisionJob {
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
    pub product_listing_raw_revision_id: ProductListingRawRevisionId,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFilterChangedJob {
    pub user_id: String,
    pub user_search_filter_id: String,
    pub version: i64,
    pub operation: CdcOperation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFilterMatchCreatedJob {
    pub user_id: String,
    pub user_search_filter_id: String,
    pub product_listing_id: String,
    pub origin_event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTierChangedJob {
    pub user_id: String,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationDeliveryCreatedJob {
    pub notification_delivery_id: String,
}

#[derive(Debug, Default, Clone)]
pub struct WorkerQueueRegistry {
    queues: HashMap<WorkerQueue, InMemoryQueueSender<DomainJob>>,
}

impl WorkerQueueRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_all_queues(
        config: QueueConfig,
    ) -> Result<(Self, WorkerQueueReceivers), QueueConfigError> {
        let mut registry = Self::new();
        let mut receivers = WorkerQueueReceivers::new();

        for queue in WorkerQueue::ALL {
            let (sender, receiver) = in_memory_queue::<DomainJob>(config)?;
            registry = registry.with_queue(queue, sender);
            receivers.insert(queue, receiver);
        }

        Ok((registry, receivers))
    }

    pub fn with_queue(
        mut self,
        queue: WorkerQueue,
        sender: InMemoryQueueSender<DomainJob>,
    ) -> Self {
        self.queues.insert(queue, sender);
        self
    }

    async fn enqueue(&self, job: DomainJob) -> Result<(), CdcFanoutError> {
        let Some(sender) = self.queues.get(&job.target_queue) else {
            return Err(CdcFanoutError::MissingQueue(job.target_queue));
        };
        sender
            .enqueue(job)
            .await
            .map_err(|error| CdcFanoutError::QueueClosed(error.0.target_queue))
    }
}

#[derive(Debug, Default)]
pub struct WorkerQueueReceivers {
    receivers: HashMap<WorkerQueue, InMemoryQueueReceiver<DomainJob>>,
}

impl WorkerQueueReceivers {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn insert(
        &mut self,
        queue: WorkerQueue,
        receiver: InMemoryQueueReceiver<DomainJob>,
    ) {
        self.receivers.insert(queue, receiver);
    }

    pub fn take(&mut self, queue: WorkerQueue) -> Option<InMemoryQueueReceiver<DomainJob>> {
        self.receivers.remove(&queue)
    }

    pub async fn recv(&mut self, queue: WorkerQueue) -> Option<DomainJob> {
        self.receivers.get_mut(&queue)?.recv().await
    }

    pub async fn recv_timeout(
        &mut self,
        queue: WorkerQueue,
        duration: std::time::Duration,
    ) -> Result<Option<DomainJob>, tokio::time::error::Elapsed> {
        tokio::time::timeout(duration, self.recv(queue)).await
    }
}

#[derive(Debug, Clone)]
pub struct CdcFanout {
    registry: WorkerQueueRegistry,
    scope: CdcFanoutScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CdcFanoutScope {
    All,
    SearchFilterPercolator,
    SearchFilterMatchNotification,
    SearchFilterProjection,
    WatchlistNotification,
    ProductListingContentAssessment,
    ProductListingTranslation,
    ProductListingEmbedding,
    ProductListingOpenSearch,
    ProductListingRawNormalization,
    NotificationDelivery,
}

impl CdcFanout {
    pub fn new(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::All,
        }
    }

    pub fn watchlist_notification(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::WatchlistNotification,
        }
    }

    pub fn search_filter_percolator(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::SearchFilterPercolator,
        }
    }

    pub fn search_filter_match_notification(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::SearchFilterMatchNotification,
        }
    }

    pub fn search_filter_projection(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::SearchFilterProjection,
        }
    }

    pub fn product_content_assessment(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::ProductListingContentAssessment,
        }
    }

    pub fn product_translation(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::ProductListingTranslation,
        }
    }

    pub fn product_embedding(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::ProductListingEmbedding,
        }
    }

    pub fn product_listing_opensearch(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::ProductListingOpenSearch,
        }
    }

    pub fn product_listing_raw_normalization(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::ProductListingRawNormalization,
        }
    }

    pub fn notification_delivery(registry: WorkerQueueRegistry) -> Self {
        Self {
            registry,
            scope: CdcFanoutScope::NotificationDelivery,
        }
    }

    pub async fn ingest_json(&self, body: &str) -> Result<usize, CdcIngestError> {
        let batch = parse_cdc_batch(body).map_err(CdcIngestError::InvalidJson)?;
        self.ingest_batch(&batch).await
    }

    pub async fn ingest_batch(&self, batch: &CdcBatch) -> Result<usize, CdcIngestError> {
        let mut enqueued = 0;

        for change in &batch.changes {
            for job in self.route_change(change)? {
                self.registry.enqueue(job).await?;
                enqueued += 1;
            }
        }

        debug!(
            changes = batch.changes.len(),
            enqueued, "CDC batch fanned out"
        );
        Ok(enqueued)
    }

    fn route_change(&self, change: &CdcChange) -> Result<Vec<DomainJob>, CdcRouteError> {
        match self.scope {
            CdcFanoutScope::All => route_change(change),
            CdcFanoutScope::WatchlistNotification => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingEvents
                ) && change.operation == CdcOperation::Insert
                {
                    let jobs = product_event_jobs(change)?;
                    Ok(jobs
                        .into_iter()
                        .filter(|job| job.target_queue == WorkerQueue::WatchlistNotification)
                        .collect())
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::SearchFilterPercolator => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingEvents
                ) && change.operation == CdcOperation::Insert
                {
                    let jobs = product_event_jobs(change)?;
                    Ok(jobs
                        .into_iter()
                        .filter(|job| job.target_queue == WorkerQueue::SearchFilterPercolator)
                        .collect())
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::SearchFilterMatchNotification => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::SearchFilterMatches
                ) && change.operation == CdcOperation::Insert
                {
                    search_filter_match_created_job(change)
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::SearchFilterProjection => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::SearchFilters
                ) {
                    search_filter_changed_job(change, change.operation)
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::ProductListingOpenSearch => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingEvents
                ) && change.operation == CdcOperation::Insert
                {
                    let jobs = product_event_jobs(change)?;
                    Ok(jobs
                        .into_iter()
                        .filter(|job| job.target_queue == WorkerQueue::ProductListingOpenSearch)
                        .collect())
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::ProductListingEmbedding => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingEvents
                ) && change.operation == CdcOperation::Insert
                {
                    let jobs = product_event_jobs(change)?;
                    Ok(jobs
                        .into_iter()
                        .filter(|job| job.target_queue == WorkerQueue::ProductListingEmbed)
                        .collect())
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::ProductListingContentAssessment => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingEvents
                ) && change.operation == CdcOperation::Insert
                {
                    let jobs = product_event_jobs(change)?;
                    Ok(jobs
                        .into_iter()
                        .filter(|job| {
                            job.target_queue == WorkerQueue::ProductListingContentAssessment
                        })
                        .collect())
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::ProductListingTranslation => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingEvents
                ) && change.operation == CdcOperation::Insert
                {
                    let jobs = product_event_jobs(change)?;
                    Ok(jobs
                        .into_iter()
                        .filter(|job| job.target_queue == WorkerQueue::ProductListingTranslate)
                        .collect())
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::ProductListingRawNormalization => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::ProductListingRawRevisions
                ) && change.operation == CdcOperation::Insert
                {
                    product_listing_raw_revision_job(change)
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
            CdcFanoutScope::NotificationDelivery => {
                if matches!(
                    CdcTable::from(change.table.as_str()),
                    CdcTable::NotificationDeliveries
                ) && change.operation == CdcOperation::Insert
                {
                    notification_delivery_created_job(change)
                } else {
                    Err(CdcRouteError::UnsupportedTableForWorker(
                        change.table.clone(),
                    ))
                }
            }
        }
    }
}

fn parse_cdc_batch(body: &str) -> Result<CdcBatch, serde_json::Error> {
    let value: Value = serde_json::from_str(body)?;

    if value.get("data").is_some() {
        return serde_json::from_value::<SequinWebhookBatch>(value).map(Into::into);
    }

    if value.get("metadata").is_some() && value.get("record").is_some() {
        return serde_json::from_value::<SequinWebhookMessage>(value).map(CdcBatch::from);
    }

    serde_json::from_value(value)
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
struct SequinWebhookBatch {
    data: Vec<SequinWebhookMessage>,
}

impl From<SequinWebhookBatch> for CdcBatch {
    fn from(batch: SequinWebhookBatch) -> Self {
        Self {
            delivery_id: None,
            source: Some("sequin-webhook".to_owned()),
            changes: batch.data.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
struct SequinWebhookMessage {
    record: Option<Value>,
    #[serde(default)]
    changes: Option<Map<String, Value>>,
    #[serde(deserialize_with = "deserialize_operation")]
    action: CdcOperation,
    metadata: SequinWebhookMetadata,
}

impl From<SequinWebhookMessage> for CdcBatch {
    fn from(message: SequinWebhookMessage) -> Self {
        Self {
            delivery_id: None,
            source: Some("sequin-webhook".to_owned()),
            changes: vec![message.into()],
        }
    }
}

impl From<SequinWebhookMessage> for CdcChange {
    fn from(message: SequinWebhookMessage) -> Self {
        let SequinWebhookMessage {
            record,
            changes,
            action,
            metadata,
        } = message;
        let changed_columns = changes
            .as_ref()
            .map(|changes| changes.keys().cloned().collect())
            .unwrap_or_default();
        let (record, old_record) = match action {
            CdcOperation::Delete => (None, record.or_else(|| changes.map(Value::Object))),
            CdcOperation::Insert | CdcOperation::Update => (record, changes.map(Value::Object)),
        };

        Self {
            schema: Some(metadata.table_schema),
            table: metadata.table_name,
            operation: action,
            primary_key: BTreeMap::new(),
            record,
            old_record,
            changed_columns,
            commit_lsn: metadata.commit_lsn.map(|value| match value {
                Value::String(value) => value,
                other => other.to_string(),
            }),
            commit_timestamp: metadata.commit_timestamp,
        }
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
struct SequinWebhookMetadata {
    table_schema: String,
    table_name: String,
    #[serde(default)]
    commit_lsn: Option<Value>,
    #[serde(default)]
    commit_timestamp: Option<String>,
}

pub fn route_change(change: &CdcChange) -> Result<Vec<DomainJob>, CdcRouteError> {
    let table = CdcTable::from(change.table.as_str());
    match (table, change.operation) {
        (CdcTable::ProductListingEvents, CdcOperation::Insert) => product_event_jobs(change),
        (CdcTable::ProductListingEvents, _) => Ok(Vec::new()),
        (CdcTable::ProductListingRawRevisions, CdcOperation::Insert) => {
            product_listing_raw_revision_job(change)
        }
        (CdcTable::ProductListingRawRevisions, _) => Ok(Vec::new()),
        (CdcTable::SearchFilters, operation) => search_filter_changed_job(change, operation),
        (CdcTable::SearchFilterMatches, CdcOperation::Insert) => {
            search_filter_match_created_job(change)
        }
        (CdcTable::SearchFilterMatches, _) => Ok(Vec::new()),
        (CdcTable::Users, CdcOperation::Update) => user_tier_changed_job(change),
        (CdcTable::Users, _) => Ok(Vec::new()),
        (CdcTable::NotificationDeliveries, CdcOperation::Insert) => {
            notification_delivery_created_job(change)
        }
        (CdcTable::NotificationDeliveries, _) => Ok(Vec::new()),
        (CdcTable::Unknown(table), _) => {
            warn!(%table, operation = %change.operation, "ignoring unregistered CDC table");
            Ok(Vec::new())
        }
        (CdcTable::ProductListings | CdcTable::ProductListingWatchlist, _) => Ok(Vec::new()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CdcTable {
    ProductListingEvents,
    ProductListingRawRevisions,
    ProductListings,
    SearchFilters,
    SearchFilterMatches,
    Users,
    ProductListingWatchlist,
    NotificationDeliveries,
    Unknown(String),
}

impl From<&str> for CdcTable {
    fn from(value: &str) -> Self {
        match value {
            "product_listing_events" => Self::ProductListingEvents,
            "product_listing_raw_revisions" => Self::ProductListingRawRevisions,
            "product_listings" => Self::ProductListings,
            "search_filters" => Self::SearchFilters,
            "search_filter_matches" => Self::SearchFilterMatches,
            "users" => Self::Users,
            "product_listing_watchlist" => Self::ProductListingWatchlist,
            "notification_deliveries" => Self::NotificationDeliveries,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

const PRODUCT_LISTING_EVENT_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProductListingRoutingEventKind {
    Discovered,
    Changed,
    Embedded,
    TranslatedTitles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProductListingEventRoutingFacts {
    event_id: EventId,
    product_listing_id: ProductListingId,
    event_kind: ProductListingRoutingEventKind,
    has_main_price_change: bool,
    has_availability_change: bool,
    has_image_change: bool,
}

fn product_event_routing_facts(
    change: &CdcChange,
) -> Result<ProductListingEventRoutingFacts, CdcRouteError> {
    let row = required_row(change)?;
    let event_id_value = required_product_event_string(row, "event_id")?;
    let event_id =
        EventId::try_from(event_id_value.as_str()).map_err(|_| CdcRouteError::InvalidEventId)?;
    if event_id.to_string() != event_id_value {
        return Err(CdcRouteError::InvalidEventId);
    }
    let product_listing_id_value = required_product_event_string(row, "product_listing_id")?;
    let product_listing_id = ProductListingId::try_from(product_listing_id_value.as_str())
        .map_err(|_| CdcRouteError::InvalidProductListingId)?;
    if product_listing_id.to_string() != product_listing_id_value {
        return Err(CdcRouteError::InvalidProductListingId);
    }
    let event_type = required_product_event_string(row, "event_type")?;
    let event_group = required_product_event_string(row, "event_group")?;
    let schema_version = required_integer(row, "event_type_schema_version")?;
    let payload = row
        .get("payload")
        .ok_or(CdcRouteError::MissingColumn("payload"))?;

    if schema_version != PRODUCT_LISTING_EVENT_SCHEMA_VERSION {
        return Err(CdcRouteError::UnsupportedProductListingEventSchemaVersion { schema_version });
    }

    let event_kind = match (event_type.as_str(), event_group.as_str()) {
        ("PRODUCT_LISTING_DISCOVERED", "DOMAIN") => {
            validate_discovered_payload(payload)?;
            ProductListingRoutingEventKind::Discovered
        }
        ("PRODUCT_LISTING_CHANGED", "DOMAIN") => {
            let (has_main_price_change, has_availability_change, has_image_change) =
                validate_changed_payload(payload)?;
            return Ok(ProductListingEventRoutingFacts {
                event_id,
                product_listing_id,
                event_kind: ProductListingRoutingEventKind::Changed,
                has_main_price_change,
                has_availability_change,
                has_image_change,
            });
        }
        ("ENRICHMENT_EMBEDDED", "ENRICHMENT") => {
            validate_embedded_payload(payload)?;
            ProductListingRoutingEventKind::Embedded
        }
        ("ENRICHMENT_TRANSLATED_TITLES", "ENRICHMENT") => {
            validate_translated_payload(payload)?;
            ProductListingRoutingEventKind::TranslatedTitles
        }
        ("PRODUCT_LISTING_DISCOVERED" | "PRODUCT_LISTING_CHANGED", _)
        | ("ENRICHMENT_EMBEDDED" | "ENRICHMENT_TRANSLATED_TITLES", _) => {
            return Err(CdcRouteError::UnsupportedProductListingEvent {
                event_type,
                event_group,
            });
        }
        _ => {
            return Err(CdcRouteError::UnsupportedProductListingEvent {
                event_type,
                event_group,
            });
        }
    };

    Ok(ProductListingEventRoutingFacts {
        event_id,
        product_listing_id,
        event_kind,
        has_main_price_change: false,
        has_availability_change: false,
        has_image_change: false,
    })
}

fn product_event_jobs(change: &CdcChange) -> Result<Vec<DomainJob>, CdcRouteError> {
    let facts = product_event_routing_facts(change)?;
    let base_job = ProductListingEventJob {
        event_id: facts.event_id,
        product_listing_id: facts.product_listing_id,
    };
    let idempotency_key = IdempotencyKey::new(format!("product-event:{}", facts.event_id));
    let ordering_key = OrderingKey::new(format!("product:{}", facts.product_listing_id));

    let mut jobs = vec![domain_job(
        WorkerQueue::ProductListingOpenSearch,
        idempotency_key.clone(),
        ordering_key.clone(),
        DomainJobPayload::ProductListingEvent(base_job.clone()),
    )];
    jobs.push(domain_job(
        WorkerQueue::SearchFilterPercolator,
        idempotency_key.clone(),
        ordering_key.clone(),
        DomainJobPayload::ProductListingEvent(base_job.clone()),
    ));

    match facts.event_kind {
        ProductListingRoutingEventKind::Discovered => {
            for target_queue in [
                WorkerQueue::ProductListingContentAssessment,
                WorkerQueue::ProductListingEmbed,
                WorkerQueue::ProductListingTranslate,
            ] {
                jobs.push(domain_job(
                    target_queue,
                    idempotency_key.clone(),
                    ordering_key.clone(),
                    DomainJobPayload::ProductListingEvent(base_job.clone()),
                ));
            }
        }
        ProductListingRoutingEventKind::Changed => {
            if facts.has_main_price_change || facts.has_availability_change {
                jobs.push(domain_job(
                    WorkerQueue::WatchlistNotification,
                    idempotency_key.clone(),
                    ordering_key.clone(),
                    DomainJobPayload::ProductListingEvent(base_job.clone()),
                ));
            }
            if facts.has_image_change {
                jobs.push(domain_job(
                    WorkerQueue::ProductListingEmbed,
                    idempotency_key,
                    ordering_key,
                    DomainJobPayload::ProductListingEvent(base_job),
                ));
            }
        }
        ProductListingRoutingEventKind::Embedded
        | ProductListingRoutingEventKind::TranslatedTitles => {}
    }

    Ok(jobs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProductListingLifecycleRoutingChange {
    Withdrawn,
    Restored,
}

fn validate_discovered_payload(value: &Value) -> Result<(), CdcRouteError> {
    let object = required_payload_object(value)?;
    require_exact_keys(
        object,
        &[
            "listingSourceId",
            "sourceListingId",
            "title",
            "description",
            "pricing",
            "availability",
            "url",
            "imageCount",
            "auction",
        ],
        "payload",
    )?;
    validate_listing_source_id(&require_string(object, "listingSourceId")?)?;
    validate_source_listing_id(&require_string(object, "sourceListingId")?)?;
    require_nullable_localized(object, "title", canonical_title)?;
    require_nullable_localized(object, "description", canonical_description)?;

    let pricing = require_object_field(object, "pricing")?;
    require_exact_keys(
        pricing,
        &["price", "priceEstimateMin", "priceEstimateMax"],
        "pricing",
    )?;
    for field in ["price", "priceEstimateMin", "priceEstimateMax"] {
        validate_price(
            pricing
                .get(field)
                .ok_or(CdcRouteError::MissingColumn(field))?,
        )?;
    }
    validate_nullable_availability(require_value(object, "availability")?, "availability")?;
    validate_canonical_url(&require_string(object, "url")?, "url")?;
    require_u64(object, "imageCount")?;
    validate_auction(require_value(object, "auction")?, "auction")?;
    Ok(())
}

fn validate_changed_payload(value: &Value) -> Result<(bool, bool, bool), CdcRouteError> {
    let object = required_payload_object(value)?;
    if object.is_empty() {
        return Err(CdcRouteError::InvalidProductListingEventPayload);
    }
    require_known_keys(
        object,
        &[
            "pricing",
            "availability",
            "url",
            "images",
            "auction",
            "lifecycle",
            "saleObservation",
        ],
        "payload",
    )?;

    let mut has_main_price_change = false;
    let mut has_availability_change = false;
    let mut has_image_change = false;
    let mut availability_change = None;
    let mut lifecycle_change = None;
    for (field, value) in object {
        match field.as_str() {
            "pricing" => {
                let pricing = required_object(value)?;
                if pricing.is_empty() {
                    return Err(CdcRouteError::InvalidProductListingEventPayload);
                }
                require_known_keys(
                    pricing,
                    &["price", "priceEstimateMin", "priceEstimateMax"],
                    "pricing",
                )?;
                for (pricing_field, value) in pricing {
                    match pricing_field.as_str() {
                        "price" => {
                            validate_price_change(value)?;
                            has_main_price_change = true;
                        }
                        "priceEstimateMin" | "priceEstimateMax" => {
                            validate_price_change(value)?;
                        }
                        _ => return Err(CdcRouteError::InvalidProductListingEventPayload),
                    }
                }
            }
            "availability" => {
                availability_change = Some(validate_code_change(value)?);
                has_availability_change = true;
            }
            "url" => validate_string_change(value)?,
            "images" => {
                let images = required_object(value)?;
                require_exact_keys(images, &["previousCount", "currentCount"], "images")?;
                require_u64(images, "previousCount")?;
                require_u64(images, "currentCount")?;
                has_image_change = true;
            }
            "auction" => validate_auction_change(value)?,
            "lifecycle" => lifecycle_change = Some(validate_lifecycle(value)?),
            "saleObservation" => validate_sale_observation(value)?,
            _ => return Err(CdcRouteError::InvalidProductListingEventPayload),
        }
    }

    match (lifecycle_change, availability_change) {
        (Some(ProductListingLifecycleRoutingChange::Withdrawn), Some((_, Some(_)))) => {
            return Err(CdcRouteError::InconsistentProductListingEvent {
                rule: "withdrawal with current availability".to_owned(),
            });
        }
        (Some(ProductListingLifecycleRoutingChange::Restored), Some((Some(_), _))) => {
            return Err(CdcRouteError::InconsistentProductListingEvent {
                rule: "restoration with previous availability".to_owned(),
            });
        }
        _ => {}
    }

    Ok((
        has_main_price_change,
        has_availability_change,
        has_image_change,
    ))
}

fn validate_price_change(value: &Value) -> Result<(), CdcRouteError> {
    let change = required_object(value)?;
    require_exact_keys(change, &["previous", "current"], "price change")?;
    let previous = require_value(change, "previous")?;
    let current = require_value(change, "current")?;
    validate_price(previous)?;
    validate_price(current)?;
    if previous == current {
        return Err(CdcRouteError::InvalidProductListingEventPayload);
    }
    Ok(())
}

fn validate_price(value: &Value) -> Result<(), CdcRouteError> {
    if value.is_null() {
        return Ok(());
    }
    let object = required_object(value)?;
    require_exact_keys(object, &["amount", "currency"], "price")?;
    require_u64(object, "amount")?;
    let currency = require_string(object, "currency")?;
    let parsed = money::Currency::from_code(currency.as_str())
        .ok_or_else(|| invalid_product_listing_field("price.currency"))?;
    if parsed.as_str() != currency {
        return Err(noncanonical_product_listing_field("price.currency"));
    }
    Ok(())
}

fn validate_code_change(
    value: &Value,
) -> Result<(Option<ListingAvailability>, Option<ListingAvailability>), CdcRouteError> {
    let change = required_object(value)?;
    require_exact_keys(change, &["previous", "current"], "availability change")?;
    let previous = validate_nullable_availability(
        require_value(change, "previous")?,
        "availability.previous",
    )?;
    let current =
        validate_nullable_availability(require_value(change, "current")?, "availability.current")?;
    if previous == current {
        return Err(CdcRouteError::InvalidProductListingEventPayload);
    }
    Ok((previous, current))
}

fn validate_string_change(value: &Value) -> Result<(), CdcRouteError> {
    let change = required_object(value)?;
    require_exact_keys(change, &["previous", "current"], "url change")?;
    let previous = validate_canonical_url(&require_string(change, "previous")?, "url.previous")?;
    let current = validate_canonical_url(&require_string(change, "current")?, "url.current")?;
    if previous == current {
        return Err(CdcRouteError::InvalidProductListingEventPayload);
    }
    Ok(())
}

fn validate_auction_change(value: &Value) -> Result<(), CdcRouteError> {
    let change = required_object(value)?;
    require_exact_keys(change, &["previous", "current"], "auction change")?;
    let previous = validate_auction(require_value(change, "previous")?, "auction.previous")?;
    let current = validate_auction(require_value(change, "current")?, "auction.current")?;
    if previous == current {
        return Err(CdcRouteError::InvalidProductListingEventPayload);
    }
    Ok(())
}

fn validate_auction(
    value: &Value,
    field: &str,
) -> Result<(Option<OffsetDateTime>, Option<OffsetDateTime>), CdcRouteError> {
    let object = required_object(value)?;
    require_exact_keys(object, &["start", "end"], field)?;
    let start =
        parse_nullable_timestamp(require_value(object, "start")?, &format!("{field}.start"))?;
    let end = parse_nullable_timestamp(require_value(object, "end")?, &format!("{field}.end"))?;
    if start.zip(end).is_some_and(|(start, end)| start > end) {
        return Err(CdcRouteError::InconsistentProductListingEvent {
            rule: format!("{field} start after end"),
        });
    }
    Ok((start, end))
}

fn validate_lifecycle(
    value: &Value,
) -> Result<ProductListingLifecycleRoutingChange, CdcRouteError> {
    let object = required_object(value)?;
    let transition = require_string(object, "transition")?;
    match transition.as_str() {
        "WITHDRAWN" => {
            require_exact_keys(object, &["transition", "previousAvailability"], "lifecycle")?;
            validate_nullable_availability(
                require_value(object, "previousAvailability")?,
                "lifecycle.previousAvailability",
            )?;
            Ok(ProductListingLifecycleRoutingChange::Withdrawn)
        }
        "RESTORED" => {
            require_exact_keys(object, &["transition"], "lifecycle")?;
            Ok(ProductListingLifecycleRoutingChange::Restored)
        }
        _ => Err(invalid_product_listing_field("lifecycle.transition")),
    }
}

fn validate_sale_observation(value: &Value) -> Result<(), CdcRouteError> {
    let object = required_object(value)?;
    require_exact_keys(object, &["transition", "observation"], "saleObservation")?;
    let transition = require_string(object, "transition")?;
    if !matches!(transition.as_str(), "OBSERVED" | "RETRACTED") {
        return Err(invalid_product_listing_field("saleObservation.transition"));
    }
    let observation = require_object_field(object, "observation")?;
    require_exact_keys(
        observation,
        &["observedAt", "fxRateId"],
        "saleObservation.observation",
    )?;
    parse_canonical_timestamp(
        &require_string(observation, "observedAt")?,
        "saleObservation.observedAt",
    )?;
    let fx_rate_id =
        fxrate_core::FxRateId::try_from(require_string(observation, "fxRateId")?.as_str())
            .map_err(|_| invalid_product_listing_field("saleObservation.fxRateId"))?;
    if fx_rate_id.to_string() != require_string(observation, "fxRateId")? {
        return Err(noncanonical_product_listing_field(
            "saleObservation.fxRateId",
        ));
    }
    Ok(())
}

fn validate_embedded_payload(value: &Value) -> Result<(), CdcRouteError> {
    let object = required_payload_object(value)?;
    require_exact_keys(object, &["sourceEventId"], "payload")?;
    validate_canonical_event_id(&require_string(object, "sourceEventId")?)
}

fn validate_translated_payload(value: &Value) -> Result<(), CdcRouteError> {
    let object = required_payload_object(value)?;
    require_exact_keys(
        object,
        &["sourceEventId", "sourceLanguage", "targetLanguages"],
        "payload",
    )?;
    validate_canonical_event_id(&require_string(object, "sourceEventId")?)?;
    let source_language = require_string(object, "sourceLanguage")?;
    validate_language(&source_language, "sourceLanguage")?;
    let target_languages = object
        .get("targetLanguages")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_product_listing_field("targetLanguages"))?;
    if target_languages.is_empty() {
        return Err(invalid_product_listing_field("targetLanguages"));
    }
    for language in target_languages {
        let language = language
            .as_str()
            .ok_or_else(|| invalid_product_listing_field("targetLanguages"))?;
        validate_language(language, "targetLanguages")?;
    }
    Ok(())
}

fn required_payload_object(value: &Value) -> Result<&Map<String, Value>, CdcRouteError> {
    required_object(value)
}

fn required_object(value: &Value) -> Result<&Map<String, Value>, CdcRouteError> {
    value
        .as_object()
        .ok_or(CdcRouteError::InvalidProductListingEventPayload)
}

fn require_exact_keys(
    object: &Map<String, Value>,
    expected: &[&str],
    context: &str,
) -> Result<(), CdcRouteError> {
    for field in expected {
        if !object.contains_key(*field) {
            return Err(CdcRouteError::MissingProductListingEventField {
                field: format!("{context}.{field}"),
            });
        }
    }
    for field in object.keys() {
        if !expected.contains(&field.as_str()) {
            return Err(CdcRouteError::UnknownProductListingEventField {
                field: format!("{context}.{field}"),
            });
        }
    }
    Ok(())
}

fn require_known_keys(
    object: &Map<String, Value>,
    expected: &[&str],
    context: &str,
) -> Result<(), CdcRouteError> {
    for field in object.keys() {
        if !expected.contains(&field.as_str()) {
            return Err(CdcRouteError::UnknownProductListingEventField {
                field: format!("{context}.{field}"),
            });
        }
    }
    Ok(())
}

fn require_value<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a Value, CdcRouteError> {
    object.get(field).ok_or(CdcRouteError::MissingColumn(field))
}

fn require_object_field<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a Map<String, Value>, CdcRouteError> {
    required_object(require_value(object, field)?)
}

fn require_string(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<String, CdcRouteError> {
    require_value(object, field)?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or(CdcRouteError::InvalidProductListingEventPayload)
}

fn require_u64(object: &Map<String, Value>, field: &'static str) -> Result<u64, CdcRouteError> {
    require_value(object, field)?
        .as_u64()
        .ok_or(CdcRouteError::InvalidProductListingEventPayload)
}

fn require_nullable_localized(
    object: &Map<String, Value>,
    field: &'static str,
    canonicalize: fn(&str) -> String,
) -> Result<(), CdcRouteError> {
    let value = require_value(object, field)?;
    if value.is_null() {
        return Ok(());
    }
    let localized = required_object(value)?;
    require_exact_keys(localized, &["language", "text"], field)?;
    let language = require_string(localized, "language")?;
    validate_language(&language, &format!("{field}.language"))?;
    let text = require_string(localized, "text")?;
    if canonicalize(text.as_str()) != text {
        return Err(noncanonical_product_listing_field(format!("{field}.text")));
    }
    Ok(())
}

fn canonical_title(value: &str) -> String {
    Title::from(value).to_string()
}

fn canonical_description(value: &str) -> String {
    Description::from(value).to_string()
}

fn validate_listing_source_id(value: &str) -> Result<(), CdcRouteError> {
    let id =
        Uuid::parse_str(value).map_err(|_| invalid_product_listing_field("listingSourceId"))?;
    if id.to_string() != value {
        return Err(noncanonical_product_listing_field("listingSourceId"));
    }
    Ok(())
}

fn validate_source_listing_id(value: &str) -> Result<(), CdcRouteError> {
    let id = SourceListingId::try_from(value)
        .map_err(|_| invalid_product_listing_field("sourceListingId"))?;
    if id.as_ref() != value {
        return Err(noncanonical_product_listing_field("sourceListingId"));
    }
    Ok(())
}

fn validate_canonical_event_id(value: &str) -> Result<(), CdcRouteError> {
    let id =
        EventId::try_from(value).map_err(|_| invalid_product_listing_field("sourceEventId"))?;
    if id.to_string() != value {
        return Err(noncanonical_product_listing_field("sourceEventId"));
    }
    Ok(())
}

fn validate_language(value: &str, field: &str) -> Result<(), CdcRouteError> {
    Language::from_code(value)
        .map(|_| ())
        .ok_or_else(|| invalid_product_listing_field(field))
}

fn validate_nullable_availability(
    value: &Value,
    field: &str,
) -> Result<Option<ListingAvailability>, CdcRouteError> {
    if value.is_null() {
        return Ok(None);
    }
    let value = value
        .as_str()
        .ok_or_else(|| invalid_product_listing_field(field))?;
    let availability = ListingAvailability::from_code(value)
        .ok_or_else(|| invalid_product_listing_field(field))?;
    if availability.as_str() != value {
        return Err(noncanonical_product_listing_field(field));
    }
    Ok(Some(availability))
}

fn validate_canonical_url(value: &str, field: &str) -> Result<Url, CdcRouteError> {
    let url = Url::parse(value).map_err(|_| invalid_product_listing_field(field))?;
    if url.as_str() != value {
        return Err(noncanonical_product_listing_field(field));
    }
    Ok(url)
}

fn parse_nullable_timestamp(
    value: &Value,
    field: &str,
) -> Result<Option<OffsetDateTime>, CdcRouteError> {
    if value.is_null() {
        return Ok(None);
    }
    let value = value
        .as_str()
        .ok_or_else(|| invalid_product_listing_field(field))?;
    parse_canonical_timestamp(value, field).map(Some)
}

fn parse_canonical_timestamp(value: &str, field: &str) -> Result<OffsetDateTime, CdcRouteError> {
    let timestamp =
        OffsetDateTime::parse(value, &Rfc3339).map_err(|_| invalid_product_listing_field(field))?;
    let canonical = timestamp
        .format(&Rfc3339)
        .map_err(|_| invalid_product_listing_field(field))?;
    if canonical != value {
        return Err(noncanonical_product_listing_field(field));
    }
    Ok(timestamp)
}

fn invalid_product_listing_field(field: impl Into<String>) -> CdcRouteError {
    CdcRouteError::InvalidProductListingEventField {
        field: field.into(),
    }
}

fn noncanonical_product_listing_field(field: impl Into<String>) -> CdcRouteError {
    CdcRouteError::NonCanonicalProductListingEventField {
        field: field.into(),
    }
}

fn required_product_event_string(
    row: &Value,
    field: &'static str,
) -> Result<String, CdcRouteError> {
    row.as_object()
        .ok_or(CdcRouteError::MissingRow)?
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or(CdcRouteError::MissingColumn(field))
}

fn search_filter_changed_job(
    change: &CdcChange,
    operation: CdcOperation,
) -> Result<Vec<DomainJob>, CdcRouteError> {
    let row = row_for_operation(change)?;
    let user_search_filter_id = required_string(row, "user_search_filter_id")?;
    let user_id = required_string(row, "user_id")?;
    let version = required_integer(row, "version")?;

    Ok(vec![domain_job(
        WorkerQueue::SearchFilterOpenSearch,
        IdempotencyKey::new(format!(
            "search-filter:{user_search_filter_id}:{version}:{operation}"
        )),
        OrderingKey::new(format!("search-filter:{user_search_filter_id}")),
        DomainJobPayload::SearchFilterChanged(SearchFilterChangedJob {
            user_id,
            user_search_filter_id,
            version,
            operation,
        }),
    )])
}

fn search_filter_match_created_job(change: &CdcChange) -> Result<Vec<DomainJob>, CdcRouteError> {
    let row = required_row(change)?;
    let user_id = required_string(row, "user_id")?;
    let user_search_filter_id = required_string(row, "user_search_filter_id")?;
    let product_listing_id = required_string(row, "product_listing_id")?;
    let origin_event_id = required_string(row, "origin_event_id")?;

    Ok(vec![domain_job(
        WorkerQueue::SearchFilterMatchNotification,
        IdempotencyKey::new(format!(
            "search-filter-match:{user_id}:{user_search_filter_id}:{product_listing_id}:{origin_event_id}"
        )),
        OrderingKey::new(format!("user:{user_id}")),
        DomainJobPayload::SearchFilterMatchCreated(SearchFilterMatchCreatedJob {
            user_id,
            user_search_filter_id,
            product_listing_id,
            origin_event_id,
        }),
    )])
}

fn product_listing_raw_revision_job(change: &CdcChange) -> Result<Vec<DomainJob>, CdcRouteError> {
    let row = required_row(change)?;
    let stream_id_value = required_string(row, "product_listing_raw_stream_id")?;
    let stream_id = uuid::Uuid::parse_str(&stream_id_value)
        .map(ProductListingRawStreamId::from_uuid)
        .map_err(|_| CdcRouteError::InvalidProductListingRawStreamId)?;
    if stream_id.as_uuid().to_string() != stream_id_value {
        return Err(CdcRouteError::InvalidProductListingRawStreamId);
    }
    let revision_id_value = required_string(row, "product_listing_raw_revision_id")?;
    let revision_id = uuid::Uuid::parse_str(&revision_id_value)
        .map(ProductListingRawRevisionId::from_uuid)
        .map_err(|_| CdcRouteError::InvalidProductListingRawRevisionId)?;
    if revision_id.as_uuid().to_string() != revision_id_value {
        return Err(CdcRouteError::InvalidProductListingRawRevisionId);
    }
    let revision = required_integer(row, "revision")?;
    let revision =
        u64::try_from(revision).map_err(|_| CdcRouteError::InvalidProductListingRawRevision)?;
    if revision == 0 {
        return Err(CdcRouteError::InvalidProductListingRawRevision);
    }

    Ok(vec![domain_job(
        WorkerQueue::ProductListingRawNormalization,
        IdempotencyKey::new(format!("product-listing-raw-revision:{revision_id_value}")),
        OrderingKey::new(format!("product-listing-raw-stream:{stream_id_value}")),
        DomainJobPayload::ProductListingRawRevision(ProductListingRawRevisionJob {
            product_listing_raw_stream_id: stream_id,
            product_listing_raw_revision_id: revision_id,
            revision,
        }),
    )])
}

fn notification_delivery_created_job(change: &CdcChange) -> Result<Vec<DomainJob>, CdcRouteError> {
    let row = required_row(change)?;
    let notification_delivery_id = required_string(row, "notification_delivery_id")?;

    Ok(vec![domain_job(
        WorkerQueue::NotificationDelivery,
        IdempotencyKey::new(format!("notification-delivery:{notification_delivery_id}")),
        OrderingKey::new(format!("notification-delivery:{notification_delivery_id}")),
        DomainJobPayload::NotificationDeliveryCreated(NotificationDeliveryCreatedJob {
            notification_delivery_id,
        }),
    )])
}

fn user_tier_changed_job(change: &CdcChange) -> Result<Vec<DomainJob>, CdcRouteError> {
    if !has_tier_change(change) {
        return Ok(Vec::new());
    }

    let row = required_row(change)?;
    let user_id = required_string(row, "user_id")?;
    let version = integer_field(row, "version").unwrap_or(0);

    Ok(vec![domain_job(
        WorkerQueue::UserTierEnforcement,
        IdempotencyKey::new(format!("user-tier:{user_id}:{version}")),
        OrderingKey::new(format!("user:{user_id}")),
        DomainJobPayload::UserTierChanged(UserTierChangedJob { user_id, version }),
    )])
}

fn domain_job(
    target_queue: WorkerQueue,
    idempotency_key: IdempotencyKey,
    ordering_key: OrderingKey,
    payload: DomainJobPayload,
) -> DomainJob {
    DomainJob {
        target_queue,
        idempotency_key,
        ordering_key,
        payload,
    }
}

fn row_for_operation(change: &CdcChange) -> Result<&Value, CdcRouteError> {
    match change.operation {
        CdcOperation::Delete => change.old_record.as_ref().ok_or(CdcRouteError::MissingRow),
        CdcOperation::Insert | CdcOperation::Update => required_row(change),
    }
}

fn required_row(change: &CdcChange) -> Result<&Value, CdcRouteError> {
    change.record.as_ref().ok_or(CdcRouteError::MissingRow)
}

fn required_string(row: &Value, field: &'static str) -> Result<String, CdcRouteError> {
    string_field(row, field).ok_or(CdcRouteError::MissingColumn(field))
}

fn string_field(row: &Value, field: &str) -> Option<String> {
    let value = row.get(field)?;
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn integer_field(row: &Value, field: &str) -> Option<i64> {
    row.get(field)?.as_i64()
}

fn required_integer(row: &Value, field: &'static str) -> Result<i64, CdcRouteError> {
    integer_field(row, field).ok_or(CdcRouteError::MissingColumn(field))
}

fn has_tier_change(change: &CdcChange) -> bool {
    if !change.changed_columns.is_empty() {
        return change.changed_columns.iter().any(|column| column == "tier");
    }

    let Some(new) = &change.record else {
        return false;
    };
    let Some(old) = &change.old_record else {
        return true;
    };

    string_field(new, "tier") != string_field(old, "tier")
}

#[derive(thiserror::Error, Debug)]
pub enum CdcIngestError {
    #[error("invalid CDC JSON")]
    InvalidJson(#[source] serde_json::Error),
    #[error(transparent)]
    Route(#[from] CdcRouteError),
    #[error(transparent)]
    Fanout(#[from] CdcFanoutError),
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum CdcRouteError {
    #[error("CDC table is not configured for this worker: {0}")]
    UnsupportedTableForWorker(String),
    #[error("CDC change has unsupported product listing event {event_type} in group {event_group}")]
    UnsupportedProductListingEvent {
        event_type: String,
        event_group: String,
    },
    #[error("CDC change has unsupported product listing event schema version {schema_version}")]
    UnsupportedProductListingEventSchemaVersion { schema_version: i64 },
    #[error("CDC change has an invalid product listing event ID")]
    InvalidEventId,
    #[error("CDC change has an invalid product listing ID")]
    InvalidProductListingId,
    #[error("CDC change has an invalid raw product listing stream ID")]
    InvalidProductListingRawStreamId,
    #[error("CDC change has an invalid raw product listing revision ID")]
    InvalidProductListingRawRevisionId,
    #[error("CDC change has an invalid raw product listing revision number")]
    InvalidProductListingRawRevision,
    #[error("CDC change has a missing ProductListing event field {field}")]
    MissingProductListingEventField { field: String },
    #[error("CDC change has an invalid ProductListing event field {field}")]
    InvalidProductListingEventField { field: String },
    #[error("CDC change has a noncanonical ProductListing event field {field}")]
    NonCanonicalProductListingEventField { field: String },
    #[error("CDC change has an unknown ProductListing event field {field}")]
    UnknownProductListingEventField { field: String },
    #[error("CDC change has an inconsistent ProductListing event: {rule}")]
    InconsistentProductListingEvent { rule: String },
    #[error("CDC change has a product listing event payload that is not an object")]
    InvalidProductListingEventPayload,
    #[error("CDC change missing row data")]
    MissingRow,
    #[error("CDC row missing required column {0}")]
    MissingColumn(&'static str),
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum CdcFanoutError {
    #[error("worker queue is not registered: {0:?}")]
    MissingQueue(WorkerQueue),
    #[error("worker queue closed before fanout: {0:?}")]
    QueueClosed(WorkerQueue),
}

impl From<mpsc::error::SendError<DomainJob>> for CdcFanoutError {
    fn from(error: mpsc::error::SendError<DomainJob>) -> Self {
        Self::QueueClosed(error.0.target_queue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueueConfig, in_memory_queue};

    fn product_event_change(event_type: &str, event_group: &str) -> CdcChange {
        let payload = match (event_type, event_group) {
            ("PRODUCT_LISTING_DISCOVERED", "DOMAIN") => serde_json::json!({
                "listingSourceId": "10000000-0000-0000-0000-000000000001",
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
            }),
            ("PRODUCT_LISTING_CHANGED", "DOMAIN") => serde_json::json!({
                "availability": {"previous": null, "current": "AVAILABLE"}
            }),
            ("ENRICHMENT_EMBEDDED", "ENRICHMENT") => serde_json::json!({
                "sourceEventId": "40000000-0000-0000-0000-000000000002"
            }),
            ("ENRICHMENT_TRANSLATED_TITLES", "ENRICHMENT") => serde_json::json!({
                "sourceEventId": "40000000-0000-0000-0000-000000000002",
                "sourceLanguage": "de",
                "targetLanguages": ["en", "fr", "es", "it"]
            }),
            _ => serde_json::json!({}),
        };
        product_event_change_with_payload(event_type, event_group, payload)
    }

    fn product_event_change_with_payload(
        event_type: &str,
        event_group: &str,
        payload: Value,
    ) -> CdcChange {
        CdcChange {
            schema: Some("public".to_owned()),
            table: "product_listing_events".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "event_id": "40000000-0000-0000-0000-000000000001",
                "product_listing_id": "30000000-0000-0000-0000-000000000001",
                "event_type": event_type,
                "event_group": event_group,
                "event_type_schema_version": 1,
                "payload": payload,
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        }
    }

    fn raw_revision_change(operation: CdcOperation) -> CdcChange {
        CdcChange {
            schema: Some("public".to_owned()),
            table: "product_listing_raw_revisions".to_owned(),
            operation,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "product_listing_raw_stream_id": "10000000-0000-0000-0000-000000000001",
                "product_listing_raw_revision_id": "20000000-0000-0000-0000-000000000001",
                "revision": 3,
                "source_payload": {"mustNotEnterQueue": true},
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        }
    }

    #[test]
    fn should_route_raw_revision_insert_with_typed_wakeup_metadata_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = product_listing_raw_revision_job(&raw_revision_change(CdcOperation::Insert))?;
        let expected_stream_id = uuid::Uuid::parse_str("10000000-0000-0000-0000-000000000001")?;
        let expected_revision_id = uuid::Uuid::parse_str("20000000-0000-0000-0000-000000000001")?;

        assert!(matches!(
            jobs.as_slice(),
            [DomainJob {
                target_queue: WorkerQueue::ProductListingRawNormalization,
                idempotency_key,
                ordering_key,
                payload: DomainJobPayload::ProductListingRawRevision(ProductListingRawRevisionJob {
                    product_listing_raw_stream_id,
                    product_listing_raw_revision_id,
                    revision: 3,
                }),
            }]
                if idempotency_key.as_str() == "product-listing-raw-revision:20000000-0000-0000-0000-000000000001"
                    && ordering_key.as_str() == "product-listing-raw-stream:10000000-0000-0000-0000-000000000001"
                    && product_listing_raw_stream_id.as_uuid() == expected_stream_id
                    && product_listing_raw_revision_id.as_uuid() == expected_revision_id
        ));
        Ok(())
    }

    #[test]
    fn should_reject_malformed_raw_revision_wakeup_metadata() {
        for (field, value) in [
            (
                "product_listing_raw_stream_id",
                serde_json::json!("invalid"),
            ),
            (
                "product_listing_raw_revision_id",
                serde_json::json!("invalid"),
            ),
            ("revision", serde_json::json!(0)),
            ("revision", serde_json::json!(-1)),
        ] {
            let mut change = raw_revision_change(CdcOperation::Insert);
            if let Some(row) = change.record.as_mut() {
                row[field] = value;
            }

            assert!(product_listing_raw_revision_job(&change).is_err());
        }
    }

    #[test]
    fn should_reject_non_insert_or_other_table_for_raw_normalization_scope() {
        let registry = WorkerQueueRegistry::new();
        let fanout = CdcFanout::product_listing_raw_normalization(registry);

        assert!(
            fanout
                .route_change(&raw_revision_change(CdcOperation::Update))
                .is_err()
        );
        assert!(
            fanout
                .route_change(&product_event_change(
                    "PRODUCT_LISTING_DISCOVERED",
                    "DOMAIN"
                ))
                .is_err()
        );
    }

    #[test]
    fn should_route_discovered_event_to_projection_percolator_assessment_embedding_and_translation()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&product_event_change(
            "PRODUCT_LISTING_DISCOVERED",
            "DOMAIN",
        ))?;

        assert_eq!(5, jobs.len());
        assert!(
            jobs.iter()
                .any(|job| job.target_queue == WorkerQueue::ProductListingOpenSearch)
        );
        assert!(
            jobs.iter()
                .any(|job| job.target_queue == WorkerQueue::SearchFilterPercolator)
        );
        assert!(
            jobs.iter()
                .any(|job| job.target_queue == WorkerQueue::ProductListingContentAssessment)
        );
        assert!(
            jobs.iter()
                .any(|job| job.target_queue == WorkerQueue::ProductListingEmbed)
        );
        assert!(
            jobs.iter()
                .any(|job| job.target_queue == WorkerQueue::ProductListingTranslate)
        );
        assert!(jobs.iter().all(|job| job.idempotency_key.as_str()
            == "product-event:40000000-0000-0000-0000-000000000001"));
        assert!(
            jobs.iter()
                .all(|job| job.ordering_key.as_str()
                    == "product:30000000-0000-0000-0000-000000000001")
        );
        let expected_event_id = EventId::try_from("40000000-0000-0000-0000-000000000001")?;
        let expected_product_listing_id =
            ProductListingId::try_from("30000000-0000-0000-0000-000000000001")?;
        assert!(jobs.iter().all(|job| {
            matches!(
                &job.payload,
                DomainJobPayload::ProductListingEvent(ProductListingEventJob {
                    event_id,
                    product_listing_id,
                    ..
                }) if *event_id == expected_event_id && *product_listing_id == expected_product_listing_id
            )
        }));
        Ok(())
    }

    #[test]
    fn should_route_all_supported_events_to_projection_and_percolator()
    -> Result<(), Box<dyn std::error::Error>> {
        for (event_type, event_group) in [
            ("PRODUCT_LISTING_DISCOVERED", "DOMAIN"),
            ("PRODUCT_LISTING_CHANGED", "DOMAIN"),
            ("ENRICHMENT_EMBEDDED", "ENRICHMENT"),
            ("ENRICHMENT_TRANSLATED_TITLES", "ENRICHMENT"),
        ] {
            let jobs = route_change(&product_event_change(event_type, event_group))?;

            assert!(
                jobs.iter()
                    .any(|job| job.target_queue == WorkerQueue::ProductListingOpenSearch),
                "{event_type}"
            );
            assert!(
                jobs.iter()
                    .any(|job| job.target_queue == WorkerQueue::SearchFilterPercolator),
                "{event_type}"
            );
        }
        Ok(())
    }

    #[test]
    fn should_route_content_assessment_and_translation_only_for_discovered_event()
    -> Result<(), Box<dyn std::error::Error>> {
        for (event_type, event_group, expected) in [
            ("PRODUCT_LISTING_DISCOVERED", "DOMAIN", true),
            ("PRODUCT_LISTING_CHANGED", "DOMAIN", false),
            ("ENRICHMENT_EMBEDDED", "ENRICHMENT", false),
            ("ENRICHMENT_TRANSLATED_TITLES", "ENRICHMENT", false),
        ] {
            let jobs = route_change(&product_event_change(event_type, event_group))?;

            assert_eq!(
                expected,
                jobs.iter()
                    .any(|job| job.target_queue == WorkerQueue::ProductListingContentAssessment),
                "assessment {event_type}"
            );
            assert_eq!(
                expected,
                jobs.iter()
                    .any(|job| job.target_queue == WorkerQueue::ProductListingTranslate),
                "translation {event_type}"
            );
        }
        Ok(())
    }

    #[test]
    fn should_route_embedding_for_discovered_and_image_changed_events()
    -> Result<(), Box<dyn std::error::Error>> {
        for (event_type, event_group, payload, expected) in [
            (
                "PRODUCT_LISTING_DISCOVERED",
                "DOMAIN",
                serde_json::json!({
                    "listingSourceId": "10000000-0000-0000-0000-000000000001",
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
                }),
                true,
            ),
            (
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({"images": {"previousCount": 1, "currentCount": 2}}),
                true,
            ),
            (
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({"availability": {"previous": null, "current": "AVAILABLE"}}),
                false,
            ),
            (
                "ENRICHMENT_EMBEDDED",
                "ENRICHMENT",
                serde_json::json!({"sourceEventId": "40000000-0000-0000-0000-000000000002"}),
                false,
            ),
        ] {
            let jobs = route_change(&product_event_change_with_payload(
                event_type,
                event_group,
                payload,
            ))?;

            assert_eq!(
                expected,
                jobs.iter()
                    .any(|job| job.target_queue == WorkerQueue::ProductListingEmbed),
                "embedding {event_type}"
            );
        }
        Ok(())
    }

    #[test]
    fn should_route_changed_event_to_one_watchlist_job_when_main_price_or_availability_changed()
    -> Result<(), Box<dyn std::error::Error>> {
        for payload in [
            serde_json::json!({
                "pricing": {
                    "price": {
                        "previous": null,
                        "current": {"amount": 900, "currency": "USD"}
                    }
                }
            }),
            serde_json::json!({"availability": {"previous": null, "current": "AVAILABLE"}}),
            serde_json::json!({
                "pricing": {
                    "price": {
                        "previous": {"amount": 1200, "currency": "USD"},
                        "current": {"amount": 900, "currency": "USD"}
                    }
                },
                "availability": {"previous": "IN_STOCK", "current": "SOLD_OUT"}
            }),
        ] {
            let jobs = route_change(&product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                payload,
            ))?;

            assert_eq!(
                1,
                jobs.iter()
                    .filter(|job| job.target_queue == WorkerQueue::WatchlistNotification)
                    .count()
            );
        }
        Ok(())
    }

    #[test]
    fn should_fan_out_embedding_and_watchlist_for_combined_image_and_price_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&product_event_change_with_payload(
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
            serde_json::json!({
                "images": {"previousCount": 1, "currentCount": 2},
                "pricing": {
                    "price": {
                        "previous": {"amount": 1200, "currency": "USD"},
                        "current": {"amount": 900, "currency": "USD"}
                    }
                }
            }),
        ))?;

        assert_eq!(
            1,
            jobs.iter()
                .filter(|job| job.target_queue == WorkerQueue::ProductListingEmbed)
                .count()
        );
        assert_eq!(
            1,
            jobs.iter()
                .filter(|job| job.target_queue == WorkerQueue::WatchlistNotification)
                .count()
        );
        Ok(())
    }

    #[test]
    fn should_not_route_estimate_only_changed_event_to_watchlist_notifications()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&product_event_change_with_payload(
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
            serde_json::json!({
                "pricing": {
                    "priceEstimateMin": {
                        "previous": {"amount": 1200, "currency": "USD"},
                        "current": {"amount": 900, "currency": "USD"}
                    },
                    "priceEstimateMax": {
                        "previous": null,
                        "current": {"amount": 1500, "currency": "USD"}
                    }
                }
            }),
        ))?;

        assert!(
            jobs.iter()
                .all(|job| job.target_queue != WorkerQueue::WatchlistNotification)
        );
        Ok(())
    }

    #[test]
    fn should_reject_product_listing_event_with_malformed_ids() {
        for (field, value, expected) in [
            ("event_id", "not-a-uuid", CdcRouteError::InvalidEventId),
            (
                "product_listing_id",
                "not-a-uuid",
                CdcRouteError::InvalidProductListingId,
            ),
        ] {
            let mut change = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
            if let Some(row) = change.record.as_mut() {
                row[field] = serde_json::json!(value);
            }

            assert!(matches!(route_change(&change), Err(error) if error == expected));
        }
    }

    #[test]
    fn should_reject_product_listing_event_with_missing_ids() {
        for field in ["event_id", "product_listing_id"] {
            let mut change = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
            if let Some(row) = change.record.as_mut()
                && let Some(row) = row.as_object_mut()
            {
                let _ = row.remove(field);
            }

            assert!(matches!(
                route_change(&change),
                Err(CdcRouteError::MissingColumn(missing)) if missing == field
            ));
        }
    }

    #[test]
    fn should_reject_unknown_or_incompatible_product_listing_event_codes() {
        for (event_type, event_group) in [
            ("PRODUCT_LISTING_UNKNOWN", "DOMAIN"),
            ("PRODUCT_LISTING_CHANGED", "ENRICHMENT"),
            ("ENRICHMENT_EMBEDDED", "DOMAIN"),
        ] {
            let result = route_change(&product_event_change(event_type, event_group));

            assert!(matches!(
                result,
                Err(CdcRouteError::UnsupportedProductListingEvent { .. })
            ));
        }
    }

    #[test]
    fn should_require_supported_product_listing_event_schema_version() {
        let mut change = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
        if let Some(row) = change.record.as_mut() {
            row["event_type_schema_version"] = serde_json::json!(2);
        }

        assert!(matches!(
            route_change(&change),
            Err(CdcRouteError::UnsupportedProductListingEventSchemaVersion { schema_version: 2 })
        ));
    }

    #[test]
    fn should_reject_malformed_changed_event_shapes_before_fanout() {
        for payload in [
            serde_json::json!({"pricing": {"price": {}}}),
            serde_json::json!({"availability": {}}),
            serde_json::json!({"images": {}}),
            serde_json::json!({}),
            serde_json::json!({
                "pricing": {
                    "price": {
                        "previous": null,
                        "current": null
                    }
                }
            }),
        ] {
            assert!(
                route_change(&product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    payload,
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn should_reject_product_listing_v1_negative_contract_matrix() {
        let mut unknown_discovery = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
        if let Some(payload) = unknown_discovery
            .record
            .as_mut()
            .and_then(|record| record.get_mut("payload"))
        {
            payload["unexpected"] = serde_json::json!(true);
        }

        let mut omitted_title = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
        if let Some(payload) = omitted_title
            .record
            .as_mut()
            .and_then(|record| record.get_mut("payload"))
            && let Some(payload) = payload.as_object_mut()
        {
            payload.remove("title");
        }

        let mut omitted_price = product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
        if let Some(payload) = omitted_price
            .record
            .as_mut()
            .and_then(|record| record.get_mut("payload"))
            .and_then(|payload| payload.get_mut("pricing"))
            && let Some(pricing) = payload.as_object_mut()
        {
            pricing.remove("price");
        }

        let mut omitted_auction_start =
            product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN");
        if let Some(payload) = omitted_auction_start
            .record
            .as_mut()
            .and_then(|record| record.get_mut("payload"))
            .and_then(|payload| payload.get_mut("auction"))
            && let Some(auction) = payload.as_object_mut()
        {
            auction.remove("start");
        }

        let cases = vec![
            ("unknown discovery field", unknown_discovery),
            ("omitted nullable discovery field", omitted_title),
            ("omitted pricing field", omitted_price),
            ("omitted auction field", omitted_auction_start),
            (
                "unknown localized field",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_DISCOVERED",
                    "DOMAIN",
                    serde_json::json!({
                        "listingSourceId": "10000000-0000-0000-0000-000000000001",
                        "sourceListingId": "fixture-source-id",
                        "title": {"language": "en", "text": "Title", "unexpected": true},
                        "description": null,
                        "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
                        "availability": null,
                        "url": "https://example.test/product",
                        "imageCount": 0,
                        "auction": {"start": null, "end": null}
                    }),
                ),
            ),
            (
                "noncanonical discovery URL",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_DISCOVERED",
                    "DOMAIN",
                    serde_json::json!({
                        "listingSourceId": "10000000-0000-0000-0000-000000000001",
                        "sourceListingId": "fixture-source-id",
                        "title": null,
                        "description": null,
                        "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
                        "availability": null,
                        "url": "https://example.com:443/product",
                        "imageCount": 0,
                        "auction": {"start": null, "end": null}
                    }),
                ),
            ),
            (
                "unparsable changed URL",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "url": {"previous": "not a URL", "current": "https://example.com/new"}
                    }),
                ),
            ),
            (
                "noncanonical changed URL",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "url": {"previous": "https://example.com/old", "current": "https://example.com:443/new"}
                    }),
                ),
            ),
            (
                "unknown image field",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({"images": {"previousCount": 1, "currentCount": 2, "unexpected": true}}),
                ),
            ),
            (
                "unknown value change field",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "availability": {"previous": null, "current": "AVAILABLE", "unexpected": true}
                    }),
                ),
            ),
            (
                "unknown price field",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "pricing": {
                            "price": {
                                "previous": {"amount": 1, "currency": "EUR", "unexpected": true},
                                "current": null
                            }
                        }
                    }),
                ),
            ),
            (
                "invalid auction timestamp",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "auction": {
                            "previous": {"start": "not a timestamp", "end": null},
                            "current": {"start": null, "end": null}
                        }
                    }),
                ),
            ),
            (
                "auction start after end",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "auction": {
                            "previous": {
                                "start": "2025-01-02T00:00:00Z",
                                "end": "2025-01-01T00:00:00Z"
                            },
                            "current": {"start": null, "end": null}
                        }
                    }),
                ),
            ),
            (
                "invalid sale observation timestamp",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "saleObservation": {
                            "transition": "OBSERVED",
                            "observation": {
                                "observedAt": "yesterday",
                                "fxRateId": "10000000-0000-0000-0000-000000000001"
                            }
                        }
                    }),
                ),
            ),
            (
                "invalid sale observation FX ID",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "saleObservation": {
                            "transition": "OBSERVED",
                            "observation": {
                                "observedAt": "1970-01-01T00:00:00Z",
                                "fxRateId": "not-a-uuid"
                            }
                        }
                    }),
                ),
            ),
            (
                "withdrawal with current availability",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "availability": {"previous": null, "current": "AVAILABLE"},
                        "lifecycle": {"transition": "WITHDRAWN", "previousAvailability": null}
                    }),
                ),
            ),
            (
                "restoration with previous availability",
                product_event_change_with_payload(
                    "PRODUCT_LISTING_CHANGED",
                    "DOMAIN",
                    serde_json::json!({
                        "availability": {"previous": "AVAILABLE", "current": null},
                        "lifecycle": {"transition": "RESTORED"}
                    }),
                ),
            ),
        ];

        for (name, change) in cases {
            assert!(route_change(&change).is_err(), "{name}");
        }
    }

    #[test]
    fn should_accept_product_listing_v1_positive_contract_matrix() {
        let cases = [
            product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN"),
            product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({"availability": {"previous": null, "current": "AVAILABLE"}}),
            ),
            product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({"images": {"previousCount": 2, "currentCount": 2}}),
            ),
            product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({
                    "pricing": {"price": {"previous": null, "current": {"amount": 10, "currency": "EUR"}}},
                    "availability": {"previous": null, "current": "AVAILABLE"},
                    "images": {"previousCount": 1, "currentCount": 1}
                }),
            ),
            product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({
                    "availability": {"previous": "AVAILABLE", "current": null},
                    "lifecycle": {"transition": "WITHDRAWN", "previousAvailability": "AVAILABLE"}
                }),
            ),
            product_event_change_with_payload(
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
                serde_json::json!({
                    "availability": {"previous": null, "current": "AVAILABLE"},
                    "lifecycle": {"transition": "RESTORED"}
                }),
            ),
            product_event_change("ENRICHMENT_EMBEDDED", "ENRICHMENT"),
            product_event_change("ENRICHMENT_TRANSLATED_TITLES", "ENRICHMENT"),
        ];

        for change in cases {
            assert!(route_change(&change).is_ok());
        }
    }

    #[test]
    fn should_accept_same_count_image_replacement_for_embedding_routing() {
        let result = route_change(&product_event_change_with_payload(
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
            serde_json::json!({"images": {"previousCount": 2, "currentCount": 2}}),
        ));
        assert!(result.is_ok());
        assert!(
            result
                .unwrap_or_default()
                .iter()
                .any(|job| { job.target_queue == WorkerQueue::ProductListingEmbed })
        );
    }

    #[test]
    fn should_ignore_product_listings_table_to_avoid_double_fire()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "product_listings".to_owned(),
            operation: CdcOperation::Update,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({ "product_listing_id": "p1" })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert!(jobs.is_empty());
        Ok(())
    }

    #[test]
    fn should_route_search_filter_when_relevant_columns_change()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "search_filters".to_owned(),
            operation: CdcOperation::Update,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "10000000-0000-0000-0000-000000000001",
                "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
                "version": 3,
            })),
            old_record: None,
            changed_columns: vec!["search".to_owned()],
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert_eq!(1, jobs.len());
        assert_eq!(WorkerQueue::SearchFilterOpenSearch, jobs[0].target_queue);
        assert_eq!(
            "search-filter:50000000-0000-0000-0000-000000000001:3:update",
            jobs[0].idempotency_key.as_str()
        );
        Ok(())
    }

    #[test]
    fn should_route_search_filter_when_any_persisted_column_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "search_filters".to_owned(),
            operation: CdcOperation::Update,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "10000000-0000-0000-0000-000000000001",
                "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
                "version": 3,
            })),
            old_record: None,
            changed_columns: vec!["name".to_owned()],
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert_eq!(1, jobs.len());
        assert_eq!(WorkerQueue::SearchFilterOpenSearch, jobs[0].target_queue);
        Ok(())
    }

    #[test]
    fn should_reject_search_filter_change_without_source_version() {
        let result = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "search_filters".to_owned(),
            operation: CdcOperation::Update,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "10000000-0000-0000-0000-000000000001",
                "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
            })),
            old_record: None,
            changed_columns: vec!["name".to_owned()],
            commit_lsn: None,
            commit_timestamp: None,
        });

        assert!(matches!(
            result,
            Err(CdcRouteError::MissingColumn("version"))
        ));
    }

    #[test]
    fn should_route_every_notification_delivery_insert_to_delivery_queue()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "notification_deliveries".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "notification_delivery_id": "60000000-0000-0000-0000-000000000001",
                "channel": "EMAIL"
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert_eq!(1, jobs.len());
        assert_eq!(WorkerQueue::NotificationDelivery, jobs[0].target_queue);
        assert_eq!(
            "notification-delivery:60000000-0000-0000-0000-000000000001",
            jobs[0].idempotency_key.as_str()
        );
        assert_eq!(
            "notification-delivery:60000000-0000-0000-0000-000000000001",
            jobs[0].ordering_key.as_str()
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_enqueue_only_delivery_inserts_for_notification_delivery_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, mut receiver) = in_memory_queue(QueueConfig::new(1))?;
        let fanout = CdcFanout::notification_delivery(
            WorkerQueueRegistry::new().with_queue(WorkerQueue::NotificationDelivery, sender),
        );
        let change = CdcChange {
            schema: Some("public".to_owned()),
            table: "notification_deliveries".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "notification_delivery_id": "60000000-0000-0000-0000-000000000001"
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        };

        assert_eq!(
            1,
            fanout
                .ingest_batch(&CdcBatch {
                    delivery_id: Some("delivery-notification".to_owned()),
                    source: Some("postgres".to_owned()),
                    changes: vec![change],
                })
                .await?
        );
        assert_eq!(
            Some(WorkerQueue::NotificationDelivery),
            receiver.recv().await.map(|job| job.target_queue)
        );
        Ok(())
    }

    #[test]
    fn should_route_search_filter_match_insert_to_notification_queue()
    -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "search_filter_matches".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "10000000-0000-0000-0000-000000000001",
                "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
                "product_listing_id": "30000000-0000-0000-0000-000000000001",
                "origin_event_id": "40000000-0000-0000-0000-000000000001"
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert_eq!(1, jobs.len());
        assert_eq!(
            WorkerQueue::SearchFilterMatchNotification,
            jobs[0].target_queue
        );
        assert_eq!(
            "search-filter-match:10000000-0000-0000-0000-000000000001:50000000-0000-0000-0000-000000000001:30000000-0000-0000-0000-000000000001:40000000-0000-0000-0000-000000000001",
            jobs[0].idempotency_key.as_str()
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_enqueue_only_match_inserts_for_search_filter_match_notification_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, mut receiver) = in_memory_queue(QueueConfig::new(1))?;
        let fanout = CdcFanout::search_filter_match_notification(
            WorkerQueueRegistry::new()
                .with_queue(WorkerQueue::SearchFilterMatchNotification, sender),
        );
        let change = CdcChange {
            schema: Some("public".to_owned()),
            table: "search_filter_matches".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "10000000-0000-0000-0000-000000000001",
                "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
                "product_listing_id": "30000000-0000-0000-0000-000000000001",
                "origin_event_id": "40000000-0000-0000-0000-000000000001"
            })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        };

        assert_eq!(
            1,
            fanout
                .ingest_batch(&CdcBatch {
                    delivery_id: Some("delivery-match".to_owned()),
                    source: Some("postgres".to_owned()),
                    changes: vec![change],
                })
                .await?
        );
        assert_eq!(
            Some(WorkerQueue::SearchFilterMatchNotification),
            receiver.recv().await.map(|job| job.target_queue)
        );
        Ok(())
    }

    #[test]
    fn should_route_user_tier_change() -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "users".to_owned(),
            operation: CdcOperation::Update,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({
                "user_id": "10000000-0000-0000-0000-000000000001",
                "tier": "PREMIUM",
                "version": 4,
            })),
            old_record: Some(serde_json::json!({
                "user_id": "10000000-0000-0000-0000-000000000001",
                "tier": "FREE",
                "version": 3,
            })),
            changed_columns: vec!["tier".to_owned()],
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert_eq!(1, jobs.len());
        assert_eq!(WorkerQueue::UserTierEnforcement, jobs[0].target_queue);
        assert_eq!(
            "user-tier:10000000-0000-0000-0000-000000000001:4",
            jobs[0].idempotency_key.as_str()
        );
        Ok(())
    }

    #[test]
    fn should_ignore_unknown_table() -> Result<(), Box<dyn std::error::Error>> {
        let jobs = route_change(&CdcChange {
            schema: Some("public".to_owned()),
            table: "future_table".to_owned(),
            operation: CdcOperation::Insert,
            primary_key: BTreeMap::new(),
            record: Some(serde_json::json!({ "id": "1" })),
            old_record: None,
            changed_columns: Vec::new(),
            commit_lsn: None,
            commit_timestamp: None,
        })?;

        assert!(jobs.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_ack_after_all_jobs_are_enqueued() -> Result<(), Box<dyn std::error::Error>> {
        let (product_sender, mut product_receiver) = in_memory_queue(QueueConfig::new(8))?;
        let (percolator_sender, mut percolator_receiver) = in_memory_queue(QueueConfig::new(8))?;
        let (embed_sender, mut embed_receiver) = in_memory_queue(QueueConfig::new(8))?;
        let (assessment_sender, mut assessment_receiver) = in_memory_queue(QueueConfig::new(8))?;
        let (translation_sender, mut translation_receiver) = in_memory_queue(QueueConfig::new(8))?;
        let registry = WorkerQueueRegistry::new()
            .with_queue(WorkerQueue::ProductListingOpenSearch, product_sender)
            .with_queue(WorkerQueue::SearchFilterPercolator, percolator_sender)
            .with_queue(WorkerQueue::ProductListingEmbed, embed_sender)
            .with_queue(
                WorkerQueue::ProductListingContentAssessment,
                assessment_sender,
            )
            .with_queue(WorkerQueue::ProductListingTranslate, translation_sender);
        let fanout = CdcFanout::new(registry);
        let batch = CdcBatch {
            delivery_id: Some("delivery-1".to_owned()),
            source: Some("postgres".to_owned()),
            changes: vec![product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN")],
        };

        let enqueued = fanout.ingest_batch(&batch).await?;

        assert_eq!(5, enqueued);
        assert!(product_receiver.recv().await.is_some());
        assert!(percolator_receiver.recv().await.is_some());
        assert!(embed_receiver.recv().await.is_some());
        assert!(assessment_receiver.recv().await.is_some());
        assert!(translation_receiver.recv().await.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn should_not_ack_partial_product_event_fanout_after_some_jobs_enqueue()
    -> Result<(), Box<dyn std::error::Error>> {
        let (product_sender, mut product_receiver) = in_memory_queue(QueueConfig::new(1))?;
        let (percolator_sender, mut percolator_receiver) = in_memory_queue(QueueConfig::new(1))?;
        let fanout = CdcFanout::new(
            WorkerQueueRegistry::new()
                .with_queue(WorkerQueue::ProductListingOpenSearch, product_sender)
                .with_queue(WorkerQueue::SearchFilterPercolator, percolator_sender),
        );
        let batch = CdcBatch {
            delivery_id: Some("delivery-partial-fanout".to_owned()),
            source: Some("postgres".to_owned()),
            changes: vec![product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN")],
        };

        let result = fanout.ingest_batch(&batch).await;

        assert!(matches!(
            result,
            Err(CdcIngestError::Fanout(CdcFanoutError::MissingQueue(
                WorkerQueue::ProductListingContentAssessment
            )))
        ));
        assert!(product_receiver.recv().await.is_some());
        assert!(percolator_receiver.recv().await.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn should_enqueue_all_discovery_jobs_after_redelivery_of_partial_fanout()
    -> Result<(), Box<dyn std::error::Error>> {
        let (product_sender, mut product_receiver) = in_memory_queue(QueueConfig::new(2))?;
        let (percolator_sender, mut percolator_receiver) = in_memory_queue(QueueConfig::new(2))?;
        let (assessment_sender, mut assessment_receiver) = in_memory_queue(QueueConfig::new(1))?;
        let (embed_sender, mut embed_receiver) = in_memory_queue(QueueConfig::new(1))?;
        let (translation_sender, mut translation_receiver) = in_memory_queue(QueueConfig::new(1))?;
        let partial_fanout = CdcFanout::new(
            WorkerQueueRegistry::new()
                .with_queue(
                    WorkerQueue::ProductListingOpenSearch,
                    product_sender.clone(),
                )
                .with_queue(
                    WorkerQueue::SearchFilterPercolator,
                    percolator_sender.clone(),
                ),
        );
        let retry_fanout = CdcFanout::new(
            WorkerQueueRegistry::new()
                .with_queue(WorkerQueue::ProductListingOpenSearch, product_sender)
                .with_queue(WorkerQueue::SearchFilterPercolator, percolator_sender)
                .with_queue(
                    WorkerQueue::ProductListingContentAssessment,
                    assessment_sender,
                )
                .with_queue(WorkerQueue::ProductListingEmbed, embed_sender)
                .with_queue(WorkerQueue::ProductListingTranslate, translation_sender),
        );
        let batch = CdcBatch {
            delivery_id: Some("delivery-redelivery".to_owned()),
            source: Some("postgres".to_owned()),
            changes: vec![product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN")],
        };

        let partial_result = partial_fanout.ingest_batch(&batch).await;

        assert!(matches!(
            partial_result,
            Err(CdcIngestError::Fanout(CdcFanoutError::MissingQueue(
                WorkerQueue::ProductListingContentAssessment
            )))
        ));
        assert_eq!(5, retry_fanout.ingest_batch(&batch).await?);

        let first_product_job = product_receiver
            .recv()
            .await
            .ok_or("product queue stopped")?;
        let retried_product_job = product_receiver
            .recv()
            .await
            .ok_or("product queue stopped")?;
        let first_percolator_job = percolator_receiver
            .recv()
            .await
            .ok_or("percolator queue stopped")?;
        let retried_percolator_job = percolator_receiver
            .recv()
            .await
            .ok_or("percolator queue stopped")?;
        assert_eq!(first_product_job, retried_product_job);
        assert_eq!(first_percolator_job, retried_percolator_job);
        assert_eq!(
            Some(WorkerQueue::ProductListingContentAssessment),
            assessment_receiver.recv().await.map(|job| job.target_queue)
        );
        assert_eq!(
            Some(WorkerQueue::ProductListingEmbed),
            embed_receiver.recv().await.map(|job| job.target_queue)
        );
        assert_eq!(
            Some(WorkerQueue::ProductListingTranslate),
            translation_receiver
                .recv()
                .await
                .map(|job| job.target_queue)
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_enqueue_only_domain_or_enrichment_product_listing_events_for_percolator_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, mut receiver) = in_memory_queue(QueueConfig::new(2))?;
        let fanout = CdcFanout::search_filter_percolator(
            WorkerQueueRegistry::new().with_queue(WorkerQueue::SearchFilterPercolator, sender),
        );

        assert_eq!(
            1,
            fanout
                .ingest_batch(&CdcBatch {
                    delivery_id: Some("delivery-domain".to_owned()),
                    source: Some("postgres".to_owned()),
                    changes: vec![product_event_change("PRODUCT_LISTING_CHANGED", "DOMAIN")],
                })
                .await?
        );
        assert_eq!(
            1,
            fanout
                .ingest_batch(&CdcBatch {
                    delivery_id: Some("delivery-enrichment".to_owned()),
                    source: Some("postgres".to_owned()),
                    changes: vec![product_event_change("ENRICHMENT_EMBEDDED", "ENRICHMENT")],
                })
                .await?
        );

        for _ in 0..2 {
            assert_eq!(
                Some(WorkerQueue::SearchFilterPercolator),
                receiver.recv().await.map(|job| job.target_queue)
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_non_search_filter_change_for_search_filter_projection_worker() {
        let (sender, _receiver) = in_memory_queue(QueueConfig::new(1))
            .unwrap_or_else(|error| panic!("queue setup failed: {error}"));
        let fanout = CdcFanout::search_filter_projection(
            WorkerQueueRegistry::new().with_queue(WorkerQueue::SearchFilterOpenSearch, sender),
        );
        let batch = CdcBatch {
            delivery_id: Some("delivery-1".to_owned()),
            source: Some("postgres".to_owned()),
            changes: vec![product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN")],
        };

        let result = fanout.ingest_batch(&batch).await;

        assert!(matches!(
            result,
            Err(CdcIngestError::Route(CdcRouteError::UnsupportedTableForWorker(table)))
                if table == "product_listing_events"
        ));
    }

    #[tokio::test]
    async fn should_fail_fanout_when_target_queue_is_missing() {
        let fanout = CdcFanout::new(WorkerQueueRegistry::new());
        let batch = CdcBatch {
            delivery_id: Some("delivery-1".to_owned()),
            source: Some("postgres".to_owned()),
            changes: vec![product_event_change("PRODUCT_LISTING_DISCOVERED", "DOMAIN")],
        };

        let result = fanout.ingest_batch(&batch).await;

        assert!(matches!(
            result,
            Err(CdcIngestError::Fanout(CdcFanoutError::MissingQueue(
                WorkerQueue::ProductListingOpenSearch
            )))
        ));
    }

    #[test]
    fn should_parse_sequin_like_json_with_aliases() -> Result<(), Box<dyn std::error::Error>> {
        let batch = parse_cdc_batch(
            r#"{
                "id": "delivery-1",
                "source": "postgres",
                "events": [
                    {
                        "table_schema": "public",
                        "relation": "product_listing_events",
                        "op": "insert",
                        "keys": { "event_id": "40000000-0000-0000-0000-000000000001" },
                        "new": {
                            "event_id": "40000000-0000-0000-0000-000000000001",
                            "product_listing_id": "30000000-0000-0000-0000-000000000001",
                            "event_type": "PRODUCT_LISTING_DISCOVERED",
                            "event_group": "DOMAIN",
                            "event_type_schema_version": 1
                        }
                    }
                ]
            }"#,
        )?;

        assert_eq!(Some("delivery-1".to_owned()), batch.delivery_id);
        assert_eq!(1, batch.changes.len());
        assert_eq!(CdcOperation::Insert, batch.changes[0].operation);
        Ok(())
    }

    #[test]
    fn should_parse_real_sequin_webhook_message() -> Result<(), Box<dyn std::error::Error>> {
        let batch = parse_cdc_batch(
            r#"{
                "record": {
                    "event_id": "40000000-0000-0000-0000-000000000001",
                    "product_listing_id": "30000000-0000-0000-0000-000000000001",
                    "event_type": "PRODUCT_LISTING_DISCOVERED",
                    "event_group": "DOMAIN",
                    "event_type_schema_version": 1
                },
                "changes": null,
                "action": "insert",
                "metadata": {
                    "table_schema": "public",
                    "table_name": "product_listing_events",
                    "commit_timestamp": "2026-07-25T12:00:00Z",
                    "commit_lsn": 123456789
                }
            }"#,
        )?;

        assert_eq!(Some("sequin-webhook".to_owned()), batch.source);
        assert_eq!(1, batch.changes.len());
        assert_eq!("product_listing_events", batch.changes[0].table);
        assert_eq!(CdcOperation::Insert, batch.changes[0].operation);
        assert_eq!(Some("123456789".to_owned()), batch.changes[0].commit_lsn);
        Ok(())
    }

    #[test]
    fn should_parse_real_sequin_delete_webhook_message_as_old_record()
    -> Result<(), Box<dyn std::error::Error>> {
        let batch = parse_cdc_batch(
            r#"{
                "record": {
                    "user_id": "10000000-0000-0000-0000-000000000001",
                    "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
                    "version": 2
                },
                "changes": null,
                "action": "delete",
                "metadata": {
                    "table_schema": "public",
                    "table_name": "search_filters"
                }
            }"#,
        )?;

        assert_eq!(CdcOperation::Delete, batch.changes[0].operation);
        assert!(batch.changes[0].record.is_none());
        assert_eq!(
            Some(&serde_json::json!({
                "user_id": "10000000-0000-0000-0000-000000000001",
                "user_search_filter_id": "50000000-0000-0000-0000-000000000001",
                "version": 2
            })),
            batch.changes[0].old_record.as_ref()
        );
        Ok(())
    }

    #[test]
    fn should_parse_real_sequin_webhook_batch() -> Result<(), Box<dyn std::error::Error>> {
        let batch = parse_cdc_batch(
            r#"{
                "data": [
                    {
                        "record": {
                            "user_id": "10000000-0000-0000-0000-000000000001",
                            "tier": "PREMIUM",
                            "version": 2
                        },
                        "changes": { "tier": "FREE" },
                        "action": "update",
                        "metadata": {
                            "table_schema": "public",
                            "table_name": "users"
                        }
                    }
                ]
            }"#,
        )?;

        assert_eq!(1, batch.changes.len());
        assert_eq!("users", batch.changes[0].table);
        assert_eq!(vec!["tier".to_owned()], batch.changes[0].changed_columns);
        Ok(())
    }
}
