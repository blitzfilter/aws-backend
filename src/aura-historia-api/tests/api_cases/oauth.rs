use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{json_response, seed_access_token_for, seed_user};
use test_api::{IntegrationTestService, aura_integration_test};
use user_core::access_token::Scope;

struct OAuthClientCredentials {
    client_id: String,
    client_secret: String,
    client_id_issued_at: i64,
}

async fn authenticated_client() -> (reqwest::Client, String) {
    let user_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensRead, Scope::AccessTokensWrite]),
    )
    .await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|error| panic!("failed to build HTTP client: {error}"));
    (client, String::from(token))
}

async fn create_oauth_client(client: &reqwest::Client, token: &str) -> OAuthClientCredentials {
    create_oauth_client_with_name(client, token, "Acceptance OAuth Client").await
}

async fn create_oauth_client_with_name(
    client: &reqwest::Client,
    token: &str,
    client_name: &str,
) -> OAuthClientCredentials {
    let response = client
        .post(format!(
            "{}/api/v1/admin/oauth-clients",
            AURA_API.base_url()
        ))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "client_name": client_name,
            "tos_uri": "https://client.example/tos",
            "policy_uri": "https://client.example/policy",
            "client_uri": "https://client.example",
            "logo_uri": "https://client.example/logo.png",
            "redirect_uris": ["https://client.example/callback"],
            "scope": ["access-tokens:read"]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create OAuth client: {error}"));
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::CREATED, status);
    let client_id = body["client_id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing client_id"))
        .to_owned();
    assert_eq!(
        Some(format!("/api/v1/admin/oauth-clients/{client_id}")),
        location
    );
    assert_eq!(Some("no-store".to_owned()), cache_control);
    OAuthClientCredentials {
        client_id,
        client_secret: body["client_secret"]
            .as_str()
            .unwrap_or_else(|| panic!("missing client_secret"))
            .to_owned(),
        client_id_issued_at: body["client_id_issued_at"]
            .as_i64()
            .unwrap_or_else(|| panic!("missing client_id_issued_at")),
    }
}

async fn admin_read_token() -> String {
    let admin_id = seed_user("ADMIN").await;
    String::from(
        seed_access_token_for(
            admin_id,
            std::collections::HashSet::from([Scope::AccessTokensRead]),
        )
        .await,
    )
}

async fn authorize_code(
    client: &reqwest::Client,
    token: &str,
    credentials: &OAuthClientCredentials,
) -> String {
    let response = client
        .get(format!("{}/api/v1/oauth/authorize", AURA_API.base_url()))
        .bearer_auth(token)
        .query(&[
            ("response_type", "code"),
            ("client_id", credentials.client_id.as_str()),
            ("redirect_uri", "https://client.example/callback"),
            ("scope", "access-tokens:read"),
            ("state", "acceptance-state"),
            (
                "code_challenge",
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            ),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to authorize OAuth client: {error}"));
    assert_eq!(reqwest::StatusCode::FOUND, response.status());
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| panic!("missing redirect location"));
    let redirect =
        url::Url::parse(location).unwrap_or_else(|error| panic!("invalid redirect URL: {error}"));
    assert_eq!(
        Some("acceptance-state".to_owned()),
        redirect
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.to_string())
    );
    redirect
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.to_string())
        .unwrap_or_else(|| panic!("missing authorization code"))
}

async fn exchange_code_response(
    client: &reqwest::Client,
    credentials: &OAuthClientCredentials,
    code: &str,
) -> (reqwest::StatusCode, serde_json::Value) {
    let response = client
        .post(format!("{}/api/v1/oauth/token", AURA_API.base_url()))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", "https://client.example/callback"),
            ("client_id", credentials.client_id.as_str()),
            ("client_secret", credentials.client_secret.as_str()),
            (
                "code_verifier",
                "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            ),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to exchange OAuth token: {error}"));
    json_response(response).await
}

async fn exchange_code(
    client: &reqwest::Client,
    credentials: &OAuthClientCredentials,
    code: &str,
) -> serde_json::Value {
    let (status, body) = exchange_code_response(client, credentials, code).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    body
}

async fn exchange_third_party_code_response(
    client: &reqwest::Client,
    code: &str,
) -> (reqwest::StatusCode, serde_json::Value) {
    let response = client
        .get(format!(
            "{}/api/v1/oauth/tokens/by-third-party-code/{}",
            AURA_API.base_url(),
            code
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to exchange third-party code: {error}"));
    json_response(response).await
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_list_get_update_and_delete_oauth_client() {
    let (client, token) = authenticated_client().await;
    let credentials = create_oauth_client(&client, &token).await;

    let admin_token = admin_read_token().await;
    let response = client
        .get(format!(
            "{}/api/v1/admin/oauth-clients",
            AURA_API.base_url()
        ))
        .bearer_auth(&admin_token)
        .query(&[("name", "Acceptance OAuth"), ("size", "100")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list OAuth clients: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(serde_json::json!(100), body["size"]);
    assert!(body.get("searchAfter").is_none());
    assert_eq!(
        Some(credentials.client_id_issued_at),
        body["items"][0]["client_id_issued_at"].as_i64()
    );
    assert_eq!(
        serde_json::json!(credentials.client_id),
        body["items"][0]["client_id"]
    );
    assert!(body["items"][0].get("client_secret").is_none());

    let response = client
        .post(format!("{}/api/v1/oauth/clients", AURA_API.base_url()))
        .bearer_auth(&admin_token)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to check removed OAuth client create route: {error}")
        });
    assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());

    let response = client
        .get(format!(
            "{}/api/v1/admin/oauth-clients/{}",
            AURA_API.base_url(),
            credentials.client_id
        ))
        .bearer_auth(&admin_token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get admin OAuth client: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(serde_json::json!(credentials.client_id), body["client_id"]);
    assert_eq!(
        serde_json::json!("Acceptance OAuth Client"),
        body["client_name"]
    );
    assert_eq!(
        serde_json::json!(["https://client.example/callback"]),
        body["redirect_uris"]
    );
    assert_eq!(serde_json::json!(["access-tokens:read"]), body["scope"]);
    assert_eq!(
        Some(credentials.client_id_issued_at),
        body["client_id_issued_at"].as_i64()
    );
    assert!(body.get("client_secret").is_none());

    let response = client
        .get(format!(
            "{}/api/v1/oauth/clients/{}",
            AURA_API.base_url(),
            credentials.client_id
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to check removed OAuth client detail route: {error}")
        });
    assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());

    let response = client
        .patch(format!(
            "{}/api/v1/oauth/clients/{}",
            AURA_API.base_url(),
            credentials.client_id
        ))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "client_name": "Legacy Route Must Be Removed" }))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to check removed OAuth client update route: {error}")
        });
    assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());

    let response = client
        .patch(format!(
            "{}/api/v1/admin/oauth-clients/{}",
            AURA_API.base_url(),
            credentials.client_id
        ))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "client_name": "Updated OAuth Client",
            "tos_uri": "https://client.example/updated-tos",
            "policy_uri": "https://client.example/updated-policy",
            "client_uri": "https://updated-client.example",
            "logo_uri": "https://updated-client.example/logo.png",
            "redirect_uris": ["https://client.example/updated-callback"],
            "scope": ["access-tokens:write"]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to update OAuth client: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(
        serde_json::json!("Updated OAuth Client"),
        body["client_name"]
    );
    assert_eq!(
        serde_json::json!("https://client.example/updated-tos"),
        body["tos_uri"]
    );
    assert_eq!(
        serde_json::json!("https://client.example/updated-policy"),
        body["policy_uri"]
    );
    assert_eq!(
        serde_json::json!("https://updated-client.example/"),
        body["client_uri"]
    );
    assert_eq!(
        serde_json::json!("https://updated-client.example/logo.png"),
        body["logo_uri"]
    );
    assert_eq!(
        serde_json::json!(["https://client.example/updated-callback"]),
        body["redirect_uris"]
    );
    assert_eq!(serde_json::json!(["access-tokens:write"]), body["scope"]);
    assert_eq!(
        Some(credentials.client_id_issued_at),
        body["client_id_issued_at"].as_i64()
    );
    assert!(body.get("client_secret").is_none());
    assert!(!body.to_string().contains(&credentials.client_secret));

    let response = client
        .patch(format!(
            "{}/api/v1/admin/oauth-clients/{}",
            AURA_API.base_url(),
            credentials.client_id
        ))
        .bearer_auth(&token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to apply OAuth client no-op patch: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(
        serde_json::json!("Updated OAuth Client"),
        body["client_name"]
    );
    assert!(body.get("client_secret").is_none());

    let response = client
        .delete(format!(
            "{}/api/v1/oauth/clients/{}",
            AURA_API.base_url(),
            credentials.client_id
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to check removed OAuth client delete route: {error}")
        });
    assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());

    let response = client
        .delete(format!(
            "{}/api/v1/admin/oauth-clients/{}",
            AURA_API.base_url(),
            credentials.client_id
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete OAuth client: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert_eq!(
        Some("no-store"),
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_admin_role_and_delegated_write_for_oauth_client_deletion() {
    let (client, admin_token) = authenticated_client().await;
    let credentials = create_oauth_client(&client, &admin_token).await;
    let url = format!(
        "{}/api/v1/admin/oauth-clients/{}",
        AURA_API.base_url(),
        credentials.client_id
    );

    let non_admin_id = seed_user("USER").await;
    let non_admin_token = seed_access_token_for(
        non_admin_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;
    let response = client
        .delete(&url)
        .bearer_auth(String::from(non_admin_token))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to reject non-admin OAuth client deletion: {error}")
        });
    let (status, body) = json_response(response).await;
    api_support::assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");

    let admin_without_write =
        seed_access_token_for(seed_user("ADMIN").await, Default::default()).await;
    let response = client
        .delete(&url)
        .bearer_auth(String::from(admin_without_write))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to reject OAuth client deletion without delegated write: {error}")
        });
    let (status, body) = json_response(response).await;
    api_support::assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");

    let response = client
        .delete(url)
        .bearer_auth(admin_token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete OAuth client as admin: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_invalidate_oauth_credentials_when_client_is_deleted() {
    let (client, admin_token) = authenticated_client().await;
    let credentials = create_oauth_client(&client, &admin_token).await;
    let pending_code = authorize_code(&client, &admin_token, &credentials).await;
    let issued = exchange_code(
        &client,
        &credentials,
        &authorize_code(&client, &admin_token, &credentials).await,
    )
    .await;
    let access_token = issued["access_token"]
        .as_str()
        .unwrap_or_else(|| panic!("missing issued access token"))
        .to_owned();
    let third_party_code = issued["third_party_exchange_code"]
        .as_str()
        .unwrap_or_else(|| panic!("missing issued third-party exchange code"))
        .to_owned();

    let delete_url = format!(
        "{}/api/v1/admin/oauth-clients/{}",
        AURA_API.base_url(),
        credentials.client_id
    );
    let response = client
        .delete(&delete_url)
        .bearer_auth(&admin_token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete OAuth client: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());

    let (status, body) = exchange_code_response(&client, &credentials, &pending_code).await;
    api_support::assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "OAUTH_CLIENT_NOT_FOUND",
    );

    let response = client
        .get(format!("{}/api/v1/oauth/authorize", AURA_API.base_url()))
        .bearer_auth(&admin_token)
        .query(&[
            ("response_type", "code"),
            ("client_id", credentials.client_id.as_str()),
            ("redirect_uri", "https://client.example/callback"),
            ("scope", "access-tokens:read"),
            (
                "code_challenge",
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            ),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to reject authorization for deleted client: {error}")
        });
    let (status, body) = json_response(response).await;
    api_support::assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "OAUTH_CLIENT_NOT_FOUND",
    );

    let response = client
        .get(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
        .bearer_auth(&access_token)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to authenticate revoked OAuth access token: {error}")
        });
    let (status, body) = json_response(response).await;
    api_support::assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );

    let response = client
        .get(format!(
            "{}/api/v1/oauth/tokens/by-third-party-code/{}",
            AURA_API.base_url(),
            third_party_code
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject deleted client's exchange code: {error}"));
    let (status, body) = json_response(response).await;
    api_support::assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "OAUTH_THIRD_PARTY_EXCHANGE_CODE_NOT_FOUND",
    );

    let response = client
        .delete(delete_url)
        .bearer_auth(admin_token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to report repeated OAuth client deletion: {error}"));
    let (status, body) = json_response(response).await;
    api_support::assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "OAUTH_CLIENT_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_canonical_problems_for_invalid_or_missing_admin_oauth_client() {
    let admin_token = admin_read_token().await;
    let client = reqwest::Client::new();

    let response = client
        .get(format!(
            "{}/api/v1/admin/oauth-clients/not-a-uuid",
            AURA_API.base_url()
        ))
        .bearer_auth(&admin_token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject invalid OAuth client ID: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    api_support::assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
    assert_eq!(Some("no-store".to_owned()), cache_control);

    let missing_client_id = uuid::Uuid::new_v4();
    let response = client
        .get(format!(
            "{}/api/v1/admin/oauth-clients/{missing_client_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(admin_token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get missing OAuth client: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    api_support::assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "OAUTH_CLIENT_NOT_FOUND",
    );
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_admin_role_and_delegated_read_for_oauth_client_detail() {
    let client = reqwest::Client::new();
    let client_id = uuid::Uuid::new_v4();
    let path = format!(
        "{}/api/v1/admin/oauth-clients/{client_id}",
        AURA_API.base_url()
    );

    let non_admin_id = seed_user("USER").await;
    let non_admin_token = seed_access_token_for(
        non_admin_id,
        std::collections::HashSet::from([Scope::AccessTokensRead]),
    )
    .await;
    let response = client
        .get(&path)
        .bearer_auth(String::from(non_admin_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin OAuth client detail: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    api_support::assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
    assert_eq!(Some("no-store".to_owned()), cache_control);

    let admin_id = seed_user("ADMIN").await;
    let admin_without_read = seed_access_token_for(admin_id, Default::default()).await;
    let response = client
        .get(path)
        .bearer_auth(String::from(admin_without_read))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to reject OAuth client detail without delegated read: {error}")
        });
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    api_support::assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_oauth_client_creation_for_non_admin() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/oauth-clients",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "client_name": "Non-admin OAuth Client",
            "tos_uri": "https://client.example/tos",
            "policy_uri": "https://client.example/policy",
            "client_uri": "https://client.example",
            "logo_uri": "https://client.example/logo.png",
            "redirect_uris": ["https://client.example/callback"],
            "scope": ["access-tokens:read"]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin OAuth client create: {error}"));
    let (status, body) = json_response(response).await;

    api_support::assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_oauth_client_creation_for_admin_without_delegated_write() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, Default::default()).await;
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/oauth-clients",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "client_name": "Missing Capability OAuth Client",
            "tos_uri": "https://client.example/tos",
            "policy_uri": "https://client.example/policy",
            "client_uri": "https://client.example",
            "logo_uri": "https://client.example/logo.png",
            "redirect_uris": ["https://client.example/callback"],
            "scope": ["access-tokens:read"]
        }))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to reject OAuth client create without write: {error}")
        });
    let (status, body) = json_response(response).await;

    api_support::assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_admin_role_and_delegated_write_for_oauth_client_update() {
    let (client, admin_token) = authenticated_client().await;
    let credentials = create_oauth_client(&client, &admin_token).await;
    let url = format!(
        "{}/api/v1/admin/oauth-clients/{}",
        AURA_API.base_url(),
        credentials.client_id
    );

    let non_admin_id = seed_user("USER").await;
    let non_admin_token = seed_access_token_for(
        non_admin_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite]),
    )
    .await;
    let response = client
        .patch(&url)
        .bearer_auth(String::from(non_admin_token))
        .json(&serde_json::json!({ "client_name": "Rejected Non-admin Update" }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin OAuth client update: {error}"));
    let (status, body) = json_response(response).await;
    api_support::assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");

    let admin_id = seed_user("ADMIN").await;
    let admin_without_write = seed_access_token_for(admin_id, Default::default()).await;
    let response = client
        .patch(url)
        .bearer_auth(String::from(admin_without_write))
        .json(&serde_json::json!({ "client_name": "Rejected Missing Capability" }))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to reject OAuth client update without delegated write: {error}")
        });
    let (status, body) = json_response(response).await;
    api_support::assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_validate_admin_oauth_client_update_and_report_not_found() {
    let (client, token) = authenticated_client().await;
    let credentials = create_oauth_client(&client, &token).await;
    let url = format!(
        "{}/api/v1/admin/oauth-clients/{}",
        AURA_API.base_url(),
        credentials.client_id
    );

    for (payload, error_code) in [
        (
            serde_json::json!({
                "redirect_uris": ["http://client.example/callback"]
            }),
            "OAUTH_INVALID_CLIENT_METADATA",
        ),
        (
            serde_json::json!({
                "redirect_uris": ["https://client.example/callback#fragment"]
            }),
            "OAUTH_INVALID_CLIENT_METADATA",
        ),
        (
            serde_json::json!({ "scope": ["not-a-supported-scope"] }),
            "BAD_BODY_VALUE",
        ),
        (
            serde_json::json!({ "tos_uri": "not-a-url" }),
            "BAD_BODY_VALUE",
        ),
        (serde_json::json!({ "client_name": null }), "BAD_BODY_VALUE"),
    ] {
        let response = client
            .patch(&url)
            .bearer_auth(&token)
            .json(&payload)
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to validate OAuth client update: {error}"));
        let cache_control = response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let (status, body) = json_response(response).await;
        api_support::assert_problem(status, &body, reqwest::StatusCode::BAD_REQUEST, error_code);
        assert_eq!(Some("no-store".to_owned()), cache_control);
    }

    let response = client
        .patch(format!(
            "{}/api/v1/admin/oauth-clients/{}",
            AURA_API.base_url(),
            uuid::Uuid::new_v4()
        ))
        .bearer_auth(token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to report missing OAuth client: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    api_support::assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "OAUTH_CLIENT_NOT_FOUND",
    );
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_oauth_client_redirect_uris_and_scopes() {
    let admin_id = seed_user("ADMIN").await;
    let token = String::from(
        seed_access_token_for(
            admin_id,
            std::collections::HashSet::from([Scope::AccessTokensWrite]),
        )
        .await,
    );
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/admin/oauth-clients", AURA_API.base_url());

    let response = client
        .post(&url)
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "client_name": "Invalid Redirect OAuth Client",
            "tos_uri": "https://client.example/tos",
            "policy_uri": "https://client.example/policy",
            "client_uri": "https://client.example",
            "logo_uri": "https://client.example/logo.png",
            "redirect_uris": ["https://client.example/callback#fragment"],
            "scope": ["access-tokens:read"]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject invalid redirect URI: {error}"));
    let (status, body) = json_response(response).await;
    api_support::assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "OAUTH_INVALID_CLIENT_METADATA",
    );

    let response = client
        .post(&url)
        .bearer_auth(token)
        .json(&serde_json::json!({
            "client_name": "Invalid Scope OAuth Client",
            "tos_uri": "https://client.example/tos",
            "policy_uri": "https://client.example/policy",
            "client_uri": "https://client.example",
            "logo_uri": "https://client.example/logo.png",
            "redirect_uris": ["https://client.example/callback"],
            "scope": ["not-a-supported-scope"]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject invalid OAuth scope: {error}"));
    let (status, body) = json_response(response).await;
    api_support::assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_paginate_and_filter_admin_oauth_clients() {
    let (client, token) = authenticated_client().await;
    let first_client =
        create_oauth_client_with_name(&client, &token, "Cursor OAuth Client A").await;
    let second_client =
        create_oauth_client_with_name(&client, &token, "Cursor OAuth Client B").await;
    let third_client =
        create_oauth_client_with_name(&client, &token, "Cursor OAuth Client C").await;
    let admin_token = admin_read_token().await;
    let url = format!("{}/api/v1/admin/oauth-clients", AURA_API.base_url());

    let first = client
        .get(&url)
        .bearer_auth(admin_token.clone())
        .query(&[("name", "Cursor OAuth Client"), ("size", "2")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get first OAuth-client page: {error}"));
    let first_cache_control = first
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (first_status, first_body) = json_response(first).await;

    assert_eq!(reqwest::StatusCode::OK, first_status);
    assert_eq!(Some("no-store".to_owned()), first_cache_control);
    assert_eq!(Some(2), first_body["items"].as_array().map(Vec::len));
    assert_eq!(serde_json::json!(2), first_body["size"]);
    let cursor = first_body["searchAfter"].clone();
    assert!(cursor.is_array());
    let first_items = first_body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("first page must contain items"));
    for item in first_items {
        assert!(item.get("client_secret").is_none());
    }

    let second = client
        .get(&url)
        .bearer_auth(admin_token.clone())
        .query(&[
            ("name", "Cursor OAuth Client"),
            ("size", "2"),
            ("searchAfter", cursor.to_string().as_str()),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get second OAuth-client page: {error}"));
    let (second_status, second_body) = json_response(second).await;

    assert_eq!(reqwest::StatusCode::OK, second_status);
    assert_eq!(Some(1), second_body["items"].as_array().map(Vec::len));
    assert!(second_body.get("searchAfter").is_none());
    assert!(second_body["items"][0].get("client_secret").is_none());

    let first_ids = first_body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("first page must contain items"));
    let second_id = second_body["items"][0]["client_id"]
        .as_str()
        .unwrap_or_else(|| panic!("second page must contain client_id"));
    assert!(first_ids.iter().all(|item| item["client_id"] != second_id));
    let returned_ids = first_ids
        .iter()
        .filter_map(|item| item["client_id"].as_str())
        .chain(std::iter::once(second_id))
        .collect::<std::collections::HashSet<_>>();
    let expected_ids = [
        first_client.client_id.as_str(),
        second_client.client_id.as_str(),
        third_client.client_id.as_str(),
    ]
    .into_iter()
    .collect::<std::collections::HashSet<_>>();
    assert_eq!(expected_ids, returned_ids);

    let exact = client
        .get(&url)
        .bearer_auth(admin_token)
        .query(&[("clientId", first_client.client_id.as_str())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to filter OAuth clients by clientId: {error}"));
    let (exact_status, exact_body) = json_response(exact).await;
    assert_eq!(reqwest::StatusCode::OK, exact_status);
    assert_eq!(Some(1), exact_body["items"].as_array().map(Vec::len));
    assert_eq!(
        serde_json::json!(first_client.client_id),
        exact_body["items"][0]["client_id"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_oauth_client_list_without_delegated_read_capability() {
    let user_id = api_support::seed_user("ADMIN").await;
    let token = api_support::seed_access_token_for(user_id, Default::default()).await;
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/oauth-clients",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call OAuth-client admin list: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    api_support::assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_oauth_client_list_for_non_admin_with_delegated_read() {
    let user_id = api_support::seed_user("USER").await;
    let token = api_support::seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensRead]),
    )
    .await;
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/oauth-clients",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call OAuth-client admin list: {error}"));
    let (status, body) = json_response(response).await;

    api_support::assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_redirect_authorized_user_with_authorization_code_and_state() {
    let (client, token) = authenticated_client().await;
    let credentials = create_oauth_client(&client, &token).await;
    let code = authorize_code(&client, &token, &credentials).await;
    assert!(!code.is_empty());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_exchange_authorization_code_for_access_token() {
    let (client, token) = authenticated_client().await;
    let credentials = create_oauth_client(&client, &token).await;
    let code = authorize_code(&client, &token, &credentials).await;
    let body = exchange_code(&client, &credentials, &code).await;
    assert_eq!(serde_json::json!("Bearer"), body["token_type"]);
    assert!(
        body["access_token"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_exchange_third_party_code_once_for_the_issued_access_token() {
    let (client, token) = authenticated_client().await;
    let credentials = create_oauth_client(&client, &token).await;
    let code = authorize_code(&client, &token, &credentials).await;
    let token_body = exchange_code(&client, &credentials, &code).await;
    let third_party_code = token_body["third_party_exchange_code"]
        .as_str()
        .unwrap_or_else(|| panic!("missing third-party exchange code"));
    let expected_access_token = token_body["access_token"].clone();

    let response = client
        .get(format!(
            "{}/api/v1/oauth/tokens/by-third-party-code/{}",
            AURA_API.base_url(),
            third_party_code
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to exchange third-party code: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(expected_access_token, body["access_token"]);

    let response = client
        .get(format!(
            "{}/api/v1/oauth/tokens/by-third-party-code/{}",
            AURA_API.base_url(),
            third_party_code
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to retry third-party exchange: {error}"));
    assert_eq!(reqwest::StatusCode::BAD_REQUEST, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_sequential_authorization_code_replay() {
    let (client, token) = authenticated_client().await;
    let credentials = create_oauth_client(&client, &token).await;
    let code = authorize_code(&client, &token, &credentials).await;

    let first = exchange_code_response(&client, &credentials, &code).await;
    assert_eq!(reqwest::StatusCode::OK, first.0);
    let second = exchange_code_response(&client, &credentials, &code).await;
    assert_eq!(reqwest::StatusCode::BAD_REQUEST, second.0);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_allow_only_one_concurrent_authorization_code_redemption() {
    let (client, token) = authenticated_client().await;
    let credentials = create_oauth_client(&client, &token).await;
    let code = authorize_code(&client, &token, &credentials).await;

    let (first, second) = tokio::join!(
        exchange_code_response(&client, &credentials, &code),
        exchange_code_response(&client, &credentials, &code),
    );
    let responses = [first, second];
    assert_eq!(
        1,
        responses
            .iter()
            .filter(|(status, _)| *status == reqwest::StatusCode::OK)
            .count()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_allow_only_one_concurrent_third_party_exchange() {
    let (client, token) = authenticated_client().await;
    let credentials = create_oauth_client(&client, &token).await;
    let code = authorize_code(&client, &token, &credentials).await;
    let token_body = exchange_code(&client, &credentials, &code).await;
    let third_party_code = token_body["third_party_exchange_code"]
        .as_str()
        .unwrap_or_else(|| panic!("missing third-party exchange code"))
        .to_owned();

    let (first, second) = tokio::join!(
        exchange_third_party_code_response(&client, &third_party_code),
        exchange_third_party_code_response(&client, &third_party_code),
    );
    let responses = [first, second];
    assert_eq!(
        1,
        responses
            .iter()
            .filter(|(status, _)| *status == reqwest::StatusCode::OK)
            .count()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_introspect_and_revoke_oauth_access_token() {
    let (client, token) = authenticated_client().await;
    let credentials = create_oauth_client(&client, &token).await;
    let code = authorize_code(&client, &token, &credentials).await;
    let token_body = exchange_code(&client, &credentials, &code).await;
    let access_token = token_body["access_token"]
        .as_str()
        .unwrap_or_else(|| panic!("missing access token"));

    let response = client
        .post(format!("{}/api/v1/oauth/introspect", AURA_API.base_url()))
        .form(&[
            ("token", access_token),
            ("client_id", credentials.client_id.as_str()),
            ("client_secret", credentials.client_secret.as_str()),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to introspect token: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(true), body["active"]);

    let response = client
        .post(format!("{}/api/v1/oauth/revoke", AURA_API.base_url()))
        .form(&[
            ("token", access_token),
            ("client_id", credentials.client_id.as_str()),
            ("client_secret", credentials.client_secret.as_str()),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to revoke token: {error}"));
    assert_eq!(reqwest::StatusCode::OK, response.status());
}
