use crate::{
    auth::protected_context,
    error::{ApiError, INVALID_UUID},
    state::PartnershipsState,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};

use partnership_core::partnership_id::PartnershipId;
use partnership_service::use_cases::queries::get_admin_partnership::{
    AdminPartnershipDetailsView, GetAdminPartnershipRequest,
};
use serde::Serialize;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminPartnershipDetailsData {
    partnership_id: Uuid,
    party: AdminPartnershipPartyData,
    member_user_ids: Vec<Uuid>,
    listing_source_ids: Vec<Uuid>,
    member_count: u64,
    listing_source_grant_count: u64,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminPartnershipPartyData {
    party_id: Uuid,
    party_slug_id: String,
    name: String,
}

impl From<AdminPartnershipDetailsView> for AdminPartnershipDetailsData {
    fn from(value: AdminPartnershipDetailsView) -> Self {
        Self {
            partnership_id: value.partnership_id.into(),
            party: AdminPartnershipPartyData::from(value.party),
            member_user_ids: value.member_user_ids.into_iter().map(Uuid::from).collect(),
            listing_source_ids: value
                .listing_source_ids
                .into_iter()
                .map(Uuid::from)
                .collect(),
            member_count: value.member_count,
            listing_source_grant_count: value.listing_source_grant_count,
            created: value.created,
            updated: value.updated,
        }
    }
}

impl From<partnership_service::use_cases::queries::list_admin_partnerships::PartnershipPartySummary>
    for AdminPartnershipPartyData
{
    fn from(
        value: partnership_service::use_cases::queries::list_admin_partnerships::PartnershipPartySummary,
    ) -> Self {
        Self {
            party_id: value.party_id.into(),
            party_slug_id: value.party_slug_id.to_string(),
            name: value.name.to_string(),
        }
    }
}

pub(super) async fn get_admin(
    State(state): State<PartnershipsState>,
    headers: HeaderMap,
    Path(raw_partnership_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let partnership_id = match parse_partnership_id(&raw_partnership_id) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };

    match state
        .get_admin
        .execute(&context, GetAdminPartnershipRequest { partnership_id })
        .await
    {
        Ok(result) => no_store(Json(AdminPartnershipDetailsData::from(result)).into_response()),
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
    };
    use listing_source_core::ListingSourceId;
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
                RevokePartnershipListingSourceResult, RevokePartnershipListingSourceUseCase,
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
    use time::macros::datetime;
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    #[derive(Clone)]
    struct FakeGetAdminPartnershipUseCase {
        outcome: Arc<Mutex<Option<Result<AdminPartnershipDetailsView, GetAdminPartnershipError>>>>,
        requests: Arc<Mutex<Vec<(OperationContext, GetAdminPartnershipRequest)>>>,
    }

    #[async_trait::async_trait]
    impl GetAdminPartnershipUseCase for FakeGetAdminPartnershipUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            request: GetAdminPartnershipRequest,
        ) -> Result<AdminPartnershipDetailsView, GetAdminPartnershipError> {
            lock(&self.requests).push((context.clone(), request));
            lock(&self.outcome).take().unwrap_or_else(|| {
                Err(GetAdminPartnershipError::Internal {
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
    struct UnusedRevokePartnershipListingSourceUseCase;

    #[async_trait::async_trait]
    impl RevokePartnershipListingSourceUseCase for UnusedRevokePartnershipListingSourceUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: RevokePartnershipListingSourceCommand,
        ) -> Result<RevokePartnershipListingSourceResult, RevokePartnershipListingSourceError>
        {
            Err(RevokePartnershipListingSourceError::Internal {
                source: static_error("listing source grant revoke is not used by this test"),
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

    fn details() -> AdminPartnershipDetailsView {
        AdminPartnershipDetailsView {
            partnership_id: PartnershipId::from(Uuid::from_u128(
                0x770e8400e29b41d4a716446655440000,
            )),
            party: partnership_service::use_cases::queries::list_admin_partnerships::PartnershipPartySummary {
                party_id: party_core::party_id::PartyId::from(Uuid::from_u128(
                    0x550e8400e29b41d4a716446655440000,
                )),
                party_slug_id: party_core::party_slug_id::PartySlugId::raw("safe-party")
                    .unwrap_or_else(|error| panic!("valid party slug: {error}")),
                name: party_core::party_name::PartyName::try_from("Safe Party")
                    .unwrap_or_else(|error| panic!("valid party name: {error}")),
            },
            member_user_ids: vec![UserId::from(Uuid::from_u128(
                0x660e8400e29b41d4a716446655440000,
            ))],
            listing_source_ids: vec![ListingSourceId::from(Uuid::from_u128(
                0x990e8400e29b41d4a716446655440000,
            ))],
            member_count: 2,
            listing_source_grant_count: 3,
            created: datetime!(2026-01-02 12:00 UTC),
            updated: datetime!(2026-02-03 12:00 UTC),
        }
    }

    fn test_router(
        outcome: Result<AdminPartnershipDetailsView, GetAdminPartnershipError>,
        requests: Arc<Mutex<Vec<(OperationContext, GetAdminPartnershipRequest)>>>,
        reject_auth: bool,
    ) -> Router {
        let get_admin_use_case = FakeGetAdminPartnershipUseCase {
            outcome: Arc::new(Mutex::new(Some(outcome))),
            requests,
        };
        let state = PartnershipsState::new(
            Arc::new(UnusedListAdminPartnershipsUseCase),
            Arc::new(get_admin_use_case),
            Arc::new(UnusedGrantPartnershipMembershipUseCase),
            Arc::new(UnusedRevokePartnershipMembershipUseCase),
            Arc::new(UnusedGrantPartnershipListingSourceUseCase),
            Arc::new(UnusedRevokePartnershipListingSourceUseCase),
            Arc::new(FakeAuthenticator {
                user_id: UserId::from(Uuid::from_u128(0x880e8400e29b41d4a716446655440000)),
                reject: reject_auth,
            }),
        );
        Router::new()
            .route(
                "/api/v1/admin/partnerships/{partnership_id}",
                axum::routing::get(super::get_admin),
            )
            .with_state(state)
    }

    async fn json(response: Response) -> Result<serde_json::Value, axum::Error> {
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&body).unwrap_or_default())
    }

    #[test]
    fn should_map_partnership_id_from_uuid_path_value() {
        let id = parse_partnership_id("770e8400-e29b-41d4-a716-446655440000")
            .unwrap_or_else(|error| panic!("failed to parse partnership ID: {error}"));
        assert_eq!("770e8400-e29b-41d4-a716-446655440000", id.to_string());
    }

    #[test]
    fn should_report_invalid_partnership_id_as_path_uuid_problem() {
        let error = parse_partnership_id("not-a-uuid")
            .err()
            .unwrap_or_else(|| panic!("invalid partnership ID was accepted"));

        assert_eq!(INVALID_UUID, error.code());
        let response = error.into_response();
        assert_eq!(StatusCode::BAD_REQUEST, response.status());
    }

    #[tokio::test]
    async fn should_return_detail_shape_and_no_store_header()
    -> Result<(), Box<dyn std::error::Error>> {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let response = test_router(Ok(details()), Arc::clone(&requests), false)
            .oneshot(
                Request::get("/api/v1/admin/partnerships/770e8400-e29b-41d4-a716-446655440000")
                    .header("Authorization", "Bearer valid")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        assert_eq!(
            serde_json::json!({
                "partnershipId": "770e8400-e29b-41d4-a716-446655440000",
                "party": {
                    "partyId": "550e8400-e29b-41d4-a716-446655440000",
                    "partySlugId": "safe-party",
                    "name": "Safe Party"
                },
                "memberUserIds": ["660e8400-e29b-41d4-a716-446655440000"],
                "listingSourceIds": ["990e8400-e29b-41d4-a716-446655440000"],
                "memberCount": 2,
                "listingSourceGrantCount": 3,
                "created": "2026-01-02T12:00:00Z",
                "updated": "2026-02-03T12:00:00Z"
            }),
            json(response).await?
        );
        assert_eq!(1, lock(&requests).len());
        Ok(())
    }

    #[tokio::test]
    async fn should_map_not_found_without_cache() -> Result<(), Box<dyn std::error::Error>> {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let response = test_router(
            Err(GetAdminPartnershipError::NotFound),
            Arc::clone(&requests),
            false,
        )
        .oneshot(
            Request::get("/api/v1/admin/partnerships/770e8400-e29b-41d4-a716-446655440000")
                .header("Authorization", "Bearer valid")
                .body(Body::empty())?,
        )
        .await?;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        assert_eq!("PARTNERSHIP_NOT_FOUND", json(response).await?["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_uuid_without_calling_service()
    -> Result<(), Box<dyn std::error::Error>> {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let response = test_router(Ok(details()), Arc::clone(&requests), false)
            .oneshot(
                Request::get("/api/v1/admin/partnerships/not-a-uuid")
                    .header("Authorization", "Bearer valid")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        let body = json(response).await?;
        assert_eq!("INVALID_UUID", body["error"]);
        assert_eq!(
            serde_json::json!({"field": "partnershipId", "type": "PATH"}),
            body["source"]
        );
        assert!(lock(&requests).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_missing_and_invalid_auth_without_calling_service()
    -> Result<(), Box<dyn std::error::Error>> {
        for (authorization, reject_auth) in [(None, false), (Some("Bearer invalid"), true)] {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let mut builder =
                Request::get("/api/v1/admin/partnerships/770e8400-e29b-41d4-a716-446655440000");
            if let Some(authorization) = authorization {
                builder = builder.header("Authorization", authorization);
            }
            let response = test_router(Ok(details()), Arc::clone(&requests), reject_auth)
                .oneshot(builder.body(Body::empty())?)
                .await?;

            assert_eq!(StatusCode::UNAUTHORIZED, response.status());
            assert_eq!(
                Some("no-store"),
                response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok())
            );
            assert_eq!("INVALID_CREDENTIALS", json(response).await?["error"]);
            assert!(lock(&requests).is_empty());
        }
        Ok(())
    }
}
