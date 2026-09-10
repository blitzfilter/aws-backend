use super::{no_store, parse_form, required_form};
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::state::OAuthState;
use crate::wire::parse_body_object_id;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use axum::{
    extract::State,
    response::{IntoResponse, Response},
};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_service::use_cases::RevokeTokenRequest;
use user_core::access_token::{RawAccessToken, RawOAuthClientSecret};

pub async fn revoke(State(state): State<OAuthState>, body: String) -> Response {
    let form = parse_form(body);
    let request = match request(&form) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state.revoke.execute(&system_context(), request).await {
        Ok(()) => no_store(axum::http::StatusCode::OK.into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn request(
    form: &std::collections::HashMap<String, String>,
) -> Result<RevokeTokenRequest, Response> {
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
    Ok(RevokeTokenRequest {
        token,
        client_id,
        client_secret,
    })
}

fn system_context() -> OperationContext {
    OperationContext {
        principal: Principal::System,
        request_id: RequestId::new("oauth-revoke"),
        correlation_id: CorrelationId::new("oauth-revoke"),
    }
}
