use application::error::box_error;
use platform_postgres::SqlxTransaction;
use search_filter_service::ports::{
    SearchFilterQuotaReadError, SearchFilterQuotaReader, SearchFilterQuotaReaderFactory,
};
use user_core::user_id::UserId;

#[derive(Debug, Clone, Default)]
pub struct SqlxSearchFilterQuotaReaderFactory;

struct SqlxSearchFilterQuotaReader<'tx> {
    tx: &'tx mut SqlxTransaction,
}

#[derive(Debug, thiserror::Error)]
#[error("search filter quota SQL query failed")]
struct SearchFilterQuotaQueryError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("search filter quota count could not convert to usize")]
struct SearchFilterQuotaCountConversionError(#[source] std::num::TryFromIntError);

impl From<SearchFilterQuotaQueryError> for SearchFilterQuotaReadError {
    fn from(source: SearchFilterQuotaQueryError) -> Self {
        Self::ReadFailed {
            source: box_error(source),
        }
    }
}

impl From<SearchFilterQuotaCountConversionError> for SearchFilterQuotaReadError {
    fn from(source: SearchFilterQuotaCountConversionError) -> Self {
        Self::ReadFailed {
            source: box_error(source),
        }
    }
}

impl SearchFilterQuotaReaderFactory<SqlxTransaction> for SqlxSearchFilterQuotaReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl SearchFilterQuotaReader + 'tx {
        SqlxSearchFilterQuotaReader { tx }
    }
}

#[async_trait::async_trait]
impl SearchFilterQuotaReader for SqlxSearchFilterQuotaReader<'_> {
    async fn count_active_for_user(
        &mut self,
        user_id: UserId,
    ) -> Result<usize, SearchFilterQuotaReadError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM search_filters WHERE user_id=$1 AND state='ACTIVE'",
        )
        .bind(user_id.into_uuid())
        .fetch_one(self.tx.connection())
        .await
        .map_err(SearchFilterQuotaQueryError)?;
        usize::try_from(count)
            .map_err(SearchFilterQuotaCountConversionError)
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error: SearchFilterQuotaReadError =
            SearchFilterQuotaQueryError(sqlx::Error::RowNotFound).into();

        let SearchFilterQuotaReadError::ReadFailed { source } = error;
        let query_error = source
            .downcast_ref::<SearchFilterQuotaQueryError>()
            .unwrap_or_else(|| panic!("expected quota query error"));
        assert!(std::error::Error::source(query_error).is_some());
    }
}
