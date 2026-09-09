use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    assert_problem, json_response, seed_access_token_for, seed_current_fx_snapshot,
    seed_listing_source, seed_operator_partnership_listing_source_grant,
    seed_partnership_membership, seed_user,
};
use listing_source_core::ListingSourceId;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::convert::Infallible;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use user_core::access_token::Scope;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn assert_test_result(result: TestResult) {
    assert!(result.is_ok(), "{result:?}");
}

struct PartnerAuth {
    listing_source_id: String,
    token: String,
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_partner_product_batch_when_all_products_are_new() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;

        let response = send_json(
            reqwest::Method::POST,
            products_path(&auth.listing_source_id),
            Some(&auth.token),
            &json!([product("post-full-first"), product("post-full-second")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(json!([]), body);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_duplicate_product_as_partial_create_failure() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let duplicate_id = "post-duplicate";

        let response = send_json(
            reqwest::Method::POST,
            products_path(&auth.listing_source_id),
            Some(&auth.token),
            &json!([product(duplicate_id), product(duplicate_id)]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(
            json!([failure(&auth.listing_source_id, duplicate_id, "CONFLICT")]),
            body
        );
        assert!(
            body[0]["listingSourceId"]
                .as_str()
                .is_some_and(|value| value.starts_with("ls_"))
        );
        assert_eq!(json!(duplicate_id), body[0]["sourceListingId"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_existing_partner_product_batch() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let product_listing_id = "patch-success";
        create_product(&auth, product_listing_id).await?;

        let response = send_json(
            reqwest::Method::PATCH,
            products_path(&auth.listing_source_id),
            Some(&auth.token),
            &json!([patch_product(product_listing_id, "SOLD_OUT")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(json!([]), body);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_unrelated_partner_from_existing_product_update() -> TestResult {
    let result: TestResult = async {
        let owner = partner_auth(product_listings_write_scope()).await?;
        let product_listing_id = "patch-unrelated-partner";
        create_product(&owner, product_listing_id).await?;

        let unrelated_user_id = seed_user("USER").await;
        let unrelated_token = String::from(
            seed_access_token_for(unrelated_user_id, product_listings_write_scope()).await,
        );
        let response = send_json(
            reqwest::Method::PATCH,
            products_path(&owner.listing_source_id),
            Some(&unrelated_token),
            &json!([patch_product(product_listing_id, "SOLD_OUT")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::FORBIDDEN, status);
        assert_eq!(json!("FORBIDDEN"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_missing_product_as_partial_update_failure() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let existing_id = "patch-partial-existing";
        let missing_id = "patch-partial-missing";
        create_product(&auth, existing_id).await?;

        let response = send_json(
            reqwest::Method::PATCH,
            products_path(&auth.listing_source_id),
            Some(&auth.token),
            &json!([
                patch_product(existing_id, "AVAILABLE"),
                patch_product(missing_id, "SOLD_OUT")
            ]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(
            json!([failure(
                &auth.listing_source_id,
                missing_id,
                "PRODUCT_LISTING_NOT_FOUND"
            )]),
            body
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_when_every_product_update_is_missing() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;

        let response = send_json(
            reqwest::Method::PATCH,
            products_path(&auth.listing_source_id),
            Some(&auth.token),
            &json!([patch_product("patch-all-missing", "SOLD_OUT")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::NOT_FOUND, status);
        assert_eq!(json!("PRODUCT_LISTING_NOT_FOUND"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_then_update_partner_product_with_upsert() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let product_listing_id = "put-create-update";

        let created = send_json(
            reqwest::Method::PUT,
            products_path(&auth.listing_source_id),
            Some(&auth.token),
            &json!([product(product_listing_id)]),
        )
        .await?;
        let (created_status, created_body) = response_json(created).await?;
        assert_eq!(reqwest::StatusCode::OK, created_status);
        assert_eq!(json!([]), created_body);

        let updated = send_json(
            reqwest::Method::PUT,
            products_path(&auth.listing_source_id),
            Some(&auth.token),
            &json!([patch_product(product_listing_id, "SOLD_OUT")]),
        )
        .await?;
        let (updated_status, updated_body) = response_json(updated).await?;

        assert_eq!(reqwest::StatusCode::OK, updated_status);
        assert_eq!(json!([]), updated_body);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_unrelated_partner_from_existing_product_upsert() -> TestResult {
    let result: TestResult = async {
        let owner = partner_auth(product_listings_write_scope()).await?;
        let product_listing_id = "put-unrelated-partner";
        create_product(&owner, product_listing_id).await?;

        let unrelated_user_id = seed_user("USER").await;
        let unrelated_token = String::from(
            seed_access_token_for(unrelated_user_id, product_listings_write_scope()).await,
        );
        let response = send_json(
            reqwest::Method::PUT,
            products_path(&owner.listing_source_id),
            Some(&unrelated_token),
            &json!([patch_product(product_listing_id, "SOLD_OUT")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::FORBIDDEN, status);
        assert_eq!(json!("FORBIDDEN"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_existing_partner_product_batch() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let first_id = "delete-full-first";
        let second_id = "delete-full-second";
        create_products(&auth, &[first_id, second_id]).await?;

        let response = send_json(
            reqwest::Method::DELETE,
            products_path(&auth.listing_source_id),
            Some(&auth.token),
            &json!([delete_product(first_id), delete_product(second_id)]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(json!([]), body);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_unrelated_partner_from_existing_product_delete() -> TestResult {
    let result: TestResult = async {
        let owner = partner_auth(product_listings_write_scope()).await?;
        let product_listing_id = "delete-unrelated-partner";
        create_product(&owner, product_listing_id).await?;

        let unrelated_user_id = seed_user("USER").await;
        let unrelated_token = String::from(
            seed_access_token_for(unrelated_user_id, product_listings_write_scope()).await,
        );
        let response = send_json(
            reqwest::Method::DELETE,
            products_path(&owner.listing_source_id),
            Some(&unrelated_token),
            &json!([delete_product(product_listing_id)]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::FORBIDDEN, status);
        assert_eq!(json!("FORBIDDEN"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_missing_product_as_partial_delete_failure() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let existing_id = "delete-partial-existing";
        let missing_id = "delete-partial-missing";
        create_product(&auth, existing_id).await?;

        let response = send_json(
            reqwest::Method::DELETE,
            products_path(&auth.listing_source_id),
            Some(&auth.token),
            &json!([delete_product(existing_id), delete_product(missing_id)]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(
            json!([failure(
                &auth.listing_source_id,
                missing_id,
                "PRODUCT_LISTING_NOT_FOUND"
            )]),
            body
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_when_every_product_delete_is_missing() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;

        let response = send_json(
            reqwest::Method::DELETE,
            products_path(&auth.listing_source_id),
            Some(&auth.token),
            &json!([delete_product("delete-all-missing")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::NOT_FOUND, status);
        assert_eq!(json!("PRODUCT_LISTING_NOT_FOUND"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_partner_product_batch_when_access_token_lacks_scope() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(HashSet::new()).await?;

        let response = send_json(
            reqwest::Method::POST,
            products_path(&auth.listing_source_id),
            Some(&auth.token),
            &json!([product("scope-less")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::FORBIDDEN, status);
        assert_eq!(json!("FORBIDDEN"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_unrelated_partner_from_product_batch() -> TestResult {
    let result: TestResult = async {
        let listing_source_id = seed_listing_source().await;
        let user_id = seed_user("USER").await;
        let token =
            String::from(seed_access_token_for(user_id, product_listings_write_scope()).await);

        let response = send_json(
            reqwest::Method::POST,
            products_path(&listing_source_type_id(listing_source_id)),
            Some(&token),
            &json!([product("unrelated-partner")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::FORBIDDEN, status);
        assert_eq!(json!("FORBIDDEN"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_upsert_concurrently_without_returning_temporary_failure() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let path = products_path(&auth.listing_source_id);
        let body = json!([product("concurrent-upsert")]);

        let (first, second) = tokio::join!(
            send_json(reqwest::Method::PUT, path.clone(), Some(&auth.token), &body,),
            send_json(reqwest::Method::PUT, path, Some(&auth.token), &body),
        );
        let (first_status, first_body) = response_json(first?).await?;
        let (second_status, second_body) = response_json(second?).await?;

        assert_eq!(reqwest::StatusCode::OK, first_status);
        assert_eq!(json!([]), first_body);
        assert_eq!(reqwest::StatusCode::OK, second_status);
        assert_eq!(json!([]), second_body);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_partner_product_batch_without_authorization() -> TestResult {
    let result: TestResult = async {
        let listing_source_id = seed_listing_source().await;

        let response = send_json(
            reqwest::Method::POST,
            products_path(&listing_source_type_id(listing_source_id)),
            None,
            &json!([product("missing-authorization")]),
        )
        .await?;
        let (status, _) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::UNAUTHORIZED, status);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_not_expose_legacy_partner_product_item_delete_route() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;

        let response = send_json(
            reqwest::Method::DELETE,
            format!("{}/legacy-item", products_path(&auth.listing_source_id)),
            Some(&auth.token),
            &json!({}),
        )
        .await?;

        assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_noncanonical_listing_source_id() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let listing_source_id = listing_source_core::ListingSourceId::new();

        for invalid_id in [
            product_listing_core::product_listing_id::ProductListingId::new().to_string(),
            listing_source_id.as_uuid().to_string(),
            "ls_not-a-typeid".to_owned(),
        ] {
            let response = send_json(
                reqwest::Method::POST,
                products_path(&invalid_id),
                Some(&auth.token),
                &json!([product("invalid-source")]),
            )
            .await?;
            let (status, body) = response_json(response).await?;
            assert_problem(
                status,
                &body,
                reqwest::StatusCode::BAD_REQUEST,
                "INVALID_OBJECT_ID",
            );
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

async fn partner_auth(scopes: HashSet<Scope>) -> Result<PartnerAuth, Infallible> {
    let pool = get_postgres_client().await;
    seed_current_fx_snapshot(&pool).await;
    let listing_source_id = seed_listing_source().await;
    let user_id = seed_user("USER").await;
    seed_partnership_membership(user_id, listing_source_id).await;
    seed_operator_partnership_listing_source_grant(listing_source_id).await;
    let token = String::from(seed_access_token_for(user_id, scopes).await);

    Ok(PartnerAuth {
        listing_source_id: listing_source_type_id(listing_source_id),
        token,
    })
}

async fn create_product(auth: &PartnerAuth, source_listing_id: &str) -> TestResult {
    create_products(auth, &[source_listing_id]).await
}

async fn create_products(auth: &PartnerAuth, source_listing_ids: &[&str]) -> TestResult {
    let response = send_json(
        reqwest::Method::POST,
        products_path(&auth.listing_source_id),
        Some(&auth.token),
        &Value::Array(
            source_listing_ids
                .iter()
                .map(|source_listing_id| product(source_listing_id))
                .collect(),
        ),
    )
    .await?;
    let (status, body) = response_json(response).await?;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(json!([]), body);
    Ok::<(), Box<dyn std::error::Error>>(())
}

async fn send_json(
    method: reqwest::Method,
    path: String,
    token: Option<&str>,
    body: &Value,
) -> Result<reqwest::Response, reqwest::Error> {
    let request =
        reqwest::Client::new().request(method, format!("{}{}", AURA_API.base_url(), path));
    let request = match token {
        Some(token) => request.bearer_auth(token),
        None => request,
    };

    request.json(body).send().await
}

async fn response_json(
    response: reqwest::Response,
) -> Result<(reqwest::StatusCode, Value), Infallible> {
    Ok(json_response(response).await)
}

fn product_listings_write_scope() -> HashSet<Scope> {
    HashSet::from([Scope::ProductListingsWrite])
}

fn products_path(listing_source_id: &str) -> String {
    format!("/api/v1/listing-sources/{listing_source_id}/product-listings")
}

fn listing_source_type_id(listing_source_id: uuid::Uuid) -> String {
    ListingSourceId::try_from(listing_source_id)
        .unwrap_or_else(|error| panic!("invalid ListingSource fixture ID: {error}"))
        .to_string()
}

fn product(source_listing_id: &str) -> Value {
    json!({
        "sourceListingId": source_listing_id,
        "title": { "text": "Synchronous Cabinet", "language": "en" },
        "description": { "text": "Created in the request transaction.", "language": "en" },
        "availability": "AVAILABLE",
        "url": format!("https://partner.example/product-listings/{source_listing_id}"),
        "images": []
    })
}

fn patch_product(source_listing_id: &str, availability: &str) -> Value {
    json!({
        "sourceListingId": source_listing_id,
        "availability": availability
    })
}

fn delete_product(source_listing_id: &str) -> Value {
    json!({ "sourceListingId": source_listing_id })
}

fn failure(listing_source_id: &str, source_listing_id: &str, error: &str) -> Value {
    json!({
        "listingSourceId": listing_source_id,
        "sourceListingId": source_listing_id,
        "error": error
    })
}
