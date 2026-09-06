use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    assert_problem, fail_session_revocation_for, json_response, seed_access_token_for, seed_user,
    seed_user_with_tier, set_user_search_fields, set_user_stripe_customer_id,
};

use test_api::{IntegrationTestService, aura_integration_test};
use time::macros::datetime;
use user_core::access_token::Scope;
use user_core::tier::UserTier;
use user_core::user_id::UserId;

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_current_user_account_when_authenticated() {
    let user_id = seed_user("USER").await;
    let token =
        seed_access_token_for(user_id, std::collections::HashSet::from([Scope::UsersRead])).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/account", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call user API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
    assert_eq!(serde_json::json!("USER"), body["role"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_current_user_profile_when_body_is_valid() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!("{}/api/v1/me/account", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "firstName": "Ada",
            "lastName": "Lovelace",
            "language": "de",
            "currency": "EUR",
            "measurementUnit": "METRIC",
            "showUnassessedOrSensitiveContent": true
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch user API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!("Ada"), body["firstName"]);
    assert_eq!(
        serde_json::json!(true),
        body["showUnassessedOrSensitiveContent"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_current_user_when_authenticated() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!("{}/api/v1/me", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete user API: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_user_when_admin_reads_user() {
    let user_id = seed_user_with_tier("USER", UserTier::Pro).await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;
    let stripe_customer_id = format!("cus_admin_detail_{user_id}");
    set_user_stripe_customer_id(user_id, &stripe_customer_id).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            user_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get admin user API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
    assert_eq!(serde_json::json!("USER"), body["role"]);
    assert_eq!(serde_json::json!("PRO"), body["tier"]);
    assert_eq!(
        serde_json::json!(stripe_customer_id),
        body["stripeCustomerId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_when_admin_reads_missing_user() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;
    let missing_user_id = UserId::new();

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            missing_user_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get missing admin user API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
    );
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_authentication_for_admin_user_detail() {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            UserId::new()
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get admin user without auth: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_search_users_when_actor_is_admin() {
    seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/users", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to search users API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert!(
        body["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_search_users_with_filters_and_sorting_when_actor_is_admin() {
    let user_id = seed_user_with_tier("USER", UserTier::Pro).await;
    set_user_search_fields(
        user_id,
        "ada@example.test",
        "Ada",
        "Lovelace",
        datetime!(2026-01-01 00:00 UTC),
        datetime!(2026-02-01 00:00 UTC),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/users", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .query(&[
            ("query", "ada"),
            ("email", "example.test"),
            ("firstName", "Ada"),
            ("lastName", "Lovelace"),
            ("tier", "PRO"),
            ("role", "USER"),
            ("created[min]", "2026-01-01T00:00:00Z"),
            ("created[max]", "2026-01-31T23:59:59Z"),
            ("updated[min]", "2026-02-01T00:00:00Z"),
            ("updated[max]", "2026-02-28T23:59:59Z"),
            ("sort", "email"),
            ("order", "asc"),
            ("size", "1"),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to search filtered users API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(1), body["size"]);
    assert_eq!(
        serde_json::json!(user_id.to_string()),
        body["items"][0]["userId"]
    );
    assert_eq!(serde_json::json!("Ada"), body["items"][0]["firstName"]);
    assert!(body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_follow_admin_user_search_cursor() {
    let first_user_id = seed_user_with_tier("USER", UserTier::Free).await;
    let second_user_id = seed_user_with_tier("USER", UserTier::Free).await;
    let third_user_id = seed_user_with_tier("USER", UserTier::Free).await;
    set_user_search_fields(
        first_user_id,
        "a@example.test",
        "A",
        "User",
        datetime!(2026-01-01 00:00 UTC),
        datetime!(2026-01-01 00:00 UTC),
    )
    .await;
    set_user_search_fields(
        second_user_id,
        "b@example.test",
        "B",
        "User",
        datetime!(2026-01-02 00:00 UTC),
        datetime!(2026-01-02 00:00 UTC),
    )
    .await;
    set_user_search_fields(
        third_user_id,
        "c@example.test",
        "C",
        "User",
        datetime!(2026-01-03 00:00 UTC),
        datetime!(2026-01-03 00:00 UTC),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;
    let client = reqwest::Client::new();
    let first = client
        .get(format!("{}/api/v1/admin/users", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .query(&[
            ("role", "USER"),
            ("sort", "email"),
            ("order", "asc"),
            ("size", "2"),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get first admin user page: {error}"));
    let (first_status, first_body) = json_response(first).await;

    assert_eq!(reqwest::StatusCode::OK, first_status);
    assert_eq!(Some(2), first_body["items"].as_array().map(Vec::len));
    let cursor = first_body["searchAfter"]
        .as_str()
        .unwrap_or_else(|| panic!("missing admin user search cursor"))
        .to_owned();

    let second = client
        .get(format!("{}/api/v1/admin/users", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .query(&[
            ("role", "USER"),
            ("sort", "email"),
            ("order", "asc"),
            ("size", "2"),
            ("searchAfter", cursor.as_str()),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get second admin user page: {error}"));
    let (second_status, second_body) = json_response(second).await;

    assert_eq!(reqwest::StatusCode::OK, second_status);
    assert_eq!(Some(1), second_body["items"].as_array().map(Vec::len));
    assert_eq!(
        serde_json::json!(third_user_id.to_string()),
        second_body["items"][0]["userId"]
    );
    assert!(second_body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_user_search_when_actor_is_not_admin() {
    let user_id = seed_user("USER").await;
    let token =
        seed_access_token_for(user_id, std::collections::HashSet::from([Scope::UsersRead])).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/users", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to search users as non-admin: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_admin_user_search_query() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/users?sort=invalid&order=asc",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate admin user search query: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_SORT_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_remove_legacy_admin_user_search_route() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/users", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call removed admin user search route: {error}"));

    assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_remove_legacy_admin_user_detail_route() {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/users/{}",
            AURA_API.base_url(),
            UserId::new()
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call removed admin user detail route: {error}"));

    assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_user_tier_when_actor_is_admin() {
    let user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            user_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"tier": "PRO"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch admin user API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
    assert_eq!(serde_json::json!("PRO"), body["tier"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_user_role_when_actor_is_admin() {
    let user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            user_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"role": "ADMIN"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch admin user role API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!("ADMIN"), body["role"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_user_profile_when_actor_is_admin() {
    let user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            user_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"firstName": "Ada", "language": "de"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch admin user profile API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(serde_json::json!("Ada"), body["firstName"]);
    assert_eq!(serde_json::json!("de"), body["language"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_profile_patch_for_non_admin_actor() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            user_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"firstName": "Ada"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin profile patch: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_mixed_admin_user_patch_categories() {
    let user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            user_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"role": "ADMIN", "tier": "PRO"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject mixed admin user patch: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_accept_idempotent_admin_user_patch() {
    let user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            user_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch admin user with empty object: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_null_admin_user_role() {
    let user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            user_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"role": null}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject null admin user role: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_remove_legacy_admin_user_patch_route() {
    let user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!("{}/api/v1/users/{}", AURA_API.base_url(), user_id))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"tier": "PRO"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call removed admin user patch route: {error}"));

    assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_protect_the_last_admin_from_self_demotion() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            admin_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"role": "USER"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to protect the last admin: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");
    assert_eq!(
        serde_json::json!("At least one active administrator must remain."),
        body["detail"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_protect_the_last_admin_from_self_deletion() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!("{}/api/v1/me", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to protect the last admin deletion: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_user_when_actor_is_admin() {
    let user_id = seed_user("USER").await;
    let target_token = String::from(seed_access_token_for(user_id, Default::default()).await);
    let admin_id = seed_user("ADMIN").await;
    let admin_token = String::from(
        seed_access_token_for(
            admin_id,
            std::collections::HashSet::from([Scope::UsersRead, Scope::UsersWrite]),
        )
        .await,
    );
    let client = reqwest::Client::new();

    let response = client
        .delete(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            user_id
        ))
        .bearer_auth(admin_token.clone())
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete admin user API: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());

    let response = client
        .get(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            user_id
        ))
        .bearer_auth(admin_token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to verify deleted admin user: {error}"));
    let (status, body) = json_response(response).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
    );

    let response = client
        .get(format!("{}/api/v1/me/account", AURA_API.base_url()))
        .bearer_auth(target_token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to verify deleted user access token: {error}"));
    let (status, body) = json_response(response).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_when_admin_deletes_missing_user() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            UserId::new()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete missing admin user: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_user_delete_when_user_id_is_invalid() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/not-a-uuid",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate admin user delete ID: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_user_delete_when_actor_is_not_admin() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            user_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin user delete: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_protect_the_last_admin_from_admin_user_deletion() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersRead, Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            admin_id
        ))
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to protect the last admin deletion: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            admin_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to verify protected administrator: {error}"));
    assert_eq!(reqwest::StatusCode::OK, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_user_read_when_actor_is_not_admin() {
    let user_id = seed_user("USER").await;
    let token =
        seed_access_token_for(user_id, std::collections::HashSet::from([Scope::UsersRead])).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/users/{}",
            AURA_API.base_url(),
            user_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get forbidden user API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_user_read_when_user_id_is_invalid() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/users/not-a-uuid",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get invalid user API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_access_token_for_current_user() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite, Scope::AccessTokensRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "name": "acceptance token",
            "scopes": ["product-listings:write", "watchlist:write"]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create access token API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
    assert!(
        body["accessToken"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_list_access_tokens_for_current_user() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list access token API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert!(body.as_array().is_some_and(|items| !items.is_empty()));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_list_admin_user_access_tokens_with_cursor_without_secrets() {
    let target_user_id = seed_user("USER").await;
    let target_authentication_token = seed_access_token_for(
        target_user_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;
    let client = reqwest::Client::new();

    for (name, expires) in [
        ("expired admin inspection token", "2020-01-01T00:00:00Z"),
        ("current admin inspection token", "2099-01-01T00:00:00Z"),
    ] {
        let response = client
            .post(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
            .bearer_auth(String::from(target_authentication_token.clone()))
            .json(&serde_json::json!({
                "name": name,
                "scopes": ["users:read"],
                "expires": expires
            }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to create inspection access token: {error}"));
        assert_eq!(reqwest::StatusCode::CREATED, response.status());
    }

    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::AccessTokensRead]),
    )
    .await;
    let endpoint = format!(
        "{}/api/v1/admin/users/{}/access-tokens",
        AURA_API.base_url(),
        target_user_id
    );
    let mut search_after = None;
    let mut listed_items = Vec::new();

    for page in 0..4 {
        let request = client
            .get(&endpoint)
            .bearer_auth(String::from(admin_token.clone()))
            .query(&[("size", "1")]);
        let request = if let Some(search_after) = search_after.as_deref() {
            request.query(&[("searchAfter", search_after)])
        } else {
            request
        };
        let response = request
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to list admin access tokens: {error}"));
        let cache_control = response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let (status, body) = json_response(response).await;

        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(Some("no-store".to_owned()), cache_control);
        let items = body["items"]
            .as_array()
            .unwrap_or_else(|| panic!("admin access-token page has items: {body}"));
        assert_eq!(1, items.len());
        listed_items.extend(items.iter().cloned());
        assert!(items.iter().all(|item| {
            item.get("accessToken").is_none()
                && item.get("token").is_none()
                && item.get("tokenShort").is_none()
                && item.get("tokenHash").is_none()
                && item.get("hash").is_none()
        }));

        if body["searchAfter"].is_null() {
            assert_eq!(2, page);
            break;
        }
        search_after = Some(
            serde_json::to_string(&body["searchAfter"])
                .unwrap_or_else(|error| panic!("admin access-token cursor serializes: {error}")),
        );
    }

    assert_eq!(3, listed_items.len());
    assert!(listed_items.iter().any(|item| {
        item["name"] == "expired admin inspection token"
            && item["expires"] == "2020-01-01T00:00:00Z"
    }));
    assert!(listed_items.iter().any(|item| {
        item["name"] == "current admin inspection token"
            && item["expires"] == "2099-01-01T00:00:00Z"
    }));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_empty_admin_access_token_page_for_existing_user() {
    let target_user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::AccessTokensRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/users/{}/access-tokens",
            AURA_API.base_url(),
            target_user_id
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list empty admin access-token page: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(serde_json::json!([]), body["items"]);
    assert_eq!(serde_json::json!(21), body["size"]);
    assert!(body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_access_token_list_for_non_admin_missing_and_invalid_user() {
    let target_user_id = seed_user("USER").await;
    let actor_id = seed_user("USER").await;
    let actor_token = seed_access_token_for(
        actor_id,
        std::collections::HashSet::from([Scope::AccessTokensRead]),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::AccessTokensRead]),
    )
    .await;
    let client = reqwest::Client::new();

    let response = client
        .get(format!(
            "{}/api/v1/admin/users/{}/access-tokens",
            AURA_API.base_url(),
            target_user_id
        ))
        .bearer_auth(String::from(actor_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin access-token list: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
    assert_eq!(Some("no-store".to_owned()), cache_control);

    let response = client
        .get(format!(
            "{}/api/v1/admin/users/{}/access-tokens",
            AURA_API.base_url(),
            UserId::new()
        ))
        .bearer_auth(String::from(admin_token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list missing user's access tokens: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
    );
    assert_eq!(Some("no-store".to_owned()), cache_control);

    let response = client
        .get(format!(
            "{}/api/v1/admin/users/not-a-uuid/access-tokens",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate admin access-token user ID: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
    assert_eq!(serde_json::json!("userId"), body["source"]["field"]);
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_get_access_token_for_current_user() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensRead, Scope::AccessTokensWrite]),
    )
    .await;
    let access_token_id = create_access_token(&token).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/me/access-tokens/{}",
            AURA_API.base_url(),
            access_token_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get access token API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!("editable token"), body["name"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_access_token_for_current_user() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensRead, Scope::AccessTokensWrite]),
    )
    .await;
    let access_token_id = create_access_token(&token).await;

    let response = reqwest::Client::new()
        .patch(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "accessTokenId": access_token_id,
            "name": "renamed token",
            "scopes": ["product-listings:write"]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch access token API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!("renamed token"), body["name"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_access_token_for_current_user() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensRead, Scope::AccessTokensWrite]),
    )
    .await;
    let access_token_id = create_access_token(&token).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/me/access-tokens/{}",
            AURA_API.base_url(),
            access_token_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete access token API: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_revoke_all_admin_user_access_tokens_and_preserve_other_users() {
    let target_user_id = seed_user("USER").await;
    let target_token_one = seed_access_token_for(
        target_user_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;
    let target_token_two = seed_access_token_for(
        target_user_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;
    let unrelated_user_id = seed_user("USER").await;
    let unrelated_token = seed_access_token_for(
        unrelated_user_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;
    let client = reqwest::Client::new();

    let response = client
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens",
            AURA_API.base_url(),
            target_user_id
        ))
        .bearer_auth(String::from(admin_token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to bulk revoke admin access tokens: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert_eq!(
        Some("no-store"),
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
    );

    for revoked_token in [target_token_one, target_token_two] {
        let response = client
            .get(format!("{}/api/v1/me/account", AURA_API.base_url()))
            .bearer_auth(String::from(revoked_token))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to check revoked access token: {error}"));
        let (status, body) = json_response(response).await;
        assert_problem(
            status,
            &body,
            reqwest::StatusCode::UNAUTHORIZED,
            "INVALID_CREDENTIALS",
        );
    }

    let response = client
        .get(format!("{}/api/v1/me/account", AURA_API.base_url()))
        .bearer_auth(String::from(unrelated_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to check unrelated access token: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(
        serde_json::json!(unrelated_user_id.to_string()),
        body["userId"]
    );

    let response = client
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens",
            AURA_API.base_url(),
            target_user_id
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to retry bulk revoke: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_bulk_revoke_existing_admin_user_with_no_tokens_idempotently() {
    let target_user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;
    let client = reqwest::Client::new();

    for _ in 0..2 {
        let response = client
            .delete(format!(
                "{}/api/v1/admin/users/{}/access-tokens",
                AURA_API.base_url(),
                target_user_id
            ))
            .bearer_auth(String::from(admin_token.clone()))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to bulk revoke empty token set: {error}"));

        assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(reqwest::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_when_bulk_revoke_target_user_is_missing() {
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;
    let missing_user_id = UserId::new();

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens",
            AURA_API.base_url(),
            missing_user_id
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to bulk revoke missing user tokens: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
    );
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_revoke_admin_target_cognito_sessions_idempotently() {
    let target_user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;
    let client = reqwest::Client::new();

    for _ in 0..2 {
        let response = client
            .post(format!(
                "{}/api/v1/admin/users/{target_user_id}/sessions/revoke",
                AURA_API.base_url()
            ))
            .bearer_auth(String::from(admin_token.clone()))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to revoke Cognito sessions: {error}"));

        assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(reqwest::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_map_temporary_cognito_session_revocation_failure() {
    let target_user_id = seed_user("USER").await;
    fail_session_revocation_for(target_user_id);
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/users/{target_user_id}/sessions/revoke",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call unavailable Cognito revocation: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "USER_TEMPORARILY_UNAVAILABLE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_session_revocation_for_non_admin_or_missing_target() {
    let target_user_id = seed_user("USER").await;
    let actor_id = seed_user("USER").await;
    let actor_token = seed_access_token_for(
        actor_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/users/{target_user_id}/sessions/revoke",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(actor_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin session revocation: {error}"));
    let (status, body) = json_response(response).await;
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");

    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/users/{}/sessions/revoke",
            AURA_API.base_url(),
            UserId::new()
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject missing session target: {error}"));
    let (status, body) = json_response(response).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_bulk_revoke_for_non_admin_actor() {
    let target_user_id = seed_user("USER").await;
    let actor_id = seed_user("USER").await;
    let actor_token = seed_access_token_for(
        actor_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens",
            AURA_API.base_url(),
            target_user_id
        ))
        .bearer_auth(String::from(actor_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin bulk revoke: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_bulk_revoke_without_valid_auth_or_user_id() {
    let target_user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;
    let client = reqwest::Client::new();

    let response = client
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens",
            AURA_API.base_url(),
            target_user_id
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject missing bulk-revoke auth: {error}"));
    let (status, body) = json_response(response).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );

    let response = client
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens",
            AURA_API.base_url(),
            target_user_id
        ))
        .bearer_auth("not-a-valid-token")
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject invalid bulk-revoke auth: {error}"));
    let (status, body) = json_response(response).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );

    let response = client
        .delete(format!(
            "{}/api/v1/admin/users/not-a-uuid/access-tokens",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject invalid bulk-revoke user ID: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
    assert_eq!(serde_json::json!("userId"), body["source"]["field"]);
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_allow_admin_to_revoke_access_token_and_make_it_unusable() {
    let target_user_id = seed_user("USER").await;
    let target_authentication_token = seed_access_token_for(
        target_user_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;
    let (access_token_id, revoked_token) =
        create_access_token_with_raw(&target_authentication_token, &["users:read"]).await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens/{}",
            AURA_API.base_url(),
            target_user_id,
            access_token_id
        ))
        .bearer_auth(String::from(admin_token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to revoke admin access token: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert_eq!(
        Some("no-store"),
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
    );

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/account", AURA_API.base_url()))
        .bearer_auth(revoked_token)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to authenticate with revoked access token: {error}")
        });
    let (status, body) = json_response(response).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens/{}",
            AURA_API.base_url(),
            target_user_id,
            access_token_id
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to retry admin access-token revoke: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens/{}",
            AURA_API.base_url(),
            target_user_id,
            UserId::new()
        ))
        .bearer_auth(String::from(
            seed_access_token_for(
                seed_user("ADMIN").await,
                std::collections::HashSet::from([Scope::AccessTokensWrite]),
            )
            .await,
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to revoke missing access token: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_not_revoke_another_users_access_token_from_admin_route() {
    let token_owner_id = seed_user("USER").await;
    let owner_authentication_token = seed_access_token_for(
        token_owner_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;
    let (access_token_id, owner_token) =
        create_access_token_with_raw(&owner_authentication_token, &["users:read"]).await;
    let target_user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens/{}",
            AURA_API.base_url(),
            target_user_id,
            access_token_id
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to test cross-user admin revoke: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/account", AURA_API.base_url()))
        .bearer_auth(owner_token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to verify cross-user token was preserved: {error}"));
    assert_eq!(reqwest::StatusCode::OK, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_access_token_revoke_for_non_admin_and_invalid_auth() {
    let target_user_id = seed_user("USER").await;
    let actor_id = seed_user("USER").await;
    let actor_token = seed_access_token_for(
        actor_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;
    let access_token_id = UserId::new();

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens/{}",
            AURA_API.base_url(),
            target_user_id,
            access_token_id
        ))
        .bearer_auth(String::from(actor_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin access-token revoke: {error}"));
    let (status, body) = json_response(response).await;
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens/{}",
            AURA_API.base_url(),
            target_user_id,
            access_token_id
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject missing admin authentication: {error}"));
    let (status, body) = json_response(response).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_validate_both_admin_access_token_revoke_path_ids() {
    let target_user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;

    for (path, field) in [
        (
            format!(
                "{}/api/v1/admin/users/not-a-uuid/access-tokens/{}",
                AURA_API.base_url(),
                UserId::new()
            ),
            "userId",
        ),
        (
            format!(
                "{}/api/v1/admin/users/{}/access-tokens/not-a-uuid",
                AURA_API.base_url(),
                target_user_id
            ),
            "accessTokenId",
        ),
    ] {
        let response = reqwest::Client::new()
            .delete(path)
            .bearer_auth(String::from(admin_token.clone()))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to validate admin access-token path: {error}"));
        let (status, body) = json_response(response).await;
        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "INVALID_UUID",
        );
        assert_eq!(serde_json::json!(field), body["source"]["field"]);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_access_token_revoke_without_admin_scope() {
    let target_user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(admin_id, Default::default()).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens/{}",
            AURA_API.base_url(),
            target_user_id,
            UserId::new()
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject admin access-token scope: {error}"));
    let (status, body) = json_response(response).await;
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_access_token_revoke_with_invalid_bearer_credentials() {
    let target_user_id = seed_user("USER").await;
    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}/access-tokens/{}",
            AURA_API.base_url(),
            target_user_id,
            UserId::new()
        ))
        .bearer_auth("not-a-valid-token")
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to reject invalid admin bearer credentials: {error}")
        });
    let (status, body) = json_response(response).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_access_token_read_when_id_is_invalid() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/me/access-tokens/not-a-uuid",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get invalid access token API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_auth_for_access_tokens() {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list missing auth token API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_suspend_user_and_repeat_idempotently_when_actor_is_admin() {
    let target_user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;
    let endpoint = format!(
        "{}/api/v1/admin/users/{target_user_id}/suspension",
        AURA_API.base_url()
    );
    let client = reqwest::Client::new();

    for attempt in 0..2 {
        let response = client
            .put(&endpoint)
            .bearer_auth(String::from(admin_token.clone()))
            .json(&serde_json::json!({"reason": "Repeated policy violations"}))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to suspend user: {error}"));
        let cache_control = response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let (status, body) = json_response(response).await;

        assert_eq!(reqwest::StatusCode::OK, status, "attempt {attempt}: {body}");
        assert_eq!(Some("no-store".to_owned()), cache_control);
        assert_eq!(
            serde_json::json!(target_user_id.to_string()),
            body["userId"]
        );
        assert_eq!(serde_json::json!(true), body["suspended"]);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_unsuspend_user_idempotently_and_restore_existing_token() {
    let target_user_id = seed_user_with_tier("USER", UserTier::Pro).await;
    let target_token = seed_access_token_for(
        target_user_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;
    let endpoint = format!(
        "{}/api/v1/admin/users/{target_user_id}/suspension",
        AURA_API.base_url()
    );
    let client = reqwest::Client::new();

    let response = client
        .put(&endpoint)
        .bearer_auth(String::from(admin_token.clone()))
        .json(&serde_json::json!({"reason": "Incident resolved"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to suspend user before restoration: {error}"));
    assert_eq!(reqwest::StatusCode::OK, response.status());

    for attempt in 0..2 {
        let response = client
            .delete(&endpoint)
            .bearer_auth(String::from(admin_token.clone()))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to reactivate user: {error}"));
        let cache_control = response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let (status, body) = json_response(response).await;

        assert_eq!(reqwest::StatusCode::OK, status, "attempt {attempt}: {body}");
        assert_eq!(Some("no-store".to_owned()), cache_control);
        assert_eq!(
            serde_json::json!(target_user_id.to_string()),
            body["userId"]
        );
        assert_eq!(serde_json::json!(false), body["suspended"]);
    }

    let response = client
        .get(format!("{}/api/v1/me/account", AURA_API.base_url()))
        .bearer_auth(String::from(target_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to authenticate restored user: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(
        serde_json::json!(target_user_id.to_string()),
        body["userId"]
    );
    assert_eq!(serde_json::json!("USER"), body["role"]);
    assert_eq!(serde_json::json!("PRO"), body["tier"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_user_reactivation_when_actor_is_not_admin() {
    let target_user_id = seed_user("USER").await;
    let actor_id = seed_user("USER").await;
    let actor_token = seed_access_token_for(
        actor_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{target_user_id}/suspension",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(actor_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin reactivation: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_for_missing_user_reactivation() {
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/{}/suspension",
            AURA_API.base_url(),
            UserId::new()
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reactivate missing user: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_user_id_for_reactivation() {
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/users/not-a-uuid/suspension",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject invalid reactivation target: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_secret_bearing_user_suspension_reason() {
    let target_user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/admin/users/{target_user_id}/suspension",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(admin_token))
        .json(&serde_json::json!({"reason": "BEARER credential supplied by mistake"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject secret-bearing reason: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_user_suspension_when_actor_is_not_admin() {
    let target_user_id = seed_user("USER").await;
    let actor_id = seed_user("USER").await;
    let actor_token = seed_access_token_for(
        actor_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/admin/users/{target_user_id}/suspension",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(actor_token))
        .json(&serde_json::json!({"reason": "Repeated policy violations"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin suspension: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_when_admin_suspends_missing_user() {
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/admin/users/{}/suspension",
            AURA_API.base_url(),
            UserId::new()
        ))
        .bearer_auth(String::from(admin_token))
        .json(&serde_json::json!({"reason": "Repeated policy violations"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to suspend missing user: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_protect_last_active_admin_from_suspension() {
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/admin/users/{admin_id}/suspension",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(admin_token))
        .json(&serde_json::json!({"reason": "Repeated policy violations"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to protect final admin: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_suspended_user_aura_token_on_later_request() {
    let target_user_id = seed_user("USER").await;
    let target_token = seed_access_token_for(
        target_user_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;
    let client = reqwest::Client::new();

    let response = client
        .put(format!(
            "{}/api/v1/admin/users/{target_user_id}/suspension",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(admin_token))
        .json(&serde_json::json!({"reason": "Repeated policy violations"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to suspend user before auth check: {error}"));
    assert_eq!(reqwest::StatusCode::OK, response.status());

    let response = client
        .get(format!("{}/api/v1/me/account", AURA_API.base_url()))
        .bearer_auth(String::from(target_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject suspended token: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}

async fn create_access_token(token: &user_core::access_token::RawAccessToken) -> String {
    create_access_token_with_raw(token, &["product-listings:write"])
        .await
        .0
}

async fn create_access_token_with_raw(
    token: &user_core::access_token::RawAccessToken,
    scopes: &[&str],
) -> (String, String) {
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"name": "editable token", "scopes": scopes}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create access token API: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::CREATED, status, "{body}");
    (
        body["accessTokenId"]
            .as_str()
            .unwrap_or_else(|| panic!("missing accessTokenId"))
            .to_owned(),
        body["accessToken"]
            .as_str()
            .unwrap_or_else(|| panic!("missing accessToken"))
            .to_owned(),
    )
}
