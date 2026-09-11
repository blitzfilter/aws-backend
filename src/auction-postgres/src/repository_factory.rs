use crate::repositories::SqlxAuctionRepository;
use auction_service::ports::{AuctionRepository, AuctionRepositoryFactory};
use platform_postgres::SqlxTransaction;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxAuctionRepositoryFactory;

impl SqlxAuctionRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl AuctionRepositoryFactory<SqlxTransaction> for SqlxAuctionRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl AuctionRepository + 'tx {
        SqlxAuctionRepository {
            connection: tx.connection(),
        }
    }
}
