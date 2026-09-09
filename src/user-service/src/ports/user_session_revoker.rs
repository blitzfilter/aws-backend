use crate::ports::CognitoSubject;
use application::error::BoxError;

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
    async fn revoke_sessions(
        &self,
        subject: &CognitoSubject,
    ) -> Result<(), UserSessionRevocationError>;
}
