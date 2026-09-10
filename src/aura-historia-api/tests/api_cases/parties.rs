use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{assert_problem, json_response, seed_access_token_for, seed_party, seed_user};
use listing_source_core::ListingSourceId;
use party_core::party_id::PartyId;
use serde_json::json;
use test_api::{IntegrationTestService, aura_integration_test};

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_unused_party_and_return_not_found_when_repeated() {
    let party_id = seed_party("Delete Party", None, None).await;
    let unrelated_id = seed_party("Unrelated Party", None, None).await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/admin/parties/{party_id}", AURA_API.base_url());

    let response = client
        .delete(&url)
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete party: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert_eq!(
        Some("no-store"),
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
    );
    let body = response
        .bytes()
        .await
        .unwrap_or_else(|error| panic!("failed to read delete response body: {error}"));
    assert!(body.is_empty());

    let repeated = client
        .delete(&url)
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to repeat party delete: {error}"));
    let (status, body) = json_response(repeated).await;
    assert_eq!(reqwest::StatusCode::NOT_FOUND, status);
    assert_eq!(json!("PARTY_NOT_FOUND"), body["error"]);

    let unrelated = client
        .get(format!(
            "{}/api/v1/admin/parties/{unrelated_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get unrelated party: {error}"));
    assert_eq!(reqwest::StatusCode::OK, unrelated.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_party_summary_for_admin_with_no_store_cache_control() {
    let party_id = seed_party(
        "Admin Party",
        Some("+49 30 123456"),
        Some("admin-party@example.test"),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .query(&[("name", "Admin Party")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to search parties API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(
        serde_json::json!(party_id.to_string()),
        body["items"][0]["partyId"]
    );
    assert_eq!(
        serde_json::json!(format!("api-acceptance-party-{}", party_id.as_uuid())),
        body["items"][0]["partySlugId"]
    );
    assert_eq!(serde_json::json!("Admin Party"), body["items"][0]["name"]);
    assert_eq!(
        serde_json::json!("+49 30 123456"),
        body["items"][0]["contact"]["phone"]
    );
    assert_eq!(
        serde_json::json!("admin-party@example.test"),
        body["items"][0]["contact"]["email"]
    );
    assert!(body["items"][0]["created"].as_str().is_some());
    assert!(body["items"][0]["updated"].as_str().is_some());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_filter_parties_by_name_and_contact_query() {
    let matching_id = seed_party(
        "Selector Party",
        Some("+49 30 555000"),
        Some("selector@example.test"),
    )
    .await;
    let _other_id = seed_party(
        "Different Party",
        Some("+49 30 111000"),
        Some("other@example.test"),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .query(&[("query", "selector")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to filter parties API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(
        serde_json::json!(matching_id.to_string()),
        body["items"][0]["partyId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_follow_party_cursor_with_deterministic_sorting() {
    let first_id = seed_party("Cursor Party A", None, None).await;
    let second_id = seed_party("Cursor Party B", None, None).await;
    let third_id = seed_party("Cursor Party C", None, None).await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();

    let first = client
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .query(&[
            ("name", "Cursor Party"),
            ("sort", "name"),
            ("order", "asc"),
            ("size", "2"),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get first party page: {error}"));
    let (first_status, first_body) = json_response(first).await;

    assert_eq!(reqwest::StatusCode::OK, first_status);
    assert_eq!(Some(2), first_body["items"].as_array().map(Vec::len));
    assert_eq!(
        serde_json::json!(first_id.to_string()),
        first_body["items"][0]["partyId"]
    );
    assert_eq!(
        serde_json::json!(second_id.to_string()),
        first_body["items"][1]["partyId"]
    );
    let cursor = first_body["searchAfter"]
        .as_str()
        .unwrap_or_else(|| panic!("missing party search cursor"))
        .to_owned();

    let second = client
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .query(&[
            ("name", "Cursor Party"),
            ("sort", "name"),
            ("order", "asc"),
            ("size", "2"),
            ("searchAfter", cursor.as_str()),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get second party page: {error}"));
    let (second_status, second_body) = json_response(second).await;

    assert_eq!(reqwest::StatusCode::OK, second_status);
    assert_eq!(Some(1), second_body["items"].as_array().map(Vec::len));
    assert_eq!(
        serde_json::json!(third_id.to_string()),
        second_body["items"][0]["partyId"]
    );
    assert!(second_body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_empty_party_collection_when_no_party_matches() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .query(&[("email", "missing-party@example.test")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to search empty party collection: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(0), body["items"].as_array().map(Vec::len));
    assert!(body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_party_search_query_values() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();

    let invalid_sort = client
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .query(&[("sort", "invalid"), ("order", "asc")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate party sort: {error}"));
    let (status, body) = json_response(invalid_sort).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_SORT_VALUE",
    );

    let invalid_size = client
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .query(&[("size", "not-a-number")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate party size: {error}"));
    let (status, body) = json_response(invalid_size).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_QUERY_PARAMETER_VALUE",
    );

    let party_id = PartyId::new();
    for invalid_id in [
        ListingSourceId::new().to_string(),
        party_id.as_uuid().to_string(),
        "pty_not-a-typeid".to_owned(),
    ] {
        let invalid_cursor = client
            .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
            .bearer_auth(String::from(token.clone()))
            .query(&[("searchAfter", invalid_id)])
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to validate party cursor: {error}"));
        let (status, body) = json_response(invalid_cursor).await;
        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "INVALID_OBJECT_ID",
        );
        assert_eq!(
            json!({"field": "searchAfter", "type": "QUERY"}),
            body["source"]
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_party_collection_for_non_admin() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin party search: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_party_with_identity_location_and_contact() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&json!({
            "name": "  Created Party  ",
            "phone": "+49 30 123456",
            "email": "created-party@example.test"
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create party: {error}"));
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;

    let party_id = body["partyId"]
        .as_str()
        .unwrap_or_else(|| panic!("created response has no party ID"))
        .parse::<PartyId>()
        .unwrap_or_else(|error| panic!("created response has invalid Party ID: {error}"));
    assert!(party_id.to_string().starts_with("pty_"));
    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(Some(format!("/api/v1/admin/parties/{party_id}")), location);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(json!("Created Party"), body["name"]);
    assert_eq!(json!("+49 30 123456"), body["contact"]["phone"]);
    assert_eq!(
        json!("created-party@example.test"),
        body["contact"]["email"]
    );
    assert_eq!(
        json!(format!("created-party-{}", party_id.as_uuid())),
        body["partySlugId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_party_with_each_optional_contact_variant() {
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let cases = [
        (json!({"name": "Party without contact"}), json!({})),
        (
            json!({"name": "Party with phone", "phone": "+49 30 111111"}),
            json!({"phone": "+49 30 111111"}),
        ),
        (
            json!({"name": "Party with email", "email": "email-party@example.test"}),
            json!({"email": "email-party@example.test"}),
        ),
        (
            json!({
                "name": "Party with all contact",
                "phone": "+49 30 222222",
                "email": "full-party@example.test"
            }),
            json!({
                "phone": "+49 30 222222",
                "email": "full-party@example.test"
            }),
        ),
    ];

    for (request, expected_contact) in cases {
        let response = client
            .post(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
            .bearer_auth(token.clone())
            .json(&request)
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to create contact variant: {error}"));
        let (status, body) = json_response(response).await;

        assert_eq!(reqwest::StatusCode::CREATED, status);
        assert_eq!(expected_contact, body["contact"]);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_party_create_names() {
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();

    for name in [
        String::new(),
        " \u{2003}\u{00a0}".to_owned(),
        "é".repeat(128),
    ] {
        let response = client
            .post(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
            .bearer_auth(token.clone())
            .json(&json!({"name": name}))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to validate party name: {error}"));
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
async fn should_reject_party_create_without_admin_authorization() {
    let missing = reqwest::Client::new()
        .post(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .json(&json!({"name": "Unauthenticated Party"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject unauthenticated party create: {error}"));
    let (missing_status, missing_body) = json_response(missing).await;
    assert_problem(
        missing_status,
        &missing_body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );

    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;
    let non_admin = reqwest::Client::new()
        .post(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&json!({"name": "Non-admin Party"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin party create: {error}"));
    let (non_admin_status, non_admin_body) = json_response(non_admin).await;
    assert_problem(
        non_admin_status,
        &non_admin_body,
        reqwest::StatusCode::FORBIDDEN,
        "FORBIDDEN",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_party_detail_for_admin_with_no_store_cache_control() {
    let party_id = seed_party(
        "Detailed Party",
        Some("+49 30 987654"),
        Some("detailed-party@example.test"),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/parties/{party_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get party detail: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(json!(party_id.to_string()), body["partyId"]);
    assert_eq!(
        json!(format!("api-acceptance-party-{}", party_id.as_uuid())),
        body["partySlugId"]
    );
    assert_eq!(json!("Detailed Party"), body["name"]);
    assert_eq!(json!("+49 30 987654"), body["contact"]["phone"]);
    assert_eq!(
        json!("detailed-party@example.test"),
        body["contact"]["email"]
    );
    assert!(body["created"].as_str().is_some());
    assert!(body["updated"].as_str().is_some());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_noncanonical_party_detail_ids() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let party_id = PartyId::new();

    for invalid_id in [
        ListingSourceId::new().to_string(),
        party_id.as_uuid().to_string(),
        "pty_not-a-typeid".to_owned(),
    ] {
        let response = reqwest::Client::new()
            .get(format!(
                "{}/api/v1/admin/parties/{invalid_id}",
                AURA_API.base_url()
            ))
            .bearer_auth(String::from(token.clone()))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to validate party detail ID: {error}"));
        let (status, body) = json_response(response).await;

        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "INVALID_OBJECT_ID",
        );
        assert_eq!(json!({"field": "partyId", "type": "PATH"}), body["source"]);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_for_missing_party_detail() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/parties/{}",
            AURA_API.base_url(),
            PartyId::new()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get missing party detail: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PARTY_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_admin_authentication_for_party_detail() {
    let party_id = seed_party("Protected Detail Party", None, None).await;
    let client = reqwest::Client::new();
    let path = format!("{}/api/v1/admin/parties/{party_id}", AURA_API.base_url());

    let missing =
        client.get(&path).send().await.unwrap_or_else(|error| {
            panic!("failed to reject unauthenticated party detail: {error}")
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
        .unwrap_or_else(|error| panic!("failed to reject non-admin party detail: {error}"));
    let (non_admin_status, non_admin_body) = json_response(non_admin).await;
    assert_problem(
        non_admin_status,
        &non_admin_body,
        reqwest::StatusCode::FORBIDDEN,
        "FORBIDDEN",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_party_name_and_contact_with_tri_state_patch() {
    let party_id = seed_party(
        "Original Party",
        Some("+49 30 111111"),
        Some("original@example.test"),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let path = format!("{}/api/v1/admin/parties/{party_id}", AURA_API.base_url());
    let original_slug = json!(format!("api-acceptance-party-{}", party_id.as_uuid()));

    let renamed = client
        .patch(&path)
        .bearer_auth(token.clone())
        .json(&json!({"name": "  Renamed Party  "}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to rename Party: {error}"));
    let renamed_cache_control = renamed
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (renamed_status, renamed_body) = json_response(renamed).await;
    assert_eq!(reqwest::StatusCode::OK, renamed_status);
    assert_eq!(Some("no-store".to_owned()), renamed_cache_control);
    assert_eq!(json!("Renamed Party"), renamed_body["name"]);
    assert_eq!(original_slug, renamed_body["partySlugId"]);
    assert_eq!(json!("+49 30 111111"), renamed_body["contact"]["phone"]);
    assert_eq!(
        json!("original@example.test"),
        renamed_body["contact"]["email"]
    );

    let set_contact = client
        .patch(&path)
        .bearer_auth(token.clone())
        .json(&json!({
            "phone": "+49 30 222222",
            "email": "updated@example.test"
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to set Party contact: {error}"));
    let (set_status, set_body) = json_response(set_contact).await;
    assert_eq!(reqwest::StatusCode::OK, set_status);
    assert_eq!(json!("Renamed Party"), set_body["name"]);
    assert_eq!(original_slug, set_body["partySlugId"]);
    assert_eq!(json!("+49 30 222222"), set_body["contact"]["phone"]);
    assert_eq!(json!("updated@example.test"), set_body["contact"]["email"]);

    let clear_contact = client
        .patch(&path)
        .bearer_auth(token.clone())
        .json(&json!({"phone": null, "email": null}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to clear Party contact: {error}"));
    let (clear_status, clear_body) = json_response(clear_contact).await;
    assert_eq!(reqwest::StatusCode::OK, clear_status);
    assert_eq!(json!({}), clear_body["contact"]);
    assert_eq!(original_slug, clear_body["partySlugId"]);

    let no_op = client
        .patch(&path)
        .bearer_auth(token)
        .json(&json!({}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to apply Party no-op patch: {error}"));
    let (no_op_status, no_op_body) = json_response(no_op).await;
    assert_eq!(reqwest::StatusCode::OK, no_op_status);
    assert_eq!(clear_body, no_op_body);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_party_update_values() {
    let party_id = seed_party("Patch Validation Party", None, None).await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let path = format!("{}/api/v1/admin/parties/{party_id}", AURA_API.base_url());

    for request in [
        json!({"name": "  "}),
        json!({"name": null}),
        json!({"email": "not-an-email"}),
    ] {
        let response = client
            .patch(&path)
            .bearer_auth(token.clone())
            .json(&request)
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to validate Party update: {error}"));
        let (status, body) = json_response(response).await;
        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "BAD_BODY_VALUE",
        );
    }

    let empty = client
        .patch(&path)
        .bearer_auth(token)
        .body("")
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate empty Party update body: {error}"));
    let (empty_status, empty_body) = json_response(empty).await;
    assert_problem(
        empty_status,
        &empty_body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_for_missing_party_update() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/parties/{}",
            AURA_API.base_url(),
            PartyId::new()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({"name": "Missing Party"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to update missing Party: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PARTY_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_admin_authentication_for_party_update() {
    let party_id = seed_party("Protected Update Party", None, None).await;
    let path = format!("{}/api/v1/admin/parties/{party_id}", AURA_API.base_url());
    let client = reqwest::Client::new();

    let missing = client
        .patch(&path)
        .json(&json!({"name": "Unauthenticated Party"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject unauthenticated Party update: {error}"));
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
        .json(&json!({"name": "Non-admin Party"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin Party update: {error}"));
    let (non_admin_status, non_admin_body) = json_response(non_admin).await;
    assert_problem(
        non_admin_status,
        &non_admin_body,
        reqwest::StatusCode::FORBIDDEN,
        "FORBIDDEN",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_malformed_party_update_id() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/parties/pty_not-a-typeid",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({"name": "Party"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate Party update ID: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_OBJECT_ID",
    );
    assert_eq!(json!({"field": "partyId", "type": "PATH"}), body["source"]);
}
