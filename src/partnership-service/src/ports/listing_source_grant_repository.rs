use listing_source_core::ListingSourceId;
use partnership_core::partnership_id::PartnershipId;

use super::PartnershipGrantError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingSourceGrantOutcome {
    Granted,
    AlreadyGranted,
}

impl ListingSourceGrantOutcome {
    pub fn changed(self) -> bool {
        matches!(self, Self::Granted)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::AlreadyGranted => "already_granted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingSourceGrantRemoveOutcome {
    Removed,
    AlreadyAbsent,
}

#[async_trait::async_trait]
pub trait ListingSourceGrantRepository: Send {
    async fn grant_source_access(
        &mut self,
        partnership_id: PartnershipId,
        listing_source_id: ListingSourceId,
    ) -> Result<ListingSourceGrantOutcome, PartnershipGrantError>;

    async fn remove_source_access(
        &mut self,
        partnership_id: PartnershipId,
        listing_source_id: ListingSourceId,
    ) -> Result<ListingSourceGrantRemoveOutcome, PartnershipGrantError>;
}

pub trait ListingSourceGrantRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ListingSourceGrantRepository + 'tx;
}
