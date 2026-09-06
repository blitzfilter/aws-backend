use crate::{
    auth::protected_context,
    error::{ApiError, INVALID_UUID},
    state::PartnershipsState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use listing_source_core::ListingSourceId;
use partnership_core::partnership_id::PartnershipId;
use partnership_service::use_cases::commands::revoke_partnership_listing_source::RevokePartnershipListingSourceCommand;
use uuid::Uuid;

pub(super) async fn delete_listing_source_grant(
    State(state): State<PartnershipsState>,
    headers: HeaderMap,
    Path((raw_partnership_id, raw_listing_source_id)): Path<(String, String)>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let partnership_id = match parse_partnership_id(&raw_partnership_id) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let listing_source_id = match parse_listing_source_id(&raw_listing_source_id) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };

    match state
        .revoke_listing_source
        .execute(
            &context,
            RevokePartnershipListingSourceCommand {
                partnership_id,
                listing_source_id,
            },
        )
        .await
    {
        Ok(_) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_partnership_id(raw: &str) -> Result<PartnershipId, ApiError> {
    Uuid::parse_str(raw).map(PartnershipId::from).map_err(|_| {
        ApiError::bad_request(INVALID_UUID)
            .with_path_field("partnershipId")
            .with_detail("Path parameter 'partnershipId' must be a UUID.")
    })
}

fn parse_listing_source_id(raw: &str) -> Result<ListingSourceId, ApiError> {
    Uuid::parse_str(raw)
        .map(ListingSourceId::from)
        .map_err(|_| {
            ApiError::bad_request(INVALID_UUID)
                .with_path_field("listingSourceId")
                .with_detail("Path parameter 'listingSourceId' must be a UUID.")
        })
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
    use application::{
        error::static_error,
        operation_context::OperationContext,
        pagination::{Cursor, CursoredResult},
    };
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
        routing::delete,
    };
    use partnership_service::use_cases::{
        commands::{
            grant_partnership_listing_source::{
                GrantPartnershipListingSourceCommand, GrantPartnershipListingSourceError,
                GrantPartnershipListingSourceResult, GrantPartnershipListingSourceUseCase,
            },
            grant_partnership_membership::{
                GrantPartnershipMembershipCommand, GrantPartnershipMembershipError,
                GrantPartnershipMembershipResult, GrantPartnershipMembershipUseCase,
            },
            revoke_partnership_listing_source::{
                RevokePartnershipListingSourceCommand, RevokePartnershipListingSourceError,
                RevokePartnershipListingSourceOutcome, RevokePartnershipListingSourceResult,
                RevokePartnershipListingSourceUseCase,
            },
            revoke_partnership_membership::{
                RevokePartnershipMembershipCommand, RevokePartnershipMembershipError,
                RevokePartnershipMembershipResult, RevokePartnershipMembershipUseCase,
            },
        },
        queries::{
            get_admin_partnership::{
                AdminPartnershipDetailsView, GetAdminPartnershipError, GetAdminPartnershipRequest,
                GetAdminPartnershipUseCase,
            },
            list_admin_partnerships::{
                ListAdminPartnershipsError, ListAdminPartnershipsRequest,
                ListAdminPartnershipsResult, ListAdminPartnershipsUseCase,
            },
        },
    };
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    type Outcome = Arc<
        Mutex<
            Option<
                Result<RevokePartnershipListingSourceResult, RevokePartnershipListingSourceError>,
            >,
        >,
    >;
    type Requests = Arc<Mutex<Vec<(OperationContext, RevokePartnershipListingSourceCommand)>>>;

    #[derive(Clone)]
    struct FakeRevokePartnershipListingSourceUseCase {
        outcome: Outcome,
        requests: Requests,
    }

    #[async_trait::async_trait]
    impl RevokePartnershipListingSourceUseCase for FakeRevokePartnershipListingSourceUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            command: RevokePartnershipListingSourceCommand,
        ) -> Result<RevokePartnershipListingSourceResult, RevokePartnershipListingSourceError>
        {
            lock(&self.requests).push((context.clone(), command));
            lock(&self.outcome).take().unwrap_or_else(|| {
                Err(RevokePartnershipListingSourceError::Internal {
                    source: static_error("test outcome was not configured"),
                })
            })
        }
    }

    #[derive(Clone, Copy)]
    struct UnusedListAdminPartnershipsUseCase;

    #[async_trait::async_trait]
    impl ListAdminPartnershipsUseCase for UnusedListAdminPartnershipsUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: ListAdminPartnershipsRequest,
        ) -> Result<ListAdminPartnershipsResult, ListAdminPartnershipsError> {
            Ok(CursoredResult {
                items: Vec::new(),
                cursor: Cursor {
                    size: 1,
                    search_after: None,
                },
                total: None,
            })
        }
    }

    #[derive(Clone, Copy)]
    struct UnusedGetAdminPartnershipUseCase;

    #[async_trait::async_trait]
    impl GetAdminPartnershipUseCase for UnusedGetAdminPartnershipUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetAdminPartnershipRequest,
        ) -> Result<AdminPartnershipDetailsView, GetAdminPartnershipError> {
            Err(GetAdminPartnershipError::Internal {
                source: static_error("admin detail is not used by this test"),
            })
        }
    }

    #[derive(Clone, Copy)]
    struct UnusedGrantPartnershipMembershipUseCase;

    #[async_trait::async_trait]
    impl GrantPartnershipMembershipUseCase for UnusedGrantPartnershipMembershipUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: GrantPartnershipMembershipCommand,
        ) -> Result<GrantPartnershipMembershipResult, GrantPartnershipMembershipError> {
            Err(GrantPartnershipMembershipError::Internal {
                source: static_error("membership grant is not used by this test"),
            })
        }
    }

    #[derive(Clone, Copy)]
    struct UnusedRevokePartnershipMembershipUseCase;

    #[async_trait::async_trait]
    impl RevokePartnershipMembershipUseCase for UnusedRevokePartnershipMembershipUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: RevokePartnershipMembershipCommand,
        ) -> Result<RevokePartnershipMembershipResult, RevokePartnershipMembershipError> {
            Err(RevokePartnershipMembershipError::Internal {
                source: static_error("membership revoke is not used by this test"),
            })
        }
    }

    #[derive(Clone, Copy)]
    struct UnusedGrantPartnershipListingSourceUseCase;

    #[async_trait::async_trait]
    impl GrantPartnershipListingSourceUseCase for UnusedGrantPartnershipListingSourceUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: GrantPartnershipListingSourceCommand,
        ) -> Result<GrantPartnershipListingSourceResult, GrantPartnershipListingSourceError>
        {
            Err(GrantPartnershipListingSourceError::Internal {
                source: static_error("listing source grant is not used by this test"),
            })
        }
    }

    #[derive(Clone, Copy)]
    struct FakeAuthenticator {
        user_id: UserId,
        reject: bool,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            if self.reject {
                Err(AuthError::InvalidCredentials)
            } else {
                Ok(TransportPrincipal::User {
                    user_id: self.user_id,
                    auth_method: AuthMethod::CognitoJwt,
                    capabilities: BTreeSet::new(),
                })
            }
        }
    }

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        match value.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn test_router(
        outcome: Result<RevokePartnershipListingSourceResult, RevokePartnershipListingSourceError>,
        requests: Requests,
        reject_auth: bool,
    ) -> Router {
        let state = PartnershipsState::new(
            Arc::new(UnusedListAdminPartnershipsUseCase),
            Arc::new(UnusedGetAdminPartnershipUseCase),
            Arc::new(UnusedGrantPartnershipMembershipUseCase),
            Arc::new(UnusedRevokePartnershipMembershipUseCase),
            Arc::new(UnusedGrantPartnershipListingSourceUseCase),
            Arc::new(FakeRevokePartnershipListingSourceUseCase {
                outcome: Arc::new(Mutex::new(Some(outcome))),
                requests,
            }),
            Arc::new(FakeAuthenticator {
                user_id: UserId::from(Uuid::from_u128(0x880e8400e29b41d4a716446655440000)),
                reject: reject_auth,
            }),
        );
        Router::new()
            .route(
                "/api/v1/admin/partnerships/{partnership_id}/listing-source-grants/{listing_source_id}",
                delete(super::delete_listing_source_grant),
            )
            .with_state(state)
    }

    fn request(uri: &str, authorization: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("DELETE")
            .uri(uri)
            .header("X-Request-Id", "request-123")
            .header("X-Correlation-Id", "correlation-456");
        if let Some(authorization) = authorization {
            builder = builder.header("Authorization", authorization);
        }
        builder
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("failed to create request: {error}"))
    }

    fn success(
        outcome: RevokePartnershipListingSourceOutcome,
    ) -> RevokePartnershipListingSourceResult {
        RevokePartnershipListingSourceResult { outcome }
    }

    #[tokio::test]
    async fn should_return_204_for_removed_and_absent_grant()
    -> Result<(), Box<dyn std::error::Error>> {
        for outcome in [
            RevokePartnershipListingSourceOutcome::Removed,
            RevokePartnershipListingSourceOutcome::AlreadyAbsent,
        ] {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let response = test_router(Ok(success(outcome)), Arc::clone(&requests), false)
                .oneshot(request(
                    "/api/v1/admin/partnerships/770e8400-e29b-41d4-a716-446655440000/listing-source-grants/990e8400-e29b-41d4-a716-446655440000",
                    Some("Bearer valid"),
                ))
                .await?;

            assert_eq!(StatusCode::NO_CONTENT, response.status());
            assert_eq!(
                Some("no-store"),
                response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok())
            );
            assert!(to_bytes(response.into_body(), usize::MAX).await?.is_empty());
            let requests = lock(&requests);
            assert_eq!(1, requests.len());
            assert_eq!(
                PartnershipId::from(Uuid::from_u128(0x770e8400e29b41d4a716446655440000)),
                requests[0].1.partnership_id
            );
            assert_eq!(
                ListingSourceId::from(Uuid::from_u128(0x990e8400e29b41d4a716446655440000)),
                requests[0].1.listing_source_id
            );
            assert_eq!("request-123", requests[0].0.request_id.as_str());
            assert_eq!("correlation-456", requests[0].0.correlation_id.as_str());
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_path_ids_without_calling_service()
    -> Result<(), Box<dyn std::error::Error>> {
        for (path, field) in [
            (
                "/api/v1/admin/partnerships/not-a-uuid/listing-source-grants/990e8400-e29b-41d4-a716-446655440000",
                "partnershipId",
            ),
            (
                "/api/v1/admin/partnerships/770e8400-e29b-41d4-a716-446655440000/listing-source-grants/not-a-uuid",
                "listingSourceId",
            ),
        ] {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let response = test_router(
                Ok(success(RevokePartnershipListingSourceOutcome::Removed)),
                Arc::clone(&requests),
                false,
            )
            .oneshot(request(path, Some("Bearer valid")))
            .await?;

            assert_eq!(StatusCode::BAD_REQUEST, response.status());
            assert_eq!(
                Some("no-store"),
                response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok())
            );
            let body = to_bytes(response.into_body(), usize::MAX).await?;
            let body: serde_json::Value = serde_json::from_slice(&body)
                .unwrap_or_else(|error| panic!("failed to decode error response: {error}"));
            assert_eq!("INVALID_UUID", body["error"]);
            assert_eq!(field, body["source"]["field"]);
            assert!(lock(&requests).is_empty());
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_missing_or_invalid_auth_without_calling_service()
    -> Result<(), Box<dyn std::error::Error>> {
        for (authorization, reject_auth) in [(None, false), (Some("Bearer invalid"), true)] {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let response = test_router(
                Ok(success(RevokePartnershipListingSourceOutcome::Removed)),
                Arc::clone(&requests),
                reject_auth,
            )
            .oneshot(request(
                "/api/v1/admin/partnerships/770e8400-e29b-41d4-a716-446655440000/listing-source-grants/990e8400-e29b-41d4-a716-446655440000",
                authorization,
            ))
            .await?;

            assert_eq!(StatusCode::UNAUTHORIZED, response.status());
            assert_eq!(
                Some("no-store"),
                response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok())
            );
            let body = to_bytes(response.into_body(), usize::MAX).await?;
            let body: serde_json::Value = serde_json::from_slice(&body)
                .unwrap_or_else(|error| panic!("failed to decode error response: {error}"));
            assert_eq!("INVALID_CREDENTIALS", body["error"]);
            assert!(lock(&requests).is_empty());
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_map_service_errors_to_api_problems() -> Result<(), Box<dyn std::error::Error>> {
        let cases = [
            (
                RevokePartnershipListingSourceError::Forbidden,
                StatusCode::FORBIDDEN,
                "FORBIDDEN",
            ),
            (
                RevokePartnershipListingSourceError::PartnershipNotFound,
                StatusCode::NOT_FOUND,
                "PARTNERSHIP_NOT_FOUND",
            ),
            (
                RevokePartnershipListingSourceError::ListingSourceNotFound,
                StatusCode::NOT_FOUND,
                "LISTING_SOURCE_NOT_FOUND",
            ),
            (
                RevokePartnershipListingSourceError::TemporarilyUnavailable {
                    source: static_error("temporary"),
                },
                StatusCode::SERVICE_UNAVAILABLE,
                "PARTNERSHIP_TEMPORARILY_UNAVAILABLE",
            ),
            (
                RevokePartnershipListingSourceError::InvalidPersistedState {
                    source: static_error("invalid"),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
                "PARTNERSHIP_INTERNAL_ERROR",
            ),
            (
                RevokePartnershipListingSourceError::Internal {
                    source: static_error("internal"),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
                "PARTNERSHIP_INTERNAL_ERROR",
            ),
            (
                RevokePartnershipListingSourceError::BeginTransactionFailed,
                StatusCode::SERVICE_UNAVAILABLE,
                "PARTNERSHIP_TEMPORARILY_UNAVAILABLE",
            ),
            (
                RevokePartnershipListingSourceError::CommitTransactionFailed,
                StatusCode::SERVICE_UNAVAILABLE,
                "PARTNERSHIP_TEMPORARILY_UNAVAILABLE",
            ),
        ];

        for (error, status, code) in cases {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let response = test_router(Err(error), Arc::clone(&requests), false)
                .oneshot(request(
                    "/api/v1/admin/partnerships/770e8400-e29b-41d4-a716-446655440000/listing-source-grants/990e8400-e29b-41d4-a716-446655440000",
                    Some("Bearer valid"),
                ))
                .await?;

            assert_eq!(status, response.status());
            assert_eq!(
                Some("no-store"),
                response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok())
            );
            let body = to_bytes(response.into_body(), usize::MAX).await?;
            let body: serde_json::Value = serde_json::from_slice(&body)
                .unwrap_or_else(|error| panic!("failed to decode error response: {error}"));
            assert_eq!(code, body["error"]);
        }
        Ok(())
    }
}
