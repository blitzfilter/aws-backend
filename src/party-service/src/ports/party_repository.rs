use application::error::BoxError;
use party_core::{party::Party, party_id::PartyId, party_slug_id::PartySlugId};
use time::OffsetDateTime;

domain_primitives::version_newtype!(PartyStorageVersion);

#[derive(Debug, Clone, PartialEq)]
pub struct StoredParty {
    pub party: Party,
    pub version: PartyStorageVersion,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyDeletionBlocker {
    ListingSources,
    Partnership,
}

#[derive(Debug, thiserror::Error)]
pub enum PartyRepositoryError {
    #[error("concurrent party update")]
    ConcurrencyConflict,
    #[error("party slug conflict")]
    SlugConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary party persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted party state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal party persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PartyRepository: Send {
    async fn find_by_id(
        &mut self,
        id: PartyId,
    ) -> Result<Option<StoredParty>, PartyRepositoryError>;

    async fn find_by_slug(
        &mut self,
        slug_id: &PartySlugId,
    ) -> Result<Option<StoredParty>, PartyRepositoryError>;

    async fn find_by_id_for_update(
        &mut self,
        id: PartyId,
    ) -> Result<Option<StoredParty>, PartyRepositoryError>;

    async fn find_deletion_blocker(
        &mut self,
        id: PartyId,
    ) -> Result<Option<PartyDeletionBlocker>, PartyRepositoryError>;

    async fn delete_unused(
        &mut self,
        id: PartyId,
        expected_version: PartyStorageVersion,
    ) -> Result<(), PartyRepositoryError>;

    async fn insert(&mut self, party: &Party) -> Result<StoredParty, PartyRepositoryError>;

    async fn update(
        &mut self,
        party: &Party,
        expected_version: PartyStorageVersion,
    ) -> Result<StoredParty, PartyRepositoryError>;
}

pub trait PartyRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartyRepository + 'tx;
}
