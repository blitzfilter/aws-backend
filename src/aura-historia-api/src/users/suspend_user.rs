use super::util::{no_store, parse_json, parse_user_id};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::state::UsersState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use user_core::user_id::UserId;
use user_service::use_cases::{SuspendUserCommand, SuspendUserResult, SuspendUserUseCase};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuspendUserData {
    reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SuspendUserResponseData {
    user_id: UserId,
    suspended: bool,
}

fn suspend_user_command(
    user_id: UserId,
    data: SuspendUserData,
) -> Result<SuspendUserCommand, ApiError> {
    if data.reason.trim().is_empty() {
        return Err(
            ApiError::bad_request(BAD_BODY_VALUE).with_detail("Suspension reason is required.")
        );
    }

    Ok(SuspendUserCommand {
        user_id,
        reason: data.reason,
    })
}

impl From<SuspendUserResult> for SuspendUserResponseData {
    fn from(result: SuspendUserResult) -> Self {
        Self {
            user_id: result.user_id,
            suspended: result.suspended,
        }
    }
}

pub async fn suspend_user(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw_user_id): Path<String>,
    body: String,
) -> Response {
    suspend_user_with(
        state.authenticator.as_ref(),
        state.suspend_user.as_ref(),
        headers,
        raw_user_id,
        body,
    )
    .await
}

async fn suspend_user_with(
    authenticator: &dyn crate::auth::TokenAuthenticator,
    suspend_user: &dyn SuspendUserUseCase,
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
    let data: SuspendUserData = match parse_json(&body) {
        Ok(value) => value,
        Err(response) => return no_store(response),
    };
    let command = match suspend_user_command(user_id, data) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };

    match suspend_user.execute(&context, command).await {
        Ok(result) => no_store(Json(SuspendUserResponseData::from(result)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_suspend_request_to_command() -> Result<(), ApiError> {
        let user_id = UserId::new();
        let command = suspend_user_command(
            user_id,
            SuspendUserData {
                reason: "Repeated policy violations".to_owned(),
            },
        )?;

        assert_eq!(user_id, command.user_id);
        assert_eq!("Repeated policy violations", command.reason);
        Ok(())
    }

    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::operation_context::OperationContext;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use axum::routing::put;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;
    use user_service::use_cases::SuspendUserError;

    #[derive(Clone, Copy)]
    enum SuspendOutcome {
        Success,
        InvalidReason,
        NotFound,
    }

    #[derive(Clone)]
    struct FakeSuspendUserUseCase {
        outcome: SuspendOutcome,
        commands: Arc<Mutex<Vec<SuspendUserCommand>>>,
    }

    #[async_trait::async_trait]
    impl SuspendUserUseCase for FakeSuspendUserUseCase {
        async fn execute(
            &self,
            _: &OperationContext,
            command: SuspendUserCommand,
        ) -> Result<SuspendUserResult, SuspendUserError> {
            lock(&self.commands).push(command.clone());
            match self.outcome {
                SuspendOutcome::Success => Ok(SuspendUserResult {
                    user_id: command.user_id,
                    suspended: true,
                }),
                SuspendOutcome::InvalidReason => Err(SuspendUserError::InvalidReason),
                SuspendOutcome::NotFound => Err(SuspendUserError::UserNotFound),
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
        suspend_user: Arc<dyn SuspendUserUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    }

    async fn test_suspend_user(
        State(state): State<TestState>,
        headers: HeaderMap,
        Path(raw_user_id): Path<String>,
        body: String,
    ) -> Response {
        suspend_user_with(
            state.authenticator.as_ref(),
            state.suspend_user.as_ref(),
            headers,
            raw_user_id,
            body,
        )
        .await
    }

    fn router(
        outcome: SuspendOutcome,
        reject_auth: bool,
    ) -> (Router, Arc<Mutex<Vec<SuspendUserCommand>>>) {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let state = TestState {
            suspend_user: Arc::new(FakeSuspendUserUseCase {
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
                    put(test_suspend_user),
                )
                .with_state(state),
            commands,
        )
    }

    async fn response(router: Router, user_id: UserId, body: &'static str) -> Response {
        router
            .oneshot(
                Request::builder()
                    .method("PUT")
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
    async fn should_map_suspend_request_and_return_no_store_response() {
        let user_id = UserId::new();
        let (router, commands) = router(SuspendOutcome::Success, false);

        let response = response(
            router,
            user_id,
            r#"{"reason":"Repeated policy violations"}"#,
        )
        .await;
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
        assert_eq!(serde_json::json!(true), body["suspended"]);
        let commands = lock(&commands);
        assert_eq!(1, commands.len());
        assert_eq!(user_id, commands[0].user_id);
        assert_eq!("Repeated policy violations", commands[0].reason);
    }

    #[tokio::test]
    async fn should_reject_missing_or_empty_suspend_reason() {
        for body in [r#"{}"#, r#"{"reason":"  "}"#] {
            let (router, commands) = router(SuspendOutcome::Success, false);
            let response = response(router, UserId::new(), body).await;

            assert_eq!(StatusCode::BAD_REQUEST, response.status());
            assert_eq!(
                Some("no-store"),
                response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|v| v.to_str().ok())
            );
            assert!(lock(&commands).is_empty());
        }
    }

    #[tokio::test]
    async fn should_reject_suspension_when_authentication_fails() {
        let (router, commands) = router(SuspendOutcome::Success, true);
        let response = response(
            router,
            UserId::new(),
            r#"{"reason":"Repeated policy violations"}"#,
        )
        .await;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert!(lock(&commands).is_empty());
    }

    #[tokio::test]
    async fn should_map_secret_bearing_suspend_reason_to_bad_request() {
        let (router, commands) = router(SuspendOutcome::InvalidReason, false);
        let response = response(
            router,
            UserId::new(),
            r#"{"reason":"Included bearer credential for review"}"#,
        )
        .await;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("failed to read response: {error}"));
        let body: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("failed to decode response: {error}"));
        assert_eq!(serde_json::json!("BAD_BODY_VALUE"), body["error"]);
        assert_eq!(
            serde_json::json!("Suspension reason is invalid."),
            body["detail"]
        );
        assert_eq!(1, lock(&commands).len());
    }

    #[tokio::test]
    async fn should_map_suspend_user_not_found_error() {
        let (router, _) = router(SuspendOutcome::NotFound, false);
        let response = response(
            router,
            UserId::new(),
            r#"{"reason":"Repeated policy violations"}"#,
        )
        .await;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("failed to read response: {error}"));
        let body: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("failed to decode response: {error}"));
        assert_eq!(serde_json::json!("USER_NOT_FOUND"), body["error"]);
    }

    #[test]
    fn should_reject_empty_suspend_reason() {
        let error = suspend_user_command(
            UserId::new(),
            SuspendUserData {
                reason: " \t ".to_owned(),
            },
        )
        .err();

        assert!(error.is_some_and(|error| error.code() == BAD_BODY_VALUE));
    }
}
