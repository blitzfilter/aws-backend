use application::error::BoxError;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
pub enum UserSessionRevocationError {
    #[error("user not found by identity provider")]
    UserNotFound,
    #[error("identity-provider session revocation is temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("identity-provider session revocation failed internally")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UserSessionRevoker: Send + Sync {
    async fn revoke_sessions(&self, user_id: UserId) -> Result<(), UserSessionRevocationError>;
}
