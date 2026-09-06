use super::util::{no_store, parse_json, parse_user_id};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE, INVALID_UUID};
use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use crate::state::UsersState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{AccessTokenId, AccessTokenName, AccessTokenOrigin, Scope};
use user_core::user_id::UserId;
use user_service::use_cases::commands::create_access_token::{
    CreateAccessTokenCommand, CreateAccessTokenResult,
};
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenCommand;
use user_service::use_cases::commands::delete_access_tokens::DeleteAccessTokensCommand;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenCommand;
use user_service::use_cases::queries::get_access_token::{AccessTokenView, GetAccessTokenRequest};
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensRequest;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostTokenData {
    name: String,
    scopes: HashSet<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    expires: Option<OffsetDateTime>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchTokenData {
    access_token_id: AccessTokenId,
    #[serde(default)]
    name: PatchValue<String>,
    #[serde(default)]
    scopes: PatchValue<HashSet<String>>,
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    expires: PatchValue<OffsetDateTime>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenData {
    user_id: UserId,
    access_token_id: AccessTokenId,
    name: String,
    scopes: Vec<String>,
    origin: String,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    expires: Option<OffsetDateTime>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatedTokenData {
    user_id: UserId,
    access_token_id: AccessTokenId,
    access_token: String,
}
impl From<AccessTokenView> for TokenData {
    fn from(v: AccessTokenView) -> Self {
        Self {
            user_id: v.user_id,
            access_token_id: v.access_token_id,
            name: v.name.to_string(),
            scopes: v
                .scopes
                .into_iter()
                .map(|s| s.as_str().to_owned())
                .collect(),
            origin: format!("{:?}", v.origin),
            expires: v.expires,
        }
    }
}
impl From<CreateAccessTokenResult> for CreatedTokenData {
    fn from(v: CreateAccessTokenResult) -> Self {
        Self {
            user_id: v.user_id,
            access_token_id: v.access_token_id,
            access_token: String::from(v.raw_access_token),
        }
    }
}

pub async fn post_access_token(
    State(state): State<UsersState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let data: PostTokenData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let scopes = match parse_scopes(data.scopes) {
        Ok(scopes) => scopes,
        Err(error) => return error.into_response(),
    };
    let command = CreateAccessTokenCommand {
        user_id,
        name: AccessTokenName::from(data.name.as_str()),
        scopes,
        expires: data.expires,
        origin: AccessTokenOrigin::User,
    };
    match state.create_access_token.execute(&ctx, command).await {
        Ok(r) => (StatusCode::CREATED, Json(CreatedTokenData::from(r))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub async fn list_access_tokens(State(state): State<UsersState>, headers: HeaderMap) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match state
        .list_access_tokens
        .execute(&ctx, ListAccessTokensRequest { user_id })
        .await
    {
        Ok(r) => no_store(
            Json(r.items.into_iter().map(TokenData::from).collect::<Vec<_>>()).into_response(),
        ),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub async fn get_access_token(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let access_token_id = match AccessTokenId::try_from(raw.as_str()) {
        Ok(v) => v,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("accessTokenId")
                .with_detail("Path parameter 'accessTokenId' must be a UUID.")
                .into_response();
        }
    };
    match state
        .get_access_token
        .execute(
            &ctx,
            GetAccessTokenRequest {
                user_id,
                access_token_id,
            },
        )
        .await
    {
        Ok(r) => no_store(Json(TokenData::from(r)).into_response()),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub async fn patch_access_token(
    State(state): State<UsersState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let data: PatchTokenData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let command = match data.into_command(user_id) {
        Ok(command) => command,
        Err(error) => return error.into_response(),
    };
    match state.update_access_token.execute(&ctx, command).await {
        Ok(result) => no_store(Json(TokenData::from(result.view)).into_response()),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub async fn delete_access_token(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let access_token_id = match AccessTokenId::try_from(raw.as_str()) {
        Ok(v) => v,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("accessTokenId")
                .with_detail("Path parameter 'accessTokenId' must be a UUID.")
                .into_response();
        }
    };
    match state
        .delete_access_token
        .execute(
            &ctx,
            DeleteAccessTokenCommand {
                user_id,
                access_token_id,
            },
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

pub async fn delete_admin_access_tokens(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw_user_id): Path<String>,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return no_store(*r),
    };
    let user_id = match parse_user_id(&raw_user_id, "userId") {
        Ok(v) => v,
        Err(r) => return no_store(r),
    };

    match state
        .admin_delete_access_tokens
        .execute(&ctx, DeleteAccessTokensCommand { user_id })
        .await
    {
        Ok(_) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

pub async fn delete_admin_access_token(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path((raw_user_id, raw_access_token_id)): Path<(String, String)>,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return no_store(*r),
    };
    let user_id = match parse_user_id(&raw_user_id, "userId") {
        Ok(v) => v,
        Err(r) => return no_store(r),
    };
    let access_token_id = match AccessTokenId::try_from(raw_access_token_id.as_str()) {
        Ok(v) => v,
        Err(_) => {
            return no_store(
                ApiError::bad_request(INVALID_UUID)
                    .with_path_field("accessTokenId")
                    .with_detail("Path parameter 'accessTokenId' must be a UUID.")
                    .into_response(),
            );
        }
    };

    match state
        .admin_delete_access_token
        .execute(
            &ctx,
            DeleteAccessTokenCommand {
                user_id,
                access_token_id,
            },
        )
        .await
    {
        Ok(_) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

impl PatchTokenData {
    fn into_command(self, user_id: UserId) -> Result<UpdateAccessTokenCommand, ApiError> {
        Ok(UpdateAccessTokenCommand {
            user_id,
            access_token_id: self.access_token_id,
            name: non_nullable_patch(
                self.name.map(|name| AccessTokenName::from(name.as_str())),
                "name",
            )?,
            scopes: non_nullable_patch(parse_scope_patch(self.scopes)?, "scopes")?,
            expires: clearable(self.expires),
        })
    }
}

fn parse_scope_patch(
    values: PatchValue<HashSet<String>>,
) -> Result<PatchValue<HashSet<Scope>>, ApiError> {
    match values {
        PatchValue::Omitted => Ok(PatchValue::Omitted),
        PatchValue::Null => Ok(PatchValue::Null),
        PatchValue::Value(values) => Ok(PatchValue::Value(parse_scopes(values)?)),
    }
}

fn parse_scopes(values: HashSet<String>) -> Result<HashSet<Scope>, ApiError> {
    values
        .into_iter()
        .map(|value| match value.as_str() {
            "product-listings:write" => Ok(Scope::ProductListingsWrite),
            "users:read" => Ok(Scope::UsersRead),
            "users:write" => Ok(Scope::UsersWrite),
            "access-tokens:read" => Ok(Scope::AccessTokensRead),
            "access-tokens:write" => Ok(Scope::AccessTokensWrite),
            "search-filters:write" => Ok(Scope::SearchFiltersWrite),
            "watchlist:read" => Ok(Scope::WatchlistRead),
            "watchlist:write" => Ok(Scope::WatchlistWrite),
            _ => Err(ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail(format!("Unsupported scope '{}'.", value))),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_accept_canonical_product_listings_write_scope() {
        let scopes = parse_scopes(HashSet::from(["product-listings:write".to_owned()]));

        assert!(matches!(
            scopes,
            Ok(scopes) if scopes == HashSet::from([Scope::ProductListingsWrite])
        ));
    }

    #[test]
    fn should_reject_unsupported_access_token_scope() {
        let scopes = parse_scopes(HashSet::from(["unsupported:scope".to_owned()]));

        assert!(scopes.is_err());
    }
}
