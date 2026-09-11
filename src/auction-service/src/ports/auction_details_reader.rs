use super::{AuctionMetadataField, StoredAuction};
use application::error::BoxError;
use auction_core::AuctionId;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq)]
pub struct AuctionDetails {
    pub stored: StoredAuction,
    pub protected_fields: BTreeSet<AuctionMetadataField>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuctionDetailsReadError {
    #[error("temporary auction details read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted auction details")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal auction details read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AuctionDetailsReader: Send + Sync {
    async fn find_by_id(
        &self,
        id: AuctionId,
    ) -> Result<Option<AuctionDetails>, AuctionDetailsReadError>;
}
