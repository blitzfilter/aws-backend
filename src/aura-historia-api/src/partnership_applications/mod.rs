mod decide;
mod get;
mod get_own;
mod list_admin;
mod list_own;
mod mark_in_review;
mod submit;
mod types;
mod util;
mod withdraw;

use crate::state::PartnershipApplicationsState;
use axum::{
    Router,
    routing::{get, post},
};

pub(crate) fn router(state: PartnershipApplicationsState) -> Router {
    Router::new()
        .route(
            "/api/v1/me/partnership-applications",
            get(list_own::list_own).post(submit::submit),
        )
        .route(
            "/api/v1/me/partnership-applications/{partnership_application_id}",
            get(get_own::get_own).delete(withdraw::withdraw),
        )
        .route(
            "/api/v1/admin/partnership-applications",
            get(list_admin::list_admin),
        )
        .route(
            "/api/v1/admin/partnership-applications/{partnership_application_id}",
            get(get::get).patch(mark_in_review::mark_in_review),
        )
        .route(
            "/api/v1/admin/partnership-applications/{partnership_application_id}/decision",
            post(decide::decide),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::operation_context::OperationContext;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use partnership_service::use_cases::{
        commands::{
            approve_partnership_application::{
                ApprovePartnershipApplicationCommand, ApprovePartnershipApplicationError,
                ApprovePartnershipApplicationResult, ApprovePartnershipApplicationUseCase,
            },
            mark_partnership_application_in_review::{
                MarkPartnershipApplicationInReviewCommand, MarkPartnershipApplicationInReviewError,
                MarkPartnershipApplicationInReviewResult,
                MarkPartnershipApplicationInReviewUseCase,
            },
            reject_partnership_application::{
                RejectPartnershipApplicationCommand, RejectPartnershipApplicationError,
                RejectPartnershipApplicationResult, RejectPartnershipApplicationUseCase,
            },
            submit_partnership_application::{
                SubmitPartnershipApplicationCommand, SubmitPartnershipApplicationError,
                SubmitPartnershipApplicationResult, SubmitPartnershipApplicationUseCase,
            },
            withdraw_partnership_application::{
                WithdrawPartnershipApplicationCommand, WithdrawPartnershipApplicationError,
                WithdrawPartnershipApplicationResult, WithdrawPartnershipApplicationUseCase,
            },
        },
        queries::{
            get_own_partnership_application::{
                GetOwnPartnershipApplicationError, GetOwnPartnershipApplicationRequest,
                GetOwnPartnershipApplicationResult, GetOwnPartnershipApplicationUseCase,
            },
            get_partnership_application::{
                GetPartnershipApplicationError, GetPartnershipApplicationRequest,
                GetPartnershipApplicationResult, GetPartnershipApplicationUseCase,
            },
            list_admin_partnership_applications::{
                ListAdminPartnershipApplicationsError, ListAdminPartnershipApplicationsRequest,
                ListAdminPartnershipApplicationsResult, ListAdminPartnershipApplicationsUseCase,
            },
            list_own_partnership_applications::{
                ListOwnPartnershipApplicationsError, ListOwnPartnershipApplicationsRequest,
                ListOwnPartnershipApplicationsResult, ListOwnPartnershipApplicationsUseCase,
            },
        },
    };
    use std::{collections::BTreeSet, sync::Arc};
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    #[derive(Clone, Copy)]
    struct FakeUseCases;

    #[async_trait::async_trait]
    impl SubmitPartnershipApplicationUseCase for FakeUseCases {
        async fn execute(
            &self,
            _: &OperationContext,
            _: SubmitPartnershipApplicationCommand,
        ) -> Result<SubmitPartnershipApplicationResult, SubmitPartnershipApplicationError> {
            Err(SubmitPartnershipApplicationError::AuthenticatedActorRequired)
        }
    }

    #[async_trait::async_trait]
    impl ListOwnPartnershipApplicationsUseCase for FakeUseCases {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ListOwnPartnershipApplicationsRequest,
        ) -> Result<ListOwnPartnershipApplicationsResult, ListOwnPartnershipApplicationsError>
        {
            Ok(ListOwnPartnershipApplicationsResult { items: Vec::new() })
        }
    }

    #[async_trait::async_trait]
    impl GetOwnPartnershipApplicationUseCase for FakeUseCases {
        async fn execute(
            &self,
            _: &OperationContext,
            _: GetOwnPartnershipApplicationRequest,
        ) -> Result<GetOwnPartnershipApplicationResult, GetOwnPartnershipApplicationError> {
            Err(GetOwnPartnershipApplicationError::NotFound)
        }
    }

    #[async_trait::async_trait]
    impl WithdrawPartnershipApplicationUseCase for FakeUseCases {
        async fn execute(
            &self,
            _: &OperationContext,
            _: WithdrawPartnershipApplicationCommand,
        ) -> Result<WithdrawPartnershipApplicationResult, WithdrawPartnershipApplicationError>
        {
            Err(WithdrawPartnershipApplicationError::NotFound)
        }
    }

    #[async_trait::async_trait]
    impl ListAdminPartnershipApplicationsUseCase for FakeUseCases {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ListAdminPartnershipApplicationsRequest,
        ) -> Result<ListAdminPartnershipApplicationsResult, ListAdminPartnershipApplicationsError>
        {
            Ok(ListAdminPartnershipApplicationsResult::default())
        }
    }

    #[async_trait::async_trait]
    impl GetPartnershipApplicationUseCase for FakeUseCases {
        async fn execute(
            &self,
            _: &OperationContext,
            _: GetPartnershipApplicationRequest,
        ) -> Result<GetPartnershipApplicationResult, GetPartnershipApplicationError> {
            Err(GetPartnershipApplicationError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl MarkPartnershipApplicationInReviewUseCase for FakeUseCases {
        async fn execute(
            &self,
            _: &OperationContext,
            _: MarkPartnershipApplicationInReviewCommand,
        ) -> Result<MarkPartnershipApplicationInReviewResult, MarkPartnershipApplicationInReviewError>
        {
            Err(MarkPartnershipApplicationInReviewError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl ApprovePartnershipApplicationUseCase for FakeUseCases {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ApprovePartnershipApplicationCommand,
        ) -> Result<ApprovePartnershipApplicationResult, ApprovePartnershipApplicationError>
        {
            Err(ApprovePartnershipApplicationError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl RejectPartnershipApplicationUseCase for FakeUseCases {
        async fn execute(
            &self,
            _: &OperationContext,
            _: RejectPartnershipApplicationCommand,
        ) -> Result<RejectPartnershipApplicationResult, RejectPartnershipApplicationError> {
            Err(RejectPartnershipApplicationError::Forbidden)
        }
    }

    struct FakeAuthenticator;

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _: &str,
            _: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Ok(TransportPrincipal::User {
                user_id: UserId::new(),
                auth_method: AuthMethod::CognitoJwt,
                capabilities: BTreeSet::new(),
            })
        }
    }

    fn test_router() -> Router {
        let use_cases = Arc::new(FakeUseCases);
        router(PartnershipApplicationsState::new(
            use_cases.clone(),
            use_cases.clone(),
            use_cases.clone(),
            use_cases.clone(),
            use_cases.clone(),
            use_cases.clone(),
            use_cases.clone(),
            use_cases.clone(),
            use_cases,
            Arc::new(FakeAuthenticator),
        ))
    }

    #[tokio::test]
    async fn should_serve_the_canonical_owned_list_without_store() {
        let request = Request::builder()
            .uri("/api/v1/me/partnership-applications")
            .header("Authorization", "Bearer test")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));

        let response = test_router()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("router failed: {error}"));

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
    }

    #[tokio::test]
    async fn should_serve_the_canonical_admin_collection_without_store() {
        let request = Request::builder()
            .uri("/api/v1/admin/partnership-applications")
            .header("Authorization", "Bearer test")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));

        let response = test_router()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("router failed: {error}"));
        let status = response.status();
        let cache_control = response
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("failed to read response body: {error}"));
        let body: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("failed to decode response body: {error}"));

        assert_eq!(StatusCode::OK, status);
        assert_eq!(Some(serde_json::json!([])), body.get("items").cloned());
        assert_eq!(Some(serde_json::json!(21)), body.get("size").cloned());
        assert_eq!(Some("no-store".to_owned()), cache_control);
    }

    #[tokio::test]
    async fn should_route_admin_detail_to_the_canonical_admin_path() {
        let request = Request::builder()
            .uri("/api/v1/admin/partnership-applications/pa_01h455vb4pex5vy7enb1p677vn")
            .header("Authorization", "Bearer test")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));

        let response = test_router()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("router failed: {error}"));

        assert_eq!(StatusCode::FORBIDDEN, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
    }

    #[tokio::test]
    async fn should_route_admin_mark_in_review_to_the_canonical_admin_path() {
        let request = Request::builder()
            .method("PATCH")
            .uri("/api/v1/admin/partnership-applications/pa_01h455vb4pex5vy7enb1p677vn")
            .header("Authorization", "Bearer test")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));

        let response = test_router()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("router failed: {error}"));

        assert_eq!(StatusCode::FORBIDDEN, response.status());
    }

    #[tokio::test]
    async fn should_route_admin_decision_to_the_canonical_admin_path() {
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/admin/partnership-applications/pa_01h455vb4pex5vy7enb1p677vn/decision")
            .header("Authorization", "Bearer test")
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"decision":"APPROVE"}"#))
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));

        let response = test_router()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("router failed: {error}"));

        assert_eq!(StatusCode::FORBIDDEN, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
    }

    #[tokio::test]
    async fn should_remove_the_legacy_admin_decision_route() {
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/partnership-applications/pa_01h455vb4pex5vy7enb1p677vn/decision")
            .header("Authorization", "Bearer test")
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"decision":"APPROVE"}"#))
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));

        let response = test_router()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("router failed: {error}"));

        assert_eq!(StatusCode::NOT_FOUND, response.status());
    }

    #[tokio::test]
    async fn should_remove_the_legacy_admin_mark_in_review_route() {
        let request = Request::builder()
            .method("PATCH")
            .uri("/api/v1/partnership-applications/pa_01h455vb4pex5vy7enb1p677vn")
            .header("Authorization", "Bearer test")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));

        let response = test_router()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("router failed: {error}"));

        assert_eq!(StatusCode::NOT_FOUND, response.status());
    }

    #[tokio::test]
    async fn should_remove_the_legacy_admin_collection_route() {
        let request = Request::builder()
            .uri("/api/v1/partnership-applications")
            .header("Authorization", "Bearer test")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));

        let response = test_router()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("router failed: {error}"));

        assert_eq!(StatusCode::NOT_FOUND, response.status());
    }
}
