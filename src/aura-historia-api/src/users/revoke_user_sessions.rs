use super::util::{no_store, parse_user_id};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::UsersState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use user_service::use_cases::RevokeUserSessionsCommand;

pub async fn revoke_user_sessions(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw_user_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let user_id = match parse_user_id(&raw_user_id, "userId") {
        Ok(value) => value,
        Err(response) => return no_store(response),
    };

    match state
        .revoke_user_sessions
        .execute(&context, RevokeUserSessionsCommand { user_id })
        .await
    {
        Ok(_) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::operation_context::OperationContext;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use axum::routing::post;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;
    use user_core::user_id::UserId;
    use user_service::use_cases::{
        RevokeUserSessionsError, RevokeUserSessionsResult, RevokeUserSessionsUseCase,
    };

    #[derive(Clone, Copy)]
    enum Outcome {
        Success,
        NotFound,
        TemporarilyUnavailable,
    }

    #[derive(Clone)]
    struct FakeUseCase {
        outcome: Outcome,
        commands: Arc<Mutex<Vec<RevokeUserSessionsCommand>>>,
    }

    #[async_trait::async_trait]
    impl RevokeUserSessionsUseCase for FakeUseCase {
        async fn execute(
            &self,
            _: &OperationContext,
            command: RevokeUserSessionsCommand,
        ) -> Result<RevokeUserSessionsResult, RevokeUserSessionsError> {
            lock(&self.commands).push(command.clone());
            match self.outcome {
                Outcome::Success => Ok(RevokeUserSessionsResult {
                    user_id: command.user_id,
                }),
                Outcome::NotFound => Err(RevokeUserSessionsError::UserNotFound),
                Outcome::TemporarilyUnavailable => {
                    Err(RevokeUserSessionsError::TemporarilyUnavailable {
                        source: application::error::box_error(std::io::Error::other("unavailable")),
                    })
                }
            }
        }
    }

    #[derive(Clone)]
    struct FakeAuthenticator {
        reject: bool,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _: &str,
            _: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            if self.reject {
                return Err(AuthError::InvalidCredentials);
            }
            Ok(TransportPrincipal::User {
                user_id: UserId::new(),
                auth_method: AuthMethod::CognitoJwt,
                capabilities: BTreeSet::new(),
            })
        }
    }

    #[derive(Clone)]
    struct TestState {
        revoke: Arc<dyn RevokeUserSessionsUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    }

    async fn endpoint(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(raw_user_id): Path<String>,
    ) -> Response {
        let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
            Ok(value) => value,
            Err(response) => return no_store(*response),
        };
        let user_id = match parse_user_id(&raw_user_id, "userId") {
            Ok(value) => value,
            Err(response) => return no_store(response),
        };
        match state
            .revoke
            .execute(&context, RevokeUserSessionsCommand { user_id })
            .await
        {
            Ok(_) => no_store(StatusCode::NO_CONTENT.into_response()),
            Err(error) => no_store(ApiError::from(error).into_response()),
        }
    }

    fn router(
        outcome: Outcome,
        reject_auth: bool,
    ) -> (Router, Arc<Mutex<Vec<RevokeUserSessionsCommand>>>) {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let state = TestState {
            revoke: Arc::new(FakeUseCase {
                outcome,
                commands: Arc::clone(&commands),
            }),
            authenticator: Arc::new(FakeAuthenticator {
                reject: reject_auth,
            }),
        };
        (
            Router::new()
                .route(
                    "/api/v1/admin/users/{user_id}/sessions/revoke",
                    post(endpoint),
                )
                .with_state(state),
            commands,
        )
    }

    async fn response(router: Router, user_id: &str) -> Response {
        router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/admin/users/{user_id}/sessions/revoke"))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("failed to build request: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("request failed: {error}"))
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[tokio::test]
    async fn should_revoke_sessions_and_return_no_content() {
        let user_id = UserId::new();
        let (router, commands) = router(Outcome::Success, false);
        let response = response(router, &user_id.to_string()).await;
        assert_eq!(StatusCode::NO_CONTENT, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        assert_eq!(
            &[RevokeUserSessionsCommand { user_id }],
            lock(&commands).as_slice()
        );
    }

    #[tokio::test]
    async fn should_reject_invalid_id_or_authentication_without_calling_use_case() {
        let (invalid_router, commands) = router(Outcome::Success, false);
        assert_eq!(
            StatusCode::BAD_REQUEST,
            response(invalid_router, "not-a-uuid").await.status()
        );
        assert!(lock(&commands).is_empty());
        let (unauthorized_router, commands) = router(Outcome::Success, true);
        assert_eq!(
            StatusCode::UNAUTHORIZED,
            response(unauthorized_router, &UserId::new().to_string())
                .await
                .status()
        );
        assert!(lock(&commands).is_empty());
    }

    #[tokio::test]
    async fn should_map_missing_user_and_provider_failure() {
        let (not_found_router, _) = router(Outcome::NotFound, false);
        assert_eq!(
            StatusCode::NOT_FOUND,
            response(not_found_router, &UserId::new().to_string())
                .await
                .status()
        );
        let (unavailable_router, _) = router(Outcome::TemporarilyUnavailable, false);
        assert_eq!(
            StatusCode::SERVICE_UNAVAILABLE,
            response(unavailable_router, &UserId::new().to_string())
                .await
                .status()
        );
    }
}
