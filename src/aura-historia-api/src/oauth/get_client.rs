use super::{list_clients::OAuthClientAdminData, no_store};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::OAuthState;
use crate::wire::parse_path_object_id;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use credential_core::oauth_client_id::OAuthClientId;

pub async fn get_client(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let client_id: OAuthClientId = match parse_path_object_id(&raw, "clientId", "OAuthClient") {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    match state.get_client.execute(&context, &client_id).await {
        Ok(result) => no_store(Json(OAuthClientAdminData::from(result)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}
