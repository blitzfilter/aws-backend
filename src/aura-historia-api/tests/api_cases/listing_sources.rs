use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    assert_problem, json_response, seed_access_token_for,
    seed_listing_source as seed_raw_listing_source,
    seed_listing_source_for_search as seed_raw_listing_source_for_search, seed_party, seed_product,
    seed_user,
};
use listing_source_core::ListingSourceId;
use party_core::party_id::PartyId;
use product_listing_core::product_listing_id::ProductListingId;
use serde_json::json;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;

async fn seed_listing_source() -> ListingSourceId {
    ListingSourceId::try_from(seed_raw_listing_source().await)
        .unwrap_or_else(|error| panic!("central ListingSource fixture must use UUIDv7: {error}"))
}

async fn seed_listing_source_for_search(
    name: &str,
    operator_name: &str,
    ingestion_method: &str,
    referral_configuration: Option<serde_json::Value>,
) -> (ListingSourceId, PartyId, String) {
    let (listing_source_id, party_id, slug_id) = seed_raw_listing_source_for_search(
        name,
        operator_name,
        ingestion_method,
        referral_configuration,
    )
    .await;
    (
        ListingSourceId::try_from(listing_source_id).unwrap_or_else(|error| {
            panic!("central ListingSource search fixture must use UUIDv7: {error}")
        }),
        PartyId::try_from(party_id).unwrap_or_else(|error| {
            panic!("central Party search fixture must use UUIDv7: {error}")
        }),
        slug_id,
    )
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_expose_public_collection_and_exact_slug_detail_anonymously() {
    let (listing_source_id, _party_id, slug_id) = seed_listing_source_for_search(
        "Müller Public Auction",
        "Public Operator",
        "WEB_CRAWL",
        Some(json!({"partnerize": {"campaign": "private"}})),
    )
    .await;
    let client = reqwest::Client::new();

    let collection_response = client
        .get(format!(
            "{}/api/v1/listing-sources?query=muller&size=21",
            AURA_API.base_url()
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("get public ListingSource collection: {error}"));
    assert_eq!(reqwest::StatusCode::OK, collection_response.status());
    assert_eq!(
        Some("no-store"),
        collection_response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
    );
    let (_, collection) = json_response(collection_response).await;
    let item = collection["items"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["listingSourceId"] == listing_source_id.to_string())
        })
        .unwrap_or_else(|| panic!("public collection must contain seeded ListingSource"));
    assert_eq!(Some("Müller Public Auction"), item["name"].as_str());
    assert_eq!(Some("Public Operator"), item["operator"]["name"].as_str());
    assert!(item.get("referralConfiguration").is_none());
    assert!(item.get("ingestionMethods").is_none());
    assert!(item.get("operatorPartyId").is_none());

    let detail_response = client
        .get(format!(
            "{}/api/v1/listing-sources/by-slug/{slug_id}",
            AURA_API.base_url()
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("get public ListingSource slug detail: {error}"));
    assert_eq!(reqwest::StatusCode::OK, detail_response.status());
    assert_eq!(
        Some("no-store"),
        detail_response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
    );
    let (_, detail) = json_response(detail_response).await;
    assert_eq!(item, &detail);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_supplied_authentication_for_public_listing_source_collection() {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/listing-sources?query=mu",
            AURA_API.base_url()
        ))
        .bearer_auth("invalid-token")
        .send()
        .await
        .unwrap_or_else(|error| panic!("reject invalid public ListingSource search auth: {error}"));
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
async fn should_return_terminal_empty_public_collection_for_insufficient_query() {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/listing-sources?query=a",
            AURA_API.base_url()
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("get insufficient public ListingSource query: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(0), body["items"].as_array().map(Vec::len));
    assert_eq!(json!(21), body["size"]);
    assert!(body.get("searchAfter").is_none());
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_malformed_public_listing_source_collection_queries() {
    let client = reqwest::Client::new();

    for query in ["unexpected=x", "query=a&query=b"] {
        let response = client
            .get(format!(
                "{}/api/v1/listing-sources?{query}",
                AURA_API.base_url()
            ))
            .send()
            .await
            .unwrap_or_else(|error| panic!("reject malformed public ListingSource query: {error}"));
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
            "BAD_QUERY_PARAMETER_VALUE",
        );
        assert_eq!(Some("no-store".to_owned()), cache_control);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_page_all_public_listing_source_search_results_without_duplicates() {
    let mut expected_ids = Vec::new();
    for suffix in ["A", "B", "C", "D", "E"] {
        let (listing_source_id, _, _) = seed_listing_source_for_search(
            &format!("Mu Public Page {suffix}"),
            "Public Page Operator",
            "WEB_CRAWL",
            None,
        )
        .await;
        expected_ids.push(listing_source_id.to_string());
    }
    let client = reqwest::Client::new();
    let mut received_ids = Vec::new();
    let mut search_after: Option<String> = None;

    loop {
        let mut url = format!(
            "{}/api/v1/listing-sources?query=mu&size=2",
            AURA_API.base_url()
        );
        if let Some(cursor) = &search_after {
            url.push_str("&searchAfter=");
            url.push_str(cursor);
        }
        let response = client
            .get(url)
            .send()
            .await
            .unwrap_or_else(|error| panic!("get public ListingSource page: {error}"));
        assert_eq!(reqwest::StatusCode::OK, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(reqwest::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        let (_, body) = json_response(response).await;
        received_ids.extend(
            body["items"]
                .as_array()
                .unwrap_or_else(|| panic!("public ListingSource response must contain items"))
                .iter()
                .map(|item| {
                    item["listingSourceId"]
                        .as_str()
                        .unwrap_or_else(|| panic!("public ListingSource item must contain an ID"))
                        .to_owned()
                }),
        );
        search_after = body["searchAfter"].as_str().map(str::to_owned);
        if search_after.is_none() {
            break;
        }
    }

    assert_eq!(expected_ids, received_ids);
    let unique_ids = received_ids
        .iter()
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(received_ids.len(), unique_ids.len());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_supplied_authentication_for_public_listing_source_slug() {
    let (_, _, slug_id) = seed_listing_source_for_search(
        "Public Slug Authentication",
        "Public Slug Operator",
        "WEB_CRAWL",
        None,
    )
    .await;
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/listing-sources/by-slug/{slug_id}",
            AURA_API.base_url()
        ))
        .bearer_auth("invalid-token")
        .send()
        .await
        .unwrap_or_else(|error| panic!("reject invalid public ListingSource slug auth: {error}"));
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
async fn should_return_public_slug_route_errors_with_no_store() {
    let client = reqwest::Client::new();
    let missing = client
        .get(format!(
            "{}/api/v1/listing-sources/by-slug/missing-listing-source",
            AURA_API.base_url()
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("get missing public ListingSource slug: {error}"));
    let missing_cache_control = missing
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (missing_status, missing_body) = json_response(missing).await;
    assert_problem(
        missing_status,
        &missing_body,
        reqwest::StatusCode::NOT_FOUND,
        "LISTING_SOURCE_NOT_FOUND",
    );
    assert_eq!(Some("no-store".to_owned()), missing_cache_control);

    let unexpected_query = client
        .get(format!(
            "{}/api/v1/listing-sources/by-slug/missing-listing-source?anything=x",
            AURA_API.base_url()
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("reject public ListingSource slug query: {error}"));
    let unexpected_query_cache_control = unexpected_query
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (unexpected_query_status, unexpected_query_body) = json_response(unexpected_query).await;
    assert_problem(
        unexpected_query_status,
        &unexpected_query_body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_QUERY_PARAMETER_VALUE",
    );
    assert_eq!(Some("no-store".to_owned()), unexpected_query_cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_unused_listing_source_and_reject_a_repeat() {
    let listing_source_id = seed_listing_source().await;
    let unrelated_listing_source_id = seed_listing_source().await;
    let pool = get_postgres_client().await;
    let partnership_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT partnership_id FROM partnerships ORDER BY partnership_id LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to find partnership for grant: {error}"));
    sqlx::query(
        "INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)",
    )
    .bind(partnership_id)
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed listing-source grant: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let path = format!(
        "{}/api/v1/admin/listing-sources/{listing_source_id}",
        AURA_API.base_url()
    );

    let product_count_before_delete = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM product_listings WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("count eligible source ProductListings: {error}"));
    assert_eq!(0, product_count_before_delete);

    let response = client
        .delete(&path)
        .bearer_auth(token.clone())
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete listing source: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert_eq!(
        Some("no-store"),
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
    );
    assert!(
        response
            .bytes()
            .await
            .unwrap_or_else(|error| panic!("failed to read deletion response: {error}"))
            .is_empty()
    );

    let source_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to count deleted listing source: {error}"));
    let method_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM listing_source_ingestion_methods WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to count deleted ingestion methods: {error}"));
    let product_count_after_delete = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM product_listings WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("count deleted source ProductListings: {error}"));
    let grant_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM partnership_listing_source_grants WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to count deleted grants: {error}"));
    assert_eq!(0, source_count);
    assert_eq!(0, method_count);
    assert_eq!(0, product_count_after_delete);
    assert_eq!(0, grant_count);
    let unrelated_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM listing_sources WHERE listing_source_id = $1)",
    )
    .bind(unrelated_listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to check unrelated source: {error}"));
    assert!(unrelated_exists);

    let detail = client
        .get(&path)
        .bearer_auth(token.clone())
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to read deleted listing source: {error}"));
    let (detail_status, detail_body) = json_response(detail).await;
    assert_problem(
        detail_status,
        &detail_body,
        reqwest::StatusCode::NOT_FOUND,
        "LISTING_SOURCE_NOT_FOUND",
    );

    let repeated = client
        .delete(path)
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to repeat listing source deletion: {error}"));
    let (repeated_status, repeated_body) = json_response(repeated).await;
    assert_problem(
        repeated_status,
        &repeated_body,
        reqwest::StatusCode::NOT_FOUND,
        "LISTING_SOURCE_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_preserve_live_and_withdrawn_product_listing_source_dependencies_when_delete_is_blocked()
 {
    let product_listing_id = seed_product().await;
    let pool = get_postgres_client().await;
    let listing_source_id = ListingSourceId::try_from(
        sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT listing_source_id FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(product_listing_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to find product listing source: {error}")),
    )
    .unwrap_or_else(|error| panic!("ProductListing fixture must reference UUIDv7: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let delete_path = format!(
        "{}/api/v1/admin/listing-sources/{listing_source_id}",
        AURA_API.base_url()
    );

    let response = reqwest::Client::new()
        .delete(&delete_path)
        .bearer_auth(token.clone())
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete used listing source: {error}"));
    let (status, body) = json_response(response).await;
    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");

    sqlx::query(
        "UPDATE product_listings SET lifecycle = 'WITHDRAWN', availability = NULL, projection_version = projection_version + 1 WHERE product_listing_id = $1",
    )
    .bind(product_listing_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("withdraw protected product listing: {error}"));
    let response = reqwest::Client::new()
        .delete(delete_path)
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to delete source retained by withdrawn product: {error}")
        });
    let (status, body) = json_response(response).await;
    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");

    let source_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM listing_sources WHERE listing_source_id = $1)",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to check blocked source: {error}"));
    let product = sqlx::query_as::<_, (String, Option<String>, i64)>(
        "SELECT lifecycle, availability, projection_version FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(product_listing_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to check protected product: {error}"));
    assert!(source_exists);
    assert_eq!("WITHDRAWN", product.0);
    assert_eq!(None, product.1);
    assert!(product.2 > 1);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_safe_listing_source_summary_for_admin_with_no_store_cache_control() {
    let (listing_source_id, operator_party_id, listing_source_slug_id) =
        seed_listing_source_for_search(
            "Referral Listing Source",
            "Referral Operator",
            "WOOCOMMERCE",
            Some(json!({"kind": "PARTNERIZE", "camref": "campaign123"})),
        )
        .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/listing-sources",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .query(&[("listingSourceId", listing_source_id.to_string())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to search listing sources API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(listing_source_id.to_string()),
        body["items"][0]["listingSourceId"]
    );
    assert_eq!(
        json!(listing_source_slug_id),
        body["items"][0]["listingSourceSlugId"]
    );
    assert_eq!(json!("Referral Listing Source"), body["items"][0]["name"]);
    assert_eq!(
        json!(operator_party_id.to_string()),
        body["items"][0]["operator"]["partyId"]
    );
    assert_eq!(
        json!(format!("api-search-party-{}", operator_party_id.as_uuid())),
        body["items"][0]["operator"]["partySlugId"]
    );
    assert_eq!(
        json!("Referral Operator"),
        body["items"][0]["operator"]["name"]
    );
    assert_eq!(json!(["WOOCOMMERCE"]), body["items"][0]["ingestionMethods"]);
    assert_eq!(
        json!("https://listing-source-search.example/"),
        body["items"][0]["presentation"]["url"]
    );
    assert_eq!(
        json!("https://listing-source-search.example/image.jpg"),
        body["items"][0]["presentation"]["image"]
    );
    assert_eq!(
        json!({"type": "PARTNERIZE", "camref": "campaign123"}),
        body["items"][0]["referralConfiguration"]
    );
    assert_eq!(json!(21), body["size"]);
    assert!(body.get("searchAfter").is_none());

    let serialized = body.to_string();
    assert!(!serialized.contains("webhookSecret"));
    assert!(!serialized.contains("crawler"));
    assert!(!serialized.contains("provider-secret"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_filter_listing_sources_by_text_name_operator_method_and_exact_identity() {
    let (matching_id, matching_operator_id, matching_slug) = seed_listing_source_for_search(
        "Filtered Listing Source",
        "Matching Operator",
        "SHOPIFY",
        None,
    )
    .await;
    let (other_id, other_operator_id, _) = seed_listing_source_for_search(
        "Different Listing Source",
        "Other Operator",
        "PARTNER_API",
        None,
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/admin/listing-sources", AURA_API.base_url());

    for (field, value) in [
        ("query", "Filtered".to_owned()),
        ("name", "Filtered Listing".to_owned()),
        ("operatorPartyId", matching_operator_id.to_string()),
        ("ingestionMethod", "SHOPIFY".to_owned()),
        ("listingSourceId", matching_id.to_string()),
        ("listingSourceSlugId", matching_slug.clone()),
    ] {
        let response = client
            .get(&url)
            .bearer_auth(token.clone())
            .query(&[(field, value)])
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to filter listing sources by {field}: {error}"));
        let (status, body) = json_response(response).await;

        assert_eq!(reqwest::StatusCode::OK, status, "filter {field}");
        assert_eq!(
            Some(1),
            body["items"].as_array().map(Vec::len),
            "filter {field}"
        );
        assert_eq!(
            json!(matching_id.to_string()),
            body["items"][0]["listingSourceId"],
            "filter {field}"
        );
    }

    let response = client
        .get(&url)
        .bearer_auth(token)
        .query(&[("operatorPartyId", other_operator_id.to_string())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to check nonmatching operator filter: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(other_id.to_string()),
        body["items"][0]["listingSourceId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_follow_listing_source_cursor_with_deterministic_sorting() {
    let (first_id, _, _) = seed_listing_source_for_search(
        "Cursor Listing Source A",
        "Cursor Operator A",
        "PARTNER_API",
        None,
    )
    .await;
    let (second_id, _, _) = seed_listing_source_for_search(
        "Cursor Listing Source B",
        "Cursor Operator B",
        "PARTNER_API",
        None,
    )
    .await;
    let (third_id, _, _) = seed_listing_source_for_search(
        "Cursor Listing Source C",
        "Cursor Operator C",
        "PARTNER_API",
        None,
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/admin/listing-sources", AURA_API.base_url());

    let first = client
        .get(&url)
        .bearer_auth(token.clone())
        .query(&[
            ("name", "Cursor Listing Source"),
            ("sort", "name"),
            ("order", "asc"),
            ("size", "2"),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get first listing-source page: {error}"));
    let (first_status, first_body) = json_response(first).await;

    assert_eq!(reqwest::StatusCode::OK, first_status);
    assert_eq!(Some(2), first_body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(first_id.to_string()),
        first_body["items"][0]["listingSourceId"]
    );
    assert_eq!(
        json!(second_id.to_string()),
        first_body["items"][1]["listingSourceId"]
    );
    let cursor = first_body["searchAfter"]
        .as_str()
        .unwrap_or_else(|| panic!("missing listing-source search cursor"))
        .to_owned();

    let second = client
        .get(&url)
        .bearer_auth(token)
        .query(&[
            ("name", "Cursor Listing Source"),
            ("sort", "name"),
            ("order", "asc"),
            ("size", "2"),
            ("searchAfter", cursor.as_str()),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get second listing-source page: {error}"));
    let (second_status, second_body) = json_response(second).await;

    assert_eq!(reqwest::StatusCode::OK, second_status);
    assert_eq!(Some(1), second_body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(third_id.to_string()),
        second_body["items"][0]["listingSourceId"]
    );
    assert!(second_body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_empty_listing_source_collection_when_no_source_matches() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/listing-sources",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .query(&[("name", "missing-listing-source")])
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to search empty listing-source collection: {error}")
        });
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(0), body["items"].as_array().map(Vec::len));
    assert!(body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_listing_source_search_query_values() {
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/admin/listing-sources", AURA_API.base_url());

    let invalid_sort = client
        .get(&url)
        .bearer_auth(token.clone())
        .query(&[("sort", "invalid"), ("order", "asc")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate listing-source sort: {error}"));
    let (status, body) = json_response(invalid_sort).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_SORT_VALUE",
    );

    for (field, value) in [
        ("size", "not-a-number"),
        ("listingSourceSlugId", "Not-A-Slug"),
        ("ingestionMethod", "UNKNOWN"),
    ] {
        let response = client
            .get(&url)
            .bearer_auth(token.clone())
            .query(&[(field, value)])
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to validate listing-source {field}: {error}"));
        let (status, body) = json_response(response).await;
        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "BAD_QUERY_PARAMETER_VALUE",
        );
    }

    let listing_source_id = ListingSourceId::new();
    let party_id = PartyId::new();
    for (field, values) in [
        (
            "searchAfter",
            [
                ProductListingId::new().to_string(),
                listing_source_id.as_uuid().to_string(),
                "ls_not-a-typeid".to_owned(),
            ],
        ),
        (
            "listingSourceId",
            [
                ProductListingId::new().to_string(),
                listing_source_id.as_uuid().to_string(),
                "ls_not-a-typeid".to_owned(),
            ],
        ),
        (
            "operatorPartyId",
            [
                UserId::new().to_string(),
                party_id.as_uuid().to_string(),
                "pty_not-a-typeid".to_owned(),
            ],
        ),
    ] {
        for value in values {
            let response = client
                .get(&url)
                .bearer_auth(token.clone())
                .query(&[(field, value)])
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("failed to validate listing-source {field}: {error}")
                });
            let (status, body) = json_response(response).await;
            assert_problem(
                status,
                &body,
                reqwest::StatusCode::BAD_REQUEST,
                "INVALID_OBJECT_ID",
            );
            assert_eq!(json!({"field": field, "type": "QUERY"}), body["source"]);
        }
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_listing_source_collection_for_non_admin() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/listing-sources",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to reject non-admin listing-source search: {error}")
        });
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_listing_source_detail_for_admin_without_provider_secrets() {
    let (listing_source_id, operator_party_id, listing_source_slug_id) =
        seed_listing_source_for_search(
            "Detailed Listing Source",
            "Detailed Listing Operator",
            "WOOCOMMERCE",
            None,
        )
        .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/listing-sources/{listing_source_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get listing-source detail: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(
        json!(listing_source_id.to_string()),
        body["listingSourceId"]
    );
    assert_eq!(json!(listing_source_slug_id), body["listingSourceSlugId"]);
    assert_eq!(json!("Detailed Listing Source"), body["name"]);
    assert_eq!(
        json!(operator_party_id.to_string()),
        body["operator"]["partyId"]
    );
    assert_eq!(
        json!(format!("api-search-party-{}", operator_party_id.as_uuid())),
        body["operator"]["partySlugId"]
    );
    assert_eq!(json!("Detailed Listing Operator"), body["operator"]["name"]);
    assert_eq!(json!(["WOOCOMMERCE"]), body["ingestionMethods"]);
    assert_eq!(json!("https://listing-source-search.example/"), body["url"]);
    assert_eq!(
        json!("https://listing-source-search.example/image.jpg"),
        body["image"]
    );
    assert!(body["created"].as_str().is_some());
    assert!(body["updated"].as_str().is_some());

    let serialized = body.to_string();
    assert!(!serialized.contains("webhookSecret"));
    assert!(!serialized.contains("provider-secret"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_noncanonical_listing_source_detail_ids() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let listing_source_id = ListingSourceId::new();

    for invalid_id in [
        ProductListingId::new().to_string(),
        listing_source_id.as_uuid().to_string(),
        "ls_not-a-typeid".to_owned(),
    ] {
        let response = reqwest::Client::new()
            .get(format!(
                "{}/api/v1/admin/listing-sources/{invalid_id}",
                AURA_API.base_url()
            ))
            .bearer_auth(String::from(token.clone()))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to validate listing-source detail ID: {error}"));
        let (status, body) = json_response(response).await;

        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "INVALID_OBJECT_ID",
        );
        assert_eq!(
            json!({"field": "listingSourceId", "type": "PATH"}),
            body["source"]
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_for_missing_listing_source_detail() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/listing-sources/{}",
            AURA_API.base_url(),
            ListingSourceId::new()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get missing listing-source detail: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "LISTING_SOURCE_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_admin_for_listing_source_detail() {
    let listing_source_id = seed_listing_source().await;
    let client = reqwest::Client::new();
    let path = format!(
        "{}/api/v1/admin/listing-sources/{listing_source_id}",
        AURA_API.base_url()
    );

    let missing = client.get(&path).send().await.unwrap_or_else(|error| {
        panic!("failed to reject unauthenticated listing-source detail: {error}")
    });
    let (missing_status, missing_body) = json_response(missing).await;
    assert_problem(
        missing_status,
        &missing_body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );

    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;
    let non_admin = client
        .get(path)
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to reject non-admin listing-source detail: {error}")
        });
    let (non_admin_status, non_admin_body) = json_response(non_admin).await;
    assert_problem(
        non_admin_status,
        &non_admin_body,
        reqwest::StatusCode::FORBIDDEN,
        "FORBIDDEN",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_listing_source_for_existing_party_at_admin_route() {
    let party_id = seed_party("Existing Listing Source Operator", None, None).await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/admin/listing-sources", AURA_API.base_url());

    let response = client
        .post(&url)
        .bearer_auth(token.clone())
        .json(&json!({
            "name": "Created Listing Source",
            "operator": {"type": "EXISTING", "partyId": party_id.to_string()},
            "ingestionConfiguration": [{"type": "PARTNER_API"}],
            "url": "https://created-listing-source.example/",
            "image": "https://created-listing-source.example/image.png",
            "referralConfiguration": {"type": "PARTNERIZE", "camref": "campaign123"}
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create listing source: {error}"));
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;

    let listing_source_id = body["listingSourceId"]
        .as_str()
        .unwrap_or_else(|| panic!("created response has no listing source ID"))
        .to_owned();
    assert!(listing_source_id.parse::<ListingSourceId>().is_ok());
    assert!(listing_source_id.starts_with("ls_"));
    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(
        Some(format!("/api/v1/admin/listing-sources/{listing_source_id}")),
        location
    );
    assert_eq!(json!(listing_source_id.as_str()), body["listingSourceId"]);
    assert!(body["listingSourceSlugId"].is_string());

    let response = client
        .get(&url)
        .bearer_auth(token)
        .query(&[("listingSourceId", listing_source_id.as_str())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to read created listing source: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(party_id.to_string()),
        body["items"][0]["operator"]["partyId"]
    );
    assert_eq!(json!("Created Listing Source"), body["items"][0]["name"]);
    assert_eq!(json!(["PARTNER_API"]), body["items"][0]["ingestionMethods"]);
    assert_eq!(
        json!({"type": "PARTNERIZE", "camref": "campaign123"}),
        body["items"][0]["referralConfiguration"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_noncanonical_party_ids_in_listing_source_body() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let party_id = PartyId::new();

    for invalid_id in [
        UserId::new().to_string(),
        party_id.as_uuid().to_string(),
        "pty_not-a-typeid".to_owned(),
    ] {
        let response = reqwest::Client::new()
            .post(format!(
                "{}/api/v1/admin/listing-sources",
                AURA_API.base_url()
            ))
            .bearer_auth(String::from(token.clone()))
            .json(&json!({
                "name": "Invalid Existing Operator",
                "operator": {"type": "EXISTING", "partyId": invalid_id},
                "ingestionConfiguration": [{"type": "PARTNER_API"}]
            }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to validate operator Party ID: {error}"));
        let (status, body) = json_response(response).await;

        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "INVALID_OBJECT_ID",
        );
        assert_eq!(
            json!({"field": "operator.partyId", "type": "BODY"}),
            body["source"]
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_listing_source_with_new_party_without_echoing_webhook_secret() {
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/admin/listing-sources", AURA_API.base_url());

    let response = client
        .post(&url)
        .bearer_auth(token.clone())
        .json(&json!({
            "name": "  WooCommerce Listing Source  ",
            "operator": {
                "type": "NEW",
                "name": "  New Listing Source Operator  ",
                "phone": "+49 30 123456",
                "email": "operator@example.test"
            },
            "ingestionConfiguration": [{
                "type": "WOOCOMMERCE",
                "currency": "EUR",
                "language": "en"
            }],
            "woocommerceWebhookSecret": "provider-secret"
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create listing source with new party: {error}"));
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;

    let listing_source_id = body["listingSourceId"]
        .as_str()
        .unwrap_or_else(|| panic!("created response has no listing source ID"))
        .to_owned();
    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(
        Some(format!("/api/v1/admin/listing-sources/{listing_source_id}")),
        location
    );
    assert!(!body.to_string().contains("provider-secret"));

    let response = client
        .get(&url)
        .bearer_auth(token)
        .query(&[("listingSourceId", listing_source_id.as_str())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to read created WooCommerce source: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!("New Listing Source Operator"),
        body["items"][0]["operator"]["name"]
    );
    assert_eq!(json!(["WOOCOMMERCE"]), body["items"][0]["ingestionMethods"]);
    assert!(!body.to_string().contains("provider-secret"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_listing_source_create_for_non_admin() {
    let party_id = seed_party("Unauthorized Listing Source Operator", None, None).await;
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/listing-sources",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({
            "name": "Unauthorized Listing Source",
            "operator": {"type": "EXISTING", "partyId": party_id.to_string()},
            "ingestionConfiguration": [{"type": "PARTNER_API"}]
        }))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to reject non-admin listing-source create: {error}")
        });
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_listing_source_create_with_secret_for_non_woocommerce_source() {
    let party_id = seed_party("Invalid Secret Listing Source Operator", None, None).await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/listing-sources",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({
            "name": "Invalid Secret Listing Source",
            "operator": {"type": "EXISTING", "partyId": party_id.to_string()},
            "ingestionConfiguration": [{"type": "PARTNER_API"}],
            "woocommerceWebhookSecret": "must-not-be-stored"
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate listing-source secret: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
    assert!(!body.to_string().contains("must-not-be-stored"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_map_duplicate_shopify_domain_to_conflict_on_admin_create() {
    let party_id = seed_party("Shopify Conflict Operator", None, None).await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let url = format!("{}/api/v1/admin/listing-sources", AURA_API.base_url());
    let client = reqwest::Client::new();

    let response = client
        .post(&url)
        .bearer_auth(token.clone())
        .json(&json!({
            "name": "First Shopify Listing Source",
            "operator": {"type": "EXISTING", "partyId": party_id.to_string()},
            "ingestionConfiguration": [{
                "type": "SHOPIFY",
                "domain": "duplicate-shop.example",
                "currency": "EUR",
                "language": "en"
            }]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create first Shopify source: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, response.status());

    let response = client
        .post(&url)
        .bearer_auth(token)
        .json(&json!({
            "name": "Second Shopify Listing Source",
            "operator": {"type": "EXISTING", "partyId": party_id.to_string()},
            "ingestionConfiguration": [{
                "type": "SHOPIFY",
                "domain": "duplicate-shop.example",
                "currency": "EUR",
                "language": "en"
            }]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to test duplicate Shopify source: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_listing_source_at_admin_route_with_tri_state_patch() {
    let listing_source_id = seed_listing_source().await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let path = format!(
        "{}/api/v1/admin/listing-sources/{listing_source_id}",
        AURA_API.base_url()
    );
    let original_slug = format!("api-acceptance-source-{}", listing_source_id.as_uuid());

    let updated = client
        .patch(&path)
        .bearer_auth(token.clone())
        .json(&json!({
            "name": "  Renamed Listing Source  ",
            "ingestionConfiguration": [{
                "type": "WOOCOMMERCE",
                "currency": "EUR",
                "language": "en"
            }],
            "woocommerceWebhookSecret": "provider-secret",
            "url": "https://updated-listing-source.example/",
            "image": "https://updated-listing-source.example/image.jpg",
            "referralConfiguration": {"type": "PARTNERIZE", "camref": "campaign123"}
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to update ListingSource: {error}"));
    let (updated_status, updated_body) = json_response(updated).await;

    assert_eq!(reqwest::StatusCode::OK, updated_status);
    assert_eq!(
        json!(listing_source_id.to_string()),
        updated_body["listingSourceId"]
    );
    assert_eq!(json!(original_slug), updated_body["listingSourceSlugId"]);
    assert!(!updated_body.to_string().contains("provider-secret"));

    let detail = client
        .get(&path)
        .bearer_auth(token.clone())
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to read updated ListingSource: {error}"));
    let (detail_status, detail_body) = json_response(detail).await;
    assert_eq!(reqwest::StatusCode::OK, detail_status);
    assert_eq!(json!("Renamed Listing Source"), detail_body["name"]);
    assert_eq!(json!(original_slug), detail_body["listingSourceSlugId"]);
    assert_eq!(json!(["WOOCOMMERCE"]), detail_body["ingestionMethods"]);
    assert_eq!(
        json!("https://updated-listing-source.example/"),
        detail_body["url"]
    );
    assert_eq!(
        json!("https://updated-listing-source.example/image.jpg"),
        detail_body["image"]
    );
    assert!(!detail_body.to_string().contains("provider-secret"));

    let summary = client
        .get(format!(
            "{}/api/v1/admin/listing-sources",
            AURA_API.base_url()
        ))
        .bearer_auth(token.clone())
        .query(&[("listingSourceId", listing_source_id.to_string())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to search updated ListingSource: {error}"));
    let (summary_status, summary_body) = json_response(summary).await;
    assert_eq!(reqwest::StatusCode::OK, summary_status);
    assert_eq!(
        json!({"type": "PARTNERIZE", "camref": "campaign123"}),
        summary_body["items"][0]["referralConfiguration"]
    );

    let cleared = client
        .patch(&path)
        .bearer_auth(token.clone())
        .json(&json!({
            "woocommerceWebhookSecret": null,
            "url": null,
            "image": null,
            "referralConfiguration": null
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to clear ListingSource values: {error}"));
    let (cleared_status, cleared_body) = json_response(cleared).await;
    assert_eq!(reqwest::StatusCode::OK, cleared_status);
    assert_eq!(updated_body, cleared_body);
    assert!(!cleared_body.to_string().contains("provider-secret"));

    let cleared_detail = client
        .get(&path)
        .bearer_auth(token.clone())
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to read cleared ListingSource: {error}"));
    let (cleared_detail_status, cleared_detail_body) = json_response(cleared_detail).await;
    assert_eq!(reqwest::StatusCode::OK, cleared_detail_status);
    assert_eq!(json!("Renamed Listing Source"), cleared_detail_body["name"]);
    assert_eq!(
        json!(original_slug),
        cleared_detail_body["listingSourceSlugId"]
    );
    assert_eq!(
        json!(["WOOCOMMERCE"]),
        cleared_detail_body["ingestionMethods"]
    );
    assert!(cleared_detail_body.get("url").is_none());
    assert!(cleared_detail_body.get("image").is_none());
    assert!(!cleared_detail_body.to_string().contains("provider-secret"));

    let no_op = client
        .patch(&path)
        .bearer_auth(token)
        .json(&json!({}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to apply ListingSource no-op patch: {error}"));
    let (no_op_status, no_op_body) = json_response(no_op).await;
    assert_eq!(reqwest::StatusCode::OK, no_op_status);
    assert_eq!(cleared_body, no_op_body);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_listing_source_update_when_configuration_is_invalid() {
    let listing_source_id = seed_listing_source().await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let path = format!(
        "{}/api/v1/admin/listing-sources/{listing_source_id}",
        AURA_API.base_url()
    );

    for request in [
        json!({
            "ingestionConfiguration": [{"type": "PARTNER_API"}, {"type": "PARTNER_API"}]
        }),
        json!({"woocommerceWebhookSecret": "must-have-woocommerce"}),
    ] {
        let response = client
            .patch(&path)
            .bearer_auth(token.clone())
            .json(&request)
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to validate ListingSource update: {error}"));
        let (status, body) = json_response(response).await;
        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "BAD_BODY_VALUE",
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_map_listing_source_update_provider_conflict_to_conflict() {
    let party_id = seed_party("ListingSource Update Conflict Operator", None, None).await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let create_path = format!("{}/api/v1/admin/listing-sources", AURA_API.base_url());
    let duplicate_domain = "duplicate-update-shop.example";

    let first = client
        .post(&create_path)
        .bearer_auth(token.clone())
        .json(&json!({
            "name": "First Update Conflict ListingSource",
            "operator": {"type": "EXISTING", "partyId": party_id.to_string()},
            "ingestionConfiguration": [{
                "type": "SHOPIFY",
                "domain": duplicate_domain
            }]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create first Shopify ListingSource: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, first.status());

    let second = client
        .post(&create_path)
        .bearer_auth(token.clone())
        .json(&json!({
            "name": "Second Update Conflict ListingSource",
            "operator": {"type": "EXISTING", "partyId": party_id.to_string()},
            "ingestionConfiguration": [{
                "type": "SHOPIFY",
                "domain": "other-update-shop.example"
            }]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create second Shopify ListingSource: {error}"));
    let (second_status, second_body) = json_response(second).await;
    assert_eq!(reqwest::StatusCode::CREATED, second_status);
    let second_id = second_body["listingSourceId"]
        .as_str()
        .unwrap_or_else(|| panic!("second ListingSource response has no ID"))
        .to_owned();

    let response = client
        .patch(format!(
            "{}/api/v1/admin/listing-sources/{second_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(token)
        .json(&json!({
            "ingestionConfiguration": [{
                "type": "SHOPIFY",
                "domain": duplicate_domain
            }]
        }))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to update conflicting Shopify ListingSource: {error}")
        });
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_for_missing_listing_source_update() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/listing-sources/{}",
            AURA_API.base_url(),
            ListingSourceId::new()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({"name": "Missing ListingSource"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to update missing ListingSource: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "LISTING_SOURCE_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_admin_for_listing_source_update() {
    let listing_source_id = seed_listing_source().await;
    let path = format!(
        "{}/api/v1/admin/listing-sources/{listing_source_id}",
        AURA_API.base_url()
    );
    let client = reqwest::Client::new();

    let missing = client
        .patch(&path)
        .json(&json!({"name": "Unauthenticated ListingSource"}))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to reject unauthenticated ListingSource update: {error}")
        });
    let (missing_status, missing_body) = json_response(missing).await;
    assert_problem(
        missing_status,
        &missing_body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );

    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;
    let non_admin = client
        .patch(path)
        .bearer_auth(String::from(token))
        .json(&json!({"name": "Non-admin ListingSource"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin ListingSource update: {error}"));
    let (non_admin_status, non_admin_body) = json_response(non_admin).await;
    assert_problem(
        non_admin_status,
        &non_admin_body,
        reqwest::StatusCode::FORBIDDEN,
        "FORBIDDEN",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_remove_legacy_listing_source_update_route() {
    let listing_source_id = seed_listing_source().await;
    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/listing-sources/{listing_source_id}",
            AURA_API.base_url()
        ))
        .json(&json!({"name": "Legacy ListingSource"}))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to call legacy ListingSource update route: {error}")
        });

    assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_legacy_listing_source_create_method() {
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/listing-sources", AURA_API.base_url()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call legacy listing-source route: {error}"));

    assert_eq!(reqwest::StatusCode::METHOD_NOT_ALLOWED, response.status());
}
