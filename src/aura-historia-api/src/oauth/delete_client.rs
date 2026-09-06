use super::no_store;
use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_UUID};
use crate::state::OAuthState;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use credential_core::oauth_client_id::OAuthClientId;

pub async fn delete_client(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let client_id = match OAuthClientId::try_from(raw.as_str()) {
        Ok(value) => value,
        Err(_) => {
            return no_store(
                ApiError::bad_request(INVALID_UUID)
                    .with_path_field("clientId")
                    .into_response(),
            );
        }
    };
    match state.delete_client.execute(&context, &client_id).await {
        Ok(_) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}
