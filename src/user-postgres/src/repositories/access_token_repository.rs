use crate::access_token_mapping::{
    ACCESS_TOKEN_COLUMNS, AccessTokenRow, access_token_origin_values, scope_values,
};
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use sqlx::{AssertSqlSafe, PgConnection};
use user_core::access_token::{AccessToken, AccessTokenId};
use user_core::user_id::UserId;
use user_service::ports::{
    AccessTokenRepository, AccessTokenRepositoryError, AccessTokenRepositoryFactory,
    AccessTokenStorageVersion, VersionedAccessToken,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxAccessTokenRepositoryFactory;

struct SqlxAccessTokenRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxAccessTokenRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl AccessTokenRepositoryFactory<SqlxTransaction> for SqlxAccessTokenRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl AccessTokenRepository + 'tx {
        SqlxAccessTokenRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl AccessTokenRepository for SqlxAccessTokenRepository<'_> {
    async fn find_by_id(
        &mut self,
        user_id: UserId,
        access_token_id: AccessTokenId,
    ) -> Result<Option<VersionedAccessToken>, AccessTokenRepositoryError> {
        let access_token_id = access_token_id.into_uuid();
        let sql = format!(
            "SELECT {ACCESS_TOKEN_COLUMNS} FROM access_tokens WHERE user_id = $1 AND access_token_id = $2"
        );
        let row = sqlx::query_as::<_, AccessTokenRow>(AssertSqlSafe(sql))
            .bind(user_id.into_uuid())
            .bind(access_token_id)
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(temporary_error)?;

        row.map(VersionedAccessToken::try_from)
            .transpose()
            .map_err(invalid_state_error)
    }

    async fn find_by_hashed_token(
        &mut self,
        hashed_token: &user_core::access_token::HashedRawAccessToken,
    ) -> Result<Option<VersionedAccessToken>, AccessTokenRepositoryError> {
        let sql = format!(
            "SELECT {ACCESS_TOKEN_COLUMNS} FROM access_tokens WHERE token_short = $1 AND token_hash = $2"
        );
        let row = sqlx::query_as::<_, AccessTokenRow>(AssertSqlSafe(sql))
            .bind(hashed_token.short_token())
            .bind(hashed_token.long_token_hash())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(temporary_error)?;

        row.map(VersionedAccessToken::try_from)
            .transpose()
            .map_err(invalid_state_error)
    }

    async fn insert(
        &mut self,
        access_token: &AccessToken,
    ) -> Result<VersionedAccessToken, AccessTokenRepositoryError> {
        let (origin, oauth_client_id) = access_token_origin_values(access_token);
        let access_token_id = access_token.id().into_uuid();
        let sql = format!(
            "INSERT INTO access_tokens (access_token_id, user_id, token_short, token_hash, name, scopes, origin, oauth_client_id, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {ACCESS_TOKEN_COLUMNS}"
        );
        let row = sqlx::query_as::<_, AccessTokenRow>(AssertSqlSafe(sql))
            .bind(access_token_id)
            .bind(access_token.user_id().into_uuid())
            .bind(access_token.hashed_token().short_token())
            .bind(access_token.hashed_token().long_token_hash())
            .bind(access_token.name().as_ref())
            .bind(scope_values(access_token.scopes()))
            .bind(origin)
            .bind(oauth_client_id)
            .bind(access_token.expires())
            .fetch_one(&mut *self.connection)
            .await
            .map_err(write_error)?;

        VersionedAccessToken::try_from(row).map_err(invalid_state_error)
    }

    async fn update(
        &mut self,
        access_token: &AccessToken,
        expected_version: AccessTokenStorageVersion,
    ) -> Result<VersionedAccessToken, AccessTokenRepositoryError> {
        let (origin, oauth_client_id) = access_token_origin_values(access_token);
        let access_token_id = access_token.id().into_uuid();
        let expected_version = version_to_i64(expected_version)?;
        let sql = format!(
            "UPDATE access_tokens SET name = $3, scopes = $4, origin = $5, oauth_client_id = $6, expires_at = $7, version = version + 1, updated = now() WHERE user_id = $1 AND access_token_id = $2 AND version = $8 RETURNING {ACCESS_TOKEN_COLUMNS}"
        );
        let row = sqlx::query_as::<_, AccessTokenRow>(AssertSqlSafe(sql))
            .bind(access_token.user_id().into_uuid())
            .bind(access_token_id)
            .bind(access_token.name().as_ref())
            .bind(scope_values(access_token.scopes()))
            .bind(origin)
            .bind(oauth_client_id)
            .bind(access_token.expires())
            .bind(expected_version)
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(write_error)?
            .ok_or(AccessTokenRepositoryError::ConcurrencyConflict)?;

        VersionedAccessToken::try_from(row).map_err(invalid_state_error)
    }

    async fn delete_by_id(
        &mut self,
        user_id: UserId,
        access_token_id: AccessTokenId,
    ) -> Result<bool, AccessTokenRepositoryError> {
        let access_token_id = access_token_id.into_uuid();
        let result =
            sqlx::query("DELETE FROM access_tokens WHERE user_id = $1 AND access_token_id = $2")
                .bind(user_id.into_uuid())
                .bind(access_token_id)
                .execute(&mut *self.connection)
                .await
                .map_err(write_error)?;

        Ok(result.rows_affected() > 0)
    }

    async fn delete_by_user_id(
        &mut self,
        user_id: UserId,
    ) -> Result<u64, AccessTokenRepositoryError> {
        let result = sqlx::query("DELETE FROM access_tokens WHERE user_id = $1")
            .bind(user_id.into_uuid())
            .execute(&mut *self.connection)
            .await
            .map_err(write_error)?;

        Ok(result.rows_affected())
    }
}

fn version_to_i64(version: AccessTokenStorageVersion) -> Result<i64, AccessTokenRepositoryError> {
    i64::try_from(version.into_inner()).map_err(|source| {
        AccessTokenRepositoryError::InvalidPersistedState {
            source: box_error(source),
        }
    })
}

fn invalid_state_error(
    source: impl std::error::Error + Send + Sync + 'static,
) -> AccessTokenRepositoryError {
    AccessTokenRepositoryError::InvalidPersistedState {
        source: box_error(source),
    }
}

fn temporary_error(source: sqlx::Error) -> AccessTokenRepositoryError {
    AccessTokenRepositoryError::TemporarilyUnavailable {
        source: box_error(source),
    }
}

fn write_error(source: sqlx::Error) -> AccessTokenRepositoryError {
    if let sqlx::Error::Database(database_error) = &source
        && database_error.is_unique_violation()
    {
        return AccessTokenRepositoryError::Conflict {
            source: box_error(source),
        };
    }

    temporary_error(source)
}
