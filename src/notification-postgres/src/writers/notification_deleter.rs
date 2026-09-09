use application::error::box_error;
use notification_core::notification_id::NotificationId;
use notification_service::ports::notification_deleter::{
    NotificationDeleteError, NotificationDeleter,
};
use sqlx::PgPool;
use user_core::user_id::UserId;

#[derive(Debug, Clone)]
pub struct SqlxNotificationDeleter {
    pool: PgPool,
}
impl SqlxNotificationDeleter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl NotificationDeleter for SqlxNotificationDeleter {
    async fn delete_one(
        &self,
        user_id: UserId,
        notification_id: NotificationId,
    ) -> Result<bool, NotificationDeleteError> {
        sqlx::query("DELETE FROM notifications WHERE user_id = $1 AND notification_id = $2")
            .bind(user_id.into_uuid())
            .bind(notification_id.into_uuid())
            .execute(&self.pool)
            .await
            .map(|result| result.rows_affected() == 1)
            .map_err(|source| NotificationDeleteError::DeleteFailed {
                source: box_error(source),
            })
    }
    async fn delete_all(&self, user_id: UserId) -> Result<u64, NotificationDeleteError> {
        sqlx::query("DELETE FROM notifications WHERE user_id = $1")
            .bind(user_id.into_uuid())
            .execute(&self.pool)
            .await
            .map(|result| result.rows_affected())
            .map_err(|source| NotificationDeleteError::DeleteFailed {
                source: box_error(source),
            })
    }
}
