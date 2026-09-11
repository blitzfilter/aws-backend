use application::error::BoxError;
use auction_core::{Auction, AuctionId, AuctionKey};
use time::OffsetDateTime;

domain_primitives::version_newtype!(AuctionStorageVersion);

#[derive(Debug, Clone, PartialEq)]
pub struct StoredAuction {
    pub auction: Auction,
    pub version: AuctionStorageVersion,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum AuctionRepositoryError {
    #[error("concurrent auction update")]
    ConcurrencyConflict,
    #[error("source auction key already exists")]
    SourceAuctionAlreadyExists {
        #[source]
        source: BoxError,
    },
    #[error("listing source does not exist")]
    ListingSourceNotFound {
        #[source]
        source: BoxError,
    },
    #[error("temporary auction persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted auction state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal auction persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AuctionRepository: Send {
    async fn find_by_id(
        &mut self,
        id: AuctionId,
    ) -> Result<Option<StoredAuction>, AuctionRepositoryError>;
    async fn find_by_key(
        &mut self,
        key: &AuctionKey,
    ) -> Result<Option<StoredAuction>, AuctionRepositoryError>;
    async fn insert(&mut self, auction: &Auction) -> Result<StoredAuction, AuctionRepositoryError>;
    async fn update(
        &mut self,
        auction: &Auction,
        expected_version: AuctionStorageVersion,
    ) -> Result<StoredAuction, AuctionRepositoryError>;
}

pub trait AuctionRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl AuctionRepository + 'tx;
}
