use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    assert_problem, json_response, seed_access_token_for, seed_listing_source, seed_user,
};
use auction_core::AuctionId;
use listing_source_core::ListingSourceId;
use serde_json::json;

use test_api::{IntegrationTestService, aura_integration_test};

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_get_and_update_auction_as_administrator() {
    let source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid seeded ListingSource ID: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();

    let created = client
        .post(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&json!({
            "listingSourceId": source_id,
            "sourceAuctionId": " catalogue / 2026-0042 "
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create Auction: {error}"));
    let location = created
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let cache_control = created
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (created_status, created_body) = json_response(created).await;

    let auction_id = created_body["auctionId"]
        .as_str()
        .unwrap_or_else(|| panic!("created Auction response has no auctionId"))
        .parse::<AuctionId>()
        .unwrap_or_else(|error| panic!("created response has invalid Auction ID: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created_status);
    assert_eq!(
        Some(format!("/api/v1/admin/auctions/{auction_id}")),
        location
    );
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(
        json!(source_id.to_string()),
        created_body["listingSourceId"]
    );
    assert_eq!(
        json!("catalogue / 2026-0042"),
        created_body["sourceAuctionId"]
    );
    assert!(created_body["name"].is_null());
    assert!(created_body["description"].is_null());
    assert!(created_body["format"].is_null());
    assert_eq!(json!(1), created_body["expectedVersion"]);
    assert_eq!(json!([]), created_body["protectedFields"]);
    assert!(created_body["schedule"]["liveStarts"].is_null());

    let fetched = client
        .get(format!(
            "{}/api/v1/admin/auctions/{auction_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get Auction: {error}"));
    let fetched_cache_control = fetched
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (fetched_status, fetched_body) = json_response(fetched).await;
    assert_eq!(reqwest::StatusCode::OK, fetched_status);
    assert_eq!(Some("no-store".to_owned()), fetched_cache_control);
    assert_eq!(json!(auction_id.to_string()), fetched_body["auctionId"]);

    let updated = client
        .patch(format!(
            "{}/api/v1/admin/auctions/{auction_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .json(&json!({
            "expectedVersion": 1,
            "name": {"language": "en", "text": "Autumn Decorative Arts"},
            "format": "TIMED",
            "schedule": {
                "lotsBeginClosing": {
                    "precision": "INSTANT",
                    "at": "2026-10-18T16:03:00Z",
                    "sourceTimezone": "Europe/Berlin"
                }
            }
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to update Auction: {error}"));
    let updated_cache_control = updated
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (updated_status, updated_body) = json_response(updated).await;
    assert_eq!(reqwest::StatusCode::OK, updated_status);
    assert_eq!(Some("no-store".to_owned()), updated_cache_control);
    assert_eq!(
        json!("Autumn Decorative Arts"),
        updated_body["name"]["text"]
    );
    assert_eq!(json!("TIMED"), updated_body["format"]);
    assert_eq!(json!(2), updated_body["expectedVersion"]);
    assert_eq!(
        json!({
            "precision": "INSTANT",
            "at": "2026-10-18T16:03:00Z",
            "sourceTimezone": "Europe/Berlin"
        }),
        updated_body["schedule"]["lotsBeginClosing"]
    );
    assert_eq!(
        json!(["NAME", "FORMAT", "LOTS_BEGIN_CLOSING"]),
        updated_body["protectedFields"]
    );

    let stale = client
        .patch(format!(
            "{}/api/v1/admin/auctions/{auction_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({"expectedVersion": 1, "name": null}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to send stale Auction update: {error}"));
    let (stale_status, stale_body) = json_response(stale).await;
    assert_problem(
        stale_status,
        &stale_body,
        reqwest::StatusCode::CONFLICT,
        "CONFLICT",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_duplicate_key_invalid_id_and_non_admin_auction_requests() {
    let source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid seeded ListingSource ID: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();
    let body = json!({
        "listingSourceId": source_id,
        "sourceAuctionId": "catalogue-42"
    });

    let first = client
        .post(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(String::from(admin_token.clone()))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create first Auction: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, first.status());

    let duplicate = client
        .post(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(String::from(admin_token.clone()))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create duplicate Auction: {error}"));
    let (duplicate_status, duplicate_body) = json_response(duplicate).await;
    assert_problem(
        duplicate_status,
        &duplicate_body,
        reqwest::StatusCode::CONFLICT,
        "CONFLICT",
    );

    for invalid_id in [
        source_id.to_string(),
        AuctionId::new().as_uuid().to_string(),
        "auc_not-a-typeid".to_owned(),
    ] {
        let response = client
            .get(format!(
                "{}/api/v1/admin/auctions/{invalid_id}",
                AURA_API.base_url()
            ))
            .bearer_auth(String::from(admin_token.clone()))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to get invalid Auction ID: {error}"));
        let (status, response_body) = json_response(response).await;
        assert_problem(
            status,
            &response_body,
            reqwest::StatusCode::BAD_REQUEST,
            "INVALID_OBJECT_ID",
        );
    }

    let missing = client
        .get(format!(
            "{}/api/v1/admin/auctions/{}",
            AURA_API.base_url(),
            AuctionId::new()
        ))
        .bearer_auth(String::from(admin_token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get missing Auction: {error}"));
    let (missing_status, missing_body) = json_response(missing).await;
    assert_problem(
        missing_status,
        &missing_body,
        reqwest::StatusCode::NOT_FOUND,
        "AUCTION_NOT_FOUND",
    );

    let user_id = seed_user("USER").await;
    let user_token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;
    let forbidden = client
        .post(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(String::from(user_token))
        .json(&json!({
            "listingSourceId": source_id,
            "sourceAuctionId": "forbidden-catalogue"
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin Auction create: {error}"));
    let (forbidden_status, forbidden_body) = json_response(forbidden).await;
    assert_problem(
        forbidden_status,
        &forbidden_body,
        reqwest::StatusCode::FORBIDDEN,
        "FORBIDDEN",
    );
}
