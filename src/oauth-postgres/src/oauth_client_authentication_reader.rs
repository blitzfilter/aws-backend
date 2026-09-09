use application::error::box_error;
use credential_core::oauth_client_id::OAuthClientId;
use oauth_service::ports::{
    OAuthClientAuthentication, OAuthClientAuthenticationReader, OAuthClientReadError,
};
use sqlx::PgPool;
use user_core::access_token::HashedRawOAuthClientSecret;

#[derive(Clone)]
pub struct SqlxOAuthClientAuthenticationReader {
    pool: PgPool,
}

impl SqlxOAuthClientAuthenticationReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl OAuthClientAuthenticationReader for SqlxOAuthClientAuthenticationReader {
    async fn find_by_id(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClientAuthentication>, OAuthClientReadError> {
        let row = sqlx::query_as::<_, (String, String)>(
            "SELECT client_secret_short_token, client_secret_long_token_hash \
             FROM oauth_clients WHERE client_id = $1",
        )
        .bind(client_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(temporarily_unavailable)?;

        Ok(
            row.map(|(short_token, long_token_hash)| OAuthClientAuthentication {
                hashed_client_secret: HashedRawOAuthClientSecret::new(short_token, long_token_hash),
            }),
        )
    }
}

fn temporarily_unavailable(source: sqlx::Error) -> OAuthClientReadError {
    OAuthClientReadError::TemporarilyUnavailable {
        source: box_error(source),
    }
}
