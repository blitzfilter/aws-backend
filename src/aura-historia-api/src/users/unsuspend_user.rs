use super::util::{no_store, parse_user_id};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::state::UsersState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use user_core::user_id::UserId;
use user_service::use_cases::{UnsuspendUserCommand, UnsuspendUserResult, UnsuspendUserUseCase};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UnsuspendUserResponseData {
    user_id: UserId,
    suspended: bool,
}

impl From<UnsuspendUserResult> for UnsuspendUserResponseData {
    fn from(result: UnsuspendUserResult) -> Self {
        Self {
            user_id: result.user_id,
            suspended: result.suspended,
        }
    }
}

pub async fn unsuspend_user(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw_user_id): Path<String>,
    body: String,
) -> Response {
    unsuspend_user_with(
        state.authenticator.as_ref(),
        state.unsuspend_user.as_ref(),
        headers,
        raw_user_id,
        body,
    )
    .await
}

async fn unsuspend_user_with(
    authenticator: &dyn crate::auth::TokenAuthenticator,
    unsuspend_user: &dyn UnsuspendUserUseCase,
    headers: HeaderMap,
    raw_user_id: String,
    body: String,
) -> Response {
    let (context, _) = match protected_context(authenticator, &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let user_id = match parse_user_id(&raw_user_id, "userId") {
        Ok(value) => value,
        Err(response) => return no_store(response),
    };
    if !body.is_empty() {
        return no_store(
            ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail("Request body is not allowed.")
                .into_response(),
        );
    }

    match unsuspend_user
        .execute(&context, UnsuspendUserCommand { user_id })
        .await
    {
        Ok(result) => no_store(Json(UnsuspendUserResponseData::from(result)).into_response()),
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
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use axum::routing::delete;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;
    use user_service::use_cases::UnsuspendUserError;

    #[derive(Clone, Copy)]
    enum UnsuspendOutcome {
        Success,
        NotFound,
    }

    #[derive(Clone)]
    struct FakeUnsuspendUserUseCase {
        outcome: UnsuspendOutcome,
        commands: Arc<Mutex<Vec<UnsuspendUserCommand>>>,
    }

    #[async_trait::async_trait]
    impl UnsuspendUserUseCase for FakeUnsuspendUserUseCase {
        async fn execute(
            &self,
            _: &OperationContext,
            command: UnsuspendUserCommand,
        ) -> Result<UnsuspendUserResult, UnsuspendUserError> {
            lock(&self.commands).push(command.clone());
            match self.outcome {
                UnsuspendOutcome::Success => Ok(UnsuspendUserResult {
                    user_id: command.user_id,
                    suspended: false,
                }),
                UnsuspendOutcome::NotFound => Err(UnsuspendUserError::UserNotFound),
            }
        }
    }

    #[derive(Clone)]
    struct FakeAuthenticator {
        user_id: UserId,
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
                user_id: self.user_id,
                auth_method: AuthMethod::CognitoJwt,
                capabilities: BTreeSet::new(),
            })
        }
    }

    #[derive(Clone)]
    struct TestState {
        unsuspend_user: Arc<dyn UnsuspendUserUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    }

    async fn test_unsuspend_user(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(raw_user_id): Path<String>,
        body: String,
    ) -> Response {
        unsuspend_user_with(
            state.authenticator.as_ref(),
            state.unsuspend_user.as_ref(),
            headers,
            raw_user_id,
            body,
        )
        .await
    }

    fn router(
        outcome: UnsuspendOutcome,
        reject_auth: bool,
    ) -> (Router, Arc<Mutex<Vec<UnsuspendUserCommand>>>) {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let state = TestState {
            unsuspend_user: Arc::new(FakeUnsuspendUserUseCase {
                outcome,
                commands: Arc::clone(&commands),
            }),
            authenticator: Arc::new(FakeAuthenticator {
                user_id: UserId::new(),
                reject: reject_auth,
            }),
        };
        (
            Router::new()
                .route(
                    "/api/v1/admin/users/{user_id}/suspension",
                    delete(test_unsuspend_user),
                )
                .with_state(state),
            commands,
        )
    }

    async fn response(router: Router, user_id: &str, body: &'static str) -> Response {
        router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/admin/users/{user_id}/suspension"))
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::from(body))
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
    async fn should_map_unsuspend_request_and_return_no_store_response() {
        let user_id = UserId::new();
        let (router, commands) = router(UnsuspendOutcome::Success, false);

        let response = response(router, &user_id.to_string(), "").await;
        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("failed to read response: {error}"));
        let body: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("failed to decode response: {error}"));
        assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
        assert_eq!(serde_json::json!(false), body["suspended"]);
        assert_eq!(
            &[UnsuspendUserCommand { user_id }],
            lock(&commands).as_slice()
        );
    }

    #[tokio::test]
    async fn should_reject_invalid_user_id_without_invoking_use_case() {
        let (router, commands) = router(UnsuspendOutcome::Success, false);

        let response = response(router, "not-a-uuid", "").await;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert!(lock(&commands).is_empty());
    }

    #[tokio::test]
    async fn should_reject_nonempty_request_body_without_invoking_use_case() {
        let (router, commands) = router(UnsuspendOutcome::Success, false);

        let response = response(router, &UserId::new().to_string(), "{}").await;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert!(lock(&commands).is_empty());
    }

    #[tokio::test]
    async fn should_reject_unsuspension_when_authentication_fails() {
        let (router, commands) = router(UnsuspendOutcome::Success, true);

        let response = response(router, &UserId::new().to_string(), "").await;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert!(lock(&commands).is_empty());
    }

    #[tokio::test]
    async fn should_map_unsuspend_user_not_found_error() {
        let (router, _) = router(UnsuspendOutcome::NotFound, false);

        let response = response(router, &UserId::new().to_string(), "").await;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("failed to read response: {error}"));
        let body: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("failed to decode response: {error}"));
        assert_eq!(serde_json::json!("USER_NOT_FOUND"), body["error"]);
    }
}
