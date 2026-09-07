use application::error::BoxError;
use domain_primitives::versioned::Versioned;
use user_core::access_token::{AccessToken, AccessTokenId, HashedRawAccessToken};
use user_core::user_id::UserId;

domain_primitives::version_newtype!(AccessTokenStorageVersion);

pub type VersionedAccessToken = Versioned<AccessToken, AccessTokenStorageVersion>;

#[derive(Debug, thiserror::Error)]
pub enum AccessTokenRepositoryError {
    #[error("concurrent access token update")]
    ConcurrencyConflict,
    #[error("access token already exists")]
    Conflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary access token persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted access token state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal access token persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AccessTokenRepository: Send {
    async fn find_by_id(
        &mut self,
        user_id: UserId,
        access_token_id: AccessTokenId,
    ) -> Result<Option<VersionedAccessToken>, AccessTokenRepositoryError>;

    async fn find_by_hashed_token(
        &mut self,
        hashed_token: &HashedRawAccessToken,
    ) -> Result<Option<VersionedAccessToken>, AccessTokenRepositoryError>;

    async fn insert(
        &mut self,
        access_token: &AccessToken,
    ) -> Result<VersionedAccessToken, AccessTokenRepositoryError>;

    async fn update(
        &mut self,
        access_token: &AccessToken,
        expected_version: AccessTokenStorageVersion,
    ) -> Result<VersionedAccessToken, AccessTokenRepositoryError>;

    async fn delete_by_id(
        &mut self,
        user_id: UserId,
        access_token_id: AccessTokenId,
    ) -> Result<bool, AccessTokenRepositoryError>;

    async fn delete_by_user_id(
        &mut self,
        user_id: UserId,
    ) -> Result<u64, AccessTokenRepositoryError>;
}

pub trait AccessTokenRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl AccessTokenRepository + 'tx;
}
