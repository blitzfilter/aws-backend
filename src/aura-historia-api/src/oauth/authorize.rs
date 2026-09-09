use super::parse_scope_string;
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE};
use crate::state::OAuthState;
use crate::wire::parse_query_object_id;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::authorization_code::{CodeChallengeMethod, OAuthCodeChallenge};
use oauth_service::use_cases::{
    AuthorizeRequest, OAuthResponseType, OAuthState as OAuthRequestState,
};
use std::collections::HashMap;

pub async fn authorize(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let request = match request(query) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state.authorize.execute(&context, request).await {
        Ok(result) => match result.redirect_to.parse() {
            Ok(location) => (
                StatusCode::FOUND,
                [
                    (header::LOCATION, location),
                    (
                        header::CACHE_CONTROL,
                        axum::http::HeaderValue::from_static("no-store"),
                    ),
                ],
            )
                .into_response(),
            Err(error) => ApiError::internal_server_error(crate::error::OAUTH_INTERNAL_ERROR)
                .with_detail(error.to_string())
                .into_response(),
        },
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn request(query: HashMap<String, String>) -> Result<AuthorizeRequest, Response> {
    let response_type = match required(&query, "response_type")? {
        "code" => OAuthResponseType::Code,
        value => {
            return Err(ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field("response_type")
                .with_detail(format!("Unsupported response_type '{value}'."))
                .into_response());
        }
    };
    let client_id: OAuthClientId =
        parse_query_object_id(required(&query, "client_id")?, "client_id", "OAuthClient")
            .map_err(IntoResponse::into_response)?;
    let redirect_uri = url::Url::parse(required(&query, "redirect_uri")?).map_err(|error| {
        ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("redirect_uri")
            .with_detail(error.to_string())
            .into_response()
    })?;
    let code_challenge_method = match required(&query, "code_challenge_method")? {
        "S256" => CodeChallengeMethod::S256,
        value => {
            return Err(ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field("code_challenge_method")
                .with_detail(format!("Unsupported code_challenge_method '{value}'."))
                .into_response());
        }
    };
    Ok(AuthorizeRequest {
        response_type,
        client_id,
        redirect_uri,
        scope: parse_scope_string(query.get("scope").map(String::as_str), "scope")?,
        state: query
            .get("state")
            .map(|value| OAuthRequestState::from(value.as_str())),
        code_challenge: OAuthCodeChallenge::from(required(&query, "code_challenge")?),
        code_challenge_method,
    })
}

fn required<'a>(
    query: &'a HashMap<String, String>,
    field: &'static str,
) -> Result<&'a str, Response> {
    query
        .get(field)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field(field)
                .with_detail(format!("Query parameter '{field}' is required."))
                .into_response()
        })
}
