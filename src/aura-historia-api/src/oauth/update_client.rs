use super::list_clients::OAuthClientAdminData;
use super::{no_store, parse_scopes};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE, INVALID_UUID};
use crate::patch_value::{PatchValue, non_nullable_option};
use crate::state::OAuthState;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_service::use_cases::UpdateOAuthClientCommand;
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct UpdateOAuthClientData {
    #[serde(default)]
    client_name: PatchValue<String>,
    #[serde(default)]
    redirect_uris: PatchValue<HashSet<url::Url>>,
    #[serde(default)]
    tos_uri: PatchValue<url::Url>,
    #[serde(default)]
    policy_uri: PatchValue<url::Url>,
    #[serde(default)]
    client_uri: PatchValue<url::Url>,
    #[serde(default)]
    logo_uri: PatchValue<url::Url>,
    #[serde(default)]
    scope: PatchValue<HashSet<String>>,
}

impl TryFrom<UpdateOAuthClientData> for UpdateOAuthClientCommand {
    type Error = Response;
    fn try_from(data: UpdateOAuthClientData) -> Result<Self, Self::Error> {
        let scopes = non_nullable_option(data.scope, "scope")
            .map_err(|error| error.into_response())?
            .map(parse_scopes)
            .transpose()?;
        Ok(Self {
            name: non_nullable_option(data.client_name, "client_name")
                .map_err(|error| error.into_response())?
                .map(oauth_core::client::OAuthClientName::from),
            redirect_uris: non_nullable_option(data.redirect_uris, "redirect_uris")
                .map_err(|error| error.into_response())?,
            tos_uri: non_nullable_option(data.tos_uri, "tos_uri")
                .map_err(|error| error.into_response())?,
            policy_uri: non_nullable_option(data.policy_uri, "policy_uri")
                .map_err(|error| error.into_response())?,
            client_uri: non_nullable_option(data.client_uri, "client_uri")
                .map_err(|error| error.into_response())?,
            logo_uri: non_nullable_option(data.logo_uri, "logo_uri")
                .map_err(|error| error.into_response())?,
            scopes,
        })
    }
}

pub async fn update_client(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    body: String,
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
    let data: UpdateOAuthClientData = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => {
            return no_store(
                ApiError::bad_request(BAD_BODY_VALUE)
                    .with_detail(error.to_string())
                    .into_response(),
            );
        }
    };
    let command = match UpdateOAuthClientCommand::try_from(data) {
        Ok(value) => value,
        Err(response) => return no_store(response),
    };
    match state
        .update_client
        .execute(&context, &client_id, command)
        .await
    {
        Ok(result) => no_store(Json(OAuthClientAdminData::from(result)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}
