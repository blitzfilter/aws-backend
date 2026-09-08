use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};
use api_support::{
    assert_problem, json_response, seed_access_token_for, seed_approved_partnership_application,
    seed_partnership_application, seed_user,
};
use serde_json::json;
use test_api::{IntegrationTestService, aura_integration_test};
use time::macros::datetime;
use uuid::Uuid;

fn existing_proposal(listing_source_id: Uuid) -> serde_json::Value {
    json!({
        "type": "EXISTING_LISTING_SOURCE",
        "listing_source_id": listing_source_id,
    })
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_submission_that_references_a_missing_listing_source() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/me/partnership-applications",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({
            "proposal": {
                "type": "EXISTING_LISTING_SOURCE",
                "listingSourceId": Uuid::new_v4(),
            },
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to submit missing ListingSource proposal: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PARTNERSHIP_APPLICATION_LISTING_SOURCE_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_get_admin_partnership_application_detail_with_approval_references() {
    let applicant_user_id = seed_user("USER").await;
    let (application_id, approved_partnership_id, approved_listing_source_id) =
        seed_approved_partnership_application(
            applicant_user_id,
            datetime!(2026-06-01 12:00 UTC),
            datetime!(2026-06-02 12:00 UTC),
        )
        .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/partnership-applications/{application_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get partnership application detail: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(json!(application_id.to_string()), body["id"]);
    assert_eq!(json!(applicant_user_id), body["applicantUserId"]);
    assert_eq!(json!("APPROVED"), body["state"]);
    assert_eq!(json!("EXISTING_LISTING_SOURCE"), body["proposal"]["type"]);
    assert_eq!(
        json!(approved_listing_source_id),
        body["proposal"]["listingSourceId"]
    );
    assert_eq!(
        json!(approved_partnership_id),
        body["approvedPartnershipId"]
    );
    assert_eq!(
        json!(approved_listing_source_id),
        body["approvedListingSourceId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_admin_partnership_application_detail_id() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/partnership-applications/not-a-uuid",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate application detail ID: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
    assert_eq!(
        json!({"field": "partnershipApplicationId", "type": "PATH"}),
        body["source"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_for_missing_admin_partnership_application_detail() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/partnership-applications/{}",
            AURA_API.base_url(),
            Uuid::new_v4()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get missing application detail: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PARTNERSHIP_APPLICATION_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_non_admin_partnership_application_detail_access() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/partnership-applications/{}",
            AURA_API.base_url(),
            Uuid::new_v4()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin application access: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_mark_submitted_partnership_application_in_review_as_admin() {
    let applicant_user_id = seed_user("USER").await;
    let application_id = seed_partnership_application(
        applicant_user_id,
        "SUBMITTED",
        existing_proposal(Uuid::new_v4()),
        datetime!(2026-06-01 12:00 UTC),
        datetime!(2026-06-01 12:00 UTC),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/partnership-applications/{application_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to mark partnership application in review: {error}")
        });
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(json!(application_id.to_string()), body["id"]);
    assert_eq!(json!(applicant_user_id), body["applicantUserId"]);
    assert_eq!(json!("IN_REVIEW"), body["state"]);
    assert_eq!(json!("EXISTING_LISTING_SOURCE"), body["proposal"]["type"]);
    assert!(body["approvedPartnershipId"].is_null());
    assert!(body["approvedListingSourceId"].is_null());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_conflict_when_marking_non_submitted_partnership_application_in_review() {
    let applicant_user_id = seed_user("USER").await;
    let application_id = seed_partnership_application(
        applicant_user_id,
        "IN_REVIEW",
        existing_proposal(Uuid::new_v4()),
        datetime!(2026-06-01 12:00 UTC),
        datetime!(2026-06-01 12:00 UTC),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/partnership-applications/{application_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject invalid application transition: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_when_marking_missing_partnership_application_in_review() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/partnership-applications/{}",
            AURA_API.base_url(),
            Uuid::new_v4()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to mark missing partnership application: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PARTNERSHIP_APPLICATION_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_non_admin_mark_in_review() {
    let applicant_user_id = seed_user("USER").await;
    let application_id = seed_partnership_application(
        applicant_user_id,
        "SUBMITTED",
        existing_proposal(Uuid::new_v4()),
        datetime!(2026-06-01 12:00 UTC),
        datetime!(2026-06-01 12:00 UTC),
    )
    .await;
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/admin/partnership-applications/{application_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin mark in review: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_remove_legacy_mark_in_review_route() {
    let applicant_user_id = seed_user("USER").await;
    let application_id = seed_partnership_application(
        applicant_user_id,
        "SUBMITTED",
        existing_proposal(Uuid::new_v4()),
        datetime!(2026-06-01 12:00 UTC),
        datetime!(2026-06-01 12:00 UTC),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/partnership-applications/{application_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call legacy mark in review route: {error}"));

    assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_approve_an_in_review_partnership_application_on_the_admin_decision_route() {
    let applicant_user_id = seed_user("USER").await;
    let listing_source_id = api_support::seed_listing_source().await;
    let application_id = seed_partnership_application(
        applicant_user_id,
        "IN_REVIEW",
        existing_proposal(listing_source_id),
        datetime!(2026-06-01 12:00 UTC),
        datetime!(2026-06-01 12:00 UTC),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/partnership-applications/{application_id}/decision",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({"decision": "APPROVE"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to approve partnership application: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(json!(application_id.to_string()), body["id"]);
    assert_eq!(json!("APPROVED"), body["state"]);
    assert_eq!(json!(listing_source_id), body["approvedListingSourceId"]);

    let applicant_listing_sources_token =
        seed_access_token_for(applicant_user_id, std::collections::HashSet::new()).await;
    let listing_sources_response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/listing-sources", AURA_API.base_url()))
        .bearer_auth(String::from(applicant_listing_sources_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list approved listing sources: {error}"));
    let (listing_sources_status, listing_sources_body) =
        json_response(listing_sources_response).await;

    assert_eq!(reqwest::StatusCode::OK, listing_sources_status);
    assert!(listing_sources_body.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["listingSourceId"] == json!(listing_source_id))
    }));

    let applicant_notifications_token =
        seed_access_token_for(applicant_user_id, std::collections::HashSet::new()).await;
    let notifications_response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/notifications", AURA_API.base_url()))
        .bearer_auth(String::from(applicant_notifications_token))
        .query(&[("size", "100")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list approval notifications: {error}"));
    let (notifications_status, notifications_body) = json_response(notifications_response).await;

    assert_eq!(reqwest::StatusCode::OK, notifications_status);
    assert!(notifications_body["items"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["kind"] == json!("PARTNERSHIP_APPLICATION_APPROVED")
                && item["payload"]["partnershipApplicationId"] == json!(application_id.to_string())
        })
    }));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_an_in_review_partnership_application_on_the_admin_decision_route() {
    let applicant_user_id = seed_user("USER").await;
    let party_name = "Rejected Proposed Party";
    let listing_source_name = "Rejected Proposed Listing Source";
    let application_id = seed_partnership_application(
        applicant_user_id,
        "IN_REVIEW",
        json!({
            "type": "PROPOSED_LISTING_SOURCE",
            "party": {"name": party_name, "email": "rejected@example.test"},
            "listing_source": {
                "name": listing_source_name,
                "url": "https://rejected.example/",
                "image": null,
                "requested_ingestion_methods": ["PARTNER_API"]
            }
        }),
        datetime!(2026-06-01 12:00 UTC),
        datetime!(2026-06-01 12:00 UTC),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/partnership-applications/{application_id}/decision",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({"decision": "REJECT"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject partnership application: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(json!(application_id.to_string()), body["id"]);
    assert_eq!(json!("REJECTED"), body["state"]);
    assert!(body["approvedPartnershipId"].is_null());
    assert!(body["approvedListingSourceId"].is_null());

    let admin_read_token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let parties_response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(admin_read_token))
        .query(&[("name", party_name)])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to search rejected party: {error}"));
    let (parties_status, parties_body) = json_response(parties_response).await;

    assert_eq!(reqwest::StatusCode::OK, parties_status);
    assert_eq!(Some(0), parties_body["items"].as_array().map(Vec::len));

    let applicant_listing_sources_token =
        seed_access_token_for(applicant_user_id, std::collections::HashSet::new()).await;
    let listing_sources_response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/listing-sources", AURA_API.base_url()))
        .bearer_auth(String::from(applicant_listing_sources_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list rejected listing sources: {error}"));
    let (listing_sources_status, listing_sources_body) =
        json_response(listing_sources_response).await;

    assert_eq!(reqwest::StatusCode::OK, listing_sources_status);
    assert_eq!(Some(0), listing_sources_body.as_array().map(Vec::len));

    let applicant_notifications_token =
        seed_access_token_for(applicant_user_id, std::collections::HashSet::new()).await;
    let notifications_response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/notifications", AURA_API.base_url()))
        .bearer_auth(String::from(applicant_notifications_token))
        .query(&[("size", "100")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list rejection notifications: {error}"));
    let (notifications_status, notifications_body) = json_response(notifications_response).await;

    assert_eq!(reqwest::StatusCode::OK, notifications_status);
    assert!(notifications_body["items"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["kind"] == json!("PARTNERSHIP_APPLICATION_REJECTED")
                && item["payload"]["partnershipApplicationId"] == json!(application_id.to_string())
        })
    }));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_an_arbitrary_partnership_application_decision_value() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/partnership-applications/{}/decision",
            AURA_API.base_url(),
            Uuid::new_v4()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({"decision": "IN_REVIEW"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate decision value: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_a_partnership_application_decision_outside_in_review() {
    let applicant_user_id = seed_user("USER").await;
    let application_id = seed_partnership_application(
        applicant_user_id,
        "SUBMITTED",
        existing_proposal(Uuid::new_v4()),
        datetime!(2026-06-01 12:00 UTC),
        datetime!(2026-06-01 12:00 UTC),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/partnership-applications/{application_id}/decision",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({"decision": "APPROVE"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate application transition: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");

    let verification_token =
        seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let detail_response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/partnership-applications/{application_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(verification_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to verify unchanged application: {error}"));
    let (detail_status, detail_body) = json_response(detail_response).await;

    assert_eq!(reqwest::StatusCode::OK, detail_status);
    assert_eq!(json!("SUBMITTED"), detail_body["state"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_for_a_missing_admin_partnership_application_decision() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/partnership-applications/{}/decision",
            AURA_API.base_url(),
            Uuid::new_v4()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({"decision": "REJECT"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get missing application decision: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PARTNERSHIP_APPLICATION_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_non_admin_partnership_application_decision() {
    let applicant_user_id = seed_user("USER").await;
    let application_id = seed_partnership_application(
        applicant_user_id,
        "IN_REVIEW",
        json!({
            "type": "PROPOSED_LISTING_SOURCE",
            "party": {"name": "Non-admin Decision Party"},
            "listing_source": {
                "name": "Non-admin Decision Listing Source",
                "requested_ingestion_methods": ["PARTNER_API"]
            }
        }),
        datetime!(2026-06-01 12:00 UTC),
        datetime!(2026-06-01 12:00 UTC),
    )
    .await;
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/partnership-applications/{application_id}/decision",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({"decision": "REJECT"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin decision: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_list_filtered_admin_partnership_application_summaries_without_secrets() {
    let applicant_user_id = seed_user("USER").await;
    let other_applicant_user_id = seed_user("USER").await;
    let listing_source_id = Uuid::new_v4();
    let matching_id = seed_partnership_application(
        applicant_user_id,
        "SUBMITTED",
        existing_proposal(listing_source_id),
        datetime!(2026-01-02 12:00 UTC),
        datetime!(2026-02-03 12:00 UTC),
    )
    .await;
    let _other_id = seed_partnership_application(
        other_applicant_user_id,
        "IN_REVIEW",
        existing_proposal(Uuid::new_v4()),
        datetime!(2026-03-02 12:00 UTC),
        datetime!(2026-04-03 12:00 UTC),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let applicant_user_id = applicant_user_id.to_string();
    let listing_source_id = listing_source_id.to_string();

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/partnership-applications",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .query(&[
            ("state", "SUBMITTED"),
            ("proposalType", "EXISTING_LISTING_SOURCE"),
            ("applicantUserId", applicant_user_id.as_str()),
            ("listingSourceId", listing_source_id.as_str()),
            ("created[min]", "2026-01-01T00:00:00Z"),
            ("created[max]", "2026-01-31T23:59:59Z"),
            ("updated[min]", "2026-02-01T00:00:00Z"),
            ("updated[max]", "2026-02-28T23:59:59Z"),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list partnership applications: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(json!(matching_id.to_string()), body["items"][0]["id"]);
    assert_eq!(
        json!(applicant_user_id),
        body["items"][0]["applicantUserId"]
    );
    assert_eq!(json!("SUBMITTED"), body["items"][0]["state"]);
    assert_eq!(
        json!("EXISTING_LISTING_SOURCE"),
        body["items"][0]["proposal"]["type"]
    );
    assert_eq!(
        json!(listing_source_id),
        body["items"][0]["proposal"]["listingSourceId"]
    );
    assert!(body["items"][0].get("version").is_none());
    assert!(body["items"][0]["created"].as_str().is_some());
    assert!(body["items"][0]["updated"].as_str().is_some());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_follow_admin_partnership_application_cursor_with_tied_timestamps() {
    let applicant_user_id = seed_user("USER").await;
    let created = datetime!(2026-05-05 12:00 UTC);
    let ids = vec![
        seed_partnership_application(
            applicant_user_id,
            "SUBMITTED",
            existing_proposal(Uuid::new_v4()),
            created,
            created,
        )
        .await,
        seed_partnership_application(
            applicant_user_id,
            "SUBMITTED",
            existing_proposal(Uuid::new_v4()),
            created,
            created,
        )
        .await,
        seed_partnership_application(
            applicant_user_id,
            "SUBMITTED",
            existing_proposal(Uuid::new_v4()),
            created,
            created,
        )
        .await,
    ];
    let mut expected_ids = ids.clone();
    expected_ids.sort_by(|left, right| right.cmp(left));
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();
    let path = format!(
        "{}/api/v1/admin/partnership-applications",
        AURA_API.base_url()
    );

    let first = client
        .get(&path)
        .bearer_auth(String::from(token.clone()))
        .query(&[("size", "2"), ("sort", "created"), ("order", "desc")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get first application page: {error}"));
    let (first_status, first_body) = json_response(first).await;

    assert_eq!(reqwest::StatusCode::OK, first_status);
    assert_eq!(Some(2), first_body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(expected_ids[0].to_string()),
        first_body["items"][0]["id"]
    );
    assert_eq!(
        json!(expected_ids[1].to_string()),
        first_body["items"][1]["id"]
    );
    assert!(first_body["searchAfter"].is_array());
    let cursor = first_body["searchAfter"].to_string();

    let second = client
        .get(&path)
        .bearer_auth(String::from(token))
        .query(&[
            ("size", "2"),
            ("sort", "created"),
            ("order", "desc"),
            ("searchAfter", cursor.as_str()),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get second application page: {error}"));
    let (second_status, second_body) = json_response(second).await;

    assert_eq!(reqwest::StatusCode::OK, second_status);
    assert_eq!(Some(1), second_body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(expected_ids[2].to_string()),
        second_body["items"][0]["id"]
    );
    assert!(second_body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_empty_admin_partnership_application_collection() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/partnership-applications",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .query(&[("applicantUserId", Uuid::new_v4().to_string())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get empty application collection: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(Some(0), body["items"].as_array().map(Vec::len));
    assert!(body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_queries_and_non_admin_collection_access() {
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();
    let path = format!(
        "{}/api/v1/admin/partnership-applications",
        AURA_API.base_url()
    );

    let invalid = client
        .get(&path)
        .bearer_auth(String::from(admin_token))
        .query(&[("state", "invalid")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate application query: {error}"));
    let (status, body) = json_response(invalid).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_QUERY_PARAMETER_VALUE",
    );

    let user_id = seed_user("USER").await;
    let user_token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;
    let response = client
        .get(&path)
        .bearer_auth(String::from(user_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin application access: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}
