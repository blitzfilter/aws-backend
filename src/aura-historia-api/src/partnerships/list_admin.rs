use crate::{
    auth::protected_context,
    error::{ApiError, BAD_QUERY_PARAMETER_VALUE, PARTNERSHIP_INTERNAL_ERROR},
    pagination_data::JsonCursoredData,
    state::PartnershipsState,
};
use application::pagination::{Cursor, CursoredResult};
use axum::{
    Json,
    extract::{RawQuery, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use listing_source_core::ListingSourceId;
use partnership_core::partnership_id::PartnershipId;
use partnership_service::use_cases::queries::list_admin_partnerships::{
    AdminPartnershipSummary, ListAdminPartnershipsRequest, PartnershipPartySummary,
    PartnershipSearchCursor,
};
use party_core::party_id::PartyId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use user_core::user_id::UserId;
use uuid::Uuid;

const DEFAULT_PAGE_SIZE: u64 = 21;
const MAX_PAGE_SIZE: u64 = 100;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAdminPartnershipsQuery {
    #[serde(default)]
    party_id: Option<String>,
    #[serde(default)]
    member_user_id: Option<String>,
    #[serde(default)]
    listing_source_id: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    search_after: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PartnershipSummaryData {
    partnership_id: Uuid,
    party: PartnershipPartySummaryData,
    member_count: u64,
    listing_source_grant_count: u64,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PartnershipPartySummaryData {
    party_id: Uuid,
    party_slug_id: String,
    name: String,
}

impl From<AdminPartnershipSummary> for PartnershipSummaryData {
    fn from(value: AdminPartnershipSummary) -> Self {
        Self {
            partnership_id: value.partnership_id.into(),
            party: PartnershipPartySummaryData::from(value.party),
            member_count: value.member_count,
            listing_source_grant_count: value.listing_source_grant_count,
            created: value.created,
            updated: value.updated,
        }
    }
}

impl From<PartnershipPartySummary> for PartnershipPartySummaryData {
    fn from(value: PartnershipPartySummary) -> Self {
        Self {
            party_id: value.party_id.into(),
            party_slug_id: value.party_slug_id.to_string(),
            name: value.name.to_string(),
        }
    }
}

pub(super) async fn list_admin(
    State(state): State<PartnershipsState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let request = match parse_list_admin_query(raw_query.as_deref()) {
        Ok(request) => request,
        Err(error) => return no_store(error.into_response()),
    };

    match state.list_admin.execute(&context, request).await {
        Ok(result) => match response_from_result(result) {
            Ok(response) => no_store(response),
            Err(error) => no_store(error.into_response()),
        },
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_list_admin_query(
    raw_query: Option<&str>,
) -> Result<ListAdminPartnershipsRequest, ApiError> {
    let query: ListAdminPartnershipsQuery = serde_qs::Config::new()
        .use_form_encoding(true)
        .deserialize_str(raw_query.unwrap_or_default())
        .map_err(|error| bad_query("query", error))?;

    Ok(ListAdminPartnershipsRequest {
        party_id: query.party_id.as_deref().map(parse_party_id).transpose()?,
        member_user_id: query
            .member_user_id
            .as_deref()
            .map(parse_member_user_id)
            .transpose()?,
        listing_source_id: query
            .listing_source_id
            .as_deref()
            .map(parse_listing_source_id)
            .transpose()?,
        cursor: parse_cursor(query.size.as_deref(), query.search_after.as_deref())?,
    })
}

fn parse_party_id(value: &str) -> Result<PartyId, ApiError> {
    Uuid::parse_str(value)
        .map(PartyId::from)
        .map_err(|error| bad_query("partyId", error))
}

fn parse_member_user_id(value: &str) -> Result<UserId, ApiError> {
    Uuid::parse_str(value)
        .map(UserId::from)
        .map_err(|error| bad_query("memberUserId", error))
}

fn parse_listing_source_id(value: &str) -> Result<ListingSourceId, ApiError> {
    Uuid::parse_str(value)
        .map(ListingSourceId::from)
        .map_err(|error| bad_query("listingSourceId", error))
}

fn parse_cursor(
    size: Option<&str>,
    search_after: Option<&str>,
) -> Result<Option<Cursor<PartnershipSearchCursor>>, ApiError> {
    let size = size
        .map(|value| {
            value
                .parse::<u64>()
                .map(|size| size.clamp(1, MAX_PAGE_SIZE))
        })
        .transpose()
        .map_err(|error| bad_query("size", error))?;
    let search_after = search_after.map(parse_search_after).transpose()?;

    if size.is_some() || search_after.is_some() {
        Ok(Some(Cursor {
            size: size.unwrap_or(DEFAULT_PAGE_SIZE),
            search_after,
        }))
    } else {
        Ok(None)
    }
}

fn parse_search_after(value: &str) -> Result<PartnershipSearchCursor, ApiError> {
    let value: Value =
        serde_json::from_str(value).map_err(|error| bad_query("searchAfter", error))?;
    let Value::Array(values) = value else {
        return Err(bad_query(
            "searchAfter",
            "searchAfter must be a JSON array containing timestamp and partnership ID.",
        ));
    };
    let [Value::String(position), Value::String(partnership_id)] = values.as_slice() else {
        return Err(bad_query(
            "searchAfter",
            "searchAfter must contain an RFC3339 timestamp and partnership UUID.",
        ));
    };
    let position = OffsetDateTime::parse(position, &Rfc3339)
        .map_err(|error| bad_query("searchAfter", error))?;
    let partnership_id = Uuid::parse_str(partnership_id)
        .map(PartnershipId::from)
        .map_err(|error| bad_query("searchAfter", error))?;

    Ok(PartnershipSearchCursor {
        position,
        partnership_id,
    })
}

fn response_from_result(
    result: CursoredResult<AdminPartnershipSummary, PartnershipSearchCursor>,
) -> Result<Response, ApiError> {
    let CursoredResult {
        items,
        cursor,
        total,
    } = result;
    let search_after = cursor
        .search_after
        .map(partnership_cursor_value)
        .transpose()?;

    Ok(Json(JsonCursoredData {
        items: items
            .into_iter()
            .map(PartnershipSummaryData::from)
            .collect(),
        size: cursor.size,
        search_after,
        total,
    })
    .into_response())
}

fn partnership_cursor_value(cursor: PartnershipSearchCursor) -> Result<Value, ApiError> {
    cursor
        .position
        .format(&Rfc3339)
        .map(|position| json!([position, Uuid::from(cursor.partnership_id)]))
        .map_err(|_| {
            ApiError::internal_server_error(PARTNERSHIP_INTERNAL_ERROR)
                .with_detail("Partnership cursor failed internally.")
        })
}

fn bad_query(field: &'static str, detail: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail.to_string())
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
        operation_context::{OperationContext, Principal},
    };
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
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
                ListAdminPartnershipsError, ListAdminPartnershipsResult,
                ListAdminPartnershipsUseCase,
            },
        },
    };
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::macros::datetime;
    use tower::ServiceExt;

    type Requests = Arc<Mutex<Vec<(OperationContext, ListAdminPartnershipsRequest)>>>;
    type Outcome =
        Arc<Mutex<Option<Result<ListAdminPartnershipsResult, ListAdminPartnershipsError>>>>;

    #[derive(Clone)]
    struct FakeListAdminPartnershipsUseCase {
        outcome: Outcome,
        requests: Requests,
    }

    #[async_trait::async_trait]
    impl ListAdminPartnershipsUseCase for FakeListAdminPartnershipsUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            request: ListAdminPartnershipsRequest,
        ) -> Result<ListAdminPartnershipsResult, ListAdminPartnershipsError> {
            lock(&self.requests).push((context.clone(), request));
            lock(&self.outcome).take().unwrap_or_else(|| {
                Err(ListAdminPartnershipsError::Internal {
                    source: static_error("test outcome was not configured"),
                })
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

    fn summary() -> AdminPartnershipSummary {
        AdminPartnershipSummary {
            partnership_id: PartnershipId::from(Uuid::from_u128(
                0x770e8400e29b41d4a716446655440000,
            )),
            party: PartnershipPartySummary {
                party_id: PartyId::from(Uuid::from_u128(0x550e8400e29b41d4a716446655440000)),
                party_slug_id: party_core::party_slug_id::PartySlugId::raw("safe-party")
                    .unwrap_or_else(|error| panic!("valid party slug: {error}")),
                name: party_core::party_name::PartyName::try_from("Safe Party")
                    .unwrap_or_else(|error| panic!("valid party name: {error}")),
            },
            member_count: 2,
            listing_source_grant_count: 3,
            created: datetime!(2026-01-02 12:00 UTC),
            updated: datetime!(2026-02-03 12:00 UTC),
        }
    }

    fn result(search_after: Option<PartnershipSearchCursor>) -> ListAdminPartnershipsResult {
        CursoredResult {
            items: vec![summary()],
            cursor: Cursor {
                size: DEFAULT_PAGE_SIZE,
                search_after,
            },
            total: None,
        }
    }

    fn test_router(
        outcome: Result<ListAdminPartnershipsResult, ListAdminPartnershipsError>,
        requests: Requests,
        reject_auth: bool,
    ) -> Router {
        let use_case = FakeListAdminPartnershipsUseCase {
            outcome: Arc::new(Mutex::new(Some(outcome))),
            requests,
        };
        let state = PartnershipsState::new(
            Arc::new(use_case),
            Arc::new(UnusedGetAdminPartnershipUseCase),
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
            .route("/api/v1/admin/partnerships", axum::routing::get(list_admin))
            .with_state(state)
    }

    async fn json(response: Response) -> Result<Value, axum::Error> {
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&body).unwrap_or_default())
    }

    #[test]
    fn should_map_admin_partnership_query_to_service_request() -> Result<(), ApiError> {
        let party_id = Uuid::from_u128(0x550e8400e29b41d4a716446655440000);
        let member_user_id = Uuid::from_u128(0x660e8400e29b41d4a716446655440000);
        let listing_source_id = Uuid::from_u128(0x770e8400e29b41d4a716446655440000);
        let partnership_id = Uuid::from_u128(0x880e8400e29b41d4a716446655440000);
        let request = parse_list_admin_query(Some(&format!(
            "partyId={party_id}&memberUserId={member_user_id}&listingSourceId={listing_source_id}&size=200&searchAfter=%5B%222026-09-04T12%3A00%3A00Z%22%2C%22{partnership_id}%22%5D"
        )))?;

        assert_eq!(Some(PartyId::from(party_id)), request.party_id);
        assert_eq!(Some(UserId::from(member_user_id)), request.member_user_id);
        assert_eq!(
            Some(ListingSourceId::from(listing_source_id)),
            request.listing_source_id
        );
        assert_eq!(
            Some(Cursor {
                size: MAX_PAGE_SIZE,
                search_after: Some(PartnershipSearchCursor {
                    position: datetime!(2026-09-04 12:00 UTC),
                    partnership_id: PartnershipId::from(partnership_id),
                }),
            }),
            request.cursor
        );
        Ok(())
    }

    #[test]
    fn should_default_and_clamp_admin_partnership_page_size() -> Result<(), ApiError> {
        assert_eq!(None, parse_list_admin_query(None)?.cursor);
        assert_eq!(
            Some(Cursor {
                size: 1,
                search_after: None,
            }),
            parse_list_admin_query(Some("size=0"))?.cursor
        );
        assert_eq!(
            Some(Cursor {
                size: MAX_PAGE_SIZE,
                search_after: None,
            }),
            parse_list_admin_query(Some("size=1000"))?.cursor
        );
        Ok(())
    }

    #[test]
    fn should_reject_invalid_admin_partnership_query_values_with_their_fields() {
        for (query, field) in [
            ("partyId=not-a-uuid", "partyId"),
            ("memberUserId=not-a-uuid", "memberUserId"),
            ("listingSourceId=not-a-uuid", "listingSourceId"),
            ("size=not-a-number", "size"),
            ("searchAfter=not-json", "searchAfter"),
            (
                "searchAfter=%5B%22not-a-timestamp%22%2C%22550e8400-e29b-41d4-a716-446655440000%22%5D",
                "searchAfter",
            ),
            (
                "searchAfter=%5B%222026-09-04T12%3A00%3A00Z%22%2C%22not-a-uuid%22%5D",
                "searchAfter",
            ),
        ] {
            let error = parse_list_admin_query(Some(query))
                .err()
                .unwrap_or_else(|| panic!("query should be rejected: {query}"));
            assert_eq!(BAD_QUERY_PARAMETER_VALUE, error.code(), "query: {query}");
            let value = serde_json::to_value(&error).unwrap_or_else(|serialization_error| {
                panic!("failed to serialize query error: {serialization_error}")
            });
            assert_eq!(
                json!({"field": field, "type": "QUERY"}),
                value["source"],
                "query: {query}"
            );
        }
    }

    #[test]
    fn should_map_only_safe_partnership_summary_fields() -> Result<(), serde_json::Error> {
        let value = serde_json::to_value(PartnershipSummaryData::from(summary()))?;

        assert_eq!(
            json!({
                "partnershipId": "770e8400-e29b-41d4-a716-446655440000",
                "party": {
                    "partyId": "550e8400-e29b-41d4-a716-446655440000",
                    "partySlugId": "safe-party",
                    "name": "Safe Party"
                },
                "memberCount": 2,
                "listingSourceGrantCount": 3,
                "created": "2026-01-02T12:00:00Z",
                "updated": "2026-02-03T12:00:00Z"
            }),
            value
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_return_safe_summary_and_forward_context_and_query_without_cache()
    -> Result<(), Box<dyn std::error::Error>> {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let cursor = PartnershipSearchCursor {
            position: datetime!(2026-01-02 12:00 UTC),
            partnership_id: PartnershipId::from(Uuid::from_u128(
                0x770e8400e29b41d4a716446655440000,
            )),
        };
        let party_id = Uuid::from_u128(0x550e8400e29b41d4a716446655440000);
        let member_user_id = Uuid::from_u128(0x660e8400e29b41d4a716446655440000);
        let listing_source_id = Uuid::from_u128(0x990e8400e29b41d4a716446655440000);
        let search_after_cursor = PartnershipSearchCursor {
            position: datetime!(2026-09-04 12:00 UTC),
            partnership_id: PartnershipId::from(Uuid::from_u128(
                0x770e8400e29b41d4a716446655440000,
            )),
        };
        let search_after =
            "%5B%222026-09-04T12%3A00%3A00Z%22%2C%22770e8400-e29b-41d4-a716-446655440000%22%5D";
        let request = Request::get(format!(
            "/api/v1/admin/partnerships?partyId={party_id}&memberUserId={member_user_id}&listingSourceId={listing_source_id}&size=200&searchAfter={search_after}"
        ))
        .header("Authorization", "Bearer valid")
        .body(Body::empty())?;

        let response = test_router(Ok(result(Some(cursor))), Arc::clone(&requests), false)
            .oneshot(request)
            .await?;
        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        let body = json(response).await?;
        assert_eq!(json!("Safe Party"), body["items"][0]["party"]["name"]);
        assert_eq!(json!(2), body["items"][0]["memberCount"]);
        assert_eq!(
            json!([
                "2026-01-02T12:00:00Z",
                "770e8400-e29b-41d4-a716-446655440000"
            ]),
            body["searchAfter"]
        );

        let requests = lock(&requests);
        assert_eq!(1, requests.len());
        assert!(matches!(
            requests[0].0.principal,
            Principal::User(actual_user_id) if actual_user_id == UserId::from(Uuid::from_u128(0x880e8400e29b41d4a716446655440000))
        ));
        assert_eq!(Some(PartyId::from(party_id)), requests[0].1.party_id);
        assert_eq!(
            Some(UserId::from(member_user_id)),
            requests[0].1.member_user_id
        );
        assert_eq!(
            Some(ListingSourceId::from(listing_source_id)),
            requests[0].1.listing_source_id
        );
        assert_eq!(
            Some(Cursor {
                size: 100,
                search_after: Some(search_after_cursor)
            }),
            requests[0].1.cursor
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_omit_terminal_cursor_and_return_default_size_without_cache()
    -> Result<(), Box<dyn std::error::Error>> {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let response = test_router(Ok(result(None)), Arc::clone(&requests), false)
            .oneshot(
                Request::get("/api/v1/admin/partnerships")
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
        let body = json(response).await?;
        assert_eq!(json!(DEFAULT_PAGE_SIZE), body["size"]);
        assert!(body.get("searchAfter").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn should_map_forbidden_service_error_without_cache()
    -> Result<(), Box<dyn std::error::Error>> {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let response = test_router(
            Err(ListAdminPartnershipsError::Forbidden),
            Arc::clone(&requests),
            false,
        )
        .oneshot(
            Request::get("/api/v1/admin/partnerships")
                .header("Authorization", "Bearer valid")
                .body(Body::empty())?,
        )
        .await?;

        assert_eq!(StatusCode::FORBIDDEN, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        assert_eq!(json!("FORBIDDEN"), json(response).await?["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_missing_auth_without_calling_service_without_cache()
    -> Result<(), Box<dyn std::error::Error>> {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let response = test_router(Ok(result(None)), Arc::clone(&requests), false)
            .oneshot(Request::get("/api/v1/admin/partnerships").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        assert_eq!(json!("INVALID_CREDENTIALS"), json(response).await?["error"]);
        assert!(lock(&requests).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_auth_without_calling_service_without_cache()
    -> Result<(), Box<dyn std::error::Error>> {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let response = test_router(Ok(result(None)), Arc::clone(&requests), true)
            .oneshot(
                Request::get("/api/v1/admin/partnerships")
                    .header("Authorization", "Bearer invalid")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        assert_eq!(json!("INVALID_CREDENTIALS"), json(response).await?["error"]);
        assert!(lock(&requests).is_empty());
        Ok(())
    }

    #[test]
    fn should_map_partnership_service_errors_to_their_http_contract() {
        let cases = [
            (
                ApiError::from(ListAdminPartnershipsError::Forbidden),
                StatusCode::FORBIDDEN,
                "FORBIDDEN",
            ),
            (
                ApiError::from(ListAdminPartnershipsError::TemporarilyUnavailable {
                    source: static_error("temporary"),
                }),
                StatusCode::SERVICE_UNAVAILABLE,
                "PARTNERSHIP_TEMPORARILY_UNAVAILABLE",
            ),
            (
                ApiError::from(ListAdminPartnershipsError::BeginTransactionFailed),
                StatusCode::SERVICE_UNAVAILABLE,
                "PARTNERSHIP_TEMPORARILY_UNAVAILABLE",
            ),
            (
                ApiError::from(ListAdminPartnershipsError::CommitTransactionFailed),
                StatusCode::SERVICE_UNAVAILABLE,
                "PARTNERSHIP_TEMPORARILY_UNAVAILABLE",
            ),
            (
                ApiError::from(ListAdminPartnershipsError::InvalidReadModel {
                    source: static_error("invalid"),
                }),
                StatusCode::INTERNAL_SERVER_ERROR,
                "PARTNERSHIP_INTERNAL_ERROR",
            ),
            (
                ApiError::from(ListAdminPartnershipsError::Internal {
                    source: static_error("internal"),
                }),
                StatusCode::INTERNAL_SERVER_ERROR,
                "PARTNERSHIP_INTERNAL_ERROR",
            ),
        ];

        for (error, status, code) in cases {
            let value = serde_json::to_value(&error).unwrap_or_else(|serialization_error| {
                panic!("failed to serialize service error: {serialization_error}")
            });
            assert_eq!(json!(status.as_u16()), value["status"]);
            assert_eq!(code, error.code().to_string());
        }
    }
}
