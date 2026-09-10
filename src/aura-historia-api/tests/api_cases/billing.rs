use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    assert_problem, json_response, seed_access_token_for, seed_user, seed_user_with_tier,
};

use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use user_core::access_token::Scope;
use user_core::tier::UserTier;

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_checkout_and_persist_stripe_customer_for_free_user() {
    let user_id = seed_user("USER").await;
    let token = users_read_token(user_id).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/me/billing/checkout",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&billing_request("PRO", "MONTHLY"))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create checkout: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(
        serde_json::json!(format!(
            "https://checkout.stripe.test/cus_{}/price_pro_monthly",
            user_id.as_uuid()
        )),
        body["url"]
    );
    let customer_id: Option<String> =
        sqlx::query_scalar("SELECT stripe_customer_id FROM users WHERE user_id = $1")
            .bind(uuid::Uuid::from(user_id))
            .fetch_one(&get_postgres_client().await)
            .await
            .unwrap_or_else(|error| panic!("failed to read Stripe customer association: {error}"));
    assert_eq!(Some(format!("cus_{}", user_id.as_uuid())), customer_id);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_checkout_when_customer_already_exists() {
    let user_id = seed_user("USER").await;
    let token = users_read_token(user_id).await;
    sqlx::query("UPDATE users SET stripe_customer_id = $1 WHERE user_id = $2")
        .bind("cus_existing")
        .bind(uuid::Uuid::from(user_id))
        .execute(&get_postgres_client().await)
        .await
        .unwrap_or_else(|error| panic!("failed to seed Stripe customer: {error}"));

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/me/billing/checkout",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&billing_request("PRO", "MONTHLY"))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create checkout: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::CONFLICT,
        "STRIPE_CUSTOMER_ALREADY_EXISTS",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_portal_for_user_with_stripe_customer() {
    let user_id = seed_user("USER").await;
    let token = users_read_token(user_id).await;
    sqlx::query("UPDATE users SET stripe_customer_id = $1 WHERE user_id = $2")
        .bind("cus_portal")
        .bind(uuid::Uuid::from(user_id))
        .execute(&get_postgres_client().await)
        .await
        .unwrap_or_else(|error| panic!("failed to seed Stripe customer: {error}"));

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/billing/portal", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create portal: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(
        serde_json::json!("https://billing.stripe.test/cus_portal"),
        body["url"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_portal_without_stripe_customer() {
    let user_id = seed_user("USER").await;
    let token = users_read_token(user_id).await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/billing/portal", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create portal: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        "STRIPE_CUSTOMER_DOES_NOT_EXIST",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_checkout_when_managing_free_user() {
    let user_id = seed_user("USER").await;
    let token = users_read_token(user_id).await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/billing/manage", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&billing_request("ULTIMATE", "YEARLY"))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to manage free billing: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(
        serde_json::json!(format!(
            "https://checkout.stripe.test/cus_{}/price_ultimate_yearly",
            user_id.as_uuid()
        )),
        body["url"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_portal_when_managing_paid_user() {
    let user_id = seed_user_with_tier("USER", UserTier::Pro).await;
    let token = users_read_token(user_id).await;
    sqlx::query("UPDATE users SET stripe_customer_id = $1 WHERE user_id = $2")
        .bind("cus_paid")
        .bind(uuid::Uuid::from(user_id))
        .execute(&get_postgres_client().await)
        .await
        .unwrap_or_else(|error| panic!("failed to seed paid Stripe customer: {error}"));

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/billing/manage", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&billing_request("PRO", "MONTHLY"))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to manage paid billing: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(
        serde_json::json!("https://billing.stripe.test/cus_paid"),
        body["url"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_billing_when_delegated_token_lacks_users_read() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, Default::default()).await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/billing/portal", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create portal: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

fn billing_request(plan: &str, cycle: &str) -> serde_json::Value {
    serde_json::json!({ "plan": plan, "cycle": cycle })
}

async fn users_read_token(
    user_id: user_core::user_id::UserId,
) -> user_core::access_token::RawAccessToken {
    seed_access_token_for(user_id, std::collections::HashSet::from([Scope::UsersRead])).await
}
