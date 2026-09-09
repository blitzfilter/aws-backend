use super::{no_store, parse_form, required_form, scope_string};
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::state::OAuthState;
use crate::wire::parse_body_object_id;
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_service::use_cases::{IntrospectTokenRequest, IntrospectTokenResponse, OAuthTokenType};
use serde::Serialize;
use user_core::access_token::{RawAccessToken, RawOAuthClientSecret};

#[derive(Debug, Serialize)]
pub(crate) struct IntrospectionResponseData {
    active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_id: Option<OAuthClientId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub: Option<user_core::user_id::UserId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exp: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iat: Option<i64>,
}

impl From<IntrospectTokenResponse> for IntrospectionResponseData {
    fn from(response: IntrospectTokenResponse) -> Self {
        Self {
            active: response.active,
            scope: response.scopes.as_ref().map(scope_string),
            client_id: response.client_id,
            sub: response.subject,
            token_type: response.token_type.map(|token_type| match token_type {
                OAuthTokenType::Bearer => "Bearer",
            }),
            exp: response.expires.map(|expires| expires.unix_timestamp()),
            iat: response
                .issued_at
                .map(|issued_at| issued_at.unix_timestamp()),
        }
    }
}

pub async fn introspect(State(state): State<OAuthState>, body: String) -> Response {
    let form = parse_form(body);
    let request = match request(&form) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state.introspect.execute(request).await {
        Ok(result) => no_store(Json(IntrospectionResponseData::from(result)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn request(
    form: &std::collections::HashMap<String, String>,
) -> Result<IntrospectTokenRequest, Response> {
    let token =
        RawAccessToken::try_from(required_form(form, "token")?.to_owned()).map_err(|error| {
            ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail(error.to_string())
                .into_response()
        })?;
    let client_id: OAuthClientId = parse_body_object_id(
        required_form(form, "client_id")?,
        "client_id",
        "OAuthClient",
    )
    .map_err(IntoResponse::into_response)?;
    let client_secret = RawOAuthClientSecret::try_from(
        required_form(form, "client_secret")?.to_owned(),
    )
    .map_err(|error| {
        ApiError::unauthorized(crate::error::INVALID_CREDENTIALS)
            .with_detail(error.to_string())
            .into_response()
    })?;
    Ok(IntrospectTokenRequest {
        token,
        client_id,
        client_secret,
    })
}
