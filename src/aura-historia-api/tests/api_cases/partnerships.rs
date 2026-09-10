use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};
use api_support::{
    assert_problem, json_response, seed_access_token_for,
    seed_approved_partnership_application as seed_raw_approved_partnership_application,
    seed_current_fx_snapshot, seed_listing_source as seed_raw_listing_source,
    seed_listing_source_for_search as seed_raw_listing_source_for_search,
    seed_operator_partnership_listing_source_grant as seed_raw_operator_partnership_listing_source_grant,
    seed_partnership_for_search as seed_raw_partnership_for_search,
    seed_partnership_membership as seed_raw_partnership_membership, seed_user,
};
use listing_source_core::ListingSourceId;
use partnership_core::{
    partnership_application_id::PartnershipApplicationId, partnership_id::PartnershipId,
};
use party_core::party_id::PartyId;
use serde_json::{Value, json};

use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use time::{OffsetDateTime, macros::datetime};
use user_core::{access_token::Scope, user_id::UserId};
use uuid::Uuid;

async fn seed_listing_source() -> ListingSourceId {
    ListingSourceId::try_from(seed_raw_listing_source().await)
        .unwrap_or_else(|error| panic!("central ListingSource fixture must use UUIDv7: {error}"))
}

async fn seed_listing_source_for_search(
    name: &str,
    operator_name: &str,
    ingestion_method: &str,
    referral_configuration: Option<Value>,
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

async fn seed_partnership_for_search(
    party_name: &str,
    created: OffsetDateTime,
    updated: OffsetDateTime,
    member_user_ids: &[UserId],
    listing_source_ids: &[ListingSourceId],
) -> (PartnershipId, PartyId) {
    let raw_listing_source_ids = listing_source_ids
        .iter()
        .map(|id| id.into_uuid())
        .collect::<Vec<_>>();
    seed_raw_partnership_for_search(
        party_name,
        created,
        updated,
        member_user_ids,
        &raw_listing_source_ids,
    )
    .await
}

async fn seed_approved_partnership_application(
    applicant_user_id: UserId,
    created: OffsetDateTime,
    updated: OffsetDateTime,
) -> (PartnershipApplicationId, PartnershipId, ListingSourceId) {
    let (application_id, partnership_id, listing_source_id) =
        seed_raw_approved_partnership_application(applicant_user_id, created, updated).await;
    (
        application_id,
        PartnershipId::try_from(partnership_id).unwrap_or_else(|error| {
            panic!("central Partnership approval fixture must use UUIDv7: {error}")
        }),
        ListingSourceId::try_from(listing_source_id).unwrap_or_else(|error| {
            panic!("central ListingSource approval fixture must use UUIDv7: {error}")
        }),
    )
}

async fn seed_partnership_membership(user_id: UserId, listing_source_id: ListingSourceId) {
    seed_raw_partnership_membership(user_id, listing_source_id.into_uuid()).await;
}

async fn seed_operator_partnership_listing_source_grant(listing_source_id: ListingSourceId) {
    seed_raw_operator_partnership_listing_source_grant(listing_source_id.into_uuid()).await;
}

async fn get_partnerships(
    token: &str,
    query: &[(&str, &str)],
) -> (reqwest::StatusCode, Value, Option<String>) {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/partnerships", AURA_API.base_url()))
        .bearer_auth(token)
        .query(query)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call admin partnerships API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    (status, body, cache_control)
}

async fn get_partnership_detail(
    token: &str,
    partnership_id: &str,
) -> (reqwest::StatusCode, Value, Option<String>) {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/partnerships/{partnership_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call admin partnership detail API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    (status, body, cache_control)
}

async fn put_partnership_member(
    token: &str,
    partnership_id: &str,
    user_id: &str,
) -> reqwest::Response {
    reqwest::Client::new()
        .put(format!(
            "{}/api/v1/admin/partnerships/{partnership_id}/members/{user_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to grant Partnership membership: {error}"))
}

async fn put_partnership_listing_source_grant(
    token: &str,
    partnership_id: &str,
    listing_source_id: &str,
) -> reqwest::Response {
    reqwest::Client::new()
        .put(format!(
            "{}/api/v1/admin/partnerships/{partnership_id}/listing-source-grants/{listing_source_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to grant Partnership ListingSource access: {error}"))
}

async fn delete_partnership_listing_source_grant(
    token: &str,
    partnership_id: &str,
    listing_source_id: &str,
) -> reqwest::Response {
    reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/partnerships/{partnership_id}/listing-source-grants/{listing_source_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to revoke Partnership ListingSource access: {error}"))
}

async fn delete_partnership_member(
    token: &str,
    partnership_id: &str,
    user_id: &str,
) -> reqwest::Response {
    reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/partnerships/{partnership_id}/members/{user_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to revoke Partnership membership: {error}"))
}

async fn delete_partnership(token: &str, partnership_id: &str) -> reqwest::Response {
    reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/admin/partnerships/{partnership_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to dissolve Partnership: {error}"))
}

fn assert_no_store(cache_control: Option<String>) {
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

fn item_ids(body: &Value) -> Vec<PartnershipId> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("partnership response did not contain an items array"))
        .iter()
        .map(|item| {
            item["partnershipId"]
                .as_str()
                .unwrap_or_else(|| panic!("partnership item did not contain partnershipId"))
                .parse::<PartnershipId>()
                .unwrap_or_else(|error| panic!("partnership ID was not canonical: {error}"))
        })
        .collect()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_safe_admin_partnership_summary_without_cache() {
    let member_one = seed_user("USER").await;
    let member_two = seed_user("USER").await;
    let (listing_source_one, _, _) = seed_listing_source_for_search(
        "Partnership Listing Source One",
        "Partnership Operator One",
        "PARTNER_API",
        None,
    )
    .await;
    let (listing_source_two, _, _) = seed_listing_source_for_search(
        "Partnership Listing Source Two",
        "Partnership Operator Two",
        "PARTNER_API",
        None,
    )
    .await;
    let created = datetime!(2026-07-01 12:00 UTC);
    let updated = datetime!(2026-07-02 12:00 UTC);
    let (partnership_id, party_id) = seed_partnership_for_search(
        "Safe Admin Partnership",
        created,
        updated,
        &[member_one, member_two],
        &[listing_source_one, listing_source_two],
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let (status, body, cache_control) = get_partnerships(&token, &[("size", "1")]).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(cache_control);
    assert_eq!(
        json!({
            "items": [{
                "partnershipId": partnership_id.to_string(),
                "party": {
                    "partyId": party_id.to_string(),
                    "partySlugId": format!("api-partnership-party-{}", party_id.as_uuid()),
                    "name": "Safe Admin Partnership"
                },
                "memberCount": 2,
                "listingSourceGrantCount": 2,
                "created": "2026-07-01T12:00:00Z",
                "updated": "2026-07-02T12:00:00Z"
            }],
            "size": 1
        }),
        body
    );
    assert!(
        body["items"][0]["partnershipId"]
            .as_str()
            .is_some_and(|value| value.starts_with("psh_"))
    );
    assert!(
        body["items"][0]["party"]["partyId"]
            .as_str()
            .is_some_and(|value| value.starts_with("pty_"))
    );
    assert!(body.to_string().find("secret").is_none());
    assert!(body.to_string().find("token").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_filter_admin_partnerships_by_party_member_and_listing_source() {
    let matching_member = seed_user("USER").await;
    let other_member = seed_user("USER").await;
    let (matching_source, _, _) = seed_listing_source_for_search(
        "Matching Partnership Source",
        "Matching Partnership Operator",
        "PARTNER_API",
        None,
    )
    .await;
    let (other_source, _, _) = seed_listing_source_for_search(
        "Other Partnership Source",
        "Other Partnership Operator",
        "PARTNER_API",
        None,
    )
    .await;
    let (matching_partnership, matching_party) = seed_partnership_for_search(
        "Matching Admin Partnership",
        datetime!(2026-07-10 12:00 UTC),
        datetime!(2026-07-10 12:00 UTC),
        &[matching_member],
        &[matching_source],
    )
    .await;
    let (other_partnership, other_party) = seed_partnership_for_search(
        "Other Admin Partnership",
        datetime!(2026-07-09 12:00 UTC),
        datetime!(2026-07-09 12:00 UTC),
        &[other_member],
        &[other_source],
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    for (field, value) in [
        ("partyId", matching_party.to_string()),
        ("memberUserId", matching_member.to_string()),
        ("listingSourceId", matching_source.to_string()),
    ] {
        let (status, body, cache_control) = get_partnerships(&token, &[(field, &value)]).await;
        assert_eq!(reqwest::StatusCode::OK, status, "filter {field}");
        assert_no_store(cache_control);
        assert_eq!(
            vec![matching_partnership],
            item_ids(&body),
            "filter {field}"
        );
    }

    let party = matching_party.to_string();
    let member = matching_member.to_string();
    let source = matching_source.to_string();
    let (status, body, cache_control) = get_partnerships(
        &token,
        &[
            ("partyId", &party),
            ("memberUserId", &member),
            ("listingSourceId", &source),
        ],
    )
    .await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(cache_control);
    assert_eq!(vec![matching_partnership], item_ids(&body));
    assert!(!item_ids(&body).contains(&other_partnership));
    assert_ne!(matching_party, other_party);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_follow_admin_partnership_cursor_with_typed_id_tie_breaking() {
    let timestamp = datetime!(2026-07-20 12:00 UTC);
    let first =
        seed_partnership_for_search("Cursor Partnership One", timestamp, timestamp, &[], &[]).await;
    let second =
        seed_partnership_for_search("Cursor Partnership Two", timestamp, timestamp, &[], &[]).await;
    let third =
        seed_partnership_for_search("Cursor Partnership Three", timestamp, timestamp, &[], &[])
            .await;
    let mut expected = [first.0, second.0, third.0];
    expected.sort_by(|left, right| right.cmp(left));

    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let (status, first_body, first_cache_control) =
        get_partnerships(&token, &[("size", "2")]).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(first_cache_control);
    assert_eq!(json!(2), first_body["size"]);
    assert_eq!(expected[..2], item_ids(&first_body)[..]);
    assert!(
        first_body["searchAfter"][1]
            .as_str()
            .is_some_and(|value| value.starts_with("psh_"))
    );
    let cursor = serde_json::to_string(&first_body["searchAfter"])
        .unwrap_or_else(|error| panic!("failed to serialize partnership cursor: {error}"));

    let (status, second_body, second_cache_control) =
        get_partnerships(&token, &[("size", "2"), ("searchAfter", &cursor)]).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(second_cache_control);
    assert_eq!(json!(2), second_body["size"]);
    assert_eq!(vec![expected[2]], item_ids(&second_body));
    assert!(second_body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_empty_admin_partnership_collection_with_default_size() {
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let missing_party = PartyId::new().to_string();

    let (status, body, cache_control) =
        get_partnerships(&token, &[("partyId", &missing_party)]).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(cache_control);
    assert_eq!(Some(0), body["items"].as_array().map(Vec::len));
    assert_eq!(json!(21), body["size"]);
    assert!(body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_admin_partnership_query_values_with_field_errors() {
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let partnership_id = PartnershipId::new();
    let party_id = PartyId::new();
    let user_id = UserId::new();
    let listing_source_id = ListingSourceId::new();

    for (field, values) in [
        (
            "partyId",
            [
                UserId::new().to_string(),
                party_id.as_uuid().to_string(),
                "pty_not-a-typeid".to_owned(),
            ],
        ),
        (
            "memberUserId",
            [
                PartyId::new().to_string(),
                user_id.as_uuid().to_string(),
                "usr_not-a-typeid".to_owned(),
            ],
        ),
        (
            "listingSourceId",
            [
                PartyId::new().to_string(),
                listing_source_id.as_uuid().to_string(),
                "ls_not-a-typeid".to_owned(),
            ],
        ),
        (
            "searchAfter",
            [
                json!(["2026-07-20T12:00:00Z", PartyId::new()]).to_string(),
                json!(["2026-07-20T12:00:00Z", partnership_id.as_uuid().to_string()]).to_string(),
                json!(["2026-07-20T12:00:00Z", "psh_not-a-typeid"]).to_string(),
            ],
        ),
    ] {
        for value in values {
            let (status, body, cache_control) = get_partnerships(&token, &[(field, &value)]).await;
            assert_no_store(cache_control);
            assert_problem(
                status,
                &body,
                reqwest::StatusCode::BAD_REQUEST,
                "INVALID_OBJECT_ID",
            );
            assert_eq!(json!({"field": field, "type": "QUERY"}), body["source"]);
        }
    }

    let typed_partnership_id = PartnershipId::new();
    let invalid_queries = [
        ("size", "not-a-number".to_owned()),
        ("searchAfter", "not-json".to_owned()),
        (
            "searchAfter",
            json!({"timestamp": "2026-07-20T12:00:00Z"}).to_string(),
        ),
        (
            "searchAfter",
            json!(["not-a-timestamp", typed_partnership_id]).to_string(),
        ),
    ];

    for (field, value) in invalid_queries {
        let (status, body, cache_control) = get_partnerships(&token, &[(field, &value)]).await;
        assert_no_store(cache_control);
        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "BAD_QUERY_PARAMETER_VALUE",
        );
        assert_eq!(json!({"field": field, "type": "QUERY"}), body["source"]);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_non_admin_admin_partnership_collection_access() {
    let user_id = seed_user("USER").await;
    let token =
        String::from(seed_access_token_for(user_id, std::collections::HashSet::new()).await);

    let (status, body, cache_control) = get_partnerships(&token, &[]).await;

    assert_no_store(cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_bounded_admin_partnership_detail_with_current_references() {
    let member_one = seed_user("USER").await;
    let member_two = seed_user("USER").await;
    let (listing_source_one, _, _) = seed_listing_source_for_search(
        "Detail Listing Source One",
        "Detail Operator One",
        "PARTNER_API",
        None,
    )
    .await;
    let (listing_source_two, _, _) = seed_listing_source_for_search(
        "Detail Listing Source Two",
        "Detail Operator Two",
        "PARTNER_API",
        None,
    )
    .await;
    let (partnership_id, party_id) = seed_partnership_for_search(
        "Admin Partnership Detail",
        datetime!(2026-08-01 12:00 UTC),
        datetime!(2026-08-02 12:00 UTC),
        &[member_two, member_one],
        &[listing_source_two, listing_source_one],
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let mut expected_member_ids = [member_one, member_two];
    expected_member_ids.sort();
    let mut expected_listing_source_ids = [listing_source_one, listing_source_two];
    expected_listing_source_ids.sort();

    let (status, body, cache_control) =
        get_partnership_detail(&token, &partnership_id.to_string()).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(cache_control);
    assert_eq!(
        json!({
            "partnershipId": partnership_id.to_string(),
            "party": {
                "partyId": party_id.to_string(),
                "partySlugId": format!("api-partnership-party-{}", party_id.as_uuid()),
                "name": "Admin Partnership Detail"
            },
            "memberUserIds": expected_member_ids,
            "listingSourceIds": expected_listing_source_ids,
            "memberCount": 2,
            "listingSourceGrantCount": 2,
            "created": "2026-08-01T12:00:00Z",
            "updated": "2026-08-02T12:00:00Z"
        }),
        body
    );
    assert!(
        body["partnershipId"]
            .as_str()
            .is_some_and(|value| value.starts_with("psh_"))
    );
    assert!(
        body["party"]["partyId"]
            .as_str()
            .is_some_and(|value| value.starts_with("pty_"))
    );
    assert!(body["memberUserIds"].as_array().is_some_and(|values| {
        values.iter().all(|value| {
            value
                .as_str()
                .is_some_and(|value| value.starts_with("usr_"))
        })
    }));
    assert!(body["listingSourceIds"].as_array().is_some_and(|values| {
        values
            .iter()
            .all(|value| value.as_str().is_some_and(|value| value.starts_with("ls_")))
    }));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_empty_admin_partnership_detail_associations() {
    let (partnership_id, party_id) = seed_partnership_for_search(
        "Empty Admin Partnership Detail",
        datetime!(2026-08-03 12:00 UTC),
        datetime!(2026-08-03 12:00 UTC),
        &[],
        &[],
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let (status, body, cache_control) =
        get_partnership_detail(&token, &partnership_id.to_string()).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(cache_control);
    assert_eq!(json!([]), body["memberUserIds"]);
    assert_eq!(json!([]), body["listingSourceIds"]);
    assert_eq!(json!(0), body["memberCount"]);
    assert_eq!(json!(0), body["listingSourceGrantCount"]);
    assert_eq!(json!(party_id.to_string()), body["party"]["partyId"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_noncanonical_admin_partnership_path_ids() {
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let partnership_id = PartnershipId::new();
    let user_id = UserId::new();
    let listing_source_id = ListingSourceId::new();

    for invalid_id in [
        PartyId::new().to_string(),
        partnership_id.as_uuid().to_string(),
        "psh_not-a-typeid".to_owned(),
    ] {
        let (status, body, cache_control) = get_partnership_detail(&token, &invalid_id).await;

        assert_no_store(cache_control);
        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "INVALID_OBJECT_ID",
        );
        assert_eq!(
            json!({"field": "partnershipId", "type": "PATH"}),
            body["source"]
        );
    }

    for invalid_id in [
        PartyId::new().to_string(),
        user_id.as_uuid().to_string(),
        "usr_not-a-typeid".to_owned(),
    ] {
        let response =
            put_partnership_member(&token, &partnership_id.to_string(), &invalid_id).await;
        let cache_control = response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let (status, body) = json_response(response).await;

        assert_no_store(cache_control);
        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "INVALID_OBJECT_ID",
        );
        assert_eq!(json!({"field": "userId", "type": "PATH"}), body["source"]);
    }

    for invalid_id in [
        PartyId::new().to_string(),
        listing_source_id.as_uuid().to_string(),
        "ls_not-a-typeid".to_owned(),
    ] {
        let response =
            put_partnership_listing_source_grant(&token, &partnership_id.to_string(), &invalid_id)
                .await;
        let cache_control = response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let (status, body) = json_response(response).await;

        assert_no_store(cache_control);
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
async fn should_return_not_found_for_missing_admin_partnership_detail() {
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let (status, body, cache_control) =
        get_partnership_detail(&token, &PartnershipId::new().to_string()).await;

    assert_no_store(cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PARTNERSHIP_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_non_admin_admin_partnership_detail_access() {
    let user_id = seed_user("USER").await;
    let token =
        String::from(seed_access_token_for(user_id, std::collections::HashSet::new()).await);

    let (status, body, cache_control) =
        get_partnership_detail(&token, &PartnershipId::new().to_string()).await;

    assert_no_store(cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_grant_admin_partnership_membership_idempotently() {
    let target_user_id = seed_user("USER").await;
    let (partnership_id, _) = seed_partnership_for_search(
        "Membership Grant Partnership",
        datetime!(2026-08-10 12:00 UTC),
        datetime!(2026-08-10 12:00 UTC),
        &[],
        &[],
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    for _ in 0..2 {
        let response = put_partnership_member(
            &token,
            &partnership_id.to_string(),
            &target_user_id.to_string(),
        )
        .await;
        assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        assert_no_store(
            response
                .headers()
                .get(reqwest::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        );
        assert!(response.bytes().await.is_ok_and(|body| body.is_empty()));
    }

    let (status, body, cache_control) =
        get_partnership_detail(&token, &partnership_id.to_string()).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(cache_control);
    assert_eq!(json!(1), body["memberCount"]);
    assert_eq!(json!([target_user_id]), body["memberUserIds"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_revoke_admin_partnership_membership_idempotently_and_preserve_related_records() {
    let pool = get_postgres_client().await;
    seed_current_fx_snapshot(&pool).await;
    let target_user_id = seed_user("USER").await;
    let (application_id, partnership_id, listing_source_id) =
        seed_approved_partnership_application(
            target_user_id,
            datetime!(2026-08-13 12:00 UTC),
            datetime!(2026-08-13 12:00 UTC),
        )
        .await;
    seed_partnership_membership(target_user_id, listing_source_id).await;
    seed_operator_partnership_listing_source_grant(listing_source_id).await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let partner_token = String::from(
        seed_access_token_for(
            target_user_id,
            std::collections::HashSet::from([Scope::ProductListingsWrite]),
        )
        .await,
    );

    let before_revoke = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/listing-sources/{listing_source_id}/product-listings",
            AURA_API.base_url()
        ))
        .bearer_auth(&partner_token)
        .json(&json!([{
            "sourceListingId": "before-revoke",
            "title": {"text": "Partner listing", "language": "en"},
            "description": {"text": "Partner listing", "language": "en"},
            "availability": "AVAILABLE",
            "url": "https://partner.example/before-revoke",
            "images": []
        }]))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to verify partner authorization before revoke: {error}")
        });
    assert_eq!(reqwest::StatusCode::OK, before_revoke.status());

    for _ in 0..2 {
        let response = delete_partnership_member(
            &admin_token,
            &partnership_id.to_string(),
            &target_user_id.to_string(),
        )
        .await;
        assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        assert_no_store(
            response
                .headers()
                .get(reqwest::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        );
        assert!(response.bytes().await.is_ok_and(|body| body.is_empty()));
    }

    let after_revoke = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/listing-sources/{listing_source_id}/product-listings",
            AURA_API.base_url()
        ))
        .bearer_auth(&partner_token)
        .json(&json!([{
            "sourceListingId": "after-revoke",
            "title": {"text": "Partner listing", "language": "en"},
            "description": {"text": "Partner listing", "language": "en"},
            "availability": "AVAILABLE",
            "url": "https://partner.example/after-revoke",
            "images": []
        }]))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to verify partner authorization after revoke: {error}")
        });
    let (status, body) = json_response(after_revoke).await;
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");

    assert_eq!(
        0,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM partnership_members WHERE user_id = $1 AND partnership_id = $2",
        )
        .bind(target_user_id.as_uuid())
        .bind(partnership_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to count revoked membership: {error}"))
    );
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE user_id = $1")
            .bind(target_user_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to verify preserved user: {error}"))
    );
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM partnerships WHERE partnership_id = $1",
        )
        .bind(partnership_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to verify preserved Partnership: {error}"))
    );
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM listing_sources WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to verify preserved ListingSource: {error}"))
    );
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM partnership_applications WHERE partnership_application_id = $1",
        )
        .bind(application_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!(
            "failed to verify preserved PartnershipApplication: {error}"
        ))
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_grant_admin_partnership_listing_source_idempotently_and_enable_partner_writes() {
    let pool = get_postgres_client().await;
    seed_current_fx_snapshot(&pool).await;
    let partner_id = seed_user("USER").await;
    let (_application_id, partnership_id, listing_source_id) =
        seed_approved_partnership_application(
            partner_id,
            datetime!(2026-08-16 12:00 UTC),
            datetime!(2026-08-16 12:00 UTC),
        )
        .await;
    seed_partnership_membership(partner_id, listing_source_id).await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let partner_token = String::from(
        seed_access_token_for(
            partner_id,
            std::collections::HashSet::from([Scope::ProductListingsWrite]),
        )
        .await,
    );

    for _ in 0..2 {
        let response = put_partnership_listing_source_grant(
            &admin_token,
            &partnership_id.to_string(),
            &listing_source_id.to_string(),
        )
        .await;
        assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        assert_no_store(
            response
                .headers()
                .get(reqwest::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        );
        assert!(response.bytes().await.is_ok_and(|body| body.is_empty()));
    }

    let (status, body, cache_control) =
        get_partnership_detail(&admin_token, &partnership_id.to_string()).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(cache_control);
    assert_eq!(json!(1), body["listingSourceGrantCount"]);
    assert_eq!(json!([listing_source_id]), body["listingSourceIds"]);

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/listing-sources/{listing_source_id}/product-listings",
            AURA_API.base_url()
        ))
        .bearer_auth(&partner_token)
        .json(&json!([{
            "sourceListingId": "granted-source-listing",
            "title": {"text": "Granted listing", "language": "en"},
            "description": {"text": "Granted listing", "language": "en"},
            "availability": "AVAILABLE",
            "url": "https://partner.example/granted-source-listing",
            "images": []
        }]))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to verify partner authorization after grant: {error}")
        });
    assert_eq!(reqwest::StatusCode::OK, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_revoke_admin_partnership_listing_source_idempotently_and_preserve_related_records()
{
    let pool = get_postgres_client().await;
    seed_current_fx_snapshot(&pool).await;
    let partner_id = seed_user("USER").await;
    let (application_id, partnership_id, listing_source_id) =
        seed_approved_partnership_application(
            partner_id,
            datetime!(2026-08-20 12:00 UTC),
            datetime!(2026-08-20 12:00 UTC),
        )
        .await;
    seed_partnership_membership(partner_id, listing_source_id).await;
    seed_operator_partnership_listing_source_grant(listing_source_id).await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let partner_token = String::from(
        seed_access_token_for(
            partner_id,
            std::collections::HashSet::from([Scope::ProductListingsWrite]),
        )
        .await,
    );

    let before_revoke = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/listing-sources/{listing_source_id}/product-listings",
            AURA_API.base_url()
        ))
        .bearer_auth(&partner_token)
        .json(&json!([{
            "sourceListingId": "listing-source-grant-before-revoke",
            "title": {"text": "Partner listing", "language": "en"},
            "description": {"text": "Partner listing", "language": "en"},
            "availability": "AVAILABLE",
            "url": "https://partner.example/listing-source-grant-before-revoke",
            "images": []
        }]))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to verify partner authorization before ListingSource revoke: {error}")
        });
    assert_eq!(reqwest::StatusCode::OK, before_revoke.status());

    for _ in 0..2 {
        let response = delete_partnership_listing_source_grant(
            &admin_token,
            &partnership_id.to_string(),
            &listing_source_id.to_string(),
        )
        .await;
        assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        assert_no_store(
            response
                .headers()
                .get(reqwest::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        );
        assert!(response.bytes().await.is_ok_and(|body| body.is_empty()));
    }

    let after_revoke = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/listing-sources/{listing_source_id}/product-listings",
            AURA_API.base_url()
        ))
        .bearer_auth(&partner_token)
        .json(&json!([{
            "sourceListingId": "listing-source-grant-after-revoke",
            "title": {"text": "Partner listing", "language": "en"},
            "description": {"text": "Partner listing", "language": "en"},
            "availability": "AVAILABLE",
            "url": "https://partner.example/listing-source-grant-after-revoke",
            "images": []
        }]))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to verify partner authorization after ListingSource revoke: {error}")
        });
    let (status, body) = json_response(after_revoke).await;
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");

    assert_eq!(
        0,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM partnership_listing_source_grants WHERE partnership_id = $1 AND listing_source_id = $2",
        )
        .bind(partnership_id.as_uuid())
        .bind(listing_source_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to count revoked ListingSource grant: {error}"))
    );
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM partnership_members WHERE user_id = $1 AND partnership_id = $2",
        )
        .bind(partner_id.as_uuid())
        .bind(partnership_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to verify preserved Partnership member: {error}"))
    );
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE user_id = $1")
            .bind(partner_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to verify preserved user: {error}"))
    );
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM partnerships WHERE partnership_id = $1",
        )
        .bind(partnership_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to verify preserved Partnership: {error}"))
    );
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM listing_sources WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to verify preserved ListingSource: {error}"))
    );
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM partnership_applications WHERE partnership_application_id = $1",
        )
        .bind(application_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!(
            "failed to verify preserved PartnershipApplication: {error}"
        ))
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_listing_source_grant_for_a_different_party() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source().await;
    let (partnership_id, _) = seed_partnership_for_search(
        "Different ListingSource Grant Party",
        datetime!(2026-08-17 12:00 UTC),
        datetime!(2026-08-17 12:00 UTC),
        &[],
        &[],
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let response = put_partnership_listing_source_grant(
        &token,
        &partnership_id.to_string(),
        &listing_source_id.to_string(),
    )
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_no_store(cache_control);
    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");
    assert_eq!(
        0,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM partnership_listing_source_grants WHERE partnership_id = $1 AND listing_source_id = $2",
        )
        .bind(partnership_id.as_uuid())
        .bind(listing_source_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to count mismatched grant: {error}"))
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_for_missing_admin_listing_source_grant_targets() {
    let listing_source_id = seed_listing_source().await;
    let (partnership_id, _) = seed_partnership_for_search(
        "Missing ListingSource Grant Targets",
        datetime!(2026-08-18 12:00 UTC),
        datetime!(2026-08-18 12:00 UTC),
        &[],
        &[],
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let response = put_partnership_listing_source_grant(
        &token,
        &partnership_id.to_string(),
        &ListingSourceId::new().to_string(),
    )
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    assert_no_store(cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "LISTING_SOURCE_NOT_FOUND",
    );

    let response = put_partnership_listing_source_grant(
        &token,
        &PartnershipId::new().to_string(),
        &listing_source_id.to_string(),
    )
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    assert_no_store(cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PARTNERSHIP_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_non_admin_partnership_listing_source_grant() {
    let actor_id = seed_user("USER").await;
    let (_application_id, partnership_id, listing_source_id) =
        seed_approved_partnership_application(
            actor_id,
            datetime!(2026-08-19 12:00 UTC),
            datetime!(2026-08-19 12:00 UTC),
        )
        .await;
    let token =
        String::from(seed_access_token_for(actor_id, std::collections::HashSet::new()).await);

    let response = put_partnership_listing_source_grant(
        &token,
        &partnership_id.to_string(),
        &listing_source_id.to_string(),
    )
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_no_store(cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_for_missing_admin_listing_source_grant_revoke_targets() {
    let listing_source_id = seed_listing_source().await;
    let (partnership_id, _) = seed_partnership_for_search(
        "Missing ListingSource Grant Revoke Targets",
        datetime!(2026-08-20 12:00 UTC),
        datetime!(2026-08-20 12:00 UTC),
        &[],
        &[],
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let response = delete_partnership_listing_source_grant(
        &token,
        &partnership_id.to_string(),
        &ListingSourceId::new().to_string(),
    )
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    assert_no_store(cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "LISTING_SOURCE_NOT_FOUND",
    );

    let response = delete_partnership_listing_source_grant(
        &token,
        &PartnershipId::new().to_string(),
        &listing_source_id.to_string(),
    )
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    assert_no_store(cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PARTNERSHIP_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_non_admin_partnership_listing_source_grant_revoke() {
    let pool = get_postgres_client().await;
    let partner_id = seed_user("USER").await;
    let (_application_id, partnership_id, listing_source_id) =
        seed_approved_partnership_application(
            partner_id,
            datetime!(2026-08-21 12:00 UTC),
            datetime!(2026-08-21 12:00 UTC),
        )
        .await;
    seed_operator_partnership_listing_source_grant(listing_source_id).await;
    let token =
        String::from(seed_access_token_for(partner_id, std::collections::HashSet::new()).await);

    let response = delete_partnership_listing_source_grant(
        &token,
        &partnership_id.to_string(),
        &listing_source_id.to_string(),
    )
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_no_store(cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM partnership_listing_source_grants WHERE partnership_id = $1 AND listing_source_id = $2",
        )
        .bind(partnership_id.as_uuid())
        .bind(listing_source_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to verify grant after rejected revoke: {error}"))
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_user_not_found_when_grant_target_is_missing() {
    let (partnership_id, _) = seed_partnership_for_search(
        "Missing Membership User Partnership",
        datetime!(2026-08-11 12:00 UTC),
        datetime!(2026-08-11 12:00 UTC),
        &[],
        &[],
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let response = put_partnership_member(
        &token,
        &partnership_id.to_string(),
        &UserId::new().to_string(),
    )
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_no_store(cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_partnership_not_found_when_grant_partnership_is_missing() {
    let target_user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let response = put_partnership_member(
        &token,
        &PartnershipId::new().to_string(),
        &target_user_id.to_string(),
    )
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_no_store(cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PARTNERSHIP_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_non_admin_partnership_membership_grant() {
    let actor_id = seed_user("USER").await;
    let target_user_id = seed_user("USER").await;
    let (partnership_id, _) = seed_partnership_for_search(
        "Non Admin Membership Grant Partnership",
        datetime!(2026-08-12 12:00 UTC),
        datetime!(2026-08-12 12:00 UTC),
        &[],
        &[],
    )
    .await;
    let token =
        String::from(seed_access_token_for(actor_id, std::collections::HashSet::new()).await);

    let response = put_partnership_member(
        &token,
        &partnership_id.to_string(),
        &target_user_id.to_string(),
    )
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_no_store(cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_user_not_found_when_revoke_target_is_missing() {
    let (partnership_id, _) = seed_partnership_for_search(
        "Missing Revoke User Partnership",
        datetime!(2026-08-14 12:00 UTC),
        datetime!(2026-08-14 12:00 UTC),
        &[],
        &[],
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let response = delete_partnership_member(
        &token,
        &partnership_id.to_string(),
        &UserId::new().to_string(),
    )
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_no_store(cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "USER_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_partnership_not_found_when_revoke_partnership_is_missing() {
    let target_user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let response = delete_partnership_member(
        &token,
        &PartnershipId::new().to_string(),
        &target_user_id.to_string(),
    )
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_no_store(cache_control);
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PARTNERSHIP_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_non_admin_partnership_membership_revoke() {
    let actor_id = seed_user("USER").await;
    let target_user_id = seed_user("USER").await;
    let (partnership_id, _) = seed_partnership_for_search(
        "Non Admin Membership Revoke Partnership",
        datetime!(2026-08-15 12:00 UTC),
        datetime!(2026-08-15 12:00 UTC),
        &[target_user_id],
        &[],
    )
    .await;
    let token =
        String::from(seed_access_token_for(actor_id, std::collections::HashSet::new()).await);

    let response = delete_partnership_member(
        &token,
        &partnership_id.to_string(),
        &target_user_id.to_string(),
    )
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_no_store(cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_dissolve_partnership_idempotently_revoke_access_and_preserve_history() {
    let pool = get_postgres_client().await;
    seed_current_fx_snapshot(&pool).await;
    let partner_id = seed_user("USER").await;
    let (application_id, partnership_id, listing_source_id) =
        seed_approved_partnership_application(
            partner_id,
            datetime!(2026-09-05 12:00 UTC),
            datetime!(2026-09-05 12:00 UTC),
        )
        .await;
    seed_partnership_membership(partner_id, listing_source_id).await;
    seed_operator_partnership_listing_source_grant(listing_source_id).await;
    let admin_id = seed_user("ADMIN").await;
    let admin_token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let partner_token = String::from(
        seed_access_token_for(
            partner_id,
            std::collections::HashSet::from([Scope::ProductListingsWrite]),
        )
        .await,
    );

    let before_dissolve = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/listing-sources/{listing_source_id}/product-listings",
            AURA_API.base_url()
        ))
        .bearer_auth(&partner_token)
        .json(&json!([{
            "sourceListingId": "before-dissolve",
            "title": {"text": "Before dissolve", "language": "en"},
            "description": {"text": "Before dissolve", "language": "en"},
            "availability": "AVAILABLE",
            "url": "https://partner.example/before-dissolve",
            "images": []
        }]))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to verify partner authorization: {error}"));
    assert_eq!(reqwest::StatusCode::OK, before_dissolve.status());

    for _ in 0..2 {
        let response = delete_partnership(&admin_token, &partnership_id.to_string()).await;
        assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        assert_no_store(
            response
                .headers()
                .get(reqwest::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        );
        assert!(response.bytes().await.is_ok_and(|body| body.is_empty()));
    }

    let after_dissolve = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/listing-sources/{listing_source_id}/product-listings",
            AURA_API.base_url()
        ))
        .bearer_auth(&partner_token)
        .json(&json!([{
            "sourceListingId": "after-dissolve",
            "title": {"text": "After dissolve", "language": "en"},
            "description": {"text": "After dissolve", "language": "en"},
            "availability": "AVAILABLE",
            "url": "https://partner.example/after-dissolve",
            "images": []
        }]))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to verify revoked partner authorization: {error}"));
    let (status, body) = json_response(after_dissolve).await;
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");

    let (status, detail, cache_control) =
        get_partnership_detail(&admin_token, &partnership_id.to_string()).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(cache_control);
    assert_eq!(json!([]), detail["memberUserIds"]);
    assert_eq!(json!([]), detail["listingSourceIds"]);
    assert_eq!(json!(0), detail["memberCount"]);
    assert_eq!(json!(0), detail["listingSourceGrantCount"]);

    assert_eq!(
        "DISSOLVED",
        sqlx::query_scalar::<_, String>(
            "SELECT business_state FROM partnerships WHERE partnership_id = $1",
        )
        .bind(partnership_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to read dissolved Partnership: {error}"))
    );
    assert_eq!(
        0,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM partnership_members WHERE partnership_id = $1",
        )
        .bind(partnership_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to count dissolved members: {error}"))
    );
    assert_eq!(
        0,
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM partnership_listing_source_grants WHERE partnership_id = $1",
        )
        .bind(partnership_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to count dissolved grants: {error}"))
    );
    assert_eq!(
        Some(*partnership_id.as_uuid()),
        sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT approved_partnership_id FROM partnership_applications WHERE partnership_application_id = $1",
        )
        .bind(application_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to read historical PartnershipApplication: {error}"))
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_missing_and_non_admin_partnership_dissolution() {
    let admin_id = seed_user("ADMIN").await;
    let admin_token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let user_id = seed_user("USER").await;
    let user_token =
        String::from(seed_access_token_for(user_id, std::collections::HashSet::new()).await);

    let response = delete_partnership(&admin_token, "psh_not-a-typeid").await;
    let (status, body) = json_response(response).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_OBJECT_ID",
    );
    assert_eq!(
        json!({"field": "partnershipId", "type": "PATH"}),
        body["source"]
    );

    let response = delete_partnership(&admin_token, &PartnershipId::new().to_string()).await;
    let (status, body) = json_response(response).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PARTNERSHIP_NOT_FOUND",
    );

    let (partnership_id, _) = seed_partnership_for_search(
        "Non Admin Partnership Dissolve",
        datetime!(2026-09-05 12:00 UTC),
        datetime!(2026-09-05 12:00 UTC),
        &[],
        &[],
    )
    .await;
    let response = delete_partnership(&user_token, &partnership_id.to_string()).await;
    let (status, body) = json_response(response).await;
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}
