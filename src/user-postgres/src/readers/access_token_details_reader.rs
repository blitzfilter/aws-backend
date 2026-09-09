use crate::access_token_mapping::AccessTokenDetailsRow;
use application::error::box_error;
use sqlx::PgPool;
use user_core::access_token::AccessTokenId;
use user_core::user_id::UserId;
use user_service::ports::{
    AccessTokenDetails, AccessTokenDetailsReadError, AccessTokenDetailsReader,
};

#[derive(Debug, Clone)]
pub struct SqlxAccessTokenDetailsReader {
    pool: PgPool,
}

impl SqlxAccessTokenDetailsReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl AccessTokenDetailsReader for SqlxAccessTokenDetailsReader {
    async fn find_by_id(
        &self,
        user_id: UserId,
        access_token_id: AccessTokenId,
    ) -> Result<Option<AccessTokenDetails>, AccessTokenDetailsReadError> {
        let row = sqlx::query_as::<_, AccessTokenDetailsRow>(
            "SELECT access_token_id, user_id, name, scopes, origin, oauth_client_id, expires_at FROM access_tokens WHERE user_id = $1 AND access_token_id = $2",
        )
        .bind(user_id.into_uuid())
        .bind(access_token_id.into_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| AccessTokenDetailsReadError::TemporarilyUnavailable {
            source: box_error(source),
        })?;

        row.map(AccessTokenDetails::try_from)
            .transpose()
            .map_err(|source| AccessTokenDetailsReadError::InvalidReadModel {
                source: box_error(source),
            })
    }
}
