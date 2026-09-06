use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};
use api_support::{
    assert_problem, json_response, seed_access_token_for, seed_listing_source,
    seed_partnership_application, seed_user, seed_user_with_tier,
};
use serde_json::json;
use test_api::{IntegrationTestService, aura_integration_test};
use time::OffsetDateTime;
use user_core::{access_token::Scope, tier::UserTier};

async fn get_overview(token: &str) -> (reqwest::StatusCode, serde_json::Value, Option<String>) {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/overview", AURA_API.base_url()))
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call admin overview API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    (status, body, cache_control)
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_empty_admin_overview_with_no_store() {
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let (status, body, cache_control) = get_overview(&token).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(
        json!({
            "schemaVersion": 1,
            "users": {
                "total": 1,
                "byTier": { "free": 1, "pro": 0, "ultimate": 0 },
                "byRole": { "user": 0, "admin": 1 }
            },
            "partnershipApplications": {
                "total": 0,
                "byState": {
                    "submitted": 0, "inReview": 0, "approved": 0, "rejected": 0,
                    "withdrawn": 0
                }
            },
            "parties": { "total": 0 },
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
        body
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_aggregate_representative_admin_overview_counts() {
    let admin_id = seed_user("ADMIN").await;
    let pro_user = seed_user_with_tier("USER", UserTier::Pro).await;
    let ultimate_user = seed_user_with_tier("USER", UserTier::Ultimate).await;
    let listing_source_id = seed_listing_source().await;
    let timestamp = OffsetDateTime::now_utc();
    seed_partnership_application(
        pro_user,
        "SUBMITTED",
        json!({
            "type": "EXISTING_LISTING_SOURCE",
            "listing_source_id": listing_source_id,
        }),
        timestamp,
        timestamp,
    )
    .await;
    seed_partnership_application(
        ultimate_user,
        "REJECTED",
        json!({
            "type": "EXISTING_LISTING_SOURCE",
            "listing_source_id": listing_source_id,
        }),
        timestamp,
        timestamp,
    )
    .await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let (status, body, cache_control) = get_overview(&token).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(json!(3), body["users"]["total"]);
    assert_eq!(json!(1), body["users"]["byTier"]["free"]);
    assert_eq!(json!(1), body["users"]["byTier"]["pro"]);
    assert_eq!(json!(1), body["users"]["byTier"]["ultimate"]);
    assert_eq!(json!(2), body["users"]["byRole"]["user"]);
    assert_eq!(json!(1), body["users"]["byRole"]["admin"]);
    assert_eq!(json!(2), body["partnershipApplications"]["total"]);
    assert_eq!(
        json!(1),
        body["partnershipApplications"]["byState"]["submitted"]
    );
    assert_eq!(
        json!(1),
        body["partnershipApplications"]["byState"]["rejected"]
    );
    assert_eq!(json!(1), body["parties"]["total"]);
    assert_eq!(json!(1), body["listingSources"]["total"]);
    assert_eq!(
        json!(1),
        body["listingSources"]["methodAssignments"]["partnerApi"]
    );
    assert_eq!(json!(1), body["partnerships"]["total"]);
    assert_eq!(json!(0), body["productListings"]["total"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_non_admin_overview_request() {
    let user_id = seed_user("USER").await;
    let token = String::from(
        seed_access_token_for(user_id, std::collections::HashSet::<Scope>::new()).await,
    );

    let (status, body, cache_control) = get_overview(&token).await;

    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}
