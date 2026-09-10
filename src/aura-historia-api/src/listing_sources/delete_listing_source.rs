use crate::{
    auth::protected_context,
    error::{ApiError, LISTING_SOURCE_INTERNAL_ERROR},
    state::ListingSourcesState,
    wire::parse_path_object_id,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use listing_source_core::ListingSourceId;
use listing_source_service::use_cases::commands::delete_listing_source::DeleteListingSourceCommand;

pub async fn delete_listing_source(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
) -> Response {
    let listing_source_id: ListingSourceId =
        match parse_path_object_id(&raw_listing_source_id, "listingSourceId", "ListingSource") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
        };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let Some(delete) = state.delete.as_ref() else {
        return no_store(
            ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                .with_detail("Listing source deletion is not configured.")
                .into_response(),
        );
    };

    match delete
        .execute(&context, DeleteListingSourceCommand { listing_source_id })
        .await
    {
        Ok(()) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::{error::static_error, operation_context::OperationContext};
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
        routing::delete,
    };
    use listing_source_service::ports::ListingSourceDeletionBlocker;
    use listing_source_service::use_cases::commands::{
        create_listing_source::{
            CreateListingSourceCommand, CreateListingSourceError, CreateListingSourceResult,
            CreateListingSourceUseCase,
        },
        delete_listing_source::{DeleteListingSourceError, DeleteListingSourceUseCase},
        update_listing_source::{
            UpdateListingSourceCommand, UpdateListingSourceError, UpdateListingSourceResult,
            UpdateListingSourceUseCase,
        },
    };
    use listing_source_service::use_cases::queries::{
        get_listing_source::{
            GetListingSourceError, GetListingSourceRequest, GetListingSourceResult,
            GetListingSourceUseCase,
        },
        search_listing_sources::{
            SearchListingSourcesError, SearchListingSourcesRequest, SearchListingSourcesResult,
            SearchListingSourcesUseCase,
        },
    };
    use partnership_service::use_cases::queries::list_administered_listing_sources::{
        ListAdministeredListingSourcesError, ListAdministeredListingSourcesRequest,
        ListAdministeredListingSourcesResult, ListAdministeredListingSourcesUseCase,
    };
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    struct Unused;

    #[async_trait::async_trait]
    impl CreateListingSourceUseCase for Unused {
        async fn execute(
            &self,
            _: &OperationContext,
            _: CreateListingSourceCommand,
        ) -> Result<CreateListingSourceResult, CreateListingSourceError> {
            Err(CreateListingSourceError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl GetListingSourceUseCase for Unused {
        async fn execute(
            &self,
            _: &OperationContext,
            _: GetListingSourceRequest,
        ) -> Result<GetListingSourceResult, GetListingSourceError> {
            Err(GetListingSourceError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl UpdateListingSourceUseCase for Unused {
        async fn execute(
            &self,
            _: &OperationContext,
            _: UpdateListingSourceCommand,
        ) -> Result<UpdateListingSourceResult, UpdateListingSourceError> {
            Err(UpdateListingSourceError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl ListAdministeredListingSourcesUseCase for Unused {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ListAdministeredListingSourcesRequest,
        ) -> Result<ListAdministeredListingSourcesResult, ListAdministeredListingSourcesError>
        {
            Err(ListAdministeredListingSourcesError::Forbidden)
        }
    }

    #[async_trait::async_trait]
    impl SearchListingSourcesUseCase for Unused {
        async fn execute(
            &self,
            _: &OperationContext,
            _: SearchListingSourcesRequest,
        ) -> Result<SearchListingSourcesResult, SearchListingSourcesError> {
            Err(SearchListingSourcesError::Forbidden)
        }
    }

    struct FakeDelete {
        outcome: Mutex<Option<Result<(), DeleteListingSourceError>>>,
        calls: Arc<Mutex<usize>>,
    }

    #[async_trait::async_trait]
    impl DeleteListingSourceUseCase for FakeDelete {
        async fn execute(
            &self,
            _: &OperationContext,
            _: DeleteListingSourceCommand,
        ) -> Result<(), DeleteListingSourceError> {
            let mut calls = self
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *calls += 1;
            self.outcome
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .unwrap_or_else(|| {
                    Err(DeleteListingSourceError::Internal {
                        source: static_error("test outcome was not configured"),
                    })
                })
        }
    }

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
                Err(AuthError::InvalidCredentials)
            } else {
                Ok(TransportPrincipal::User {
                    user_id: UserId::new(),
                    auth_method: AuthMethod::CognitoJwt,
                    capabilities: Default::default(),
                })
            }
        }
    }

    fn router(
        outcome: Result<(), DeleteListingSourceError>,
        reject_auth: bool,
        calls: Arc<Mutex<usize>>,
    ) -> Router {
        let state = ListingSourcesState::new(
            Arc::new(Unused),
            Arc::new(Unused),
            Arc::new(Unused),
            Arc::new(Unused),
            Arc::new(Unused),
            Arc::new(FakeAuthenticator {
                reject: reject_auth,
            }),
        )
        .with_delete(Arc::new(FakeDelete {
            outcome: Mutex::new(Some(outcome)),
            calls,
        }));
        Router::new()
            .route(
                "/api/v1/admin/listing-sources/{listing_source_id}",
                delete(delete_listing_source),
            )
            .with_state(state)
    }

    async fn request(app: Router, path: &str) -> axum::response::Response {
        app.oneshot(
            Request::delete(path)
                .header(header::AUTHORIZATION, "Bearer valid")
                .body(Body::empty())
                .unwrap_or_else(|error| panic!("failed to build request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("request failed: {error}"))
    }

    fn execute_calls(calls: &Arc<Mutex<usize>>) -> usize {
        *calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[tokio::test]
    async fn should_return_empty_no_content_when_listing_source_is_deleted() {
        let calls = Arc::new(Mutex::new(0));
        let response = request(
            router(Ok(()), false, Arc::clone(&calls)),
            &format!("/api/v1/admin/listing-sources/{}", ListingSourceId::new()),
        )
        .await;

        assert_eq!(StatusCode::NO_CONTENT, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        assert!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap_or_else(|error| panic!("failed to read response body: {error}"))
                .is_empty()
        );
        assert_eq!(1, execute_calls(&calls));
    }

    #[tokio::test]
    async fn should_reject_invalid_listing_source_id_with_canonical_path_problem() {
        let calls = Arc::new(Mutex::new(0));
        let response = request(
            router(Ok(()), false, Arc::clone(&calls)),
            "/api/v1/admin/listing-sources/not-an-object-id",
        )
        .await;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        assert_eq!(
            serde_json::json!({
                "status": 400,
                "title": "Bad Request",
                "error": "INVALID_OBJECT_ID",
                "source": {"field": "listingSourceId", "type": "PATH"},
                "detail": "must be a valid ListingSource ID"
            }),
            serde_json::from_slice::<serde_json::Value>(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap_or_else(|error| panic!("failed to read response body: {error}"))
            )
            .unwrap_or_else(|error| panic!("failed to decode response: {error}")),
        );
        assert_eq!(0, execute_calls(&calls));
    }

    #[tokio::test]
    async fn should_reject_invalid_credentials_without_calling_delete_use_case() {
        let calls = Arc::new(Mutex::new(0));
        let response = request(
            router(Ok(()), true, Arc::clone(&calls)),
            &format!("/api/v1/admin/listing-sources/{}", ListingSourceId::new()),
        )
        .await;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        assert_eq!(0, execute_calls(&calls));
    }

    #[tokio::test]
    async fn should_map_delete_service_errors_to_canonical_problems() {
        for (error, status, code) in [
            (
                DeleteListingSourceError::Forbidden,
                StatusCode::FORBIDDEN,
                "FORBIDDEN",
            ),
            (
                DeleteListingSourceError::NotFound,
                StatusCode::NOT_FOUND,
                "LISTING_SOURCE_NOT_FOUND",
            ),
            (
                DeleteListingSourceError::DependencyConflict {
                    blocker: ListingSourceDeletionBlocker::RawStreams,
                },
                StatusCode::CONFLICT,
                "CONFLICT",
            ),
            (
                DeleteListingSourceError::TemporarilyUnavailable {
                    source: static_error("temporary test failure"),
                },
                StatusCode::SERVICE_UNAVAILABLE,
                "LISTING_SOURCE_TEMPORARILY_UNAVAILABLE",
            ),
            (
                DeleteListingSourceError::Internal {
                    source: static_error("internal test failure"),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
                "LISTING_SOURCE_INTERNAL_ERROR",
            ),
        ] {
            let calls = Arc::new(Mutex::new(0));
            let response = request(
                router(Err(error), false, Arc::clone(&calls)),
                &format!("/api/v1/admin/listing-sources/{}", ListingSourceId::new()),
            )
            .await;
            assert_eq!(status, response.status());
            assert_eq!(
                Some("no-store"),
                response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok())
            );
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap_or_else(|error| panic!("failed to read response body: {error}"));
            let body: serde_json::Value = serde_json::from_slice(&body)
                .unwrap_or_else(|error| panic!("failed to decode response: {error}"));
            assert_eq!(serde_json::json!(code), body["error"]);
            assert_eq!(1, execute_calls(&calls));
        }
    }
}
