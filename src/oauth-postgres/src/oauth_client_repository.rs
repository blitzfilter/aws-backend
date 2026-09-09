use crate::mapping::{OAUTH_CLIENT_COLUMNS, scope_values};
use crate::rows::OAuthClientRow;
use application::error::box_error;
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::client::OAuthClient;
use oauth_service::ports::{
    OAuthClientRepository, OAuthClientRepositoryError, OAuthClientRepositoryFactory,
    OAuthClientStorageVersion, VersionedOAuthClient,
};
use platform_postgres::SqlxTransaction;
use sqlx::{PgConnection, Postgres, QueryBuilder};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxOAuthClientRepositoryFactory;

struct SqlxOAuthClientRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxOAuthClientRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl OAuthClientRepositoryFactory<SqlxTransaction> for SqlxOAuthClientRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl OAuthClientRepository + 'tx {
        SqlxOAuthClientRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl OAuthClientRepository for SqlxOAuthClientRepository<'_> {
    async fn find_by_id(
        &mut self,
        client_id: OAuthClientId,
    ) -> Result<Option<VersionedOAuthClient>, OAuthClientRepositoryError> {
        find_client(self.connection, client_id).await
    }

    async fn insert(
        &mut self,
        client: &OAuthClient,
    ) -> Result<VersionedOAuthClient, OAuthClientRepositoryError> {
        let client_id = client.client_id().into_uuid();
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO oauth_clients (client_id, client_secret_short_token, client_secret_long_token_hash, name, redirect_uris, tos_uri, policy_uri, client_uri, logo_uri, scopes) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING ",
        );
        query.push(OAUTH_CLIENT_COLUMNS);
        let row = query
            .build_query_as::<OAuthClientRow>()
            .bind(client_id)
            .bind(client.hashed_client_secret().short_token())
            .bind(client.hashed_client_secret().long_token_hash())
            .bind(client.name().as_ref())
            .bind(
                client
                    .redirect_uris()
                    .iter()
                    .map(url::Url::to_string)
                    .collect::<Vec<_>>(),
            )
            .bind(client.tos_uri().as_str())
            .bind(client.policy_uri().as_str())
            .bind(client.client_uri().as_str())
            .bind(client.logo_uri().as_str())
            .bind(scope_values(client.scopes()))
            .fetch_one(&mut *self.connection)
            .await
            .map_err(write_error)?;

        VersionedOAuthClient::try_from(row).map_err(invalid_client_state)
    }

    async fn update(
        &mut self,
        client: &OAuthClient,
        expected_version: OAuthClientStorageVersion,
    ) -> Result<VersionedOAuthClient, OAuthClientRepositoryError> {
        let client_id = client.client_id().into_uuid();
        let expected_version = version_to_i64(expected_version)?;
        let mut query = QueryBuilder::<Postgres>::new(
            "UPDATE oauth_clients SET name = $1, redirect_uris = $2, tos_uri = $3, policy_uri = $4, client_uri = $5, logo_uri = $6, scopes = $7, version = version + 1, updated = now() WHERE client_id = $8 AND version = $9 RETURNING ",
        );
        query.push(OAUTH_CLIENT_COLUMNS);
        let row = query
            .build_query_as::<OAuthClientRow>()
            .bind(client.name().as_ref())
            .bind(
                client
                    .redirect_uris()
                    .iter()
                    .map(url::Url::to_string)
                    .collect::<Vec<_>>(),
            )
            .bind(client.tos_uri().as_str())
            .bind(client.policy_uri().as_str())
            .bind(client.client_uri().as_str())
            .bind(client.logo_uri().as_str())
            .bind(scope_values(client.scopes()))
            .bind(client_id)
            .bind(expected_version)
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(write_error)?
            .ok_or(OAuthClientRepositoryError::ConcurrencyConflict)?;

        VersionedOAuthClient::try_from(row).map_err(invalid_client_state)
    }

    async fn delete_by_id(
        &mut self,
        client_id: OAuthClientId,
    ) -> Result<bool, OAuthClientRepositoryError> {
        let client_id = client_id.into_uuid();
        let result = sqlx::query("DELETE FROM oauth_clients WHERE client_id = $1")
            .bind(client_id)
            .execute(&mut *self.connection)
            .await
            .map_err(write_error)?;

        Ok(result.rows_affected() > 0)
    }
}

async fn find_client(
    connection: &mut PgConnection,
    client_id: OAuthClientId,
) -> Result<Option<VersionedOAuthClient>, OAuthClientRepositoryError> {
    let client_id = client_id.into_uuid();
    let mut query = QueryBuilder::<Postgres>::new("SELECT ");
    query
        .push(OAUTH_CLIENT_COLUMNS)
        .push(" FROM oauth_clients WHERE client_id = $1");
    let row = query
        .build_query_as::<OAuthClientRow>()
        .bind(client_id)
        .fetch_optional(connection)
        .await
        .map_err(temporary_error)?;

    row.map(VersionedOAuthClient::try_from)
        .transpose()
        .map_err(invalid_client_state)
}

fn version_to_i64(version: OAuthClientStorageVersion) -> Result<i64, OAuthClientRepositoryError> {
    i64::try_from(version.into_inner()).map_err(invalid_client_state)
}

fn invalid_client_state(
    source: impl std::error::Error + Send + Sync + 'static,
) -> OAuthClientRepositoryError {
    OAuthClientRepositoryError::InvalidPersistedState {
        source: box_error(source),
    }
}

fn temporary_error(source: sqlx::Error) -> OAuthClientRepositoryError {
    OAuthClientRepositoryError::TemporarilyUnavailable {
        source: box_error(source),
    }
}

fn write_error(source: sqlx::Error) -> OAuthClientRepositoryError {
    if let sqlx::Error::Database(database_error) = &source
        && database_error.is_unique_violation()
    {
        return OAuthClientRepositoryError::Conflict {
            source: box_error(source),
        };
    }

    temporary_error(source)
}
