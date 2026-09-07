use crate::ports::{UserAuthenticationReadError, UserAuthenticationReader};
use application::error::BoxError;
use application::operation_context::OperationContext;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticateUserRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticateUserResult {
    pub user_id: UserId,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthenticateUserError {
    #[error("user not found")]
    NotFound,
    #[error("user is suspended")]
    Suspended,
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
pub trait AuthenticateUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: AuthenticateUserRequest,
    ) -> Result<AuthenticateUserResult, AuthenticateUserError>;
}

pub struct AuthenticateUserHandler<R> {
    reader: R,
}

impl<R> AuthenticateUserHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

#[async_trait::async_trait]
impl<R> AuthenticateUserUseCase for AuthenticateUserHandler<R>
where
    R: UserAuthenticationReader,
{
    #[tracing::instrument(
        name = "authenticate_user",
        skip_all,
        fields(
            user_id = %request.user_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: AuthenticateUserRequest,
    ) -> Result<AuthenticateUserResult, AuthenticateUserError> {
        match self.reader.find_suspension(request.user_id).await? {
            None => Err(AuthenticateUserError::NotFound),
            Some(true) => Err(AuthenticateUserError::Suspended),
            Some(false) => Ok(AuthenticateUserResult {
                user_id: request.user_id,
            }),
        }
    }
}

impl From<UserAuthenticationReadError> for AuthenticateUserError {
    fn from(error: UserAuthenticationReadError) -> Self {
        match error {
            UserAuthenticationReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAuthenticationReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthenticateUserError, AuthenticateUserHandler, AuthenticateUserRequest,
        AuthenticateUserUseCase,
    };
    use crate::ports::{UserAuthenticationReadError, UserAuthenticationReader};
    use application::error::box_error;
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct State {
        suspension: Option<bool>,
        unavailable: bool,
        calls: usize,
    }

    #[derive(Clone, Default)]
    struct FakeUserAuthenticationReader(Arc<Mutex<State>>);

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("req-test"),
            correlation_id: CorrelationId::new("corr-test"),
        }
    }

    #[async_trait::async_trait]
    impl UserAuthenticationReader for FakeUserAuthenticationReader {
        async fn find_suspension(
            &self,
            _user_id: UserId,
        ) -> Result<Option<bool>, UserAuthenticationReadError> {
            let mut state = lock(&self.0);
            state.calls += 1;
            if state.unavailable {
                Err(UserAuthenticationReadError::TemporarilyUnavailable {
                    source: box_error(std::io::Error::other("unavailable")),
                })
            } else {
                Ok(state.suspension)
            }
        }
    }

    #[tokio::test]
    async fn should_authenticate_known_active_user() {
        let user_id = UserId::new();
        let reader = FakeUserAuthenticationReader::default();
        lock(&reader.0).suspension = Some(false);

        let result = AuthenticateUserHandler::new(reader.clone())
            .execute(&context(), AuthenticateUserRequest { user_id })
            .await;

        assert!(matches!(result, Ok(result) if result.user_id == user_id));
        assert_eq!(1, lock(&reader.0).calls);
    }

    #[tokio::test]
    async fn should_reject_missing_suspended_and_temporarily_unavailable_users() {
        let user_id = UserId::new();
        let reader = FakeUserAuthenticationReader::default();
        let handler = AuthenticateUserHandler::new(reader.clone());

        let missing = handler
            .execute(&context(), AuthenticateUserRequest { user_id })
            .await;
        assert!(matches!(missing, Err(AuthenticateUserError::NotFound)));

        lock(&reader.0).suspension = Some(true);
        let suspended = handler
            .execute(&context(), AuthenticateUserRequest { user_id })
            .await;
        assert!(matches!(suspended, Err(AuthenticateUserError::Suspended)));

        lock(&reader.0).unavailable = true;
        let unavailable = handler
            .execute(&context(), AuthenticateUserRequest { user_id })
            .await;
        assert!(matches!(
            unavailable,
            Err(AuthenticateUserError::TemporarilyUnavailable { .. })
        ));
    }
}
