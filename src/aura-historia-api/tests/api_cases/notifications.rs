use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    assert_problem, json_response, seed_access_token_for, seed_product, seed_user,
    seed_user_with_consent,
};
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use notification_core::notification_id::NotificationId;
use partnership_core::partnership_application_id::PartnershipApplicationId;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use serde_json::Value;
use std::collections::HashSet;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use user_core::access_token::RawAccessToken;
use user_core::user_id::UserId;

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_valid_authentication_for_notifications() {
    let client = reqwest::Client::new();

    let missing = client
        .get(notifications_path())
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list notifications without auth: {error}"));
    let (missing_status, missing_body) = json_response(missing).await;
    assert_problem(
        missing_status,
        &missing_body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );

    let invalid = client
        .get(notifications_path())
        .bearer_auth("not-an-aura-access-token")
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list notifications with invalid auth: {error}"));
    let (invalid_status, invalid_body) = json_response(invalid).await;
    assert_problem(
        invalid_status,
        &invalid_body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_list_canonical_notifications_without_legacy_fields_and_follow_cursor() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let first_notification_id = seed_notification(user_id, false).await;
    let second_notification_id = seed_notification(user_id, false).await;
    let client = reqwest::Client::new();

    let response = client
        .get(notifications_path())
        .bearer_auth(String::from(token.clone()))
        .query(&[("size", "1")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list notifications: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store"), cache_control.as_deref());
    assert_eq!(serde_json::json!(1), body["size"]);
    assert!(body.get("total").is_none());

    let item = &body["items"][0];
    let first_page_id = match item["notificationId"].as_str() {
        Some(value) => value,
        None => panic!("notification list item is missing notificationId"),
    };
    assert!(first_page_id.starts_with("ntf_"));
    assert!(
        [
            first_notification_id.to_string(),
            second_notification_id.to_string()
        ]
        .contains(&first_page_id.to_owned())
    );
    assert_eq!(serde_json::json!(false), item["seen"]);
    assert_eq!(
        serde_json::json!("PARTNERSHIP_APPLICATION_APPROVED"),
        item["kind"]
    );
    assert!(
        item["payload"]["partnershipApplicationId"]
            .as_str()
            .is_some_and(|value| value.starts_with("pa_")),
        "payload is missing canonical partnershipApplicationId"
    );
    assert_eq!(serde_json::json!("APPROVED"), item["payload"]["decision"]);
    for legacy_field in [
        "originEventId",
        "external",
        "createdBy",
        "updatedBy",
        "userId",
        "notificationType",
    ] {
        assert!(item.get(legacy_field).is_none(), "found {legacy_field}");
    }
    assert_eq!(
        serde_json::json!("Acceptance Party"),
        item["payload"]["partyName"]
    );
    assert_eq!(
        serde_json::json!("Acceptance Listing Source"),
        item["payload"]["listingSourceName"]
    );
    assert!(item["payload"].get("image").is_some());
    for internal_field in [
        "originEventId",
        "deliveryChannel",
        "deliveryStatus",
        "providerMessageId",
    ] {
        assert!(
            item["payload"].get(internal_field).is_none(),
            "found payload.{internal_field}"
        );
    }

    let cursor = match body.get("searchAfter") {
        Some(value) => value.to_string(),
        None => panic!("notification list page is missing searchAfter cursor"),
    };
    assert_eq!(serde_json::json!(first_page_id), body["searchAfter"][1]);
    assert!(
        body["searchAfter"][1]
            .as_str()
            .is_some_and(|value| value.starts_with("ntf_"))
    );
    assert!(body["searchAfter"][0].as_str().is_some());

    let response = client
        .get(notifications_path())
        .bearer_auth(String::from(token))
        .query(&[("size", "1"), ("searchAfter", cursor.as_str())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to follow notification cursor: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_ne!(
        serde_json::json!(first_page_id),
        body["items"][0]["notificationId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_paginate_notifications_without_advertising_terminal_pages() {
    let empty_user_id = seed_user("USER").await;
    let empty_token = notification_token(empty_user_id).await;
    let empty = list_notification_page(&empty_token, 2, None).await;
    assert!(empty["items"].as_array().is_some_and(Vec::is_empty));
    assert!(empty.get("searchAfter").is_none());

    let partial_user_id = seed_user("USER").await;
    let partial_token = notification_token(partial_user_id).await;
    let partial_created = OffsetDateTime::from_unix_timestamp(1_777_777_700)
        .unwrap_or_else(|error| panic!("invalid partial timestamp: {error}"));
    seed_notification_at(partial_user_id, NotificationId::new(), partial_created).await;
    let partial = list_notification_page(&partial_token, 2, None).await;
    assert_eq!(1, partial["items"].as_array().map_or(0, Vec::len));
    assert!(partial.get("searchAfter").is_none());

    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let created = OffsetDateTime::from_unix_timestamp(1_777_777_777)
        .unwrap_or_else(|error| panic!("invalid pagination timestamp: {error}"));
    let mut notification_ids = [
        NotificationId::new(),
        NotificationId::new(),
        NotificationId::new(),
    ];
    notification_ids.sort();
    let [oldest, middle, newest] = notification_ids;
    seed_notification_at(user_id, oldest, created).await;
    seed_notification_at(user_id, middle, created).await;
    seed_notification_at(user_id, newest, created).await;

    let first = list_notification_page(&token, 2, None).await;
    let first_items = match first["items"].as_array() {
        Some(items) => items,
        None => panic!("first page has no items"),
    };
    assert_eq!(2, first_items.len());
    assert_eq!(
        Some(newest.to_string()),
        first_items[0]["notificationId"].as_str().map(String::from)
    );
    assert_eq!(
        Some(middle.to_string()),
        first_items[1]["notificationId"].as_str().map(String::from)
    );
    let first_cursor = match first.get("searchAfter") {
        Some(cursor) => cursor.to_string(),
        None => panic!("full first page is missing continuation cursor"),
    };
    let first_timestamp = match first["items"][0]["created"].as_str() {
        Some(timestamp) => timestamp,
        None => panic!("first notification is missing created timestamp"),
    };
    assert!(OffsetDateTime::parse(first_timestamp, &Rfc3339).is_ok());

    let terminal = list_notification_page(&token, 2, Some(first_cursor.as_str())).await;
    let terminal_items = match terminal["items"].as_array() {
        Some(items) => items,
        None => panic!("terminal page has no items"),
    };
    assert_eq!(1, terminal_items.len());
    assert_eq!(
        Some(oldest.to_string()),
        terminal_items[0]["notificationId"]
            .as_str()
            .map(String::from)
    );
    assert!(terminal.get("searchAfter").is_none());

    let exact_user_id = seed_user("USER").await;
    let exact_token = notification_token(exact_user_id).await;
    seed_notification_at(exact_user_id, NotificationId::new(), created).await;
    seed_notification_at(exact_user_id, NotificationId::new(), created).await;
    let exact = list_notification_page(&exact_token, 2, None).await;
    assert_eq!(2, exact["items"].as_array().map_or(0, Vec::len));
    assert!(exact.get("searchAfter").is_none());

    let client = reqwest::Client::new();
    let malformed = client
        .get(notifications_path())
        .bearer_auth(String::from(token.clone()))
        .query(&[("searchAfter", "not-a-json-cursor")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list malformed cursor: {error}"));
    let (status, body) = json_response(malformed).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_QUERY_PARAMETER_VALUE",
    );

    let notification_id = NotificationId::new();
    for invalid_id in [
        "ntf_not-a-typeid".to_owned(),
        notification_id.as_uuid().to_string(),
        ProductListingId::new().to_string(),
    ] {
        let cursor = serde_json::json!([
            created.format(&Rfc3339).unwrap_or_else(|error| panic!(
                "failed to format notification cursor timestamp: {error}"
            )),
            invalid_id
        ])
        .to_string();
        let response = client
            .get(notifications_path())
            .bearer_auth(String::from(token.clone()))
            .query(&[("searchAfter", cursor)])
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to validate notification cursor: {error}"));
        let (status, body) = json_response(response).await;
        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "INVALID_OBJECT_ID",
        );
        assert_eq!(
            serde_json::json!({"field": "searchAfter", "type": "QUERY"}),
            body["source"]
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_localized_reason_specific_notification_payloads() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    seed_notification_payloads(user_id).await;

    let response = reqwest::Client::new()
        .get(notifications_path())
        .bearer_auth(String::from(token))
        .query(&[("size", "100"), ("language", "de")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list localized notifications: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    let items = match body["items"].as_array() {
        Some(items) => items,
        None => panic!("notification list response is missing items"),
    };
    let price_change = notification_with_kind(items, "WATCHLIST_PRICE_CHANGED");
    assert_eq!(
        serde_json::json!("Geigentitel"),
        price_change["payload"]["title"]["text"]
    );
    assert_eq!(
        serde_json::json!("de"),
        price_change["payload"]["title"]["language"]
    );
    assert_eq!(
        serde_json::json!("EUR"),
        price_change["payload"]["change"]["oldPrice"]["currency"]
    );
    assert_eq!(
        serde_json::json!(1000),
        price_change["payload"]["change"]["oldPrice"]["amount"]
    );
    assert_eq!(
        serde_json::json!({ "url": null }),
        price_change["payload"]["image"]
    );
    assert!(
        price_change["payload"]["productListingId"]
            .as_str()
            .is_some_and(|value| value.starts_with("pl_"))
    );
    assert!(
        price_change["payload"]["listingSourceId"]
            .as_str()
            .is_some_and(|value| value.starts_with("ls_"))
    );
    assert!(
        price_change["payload"]["sourceListingId"]
            .as_str()
            .is_some()
    );
    assert!(
        price_change["payload"]["listingSourceSlugId"]
            .as_str()
            .is_some()
    );
    assert!(
        price_change["payload"]["productListingTitleSlugId"]
            .as_str()
            .is_some()
    );
    assert!(price_change["payload"]["url"].as_str().is_some());
    assert!(price_change["payload"]["viewUrl"].as_str().is_some());

    let availability_change = notification_with_kind(items, "WATCHLIST_AVAILABILITY_CHANGED");
    assert!(availability_change["payload"]["title"].is_null());
    assert!(availability_change["payload"]["image"].is_null());
    assert_eq!(
        serde_json::json!("AVAILABLE"),
        availability_change["payload"]["change"]["oldAvailability"]
    );
    assert_eq!(
        serde_json::json!("SOLD_OUT"),
        availability_change["payload"]["change"]["newAvailability"]
    );

    let search_filter = notification_with_kind(items, "SEARCH_FILTER_MATCH");
    assert_eq!(
        serde_json::json!("Geigentitel"),
        search_filter["payload"]["title"]["text"]
    );
    assert_eq!(
        serde_json::json!("Saved Violins"),
        search_filter["payload"]["userSearchFilterName"]
    );
    assert!(
        search_filter["payload"]["productListingId"]
            .as_str()
            .is_some_and(|value| value.starts_with("pl_"))
    );
    assert!(
        search_filter["payload"]["listingSourceId"]
            .as_str()
            .is_some_and(|value| value.starts_with("ls_"))
    );
    assert!(
        search_filter["payload"]["userSearchFilterId"]
            .as_str()
            .is_some_and(|value| value.starts_with("sf_"))
    );

    let approved = notification_with_kind(items, "PARTNERSHIP_APPLICATION_APPROVED");
    assert_eq!(
        serde_json::json!("APPROVED"),
        approved["payload"]["decision"]
    );
    assert!(
        approved["payload"]["partnershipApplicationId"]
            .as_str()
            .is_some_and(|value| value.starts_with("pa_"))
    );
    assert_eq!(
        serde_json::json!("Approved Party"),
        approved["payload"]["partyName"]
    );
    assert_eq!(
        serde_json::json!("Approved Listing Source"),
        approved["payload"]["listingSourceName"]
    );
    assert_eq!(
        serde_json::json!("https://listing-source.example/approved.jpg"),
        approved["payload"]["image"]
    );

    let rejected = notification_with_kind(items, "PARTNERSHIP_APPLICATION_REJECTED");
    assert_eq!(
        serde_json::json!("REJECTED"),
        rejected["payload"]["decision"]
    );
    assert!(
        rejected["payload"]["partnershipApplicationId"]
            .as_str()
            .is_some_and(|value| value.starts_with("pa_"))
    );
    assert_eq!(
        serde_json::json!("Rejected Party"),
        rejected["payload"]["partyName"]
    );
    assert_eq!(
        serde_json::json!("Rejected Listing Source"),
        rejected["payload"]["listingSourceName"]
    );
    assert!(rejected["payload"]["image"].is_null());

    for item in items {
        assert!(
            item["notificationId"]
                .as_str()
                .is_some_and(|value| value.starts_with("ntf_"))
        );
        assert!(item.get("originEventId").is_none());
        assert!(item.get("deliveryChannel").is_none());
        assert!(item.get("deliveryStatus").is_none());
        assert!(item.get("providerMessageId").is_none());
        assert!(
            item["created"]
                .as_str()
                .is_some_and(|value| value.ends_with('Z'))
        );
        assert!(
            item["updated"]
                .as_str()
                .is_some_and(|value| value.ends_with('Z'))
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_present_notification_images_from_immutable_snapshot_and_current_preference() {
    let user_id = seed_user_with_consent("USER", false).await;
    let token = notification_token(user_id).await;
    let product_listing_id = seed_product().await;

    set_current_assessment(product_listing_id, "REQUIRES_CONSENT", Some("NAZI_GERMANY")).await;
    seed_unsafe_image_notification(
        user_id,
        product_listing_id,
        serde_json::json!({ "decision": "ALLOWED", "category": null }),
    )
    .await;
    let allowed_snapshot = list_notification_page(&token, 10, None).await;
    assert_eq!(
        serde_json::json!({ "url": "https://unsafe.listing-source.example/image.jpg" }),
        allowed_snapshot["items"][0]["payload"]["image"]
    );

    set_current_assessment(product_listing_id, "ALLOWED", None).await;
    seed_unsafe_image_notification(
        user_id,
        product_listing_id,
        serde_json::json!({ "decision": "REQUIRES_CONSENT", "category": "NAZI_GERMANY" }),
    )
    .await;
    seed_unsafe_image_notification(user_id, product_listing_id, Value::Null).await;

    let hidden = list_notification_page(&token, 10, None).await;
    let hidden_images = hidden["items"]
        .as_array()
        .unwrap_or_else(|| panic!("notification items"))
        .iter()
        .map(|item| item["payload"]["image"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        1,
        hidden_images
            .iter()
            .filter(|image| **image
                == serde_json::json!({ "url": "https://unsafe.listing-source.example/image.jpg" }))
            .count()
    );
    assert_eq!(
        2,
        hidden_images
            .iter()
            .filter(|image| **image == serde_json::json!({ "url": null }))
            .count()
    );

    set_user_content_preference(user_id, true).await;
    let visible = list_notification_page(&token, 10, None).await;
    for item in visible["items"]
        .as_array()
        .unwrap_or_else(|| panic!("notification items"))
    {
        assert_eq!(
            serde_json::json!({ "url": "https://unsafe.listing-source.example/image.jpg" }),
            item["payload"]["image"]
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_noncanonical_notification_ids() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let notification_id = NotificationId::new();

    for invalid_id in [
        "ntf_not-a-typeid".to_owned(),
        notification_id.as_uuid().to_string(),
        ProductListingId::new().to_string(),
    ] {
        let response = reqwest::Client::new()
            .patch(format!("{}/{invalid_id}", notifications_path()))
            .bearer_auth(String::from(token.clone()))
            .json(&serde_json::json!({"seen": true}))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to validate notification ID: {error}"));
        let (status, body) = json_response(response).await;

        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "INVALID_OBJECT_ID",
        );
        assert_eq!(
            serde_json::json!({"field": "notificationId", "type": "PATH"}),
            body["source"]
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_empty_notification_batch() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;

    let response = reqwest::Client::new()
        .patch(notifications_path())
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"notificationIds": [], "seen": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch empty notification batch: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_one_notification_seen_state() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let notification_id = seed_notification(user_id, false).await;

    let response = reqwest::Client::new()
        .patch(notification_path(notification_id))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"seen": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch notification: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert!(notification_seen(&token, notification_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_selected_notification_seen_states() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let first_notification_id = seed_notification(user_id, false).await;
    let second_notification_id = seed_notification(user_id, false).await;

    let response = reqwest::Client::new()
        .patch(notifications_path())
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({
            "notificationIds": [first_notification_id, second_notification_id],
            "seen": true,
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch selected notifications: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert!(notification_seen(&token, first_notification_id).await);
    assert!(notification_seen(&token, second_notification_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_all_notification_seen_states() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let first_notification_id = seed_notification(user_id, false).await;
    let second_notification_id = seed_notification(user_id, true).await;

    let response = reqwest::Client::new()
        .patch(format!("{}/all", notifications_path()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"seen": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch all notifications: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert!(notification_seen(&token, first_notification_id).await);
    assert!(notification_seen(&token, second_notification_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_one_notification() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let deleted_notification_id = seed_notification(user_id, false).await;
    let retained_notification_id = seed_notification(user_id, false).await;

    let response = reqwest::Client::new()
        .delete(notification_path(deleted_notification_id))
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete notification: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    let notification_ids = listed_notification_ids(&token).await;
    assert!(!notification_ids.contains(&deleted_notification_id.to_string()));
    assert!(notification_ids.contains(&retained_notification_id.to_string()));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_all_notifications() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let _ = seed_notification(user_id, false).await;
    let _ = seed_notification(user_id, false).await;

    let response = reqwest::Client::new()
        .delete(notifications_path())
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete all notifications: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert!(listed_notification_ids(&token).await.is_empty());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_for_missing_notification_mutations() {
    let user_id = seed_user("USER").await;
    let token = notification_token(user_id).await;
    let notification_id = NotificationId::new();
    let client = reqwest::Client::new();

    let update = client
        .patch(notification_path(notification_id))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"seen": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch missing notification: {error}"));
    let (update_status, update_body) = json_response(update).await;
    assert_problem(
        update_status,
        &update_body,
        reqwest::StatusCode::NOT_FOUND,
        "NOTIFICATION_NOT_FOUND",
    );

    let delete = client
        .delete(notification_path(notification_id))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete missing notification: {error}"));
    let (delete_status, delete_body) = json_response(delete).await;
    assert_problem(
        delete_status,
        &delete_body,
        reqwest::StatusCode::NOT_FOUND,
        "NOTIFICATION_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_preserve_source_currency_for_opposite_user_preferences() {
    let eur_source_user = seed_user("USER").await;
    set_user_currency(eur_source_user, "USD").await;
    seed_price_notification(eur_source_user, "EUR", Some(1000), Some(0)).await;
    let eur_body =
        list_notification_page(&notification_token(eur_source_user).await, 10, None).await;
    let eur_items = eur_body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("EUR notification list is missing items"));
    let eur_payload = notification_with_kind(eur_items, "WATCHLIST_PRICE_CHANGED");
    assert_eq!(
        serde_json::json!("EUR"),
        eur_payload["payload"]["change"]["oldPrice"]["currency"]
    );
    assert_eq!(
        serde_json::json!(1000),
        eur_payload["payload"]["change"]["oldPrice"]["amount"]
    );
    assert_eq!(
        serde_json::json!("EUR"),
        eur_payload["payload"]["change"]["newPrice"]["currency"]
    );
    assert_eq!(
        serde_json::json!(0),
        eur_payload["payload"]["change"]["newPrice"]["amount"]
    );

    let usd_source_user = seed_user("USER").await;
    set_user_currency(usd_source_user, "EUR").await;
    seed_price_notification(usd_source_user, "USD", None, Some(1100)).await;
    let usd_body =
        list_notification_page(&notification_token(usd_source_user).await, 10, None).await;
    let usd_items = usd_body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("USD notification list is missing items"));
    let usd_payload = notification_with_kind(usd_items, "WATCHLIST_PRICE_CHANGED");
    assert!(usd_payload["payload"]["change"]["oldPrice"].is_null());
    assert_eq!(
        serde_json::json!("USD"),
        usd_payload["payload"]["change"]["newPrice"]["currency"]
    );
    assert_eq!(
        serde_json::json!(1100),
        usd_payload["payload"]["change"]["newPrice"]["amount"]
    );
}

fn notifications_path() -> String {
    format!("{}/api/v1/me/notifications", AURA_API.base_url())
}

fn notification_path(notification_id: NotificationId) -> String {
    format!("{}/{}", notifications_path(), notification_id)
}

async fn notification_token(user_id: UserId) -> RawAccessToken {
    seed_access_token_for(user_id, HashSet::new()).await
}

async fn seed_notification(user_id: UserId, seen: bool) -> NotificationId {
    let notification_id = NotificationId::new();
    let partnership_application_id = PartnershipApplicationId::new();
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO notifications (
            notification_id,
            user_id,
            kind,
            partnership_application_id,
            payload,
            seen
        ) VALUES ($1, $2, 'PARTNERSHIP_APPLICATION_APPROVED', $3, $4, $5)
        "#,
    )
    .bind(notification_id.into_uuid())
    .bind(user_id.into_uuid())
    .bind(partnership_application_id.into_uuid())
    .bind(serde_json::json!({
        "type": "PARTNERSHIP_APPLICATION",
        "snapshot": {
            "party_name": "Acceptance Party",
            "listing_source_name": "Acceptance Listing Source",
            "image": null
        },
    }))
    .bind(seen)
    .execute(&pool)
    .await
    {
        panic!("failed to seed notification: {error}");
    }
    notification_id
}

async fn seed_unsafe_image_notification(
    user_id: UserId,
    product_listing_id: ProductListingId,
    content_policy: Value,
) {
    seed_notification_with_payload(
        user_id,
        "WATCHLIST_AVAILABILITY_CHANGED",
        Some(EventId::new()),
        Some(product_listing_id),
        None,
        None,
        serde_json::json!({
            "type": "WATCHLIST",
            "snapshot": {
                "listing_source_id": ListingSourceId::new().into_uuid().to_string(),
                "source_listing_id": "unsafe-product",
                "listing_source_slug_id": "unsafe-listing-source",
                "product_listing_title_slug_id": "unsafe-product-abcdef",
                "listing_source_name": "Unsafe Listing Source",
                "title": null,
                "image": "https://unsafe.listing-source.example/image.jpg",
                "content_policy": content_policy,
                "url": "https://unsafe.listing-source.example/product",
                "view_url": "https://aura-historia.example/product"
            },
            "change": {
                "type": "AVAILABILITY_CHANGE",
                "old_availability": "AVAILABLE",
                "new_availability": null
            }
        }),
    )
    .await;
}

async fn seed_notification_at(
    user_id: UserId,
    notification_id: NotificationId,
    created: OffsetDateTime,
) {
    let partnership_application_id = PartnershipApplicationId::new();
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO notifications (
            notification_id, user_id, kind, partnership_application_id, payload, seen, created, updated
        ) VALUES ($1, $2, 'PARTNERSHIP_APPLICATION_APPROVED', $3, $4, false, $5, $5)
        "#,
    )
    .bind(notification_id.into_uuid())
    .bind(user_id.into_uuid())
    .bind(partnership_application_id.into_uuid())
    .bind(serde_json::json!({
        "type": "PARTNERSHIP_APPLICATION",
        "snapshot": {
            "party_name": "Acceptance Party",
            "listing_source_name": "Acceptance Listing Source",
            "image": null
        },
    }))
    .bind(created)
    .execute(&pool)
    .await
    {
        panic!("failed to seed dated notification: {error}");
    }
}

async fn list_notification_page(
    token: &RawAccessToken,
    size: u32,
    search_after: Option<&str>,
) -> Value {
    let client = reqwest::Client::new();
    let mut request = client
        .get(notifications_path())
        .bearer_auth(String::from(token.clone()))
        .query(&[("size", size.to_string())]);
    if let Some(search_after) = search_after {
        request = request.query(&[("searchAfter", search_after)]);
    }
    let response = request
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list notification page: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status, "notification page: {body}");
    body
}

async fn set_current_assessment(
    product_listing_id: ProductListingId,
    decision: &str,
    category: Option<&str>,
) {
    let pool = get_postgres_client().await;
    sqlx::query(
        "INSERT INTO product_listing_content_assessments (product_listing_id, source_event_id, decision, category) SELECT product_listing_id, content_source_event_id, $2, $3 FROM product_listings WHERE product_listing_id = $1 ON CONFLICT (product_listing_id) DO UPDATE SET decision = EXCLUDED.decision, category = EXCLUDED.category, source_event_id = EXCLUDED.source_event_id",
    )
    .bind(product_listing_id.into_uuid())
    .bind(decision)
    .bind(category)
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to set current content assessment: {error}"));
}

async fn set_user_content_preference(user_id: UserId, show: bool) {
    let pool = get_postgres_client().await;
    sqlx::query("UPDATE users SET show_unassessed_or_sensitive_content = $2 WHERE user_id = $1")
        .bind(user_id.into_uuid())
        .bind(show)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to set content preference: {error}"));
}

async fn set_user_currency(user_id: UserId, currency: &str) {
    let pool = get_postgres_client().await;
    sqlx::query("UPDATE users SET currency = $2 WHERE user_id = $1")
        .bind(user_id.into_uuid())
        .bind(currency)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to set user currency: {error}"));
}

async fn seed_price_notification(
    user_id: UserId,
    currency: &str,
    old_amount: Option<u64>,
    new_amount: Option<u64>,
) {
    let product_listing_id = ProductListingId::new();
    let price = |amount| serde_json::json!({ "currency": currency, "amount": amount });
    seed_notification_with_payload(
        user_id,
        "WATCHLIST_PRICE_CHANGED",
        Some(EventId::new()),
        Some(product_listing_id),
        None,
        None,
        serde_json::json!({
            "type": "WATCHLIST",
            "snapshot": {
                "listing_source_id": ListingSourceId::new().into_uuid().to_string(),
                "source_listing_id": "source-currency-product",
                "listing_source_slug_id": "source-currency-listing-source",
                "product_listing_title_slug_id": "source-currency-product-a1b2c3",
                "listing_source_name": "Source Currency Listing Source",
                "title": null,
                "image": null,
                "url": "https://listing-source.example/source-currency-product",
                "view_url": "https://aura-historia.example/source-currency-product"
            },
            "change": {
                "type": "PRICE_CHANGE",
                "old_price": old_amount.map(price).unwrap_or(Value::Null),
                "new_price": new_amount.map(price).unwrap_or(Value::Null)
            }
        }),
    )
    .await;
}

async fn seed_notification_payloads(user_id: UserId) {
    let product_listing_id = ProductListingId::new();
    let product_snapshot = |title: serde_json::Value, image: serde_json::Value| {
        serde_json::json!({
            "listing_source_id": ListingSourceId::new().into_uuid().to_string(),
            "source_listing_id": "listing-source-product-123",
            "listing_source_slug_id": "test-listing-source",
            "product_listing_title_slug_id": "test-product-a1b2c3",
            "listing_source_name": "Snapshot Listing Source",
            "title": title,
            "image": image,
            "url": "https://listing-source.example/product",
            "view_url": "https://listing-source.example/product?view=1"
        })
    };
    let localized_title = serde_json::json!([
        { "language": "en", "title": "Violin title" },
        { "language": "de", "title": "Geigentitel" }
    ]);
    let image = serde_json::json!("https://listing-source.example/product.jpg");

    seed_notification_with_payload(
        user_id,
        "WATCHLIST_PRICE_CHANGED",
        Some(EventId::new()),
        Some(product_listing_id),
        None,
        None,
        serde_json::json!({
            "type": "WATCHLIST",
            "snapshot": product_snapshot(localized_title.clone(), image),
            "change": {
                "type": "PRICE_CHANGE",
                "old_price": { "currency": "EUR", "amount": 1000 },
                "new_price": { "currency": "EUR", "amount": 900 }
            }
        }),
    )
    .await;
    seed_notification_with_payload(
        user_id,
        "WATCHLIST_AVAILABILITY_CHANGED",
        Some(EventId::new()),
        Some(product_listing_id),
        None,
        None,
        serde_json::json!({
            "type": "WATCHLIST",
            "snapshot": product_snapshot(serde_json::Value::Null, serde_json::Value::Null),
            "change": { "type": "AVAILABILITY_CHANGE", "old_availability": "AVAILABLE", "new_availability": "SOLD_OUT" }
        }),
    )
    .await;
    let filter_id = UserSearchFilterId::new();
    seed_notification_with_payload(
        user_id,
        "SEARCH_FILTER_MATCH",
        Some(EventId::new()),
        Some(product_listing_id),
        Some(filter_id),
        None,
        serde_json::json!({
            "type": "SEARCH_FILTER",
            "snapshot": product_snapshot(localized_title, serde_json::Value::Null),
            "user_search_filter_name": "Saved Violins"
        }),
    )
    .await;
    seed_notification_with_payload(
        user_id,
        "PARTNERSHIP_APPLICATION_APPROVED",
        None,
        None,
        None,
        Some(PartnershipApplicationId::new()),
        serde_json::json!({
            "type": "PARTNERSHIP_APPLICATION",
            "snapshot": {
                "party_name": "Approved Party",
                "listing_source_name": "Approved Listing Source",
                "image": "https://listing-source.example/approved.jpg"
            }
        }),
    )
    .await;
    seed_notification_with_payload(
        user_id,
        "PARTNERSHIP_APPLICATION_REJECTED",
        None,
        None,
        None,
        Some(PartnershipApplicationId::new()),
        serde_json::json!({
            "type": "PARTNERSHIP_APPLICATION",
            "snapshot": {
                "party_name": "Rejected Party",
                "listing_source_name": "Rejected Listing Source",
                "image": null
            }
        }),
    )
    .await;
}

async fn seed_notification_with_payload(
    user_id: UserId,
    kind: &str,
    origin_event_id: Option<EventId>,
    product_listing_id: Option<ProductListingId>,
    user_search_filter_id: Option<UserSearchFilterId>,
    partnership_application_id: Option<PartnershipApplicationId>,
    payload: serde_json::Value,
) {
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO notifications (
            notification_id, user_id, kind, origin_event_id, product_listing_id,
            user_search_filter_id, partnership_application_id, payload, seen
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, false)
        "#,
    )
    .bind(NotificationId::new().into_uuid())
    .bind(user_id.into_uuid())
    .bind(kind)
    .bind(origin_event_id.map(EventId::into_uuid))
    .bind(product_listing_id.map(ProductListingId::into_uuid))
    .bind(user_search_filter_id.map(UserSearchFilterId::into_uuid))
    .bind(partnership_application_id.map(PartnershipApplicationId::into_uuid))
    .bind(payload)
    .execute(&pool)
    .await
    {
        panic!("failed to seed notification payload: {error}");
    }
}

fn notification_with_kind<'a>(items: &'a [Value], kind: &str) -> &'a Value {
    match items.iter().find(|item| item["kind"] == kind) {
        Some(item) => item,
        None => panic!("notification list is missing {kind}"),
    }
}

async fn notification_seen(token: &RawAccessToken, notification_id: NotificationId) -> bool {
    let notification = listed_notifications(token)
        .await
        .into_iter()
        .find(|notification| notification["notificationId"] == notification_id.to_string());
    match notification.and_then(|notification| notification["seen"].as_bool()) {
        Some(seen) => seen,
        None => panic!("notification {notification_id} was not listed with a seen state"),
    }
}

async fn listed_notification_ids(token: &RawAccessToken) -> Vec<String> {
    listed_notifications(token)
        .await
        .into_iter()
        .filter_map(|notification| {
            notification["notificationId"]
                .as_str()
                .map(ToOwned::to_owned)
        })
        .collect()
}

async fn listed_notifications(token: &RawAccessToken) -> Vec<Value> {
    let response = reqwest::Client::new()
        .get(notifications_path())
        .bearer_auth(String::from(token.clone()))
        .query(&[("size", "100")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to verify listed notifications: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    match body["items"].as_array() {
        Some(items) => items.clone(),
        None => panic!("notification list response is missing items"),
    }
}
