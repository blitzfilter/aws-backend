use crate::access_token_mapping::AccessTokenAuthenticationRow;
use application::error::box_error;
use sqlx::PgPool;
use user_core::access_token::HashedRawAccessToken;
use user_service::ports::{
    AccessTokenAuthentication, AccessTokenAuthenticationReadError, AccessTokenAuthenticationReader,
};

#[derive(Debug, Clone)]
pub struct SqlxAccessTokenAuthenticationReader {
    pool: PgPool,
}

impl SqlxAccessTokenAuthenticationReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl AccessTokenAuthenticationReader for SqlxAccessTokenAuthenticationReader {
    async fn find_authentication_by_hashed_token(
        &self,
        hashed_token: &HashedRawAccessToken,
    ) -> Result<Option<AccessTokenAuthentication>, AccessTokenAuthenticationReadError> {
        let row = sqlx::query_as::<_, AccessTokenAuthenticationRow>(
            "SELECT access_tokens.access_token_id, access_tokens.user_id, access_tokens.scopes, \
                    access_tokens.origin, access_tokens.oauth_client_id, access_tokens.expires_at \
             FROM access_tokens \
             JOIN users ON users.user_id = access_tokens.user_id \
             WHERE access_tokens.token_short = $1 \
               AND access_tokens.token_hash = $2 \
               AND users.suspended = false",
        )
        .bind(hashed_token.short_token())
        .bind(hashed_token.long_token_hash())
        .fetch_optional(&self.pool)
        .await
        .map_err(
            |source| AccessTokenAuthenticationReadError::TemporarilyUnavailable {
                source: box_error(source),
            },
        )?;

        row.map(AccessTokenAuthentication::try_from)
            .transpose()
            .map_err(
                |source| AccessTokenAuthenticationReadError::InvalidReadModel {
                    source: box_error(source),
                },
            )
    }
}
