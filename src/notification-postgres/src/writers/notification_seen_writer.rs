use application::error::box_error;
use notification_core::notification_id::NotificationId;
use notification_service::ports::notification_seen_writer::{
    NotificationSeenWriteError, NotificationSeenWriter,
};
use sqlx::PgPool;
use user_core::user_id::UserId;

#[derive(Debug, Clone)]
pub struct SqlxNotificationSeenWriter {
    pool: PgPool,
}
impl SqlxNotificationSeenWriter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl NotificationSeenWriter for SqlxNotificationSeenWriter {
    async fn set_seen(
        &self,
        user_id: UserId,
        notification_id: NotificationId,
        seen: bool,
    ) -> Result<bool, NotificationSeenWriteError> {
        let result = sqlx::query("UPDATE notifications SET seen = $3, updated = now() WHERE user_id = $1 AND notification_id = $2")
            .bind(user_id.into_uuid()).bind(notification_id.into_uuid()).bind(seen).execute(&self.pool).await
            .map_err(|source| NotificationSeenWriteError::UpdateFailed { source: box_error(source) })?;
        Ok(result.rows_affected() == 1)
    }
    async fn set_seen_many(
        &self,
        user_id: UserId,
        notification_ids: &[NotificationId],
        seen: bool,
    ) -> Result<u64, NotificationSeenWriteError> {
        if notification_ids.is_empty() {
            return Ok(0);
        }
        let ids = notification_ids
            .iter()
            .copied()
            .map(NotificationId::into_uuid)
            .collect::<Vec<_>>();
        sqlx::query("UPDATE notifications SET seen = $3, updated = now() WHERE user_id = $1 AND notification_id = ANY($2)")
            .bind(user_id.into_uuid()).bind(ids).bind(seen).execute(&self.pool).await
            .map(|result| result.rows_affected()).map_err(|source| NotificationSeenWriteError::UpdateFailed { source: box_error(source) })
    }
    async fn set_seen_all(
        &self,
        user_id: UserId,
        seen: bool,
    ) -> Result<u64, NotificationSeenWriteError> {
        sqlx::query("UPDATE notifications SET seen = $2, updated = now() WHERE user_id = $1")
            .bind(user_id.into_uuid())
            .bind(seen)
            .execute(&self.pool)
            .await
            .map(|result| result.rows_affected())
            .map_err(|source| NotificationSeenWriteError::UpdateFailed {
                source: box_error(source),
            })
    }
}
