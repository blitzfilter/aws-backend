use crate::access_token_mapping::AccessTokenDetailsRow;
use application::error::box_error;
use sqlx::PgPool;
use user_core::user_id::UserId;
use user_service::ports::{AccessTokenDetails, AccessTokenListReadError, AccessTokenListReader};

#[derive(Debug, Clone)]
pub struct SqlxAccessTokenListReader {
    pool: PgPool,
}

impl SqlxAccessTokenListReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl AccessTokenListReader for SqlxAccessTokenListReader {
    async fn list_for_user(
        &self,
        user_id: UserId,
    ) -> Result<Vec<AccessTokenDetails>, AccessTokenListReadError> {
        let rows = sqlx::query_as::<_, AccessTokenDetailsRow>(
            "SELECT access_token_id, user_id, name, scopes, origin, oauth_client_id, expires_at FROM access_tokens WHERE user_id = $1 ORDER BY created ASC, access_token_id ASC",
        )
        .bind(user_id.into_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(|source| AccessTokenListReadError::TemporarilyUnavailable {
            source: box_error(source),
        })?;

        rows.into_iter()
            .map(AccessTokenDetails::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| AccessTokenListReadError::InvalidReadModel {
                source: box_error(source),
            })
    }
}
