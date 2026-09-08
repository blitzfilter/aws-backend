//! Worker-local jobs. PostgreSQL and target-side guards, not transport memory, own idempotency.
use crate::{WorkerScope, cdc::CdcOperation};
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{ProductListingRawRevisionId, ProductListingRawStreamId};
use strum::IntoEnumIterator;
use uuid::Uuid;

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
    /// Legacy in-memory API only. No production scope or wire representation.
    UserTierEnforcement,
    NotificationDelivery,
}

impl WorkerQueue {
    pub const ALL: [Self; 10] = [
        Self::ProductListingOpenSearch,
        Self::ProductListingRawNormalization,
        Self::WatchlistNotification,
        Self::SearchFilterPercolator,
        Self::SearchFilterMatchNotification,
        Self::ProductListingContentAssessment,
        Self::ProductListingEmbed,
        Self::ProductListingTranslate,
        Self::SearchFilterOpenSearch,
        Self::NotificationDelivery,
    ];

    pub(crate) fn scope(self) -> Option<WorkerScope> {
        WorkerScope::iter().find(|scope| scope.consumer_queue() == self)
    }
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

/// Compact wake-up metadata; immutable revision content is reread from PostgreSQL.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("job identifiers, version, scope, or logical keys are invalid")]
pub(crate) struct InvalidJob;

pub(crate) fn canonical_uuid(value: &str) -> Result<Uuid, InvalidJob> {
    let id = Uuid::parse_str(value).map_err(|_| InvalidJob)?;
    if id.is_nil() || id.to_string() != value {
        return Err(InvalidJob);
    }
    Ok(id)
}

impl DomainJob {
    pub(crate) fn validate(&self) -> Result<WorkerScope, InvalidJob> {
        let scope = self.target_queue.scope().ok_or(InvalidJob)?;
        let (idempotency, ordering) = self.payload.logical_keys()?;
        let permitted = match &self.payload {
            DomainJobPayload::ProductListingEvent(_) => matches!(
                scope,
                WorkerScope::ProductListingOpenSearch
                    | WorkerScope::SearchFilterPercolator
                    | WorkerScope::WatchlistNotification
                    | WorkerScope::ProductListingContentAssessment
                    | WorkerScope::ProductListingEmbedding
                    | WorkerScope::ProductListingTranslation
            ),
            DomainJobPayload::ProductListingRawRevision(_) => {
                scope == WorkerScope::ProductListingRawNormalization
            }
            DomainJobPayload::SearchFilterChanged(_) => {
                scope == WorkerScope::SearchFilterProjection
            }
            DomainJobPayload::SearchFilterMatchCreated(_) => {
                scope == WorkerScope::SearchFilterMatchNotification
            }
            DomainJobPayload::NotificationDeliveryCreated(_) => {
                scope == WorkerScope::NotificationDelivery
            }
            DomainJobPayload::UserTierChanged(_) => false,
        };
        if !permitted
            || self.idempotency_key.as_str() != idempotency
            || self.ordering_key.as_str() != ordering
        {
            return Err(InvalidJob);
        }
        Ok(scope)
    }
}

impl DomainJobPayload {
    pub(crate) fn logical_keys(&self) -> Result<(String, String), InvalidJob> {
        Ok(match self {
            Self::ProductListingEvent(event) => {
                canonical_uuid(&event.event_id.to_string())?;
                canonical_uuid(&event.product_listing_id.to_string())?;
                (
                    format!("product-event:{}", event.event_id),
                    format!("product:{}", event.product_listing_id),
                )
            }
            Self::ProductListingRawRevision(revision) => {
                let stream = revision.product_listing_raw_stream_id.as_uuid().to_string();
                let id = revision
                    .product_listing_raw_revision_id
                    .as_uuid()
                    .to_string();
                canonical_uuid(&stream)?;
                canonical_uuid(&id)?;
                if revision.revision == 0 || revision.revision > i64::MAX as u64 {
                    return Err(InvalidJob);
                }
                (
                    format!("product-listing-raw-revision:{id}"),
                    format!("product-listing-raw-stream:{stream}"),
                )
            }
            Self::SearchFilterChanged(change) => {
                canonical_uuid(&change.user_id)?;
                canonical_uuid(&change.user_search_filter_id)?;
                if change.version <= 0 {
                    return Err(InvalidJob);
                }
                (
                    format!(
                        "search-filter:{}:{}:{}",
                        change.user_search_filter_id, change.version, change.operation
                    ),
                    format!("search-filter:{}", change.user_search_filter_id),
                )
            }
            Self::SearchFilterMatchCreated(change) => {
                for id in [
                    &change.user_id,
                    &change.user_search_filter_id,
                    &change.product_listing_id,
                    &change.origin_event_id,
                ] {
                    canonical_uuid(id)?;
                }
                (
                    format!(
                        "search-filter-match:{}:{}:{}:{}",
                        change.user_id,
                        change.user_search_filter_id,
                        change.product_listing_id,
                        change.origin_event_id
                    ),
                    format!("user:{}", change.user_id),
                )
            }
            Self::NotificationDeliveryCreated(delivery) => {
                canonical_uuid(&delivery.notification_delivery_id)?;
                let key = format!(
                    "notification-delivery:{}",
                    delivery.notification_delivery_id
                );
                (key.clone(), key)
            }
            Self::UserTierChanged(_) => return Err(InvalidJob),
        })
    }
}
