use crate::ports::{CognitoIdentity, CognitoUserIdentityReadError, CognitoUserIdentityReader};
use application::error::BoxError;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveCognitoUserRequest {
    pub identity: CognitoIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolveCognitoUserResult {
    pub user_id: UserId,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveCognitoUserError {
    #[error("Cognito identity is not registered")]
    NotFound,
    #[error("Cognito identity resolution is temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted Cognito identity")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal Cognito identity resolution failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ResolveCognitoUserUseCase: Send + Sync {
    async fn execute(
        &self,
        request: ResolveCognitoUserRequest,
    ) -> Result<ResolveCognitoUserResult, ResolveCognitoUserError>;
}

pub struct ResolveCognitoUserHandler<R> {
    identities: R,
}

impl<R> ResolveCognitoUserHandler<R> {
    pub fn new(identities: R) -> Self {
        Self { identities }
    }
}

#[async_trait::async_trait]
impl<R> ResolveCognitoUserUseCase for ResolveCognitoUserHandler<R>
where
    R: CognitoUserIdentityReader,
{
    #[tracing::instrument(name = "resolve_cognito_user", skip_all)]
    async fn execute(
        &self,
        request: ResolveCognitoUserRequest,
    ) -> Result<ResolveCognitoUserResult, ResolveCognitoUserError> {
        self.identities
            .find_user_id(&request.identity)
            .await?
            .map(|user_id| ResolveCognitoUserResult { user_id })
            .ok_or(ResolveCognitoUserError::NotFound)
    }
}

impl From<CognitoUserIdentityReadError> for ResolveCognitoUserError {
    fn from(error: CognitoUserIdentityReadError) -> Self {
        match error {
            CognitoUserIdentityReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            CognitoUserIdentityReadError::InvalidPersistedIdentity { source } => {
                Self::InvalidPersistedState { source }
            }
            CognitoUserIdentityReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{CognitoIssuer, CognitoSubject};

    use application::error::box_error;

    enum ReaderResult {
        Found(UserId),
        Missing,
        Temporary,
        Invalid,
        Internal,
    }

    struct Reader(ReaderResult);

    #[async_trait::async_trait]
    impl CognitoUserIdentityReader for Reader {
        async fn find_user_id(
            &self,
            _: &CognitoIdentity,
        ) -> Result<Option<UserId>, CognitoUserIdentityReadError> {
            match self.0 {
                ReaderResult::Found(user_id) => Ok(Some(user_id)),
                ReaderResult::Missing => Ok(None),
                ReaderResult::Temporary => {
                    Err(CognitoUserIdentityReadError::TemporarilyUnavailable {
                        source: box_error(std::io::Error::other("temporarily unavailable")),
                    })
                }
                ReaderResult::Invalid => {
                    Err(CognitoUserIdentityReadError::InvalidPersistedIdentity {
                        source: box_error(std::io::Error::other("invalid persisted identity")),
                    })
                }
                ReaderResult::Internal => Err(CognitoUserIdentityReadError::Internal {
                    source: box_error(std::io::Error::other("internal failure")),
                }),
            }
        }
    }

    fn request() -> ResolveCognitoUserRequest {
        ResolveCognitoUserRequest {
            identity: CognitoIdentity {
                issuer: CognitoIssuer::try_from("https://issuer.example")
                    .unwrap_or_else(|error| panic!("invalid issuer: {error}")),
                subject: CognitoSubject::try_from("opaque|subject")
                    .unwrap_or_else(|error| panic!("invalid subject: {error}")),
            },
        }
    }

    #[tokio::test]
    async fn should_resolve_registered_cognito_identity() {
        let user_id = UserId::new();
        let result = ResolveCognitoUserHandler::new(Reader(ReaderResult::Found(user_id)))
            .execute(request())
            .await;

        assert!(matches!(result, Ok(result) if result.user_id == user_id));
    }

    #[tokio::test]
    async fn should_report_unregistered_cognito_identity() {
        let result = ResolveCognitoUserHandler::new(Reader(ReaderResult::Missing))
            .execute(request())
            .await;

        assert!(matches!(result, Err(ResolveCognitoUserError::NotFound)));
    }

    #[tokio::test]
    async fn should_map_identity_reader_failures() {
        let temporary = ResolveCognitoUserHandler::new(Reader(ReaderResult::Temporary))
            .execute(request())
            .await;
        let invalid = ResolveCognitoUserHandler::new(Reader(ReaderResult::Invalid))
            .execute(request())
            .await;
        let internal = ResolveCognitoUserHandler::new(Reader(ReaderResult::Internal))
            .execute(request())
            .await;

        assert!(matches!(
            temporary,
            Err(ResolveCognitoUserError::TemporarilyUnavailable { .. })
        ));
        assert!(matches!(
            invalid,
            Err(ResolveCognitoUserError::InvalidPersistedState { .. })
        ));
        assert!(matches!(
            internal,
            Err(ResolveCognitoUserError::Internal { .. })
        ));
    }
}
