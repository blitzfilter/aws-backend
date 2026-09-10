use crate::mapping::OAUTH_CLIENT_VIEW_COLUMNS;
use crate::rows::OAuthClientViewRow;
use application::error::box_error;
use credential_core::oauth_client_id::OAuthClientId;
use oauth_service::ports::{OAuthClientDetailsReader, OAuthClientReadError, OAuthClientView};
use sqlx::{PgPool, Postgres, QueryBuilder};

#[derive(Clone)]
pub struct SqlxOAuthClientDetailsReader {
    pool: PgPool,
}

impl SqlxOAuthClientDetailsReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl OAuthClientDetailsReader for SqlxOAuthClientDetailsReader {
    async fn find(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClientView>, OAuthClientReadError> {
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(OAUTH_CLIENT_VIEW_COLUMNS)
            .push(" FROM oauth_clients WHERE client_id = $1");
        let row = query
            .build_query_as::<OAuthClientViewRow>()
            .bind(client_id.as_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(temporarily_unavailable)?;

        row.map(TryInto::try_into)
            .transpose()
            .map_err(invalid_persisted_state)
    }
}

fn temporarily_unavailable(source: sqlx::Error) -> OAuthClientReadError {
    OAuthClientReadError::TemporarilyUnavailable {
        source: box_error(source),
    }
}

fn invalid_persisted_state(
    source: impl std::error::Error + Send + Sync + 'static,
) -> OAuthClientReadError {
    OAuthClientReadError::InvalidPersistedState {
        source: box_error(source),
    }
}
