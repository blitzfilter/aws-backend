use crate::auth::{AuthError, RequestMetadata, TokenAuthenticator, TransportPrincipal};
use user_service::use_cases::{
    AuthenticateUserError, AuthenticateUserRequest, AuthenticateUserUseCase,
};

pub struct UserAuthenticationAuthenticator<A, U> {
    authenticator: A,
    authenticate_user: U,
}

impl<A, U> UserAuthenticationAuthenticator<A, U> {
    pub fn new(authenticator: A, authenticate_user: U) -> Self {
        Self {
            authenticator,
            authenticate_user,
        }
    }
}

#[async_trait::async_trait]
impl<A, U> TokenAuthenticator for UserAuthenticationAuthenticator<A, U>
where
    A: TokenAuthenticator,
    U: AuthenticateUserUseCase,
{
    async fn authenticate(
        &self,
        bearer_token: &str,
        metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        let principal = self
            .authenticator
            .authenticate(bearer_token, metadata)
            .await?;
        let TransportPrincipal::User { user_id, .. } = &principal else {
            return Ok(principal);
        };
        let context = principal.operation_context(metadata.clone());

        self.authenticate_user
            .execute(&context, AuthenticateUserRequest { user_id: *user_id })
            .await
            .map_err(map_authenticate_user_error)?;

        Ok(principal)
    }
}

fn map_authenticate_user_error(error: AuthenticateUserError) -> AuthError {
    match error {
        AuthenticateUserError::NotFound | AuthenticateUserError::Suspended => {
            AuthError::InvalidCredentials
        }
        AuthenticateUserError::TemporarilyUnavailable { .. } => AuthError::TemporarilyUnavailable,
        AuthenticateUserError::Internal { .. } => AuthError::Internal(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthMethod;
    use application::error::{BoxError, box_error};
    use application::operation_context::OperationContext;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::user_id::UserId;
    use user_service::use_cases::AuthenticateUserResult;

    #[derive(Clone, Copy)]
    enum UserOutcome {
        Active,
        NotFound,
        Suspended,
        TemporarilyUnavailable,
        Internal,
    }

    type UserAuthenticationCall = (OperationContext, AuthenticateUserRequest);
    type UserAuthenticationCalls = Arc<Mutex<Vec<UserAuthenticationCall>>>;

    #[derive(Clone)]
    struct FakeTokenAuthenticator {
        principal: TransportPrincipal,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeTokenAuthenticator {
        async fn authenticate(
            &self,
            _: &str,
            _: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Ok(self.principal.clone())
        }
    }

    #[derive(Clone)]
    struct FakeAuthenticateUserUseCase {
        outcome: UserOutcome,
        calls: UserAuthenticationCalls,
    }

    #[async_trait::async_trait]
    impl AuthenticateUserUseCase for FakeAuthenticateUserUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            request: AuthenticateUserRequest,
        ) -> Result<AuthenticateUserResult, AuthenticateUserError> {
            lock(&self.calls).push((context.clone(), request.clone()));
            match self.outcome {
                UserOutcome::Active => Ok(AuthenticateUserResult {
                    user_id: request.user_id,
                }),
                UserOutcome::NotFound => Err(AuthenticateUserError::NotFound),
                UserOutcome::Suspended => Err(AuthenticateUserError::Suspended),
                UserOutcome::TemporarilyUnavailable => {
                    Err(AuthenticateUserError::TemporarilyUnavailable { source: boxed() })
                }
                UserOutcome::Internal => Err(AuthenticateUserError::Internal { source: boxed() }),
            }
        }
    }

    fn boxed() -> BoxError {
        box_error(std::io::Error::other("boom"))
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn metadata() -> RequestMetadata {
        RequestMetadata::new("req-1", "corr-1")
    }

    type TestAuthenticator =
        UserAuthenticationAuthenticator<FakeTokenAuthenticator, FakeAuthenticateUserUseCase>;

    fn authenticator(
        user_id: UserId,
        outcome: UserOutcome,
    ) -> (TestAuthenticator, UserAuthenticationCalls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            UserAuthenticationAuthenticator::new(
                FakeTokenAuthenticator {
                    principal: TransportPrincipal::User {
                        user_id,
                        auth_method: AuthMethod::AuraAccessToken,
                        capabilities: BTreeSet::new(),
                    },
                },
                FakeAuthenticateUserUseCase {
                    outcome,
                    calls: Arc::clone(&calls),
                },
            ),
            calls,
        )
    }

    #[tokio::test]
    async fn should_verify_authenticated_user_before_returning_principal() {
        let user_id = UserId::new();
        let (authenticator, calls) = authenticator(user_id, UserOutcome::Active);

        let principal = authenticator
            .authenticate("token", &metadata())
            .await
            .unwrap_or_else(|error| panic!("active user should authenticate: {error}"));

        assert!(
            matches!(principal, TransportPrincipal::User { user_id: actual, .. } if actual == user_id)
        );
        let calls = lock(&calls);
        assert_eq!(1, calls.len());
        assert_eq!(user_id, calls[0].1.user_id);
        assert_eq!("req-1", calls[0].0.request_id.as_str());
        assert_eq!("corr-1", calls[0].0.correlation_id.as_str());
    }

    #[tokio::test]
    async fn should_map_missing_and_suspended_users_to_invalid_credentials() {
        for outcome in [UserOutcome::NotFound, UserOutcome::Suspended] {
            let (authenticator, _) = authenticator(UserId::new(), outcome);
            let result = authenticator.authenticate("token", &metadata()).await;

            assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        }
    }

    #[tokio::test]
    async fn should_reject_suspended_cognito_jwt_user() {
        let user_id = UserId::new();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let authenticator = UserAuthenticationAuthenticator::new(
            FakeTokenAuthenticator {
                principal: TransportPrincipal::User {
                    user_id,
                    auth_method: AuthMethod::CognitoJwt,
                    capabilities: BTreeSet::new(),
                },
            },
            FakeAuthenticateUserUseCase {
                outcome: UserOutcome::Suspended,
                calls: Arc::clone(&calls),
            },
        );

        let result = authenticator.authenticate("token", &metadata()).await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        assert_eq!(1, lock(&calls).len());
    }

    #[tokio::test]
    async fn should_map_temporary_and_internal_user_checks_to_auth_errors() {
        let (temporary, _) = authenticator(UserId::new(), UserOutcome::TemporarilyUnavailable);
        let temporary_result = temporary.authenticate("token", &metadata()).await;
        assert!(matches!(
            temporary_result,
            Err(AuthError::TemporarilyUnavailable)
        ));

        let (internal, _) = authenticator(UserId::new(), UserOutcome::Internal);
        let internal_result = internal.authenticate("token", &metadata()).await;
        assert!(matches!(internal_result, Err(AuthError::Internal(_))));
    }
}
