use application::error::box_error;
use notification_core::notification_delivery::NotificationDeliveryTargetKey;
use notification_email::{
    EmailDeliveryTarget, EmailDeliveryTargetReadError, EmailDeliveryTargetReader,
};
use serde_email::Email;
use sqlx::PgPool;
use user_core::user_id::UserId;

#[derive(Debug, Clone)]
pub struct SqlxEmailDeliveryTargetReader {
    pool: PgPool,
}

impl SqlxEmailDeliveryTargetReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct EmailDeliveryTargetRow {
    email: String,
    first_name: Option<String>,
}

#[async_trait::async_trait]
impl EmailDeliveryTargetReader for SqlxEmailDeliveryTargetReader {
    async fn find_email_target(
        &self,
        user_id: UserId,
        target_key: &NotificationDeliveryTargetKey,
    ) -> Result<Option<EmailDeliveryTarget>, EmailDeliveryTargetReadError> {
        if target_key.as_str() != "PRIMARY" {
            return Ok(None);
        }

        let row = sqlx::query_as::<_, EmailDeliveryTargetRow>(
            "SELECT email, first_name FROM users WHERE user_id = $1",
        )
        .bind(user_id.into_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| EmailDeliveryTargetReadError::ReadFailed {
            source: box_error(source),
        })?;

        row.map(|row| {
            let address = Email::try_from(row.email).map_err(|source| {
                EmailDeliveryTargetReadError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?;
            Ok(EmailDeliveryTarget {
                address,
                first_name: row.first_name,
            })
        })
        .transpose()
    }
}
