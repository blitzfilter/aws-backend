use application::error::BoxError;
use domain_primitives::versioned::Versioned;
use partnership_core::{
    partnership_application::PartnershipApplication,
    partnership_application_id::PartnershipApplicationId,
};
use user_core::user_id::UserId;

domain_primitives::version_newtype!(PartnershipApplicationStorageVersion);
pub type VersionedPartnershipApplication =
    Versioned<PartnershipApplication, PartnershipApplicationStorageVersion>;

#[derive(Debug, thiserror::Error)]
pub enum PartnershipApplicationRepositoryError {
    #[error("existing listing source not found")]
    ListingSourceNotFound,
    #[error("concurrent partnership application update")]
    ConcurrencyConflict,
    #[error("temporary partnership application persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted partnership application state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership application persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PartnershipApplicationRepository: Send {
    async fn find_by_id(
        &mut self,
        id: PartnershipApplicationId,
    ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>;
    async fn find_by_id_for_update(
        &mut self,
        id: PartnershipApplicationId,
    ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>;
    async fn find_by_user_and_id(
        &mut self,
        user_id: UserId,
        id: PartnershipApplicationId,
    ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>;
    async fn insert(
        &mut self,
        application: &PartnershipApplication,
    ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>;
    async fn update(
        &mut self,
        application: &PartnershipApplication,
        expected: PartnershipApplicationStorageVersion,
    ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>;
}

pub trait PartnershipApplicationRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl PartnershipApplicationRepository + 'tx;
}
