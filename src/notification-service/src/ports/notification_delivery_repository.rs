use crate::presentation::NotificationPresentationPreferences;
use application::error::BoxError;
use notification_core::{
    notification::NotificationContent,
    notification_delivery::{NotificationDeliveryChannel, NotificationDeliveryTargetKey},
    notification_delivery_id::NotificationDeliveryId,
    notification_id::NotificationId,
};
use time::OffsetDateTime;
use user_core::user_id::UserId;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationDeliveryStatus {
    Pending,
    Processing,
    Delivered,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedNotificationDelivery {
    pub notification_delivery_id: NotificationDeliveryId,
    pub notification_id: NotificationId,
    pub lease_token: Uuid,
    pub lease_expires_at: OffsetDateTime,
    pub attempt_count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotificationDeliverySource {
    pub notification_delivery_id: NotificationDeliveryId,
    pub notification_id: NotificationId,
    pub user_id: UserId,
    pub channel: NotificationDeliveryChannel,
    pub target_key: NotificationDeliveryTargetKey,
    pub content: NotificationContent,
    pub presentation_preferences: NotificationPresentationPreferences,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaimNotificationDeliveryOutcome {
    Claimed {
        delivery: ClaimedNotificationDelivery,
        source: Box<Option<NotificationDeliverySource>>,
    },
    Missing,
    Delivered,
    PermanentlyFailed,
    /// Another attempt owns this persisted PROCESSING lease. Never acknowledge it.
    AlreadyClaimed {
        lease_expires_at: OffsetDateTime,
    },
    /// The failed claim raced with a release or expiry. Retry shortly, without acknowledging.
    Reclaimable,
}

#[derive(Debug, thiserror::Error)]
pub enum NotificationDeliveryError {
    #[error("notification delivery operation failed")]
    OperationFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted notification delivery state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait NotificationDeliveryRepository: Send + Sync {
    async fn claim_and_load_source(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        now: OffsetDateTime,
        lease_expires_at: OffsetDateTime,
        lease_token: Uuid,
    ) -> Result<ClaimNotificationDeliveryOutcome, NotificationDeliveryError>;

    /// Finalization methods return true for a committed write OR an exact replay of the
    /// original token, result, and completion timestamp. False means ownership was lost.
    /// Errors (including an ambiguous commit) must never be treated as completion.
    async fn mark_delivered(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        provider_message_id: &str,
        delivered_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError>;

    async fn mark_retryable_failure(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        error_code: &str,
        completed_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError>;

    async fn mark_permanent_failure(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        error_code: &str,
        completed_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError>;
}
