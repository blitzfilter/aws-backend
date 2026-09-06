use application::error::BoxError;
use domain_primitives::versioned::Versioned;
use partnership_core::{partnership::Partnership, partnership_id::PartnershipId};
use party_core::party_id::PartyId;

domain_primitives::version_newtype!(PartnershipStorageVersion);
pub type VersionedPartnership = Versioned<Partnership, PartnershipStorageVersion>;

#[derive(Debug, thiserror::Error)]
pub enum PartnershipRepositoryError {
    #[error("temporary partnership persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted partnership state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PartnershipRepository: Send {
    async fn find_by_id(
        &mut self,
        partnership_id: PartnershipId,
    ) -> Result<Option<VersionedPartnership>, PartnershipRepositoryError>;

    async fn find_or_create_for_party(
        &mut self,
        party_id: PartyId,
        new_partnership_id: PartnershipId,
    ) -> Result<VersionedPartnership, PartnershipRepositoryError>;
}

pub trait PartnershipRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartnershipRepository + 'tx;
}
