use crate::mapping::{
    THIRD_PARTY_EXCHANGE_CODE_COLUMNS, access_token_id_uuid, scope_values,
    third_party_exchange_code_uuid,
};
use crate::rows::ThirdPartyExchangeCodeRow;
use application::error::box_error;
use oauth_core::third_party_exchange_code::{ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant};
use oauth_service::ports::{
    OAuthCodeRepositoryError, ThirdPartyExchangeCodeRepository,
    ThirdPartyExchangeCodeRepositoryFactory,
};
use platform_postgres::SqlxTransaction;
use sqlx::{PgConnection, Postgres, QueryBuilder};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxThirdPartyExchangeCodeRepositoryFactory;

struct SqlxThirdPartyExchangeCodeRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxThirdPartyExchangeCodeRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ThirdPartyExchangeCodeRepositoryFactory<SqlxTransaction>
    for SqlxThirdPartyExchangeCodeRepositoryFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ThirdPartyExchangeCodeRepository + 'tx {
        SqlxThirdPartyExchangeCodeRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ThirdPartyExchangeCodeRepository for SqlxThirdPartyExchangeCodeRepository<'_> {
    async fn insert(
        &mut self,
        grant: ThirdPartyExchangeCodeGrant,
    ) -> Result<(), OAuthCodeRepositoryError> {
        let code_uuid =
            third_party_exchange_code_uuid(&grant.code()).map_err(invalid_code_state)?;
        let access_token_id =
            access_token_id_uuid(grant.access_token_id()).map_err(invalid_code_state)?;
        let access_token: String = grant.access_token().clone().into();
        sqlx::query(
            "INSERT INTO oauth_third_party_exchange_codes (\
                third_party_exchange_code, access_token_id, access_token, access_token_expires_at, scopes, expires_at\
             ) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(code_uuid)
        .bind(access_token_id)
        .bind(access_token)
        .bind(grant.access_token_expires())
        .bind(scope_values(grant.scopes()))
        .bind(grant.expires())
        .execute(&mut *self.connection)
        .await
        .map(|_| ())
        .map_err(write_code_error)
    }

    async fn consume_by_code(
        &mut self,
        code: &ThirdPartyExchangeCode,
    ) -> Result<Option<ThirdPartyExchangeCodeGrant>, OAuthCodeRepositoryError> {
        let code_uuid = third_party_exchange_code_uuid(code).map_err(invalid_code_state)?;
        let mut query = QueryBuilder::<Postgres>::new(
            "DELETE FROM oauth_third_party_exchange_codes WHERE third_party_exchange_code = $1 RETURNING ",
        );
        query.push(THIRD_PARTY_EXCHANGE_CODE_COLUMNS);
        let row = query
            .build_query_as::<ThirdPartyExchangeCodeRow>()
            .bind(code_uuid)
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(temporary_code_error)?;

        row.map(TryInto::try_into)
            .transpose()
            .map_err(invalid_code_state)
    }
}

fn write_code_error(source: sqlx::Error) -> OAuthCodeRepositoryError {
    if let sqlx::Error::Database(database_error) = &source
        && database_error.is_unique_violation()
    {
        return OAuthCodeRepositoryError::Conflict {
            source: box_error(source),
        };
    }

    temporary_code_error(source)
}

fn temporary_code_error(source: sqlx::Error) -> OAuthCodeRepositoryError {
    OAuthCodeRepositoryError::TemporarilyUnavailable {
        source: box_error(source),
    }
}

fn invalid_code_state(
    source: impl std::error::Error + Send + Sync + 'static,
) -> OAuthCodeRepositoryError {
    OAuthCodeRepositoryError::InvalidPersistedState {
        source: box_error(source),
    }
}
