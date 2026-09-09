use crate::mapping::WatchlistViewRow;
use platform_postgres::SqlxTransaction;
use product_listing_core::product_listing_id::ProductListingId;
use user_core::user_id::UserId;
use watchlist_service::ports::{
    WatchlistProductListingView, WatchlistReadError, WatchlistReader, WatchlistReaderFactory,
};

#[derive(Debug, Clone, Default)]
pub struct SqlxWatchlistReaderFactory;

struct SqlxWatchlistReader<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl WatchlistReaderFactory<SqlxTransaction> for SqlxWatchlistReaderFactory {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut SqlxTransaction) -> impl WatchlistReader + 'tx {
        SqlxWatchlistReader { tx }
    }
}

#[async_trait::async_trait]
impl WatchlistReader for SqlxWatchlistReader<'_> {
    async fn find_for_user(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<WatchlistProductListingView>, WatchlistReadError> {
        sqlx::query_as::<_, WatchlistViewRow>(
            "SELECT user_id, product_listing_id, notifications, state, created, updated \
             FROM product_listing_watchlist WHERE user_id = $1 ORDER BY created DESC, product_listing_id ASC",
        )
        .bind(user_id.into_uuid())
        .fetch_all(self.tx.connection())
        .await
        .map_err(|_| WatchlistReadError::ReadFailed)?
        .into_iter()
        .map(WatchlistProductListingView::try_from)
        .collect()
    }

    async fn find_user_ids_for_product(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<Vec<UserId>, WatchlistReadError> {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT user_id FROM product_listing_watchlist WHERE product_listing_id = $1 ORDER BY user_id ASC",
        )
        .bind(product_listing_id.into_uuid())
        .fetch_all(self.tx.connection())
        .await
        .map_err(|_| WatchlistReadError::ReadFailed)?
        .into_iter()
        .map(|id| UserId::try_from(id).map_err(|_| WatchlistReadError::InvalidPersistedState))
        .collect()
    }
}
