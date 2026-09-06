use crate::{auth::protected_context, error::ApiError, state::AdminOverviewState};
use admin_overview_service::use_cases::get_admin_overview::{
    AdminOverview, AdminOverviewActiveListingAvailabilityCounts,
    AdminOverviewListingSourceMethodAssignmentCounts, AdminOverviewListingSources,
    AdminOverviewPartnershipApplicationStateCounts, AdminOverviewPartnershipApplications,
    AdminOverviewProductListingLifecycleCounts, AdminOverviewProductListings,
    AdminOverviewUserRoleCounts, AdminOverviewUserTierCounts, AdminOverviewUsers,
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

const ADMIN_OVERVIEW_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOverviewData {
    schema_version: u8,
    users: AdminOverviewUsersData,
    partnership_applications: AdminOverviewPartnershipApplicationsData,
    parties: AdminOverviewCountData,
    listing_sources: AdminOverviewListingSourcesData,
    partnerships: AdminOverviewCountData,
    product_listings: AdminOverviewProductListingsData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOverviewCountData {
    total: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOverviewUsersData {
    total: u64,
    by_tier: AdminOverviewUserTierCountsData,
    by_role: AdminOverviewUserRoleCountsData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOverviewUserTierCountsData {
    free: u64,
    pro: u64,
    ultimate: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOverviewUserRoleCountsData {
    user: u64,
    admin: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOverviewPartnershipApplicationsData {
    total: u64,
    by_state: AdminOverviewPartnershipApplicationStateCountsData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOverviewPartnershipApplicationStateCountsData {
    submitted: u64,
    in_review: u64,
    approved: u64,
    rejected: u64,
    withdrawn: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOverviewListingSourcesData {
    total: u64,
    without_ingestion_method: u64,
    method_assignments: AdminOverviewListingSourceMethodAssignmentCountsData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOverviewListingSourceMethodAssignmentCountsData {
    web_crawl: u64,
    shopify: u64,
    woocommerce: u64,
    partner_api: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOverviewProductListingsData {
    total: u64,
    by_lifecycle: AdminOverviewProductListingLifecycleCountsData,
    active_availability: AdminOverviewActiveListingAvailabilityCountsData,
    active_without_availability: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOverviewProductListingLifecycleCountsData {
    active: u64,
    withdrawn: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminOverviewActiveListingAvailabilityCountsData {
    available: u64,
    in_stock: u64,
    limited_availability: u64,
    back_order: u64,
    made_to_order: u64,
    pre_order: u64,
    pre_sale: u64,
    unavailable: u64,
    reserved: u64,
    out_of_stock: u64,
    sold_out: u64,
}

impl From<AdminOverview> for AdminOverviewData {
    fn from(value: AdminOverview) -> Self {
        Self {
            schema_version: ADMIN_OVERVIEW_SCHEMA_VERSION,
            users: value.users.into(),
            partnership_applications: value.partnership_applications.into(),
            parties: AdminOverviewCountData {
                total: value.parties_total,
            },
            listing_sources: value.listing_sources.into(),
            partnerships: AdminOverviewCountData {
                total: value.partnerships_total,
            },
            product_listings: value.product_listings.into(),
        }
    }
}

impl From<AdminOverviewUsers> for AdminOverviewUsersData {
    fn from(value: AdminOverviewUsers) -> Self {
        Self {
            total: value.total,
            by_tier: value.by_tier.into(),
            by_role: value.by_role.into(),
        }
    }
}

impl From<AdminOverviewUserTierCounts> for AdminOverviewUserTierCountsData {
    fn from(value: AdminOverviewUserTierCounts) -> Self {
        Self {
            free: value.free,
            pro: value.pro,
            ultimate: value.ultimate,
        }
    }
}

impl From<AdminOverviewUserRoleCounts> for AdminOverviewUserRoleCountsData {
    fn from(value: AdminOverviewUserRoleCounts) -> Self {
        Self {
            user: value.user,
            admin: value.admin,
        }
    }
}

impl From<AdminOverviewPartnershipApplications> for AdminOverviewPartnershipApplicationsData {
    fn from(value: AdminOverviewPartnershipApplications) -> Self {
        Self {
            total: value.total,
            by_state: value.by_state.into(),
        }
    }
}

impl From<AdminOverviewPartnershipApplicationStateCounts>
    for AdminOverviewPartnershipApplicationStateCountsData
{
    fn from(value: AdminOverviewPartnershipApplicationStateCounts) -> Self {
        Self {
            submitted: value.submitted,
            in_review: value.in_review,
            approved: value.approved,
            rejected: value.rejected,
            withdrawn: value.withdrawn,
        }
    }
}

impl From<AdminOverviewListingSources> for AdminOverviewListingSourcesData {
    fn from(value: AdminOverviewListingSources) -> Self {
        Self {
            total: value.total,
            without_ingestion_method: value.without_ingestion_method,
            method_assignments: value.method_assignments.into(),
        }
    }
}

impl From<AdminOverviewListingSourceMethodAssignmentCounts>
    for AdminOverviewListingSourceMethodAssignmentCountsData
{
    fn from(value: AdminOverviewListingSourceMethodAssignmentCounts) -> Self {
        Self {
            web_crawl: value.web_crawl,
            shopify: value.shopify,
            woocommerce: value.woocommerce,
            partner_api: value.partner_api,
        }
    }
}

impl From<AdminOverviewProductListings> for AdminOverviewProductListingsData {
    fn from(value: AdminOverviewProductListings) -> Self {
        Self {
            total: value.total,
            by_lifecycle: value.by_lifecycle.into(),
            active_availability: value.active_availability.into(),
            active_without_availability: value.active_without_availability,
        }
    }
}

impl From<AdminOverviewProductListingLifecycleCounts>
    for AdminOverviewProductListingLifecycleCountsData
{
    fn from(value: AdminOverviewProductListingLifecycleCounts) -> Self {
        Self {
            active: value.active,
            withdrawn: value.withdrawn,
        }
    }
}

impl From<AdminOverviewActiveListingAvailabilityCounts>
    for AdminOverviewActiveListingAvailabilityCountsData
{
    fn from(value: AdminOverviewActiveListingAvailabilityCounts) -> Self {
        Self {
            available: value.available,
            in_stock: value.in_stock,
            limited_availability: value.limited_availability,
            back_order: value.back_order,
            made_to_order: value.made_to_order,
            pre_order: value.pre_order,
            pre_sale: value.pre_sale,
            unavailable: value.unavailable,
            reserved: value.reserved,
            out_of_stock: value.out_of_stock,
            sold_out: value.sold_out,
        }
    }
}

pub(crate) async fn get_admin_overview(
    State(state): State<AdminOverviewState>,
    headers: HeaderMap,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };

    match state.get_overview.execute(&context).await {
        Ok(overview) => no_store(Json(AdminOverviewData::from(overview)).into_response()),
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
    use admin_overview_service::GetAdminOverviewError;
    use application::{
        error::static_error,
        operation_context::{OperationContext, Principal},
    };
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use std::{
        collections::BTreeSet,
        sync::{Arc, Mutex, MutexGuard},
    };
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    type Requests = Arc<Mutex<Vec<OperationContext>>>;
    type Outcome = Arc<Mutex<Option<Result<AdminOverview, GetAdminOverviewError>>>>;

    #[derive(Clone)]
    struct FakeGetAdminOverviewUseCase {
        requests: Requests,
        outcome: Outcome,
    }

    #[async_trait::async_trait]
    impl admin_overview_service::GetAdminOverviewUseCase for FakeGetAdminOverviewUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
        ) -> Result<AdminOverview, GetAdminOverviewError> {
            lock(&self.requests).push(context.clone());
            lock(&self.outcome).take().unwrap_or_else(|| {
                Err(GetAdminOverviewError::ReaderInternal {
                    source: static_error("test outcome was not configured"),
                })
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

    fn test_router(
        outcome: Result<AdminOverview, GetAdminOverviewError>,
        requests: Requests,
        reject_auth: bool,
    ) -> Router {
        let state = AdminOverviewState::new(
            Arc::new(FakeGetAdminOverviewUseCase {
                requests,
                outcome: Arc::new(Mutex::new(Some(outcome))),
            }),
            Arc::new(FakeAuthenticator {
                user_id: UserId::new(),
                reject: reject_auth,
            }),
        );
        Router::new()
            .route(
                "/api/v1/admin/overview",
                axum::routing::get(get_admin_overview),
            )
            .with_state(state)
    }

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        match value.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    async fn json(response: Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("failed to read response: {error}"));
        serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("failed to decode response: {error}"))
    }

    #[tokio::test]
    async fn should_return_versioned_admin_overview_without_store() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let overview = AdminOverview {
            users: AdminOverviewUsers {
                total: 3,
                by_tier: AdminOverviewUserTierCounts {
                    free: 1,
                    pro: 1,
                    ultimate: 1,
                },
                by_role: AdminOverviewUserRoleCounts { user: 2, admin: 1 },
            },
            parties_total: 2,
            ..Default::default()
        };
        let response = test_router(Ok(overview), Arc::clone(&requests), false)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/admin/overview")
                    .header("authorization", "Bearer valid")
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("failed to build request: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("request failed: {error}"));

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
                "schemaVersion": 1,
                "users": {
                    "total": 3,
                    "byTier": { "free": 1, "pro": 1, "ultimate": 1 },
                    "byRole": { "user": 2, "admin": 1 }
                },
                "partnershipApplications": {
                    "total": 0,
                    "byState": {
                        "submitted": 0, "inReview": 0, "approved": 0, "rejected": 0,
                        "withdrawn": 0
                    }
                },
                "parties": { "total": 2 },
                "listingSources": {
                    "total": 0,
                    "withoutIngestionMethod": 0,
                    "methodAssignments": {
                        "webCrawl": 0, "shopify": 0, "woocommerce": 0, "partnerApi": 0
                    }
                },
                "partnerships": { "total": 0 },
                "productListings": {
                    "total": 0,
                    "byLifecycle": { "active": 0, "withdrawn": 0 },
                    "activeAvailability": {
                        "available": 0, "inStock": 0, "limitedAvailability": 0,
                        "backOrder": 0, "madeToOrder": 0, "preOrder": 0, "preSale": 0,
                        "unavailable": 0, "reserved": 0, "outOfStock": 0, "soldOut": 0
                    },
                    "activeWithoutAvailability": 0
                }
            }),
            json(response).await
        );
        let requests = lock(&requests);
        assert_eq!(1, requests.len());
        assert!(matches!(requests[0].principal, Principal::User(_)));
    }

    #[tokio::test]
    async fn should_map_forbidden_overview_read_without_store() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let response = test_router(Err(GetAdminOverviewError::Forbidden), requests, false)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/admin/overview")
                    .header("authorization", "Bearer valid")
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("failed to build request: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("request failed: {error}"));

        assert_eq!(StatusCode::FORBIDDEN, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        assert_eq!(
            serde_json::json!({
                "status": 403,
                "title": "Forbidden",
                "error": "FORBIDDEN",
                "detail": "Operation is not permitted."
            }),
            json(response).await
        );
    }

    #[tokio::test]
    async fn should_reject_invalid_credentials_without_calling_overview() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let response = test_router(Ok(AdminOverview::default()), Arc::clone(&requests), true)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/admin/overview")
                    .header("authorization", "Bearer invalid")
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("failed to build request: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("request failed: {error}"));

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert_eq!(0, lock(&requests).len());
    }
}
