use application::error::BoxError;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
pub enum UserAuthenticationReadError {
    #[error("temporary user authentication read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("internal user authentication read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UserAuthenticationReader: Send + Sync {
    async fn find_suspension(
        &self,
        user_id: UserId,
    ) -> Result<Option<bool>, UserAuthenticationReadError>;
}
