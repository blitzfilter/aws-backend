use application::error::box_error;
use sqlx::PgPool;
use user_core::user_id::UserId;
use user_service::ports::{UserAuthenticationReadError, UserAuthenticationReader};

#[derive(Debug, Clone)]
pub struct SqlxUserAuthenticationReader {
    pool: PgPool,
}

impl SqlxUserAuthenticationReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl UserAuthenticationReader for SqlxUserAuthenticationReader {
    async fn find_suspension(
        &self,
        user_id: UserId,
    ) -> Result<Option<bool>, UserAuthenticationReadError> {
        sqlx::query_scalar::<_, bool>("SELECT suspended FROM users WHERE user_id = $1")
            .bind(uuid::Uuid::from(user_id))
            .fetch_optional(&self.pool)
            .await
            .map_err(
                |source| UserAuthenticationReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )
    }
}
