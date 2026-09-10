use application::error::box_error;
use platform_postgres::SqlxTransaction;
use user_core::user_id::UserId;
use watchlist_service::ports::{
    WatchlistQuotaReadError, WatchlistQuotaReader, WatchlistQuotaReaderFactory,
};

#[derive(Debug, Clone, Default)]
pub struct SqlxWatchlistQuotaReaderFactory;

struct SqlxWatchlistQuotaReader<'tx> {
    tx: &'tx mut SqlxTransaction,
}

#[derive(Debug, thiserror::Error)]
#[error("watchlist quota SQL query failed")]
struct WatchlistQuotaQueryError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("watchlist quota count could not convert to usize")]
struct WatchlistQuotaCountConversionError(#[source] std::num::TryFromIntError);

impl From<WatchlistQuotaQueryError> for WatchlistQuotaReadError {
    fn from(source: WatchlistQuotaQueryError) -> Self {
        Self::ReadFailed {
            source: box_error(source),
        }
    }
}

impl From<WatchlistQuotaCountConversionError> for WatchlistQuotaReadError {
    fn from(source: WatchlistQuotaCountConversionError) -> Self {
        Self::ReadFailed {
            source: box_error(source),
        }
    }
}

impl WatchlistQuotaReaderFactory<SqlxTransaction> for SqlxWatchlistQuotaReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl WatchlistQuotaReader + 'tx {
        SqlxWatchlistQuotaReader { tx }
    }
}

#[async_trait::async_trait]
impl WatchlistQuotaReader for SqlxWatchlistQuotaReader<'_> {
    async fn count_active_for_user(
        &mut self,
        user_id: UserId,
    ) -> Result<usize, WatchlistQuotaReadError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM product_listing_watchlist WHERE user_id = $1 AND state = 'ACTIVE'",
        )
        .bind(user_id.into_uuid())
        .fetch_one(self.tx.connection())
        .await
        .map_err(WatchlistQuotaQueryError)?;
        usize::try_from(count)
            .map_err(WatchlistQuotaCountConversionError)
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error: WatchlistQuotaReadError =
            WatchlistQuotaQueryError(sqlx::Error::RowNotFound).into();

        let WatchlistQuotaReadError::ReadFailed { source } = error;
        let query_error = source
            .downcast_ref::<WatchlistQuotaQueryError>()
            .unwrap_or_else(|| panic!("expected quota query error"));
        assert!(std::error::Error::source(query_error).is_some());
    }
}
