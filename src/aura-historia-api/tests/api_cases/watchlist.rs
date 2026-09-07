use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    assert_problem, json_response, seed_access_token_for, seed_active_watchlist_entries,
    seed_inactive_watchlist_entry, seed_product, seed_user, seed_user_with_tier,
};

async fn withdraw_product(product_listing_id: ProductListingId) {
    let pool = get_postgres_client().await;
    sqlx::query(
        "UPDATE product_listings SET lifecycle = 'WITHDRAWN', availability = NULL WHERE product_listing_id = $1",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to withdraw product fixture: {error}"));
}
use product_listing_core::product_listing_id::ProductListingId;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use user_core::{access_token::Scope, tier::UserTier};

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_add_product_to_watchlist_when_authenticated() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_product().await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "productListingId": product_listing_id.to_string(),
            "notifications": true
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    let location = response.headers().get(reqwest::header::LOCATION).cloned();
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert!(location.is_none());
    assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
    assert_eq!(
        serde_json::json!(product_listing_id.to_string()),
        body["productListingId"]
    );
    assert_eq!(serde_json::json!(true), body["notifications"]);
    assert_eq!(serde_json::json!("ACTIVE"), body["state"]);
    assert!(body.get("item").is_none());
    assert!(body.get("userState").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_watchlist_create_when_entry_exists() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_product().await;
    let client = reqwest::Client::new();

    let created = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let duplicate = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to duplicate watchlist API: {error}"));
    let (status, body) = json_response(duplicate).await;

    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_product_listing_not_found_when_watching_missing_product() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"productListingId": ProductListingId::new().to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to watch missing product API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PRODUCT_LISTING_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_product_listing_unavailable_when_watching_withdrawn_product() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_product().await;
    withdraw_product(product_listing_id).await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to watch withdrawn product API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::CONFLICT,
        "PRODUCT_LISTING_UNAVAILABLE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_leave_existing_watchlist_entry_unchanged_when_watching_duplicate() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_product().await;
    let client = reqwest::Client::new();

    let created = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string(), "notifications": false}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let duplicate = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string(), "notifications": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to duplicate watchlist API: {error}"));
    let (status, body) = json_response(duplicate).await;
    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");

    let pool = get_postgres_client().await;
    let (notifications, state): (bool, String) = sqlx::query_as(
        "SELECT notifications, state FROM product_listing_watchlist WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to read duplicate watchlist entry: {error}"));
    assert!(!notifications);
    assert_eq!("ACTIVE", state);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_list_current_user_watchlist() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistRead, Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_product().await;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let response = client
        .get(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list watchlist API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(21), body["size"]);
    assert_eq!(
        serde_json::json!(product_listing_id.to_string()),
        body["items"][0]["item"]["productListingId"]
    );
    assert_eq!(
        "CURRENT",
        body["items"][0]["item"]["pricing"]["valuation"]["type"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_watchlist_notifications() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_product().await;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let response = client
        .patch(format!(
            "{}/api/v1/me/watchlist/{}",
            AURA_API.base_url(),
            product_listing_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"notifications": false}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch watchlist API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(false), body["notifications"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reactivate_inactive_watchlist_entry_for_active_product() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_inactive_watchlist_entry(user_id).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/me/watchlist/{product_listing_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"state": "ACTIVE"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reactivate active product API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!("ACTIVE"), body["state"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_product_listing_unavailable_when_reactivating_withdrawn_product() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_inactive_watchlist_entry(user_id).await;
    withdraw_product(product_listing_id).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/me/watchlist/{product_listing_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"state": "ACTIVE"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reactivate withdrawn product API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::CONFLICT,
        "PRODUCT_LISTING_UNAVAILABLE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_allow_notification_only_and_deactivation_updates_for_withdrawn_product() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_product().await;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());
    withdraw_product(product_listing_id).await;

    let notifications = client
        .patch(format!(
            "{}/api/v1/me/watchlist/{product_listing_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"notifications": false}))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to update notifications for withdrawn product API: {error}")
        });
    let (status, body) = json_response(notifications).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(false), body["notifications"]);

    let deactivated = client
        .patch(format!(
            "{}/api/v1/me/watchlist/{product_listing_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"state": "INACTIVE_BY_USER"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to deactivate withdrawn product API: {error}"));
    let (status, body) = json_response(deactivated).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!("INACTIVE_BY_USER"), body["state"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_watchlist_entry() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_product().await;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let response = client
        .delete(format!(
            "{}/api/v1/me/watchlist/{}",
            AURA_API.base_url(),
            product_listing_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete watchlist API: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_watchlist_update_when_product_listing_id_is_invalid() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/me/watchlist/not-a-uuid",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"notifications": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch invalid watchlist API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_when_updating_missing_watchlist_entry() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/me/watchlist/{}",
            AURA_API.base_url(),
            ProductListingId::new()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"notifications": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch missing watchlist API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "WATCHLIST_ENTRY_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_watchlist_create_at_free_tier_quota() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    seed_active_watchlist_entries(user_id, 20).await;
    let product_listing_id = seed_product().await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create over quota: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        "WATCHLIST_QUOTA_EXCEEDED",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_serialize_concurrent_watchlist_creates_at_free_quota() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    seed_active_watchlist_entries(user_id, 19).await;
    let first_product_listing_id = seed_product().await;
    let second_product_listing_id = seed_product().await;
    let client = reqwest::Client::new();

    let first_request = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({ "productListingId": first_product_listing_id }))
        .send();
    let second_request = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({ "productListingId": second_product_listing_id }))
        .send();

    let (first, second) = tokio::join!(first_request, second_request);
    let mut statuses = [
        first
            .unwrap_or_else(|error| panic!("first concurrent create failed: {error}"))
            .status(),
        second
            .unwrap_or_else(|error| panic!("second concurrent create failed: {error}"))
            .status(),
    ];
    statuses.sort();

    assert_eq!(
        [
            reqwest::StatusCode::CREATED,
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        ],
        statuses
    );
    let pool = get_postgres_client().await;
    let active_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_watchlist WHERE user_id = $1 AND state = 'ACTIVE'",
    )
    .bind(uuid::Uuid::from(user_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to count active watchlist entries: {error}"));
    assert_eq!(20, active_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_allow_ultimate_tier_to_create_beyond_free_quota() {
    let user_id = seed_user_with_tier("USER", UserTier::Ultimate).await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    seed_active_watchlist_entries(user_id, 20).await;
    let product_listing_id = seed_product().await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create unlimited watchlist entry: {error}"));

    assert_eq!(reqwest::StatusCode::CREATED, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_free_tier_watchlist_reactivation_at_quota() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    seed_active_watchlist_entries(user_id, 20).await;
    let product_listing_id = seed_inactive_watchlist_entry(user_id).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/me/watchlist/{}",
            AURA_API.base_url(),
            product_listing_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"state": "ACTIVE"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reactivate over quota: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        "WATCHLIST_QUOTA_EXCEEDED",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_serialize_concurrent_watchlist_reactivations_at_free_quota() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    seed_active_watchlist_entries(user_id, 19).await;
    let first_product_listing_id = seed_inactive_watchlist_entry(user_id).await;
    let second_product_listing_id = seed_inactive_watchlist_entry(user_id).await;
    let client = reqwest::Client::new();

    let first_request = client
        .patch(format!(
            "{}/api/v1/me/watchlist/{first_product_listing_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({ "state": "ACTIVE" }))
        .send();
    let second_request = client
        .patch(format!(
            "{}/api/v1/me/watchlist/{second_product_listing_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({ "state": "ACTIVE" }))
        .send();

    let (first, second) = tokio::join!(first_request, second_request);
    let mut statuses = [
        first
            .unwrap_or_else(|error| panic!("first concurrent reactivation failed: {error}"))
            .status(),
        second
            .unwrap_or_else(|error| panic!("second concurrent reactivation failed: {error}"))
            .status(),
    ];
    statuses.sort();

    assert_eq!(
        [
            reqwest::StatusCode::OK,
            reqwest::StatusCode::UNPROCESSABLE_ENTITY
        ],
        statuses
    );
    let pool = get_postgres_client().await;
    let active_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_watchlist WHERE user_id = $1 AND state = 'ACTIVE'",
    )
    .bind(uuid::Uuid::from(user_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to count active watchlist entries: {error}"));
    assert_eq!(20, active_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_allow_ultimate_tier_watchlist_reactivation_beyond_free_quota() {
    let user_id = seed_user_with_tier("USER", UserTier::Ultimate).await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    seed_active_watchlist_entries(user_id, 20).await;
    let product_listing_id = seed_inactive_watchlist_entry(user_id).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/me/watchlist/{}",
            AURA_API.base_url(),
            product_listing_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"state": "ACTIVE"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reactivate unlimited watchlist entry: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!("ACTIVE"), body["state"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_auth_for_watchlist() {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list watchlist missing auth API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}
