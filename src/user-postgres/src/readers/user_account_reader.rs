use crate::mapping::{UserRow, user_columns};
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use sqlx::AssertSqlSafe;
use user_core::user_id::UserId;
use user_service::ports::{
    UserAccountReadError, UserAccountReader, UserAccountReaderFactory, UserDetailsView,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxUserAccountReaderFactory;

struct SqlxUserAccountReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxUserAccountReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl UserAccountReaderFactory<SqlxTransaction> for SqlxUserAccountReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl UserAccountReader + 'tx {
        SqlxUserAccountReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl UserAccountReader for SqlxUserAccountReader<'_> {
    async fn find_by_id(
        &mut self,
        user_id: UserId,
    ) -> Result<Option<UserDetailsView>, UserAccountReadError> {
        find_user_details_by_id(self.connection, user_id).await
    }
}

pub(crate) async fn find_user_details_by_id(
    connection: &mut sqlx::PgConnection,
    user_id: UserId,
) -> Result<Option<UserDetailsView>, UserAccountReadError> {
    let sql = format!("SELECT {} FROM users WHERE user_id = $1", user_columns());
    let row = sqlx::query_as::<_, UserRow>(AssertSqlSafe(sql))
        .bind(user_id.into_uuid())
        .fetch_optional(connection)
        .await
        .map_err(|source| UserAccountReadError::TemporarilyUnavailable {
            source: box_error(source),
        })?;

    row.map(UserDetailsView::try_from)
        .transpose()
        .map_err(|source| UserAccountReadError::InvalidReadModel {
            source: box_error(source),
        })
}
