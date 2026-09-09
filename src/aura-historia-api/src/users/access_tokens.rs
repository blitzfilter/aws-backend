use super::util::{no_store, parse_json, parse_user_id};
use crate::auth::protected_context;
use crate::error::{
    ACCESS_TOKEN_INTERNAL_ERROR, ApiError, BAD_BODY_VALUE, BAD_QUERY_PARAMETER_VALUE,
};
use crate::pagination_data::JsonCursoredData;
use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use crate::state::UsersState;
use crate::wire::{parse_body_object_id, parse_path_object_id, parse_query_object_id};
use application::pagination::{Cursor, CursoredResult};
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
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
use user_service::use_cases::queries::list_admin_access_tokens::{
    AccessTokenSearchCursor, ListAdminAccessTokensRequest, ListAdminAccessTokensResult,
};

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
    access_token_id: String,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAdminAccessTokensQuery {
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    search_after: Option<String>,
}

const DEFAULT_PAGE_SIZE: u64 = 21;
const MAX_PAGE_SIZE: u64 = 100;

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
pub async fn list_admin_access_tokens(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw_user_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let user_id = match parse_user_id(&raw_user_id, "userId") {
        Ok(value) => value,
        Err(response) => return no_store(response),
    };
    let request = match parse_list_admin_access_tokens_query(user_id, raw_query.as_deref()) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };

    match state.admin_list_access_tokens.execute(&ctx, request).await {
        Ok(result) => match response_from_admin_result(result) {
            Ok(data) => no_store(Json(data).into_response()),
            Err(error) => no_store(error.into_response()),
        },
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_list_admin_access_tokens_query(
    user_id: UserId,
    raw_query: Option<&str>,
) -> Result<ListAdminAccessTokensRequest, ApiError> {
    let query: ListAdminAccessTokensQuery = serde_qs::Config::new()
        .use_form_encoding(true)
        .deserialize_str(raw_query.unwrap_or_default())
        .map_err(|error| bad_query("query", error))?;
    let size = query
        .size
        .map(|value| {
            value
                .parse::<u64>()
                .map(|size| size.clamp(1, MAX_PAGE_SIZE))
                .map_err(|error| bad_query("size", error))
        })
        .transpose()?;
    let search_after = query
        .search_after
        .as_deref()
        .map(parse_access_token_search_after)
        .transpose()?;

    Ok(ListAdminAccessTokensRequest {
        user_id,
        cursor: if size.is_some() || search_after.is_some() {
            Some(Cursor {
                size: size.unwrap_or(DEFAULT_PAGE_SIZE),
                search_after,
            })
        } else {
            None
        },
    })
}

fn parse_access_token_search_after(value: &str) -> Result<AccessTokenSearchCursor, ApiError> {
    let value: Value = serde_json::from_str(value).map_err(|error| {
        bad_query(
            "searchAfter",
            format!(
                "searchAfter must be a JSON array containing timestamp and AccessToken ID: {error}"
            ),
        )
    })?;
    let Value::Array(values) = value else {
        return Err(bad_query(
            "searchAfter",
            "searchAfter must contain an RFC3339 timestamp and AccessToken ID.",
        ));
    };
    let [Value::String(position), Value::String(access_token_id)] = values.as_slice() else {
        return Err(bad_query(
            "searchAfter",
            "searchAfter must contain an RFC3339 timestamp and AccessToken ID.",
        ));
    };
    let position = OffsetDateTime::parse(position, &Rfc3339)
        .map_err(|error| bad_query("searchAfter", error))?;
    let access_token_id = parse_query_object_id(access_token_id, "searchAfter", "AccessToken")?;

    Ok(AccessTokenSearchCursor {
        position,
        access_token_id,
    })
}

fn response_from_admin_result(
    result: ListAdminAccessTokensResult,
) -> Result<JsonCursoredData<TokenData>, ApiError> {
    let CursoredResult {
        items,
        cursor,
        total,
    } = result;
    let search_after = cursor
        .search_after
        .map(serialize_access_token_search_after)
        .transpose()?;

    Ok(JsonCursoredData {
        items: items.into_iter().map(TokenData::from).collect(),
        size: cursor.size,
        search_after,
        total,
    })
}

fn serialize_access_token_search_after(cursor: AccessTokenSearchCursor) -> Result<Value, ApiError> {
    let position = cursor
        .position
        .format(&Rfc3339)
        .map_err(|_| ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR))?;
    Ok(json!([position, cursor.access_token_id]))
}

fn bad_query(field: &'static str, detail: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail.to_string())
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
    let access_token_id = match parse_path_object_id(&raw, "accessTokenId", "AccessToken") {
        Ok(value) => value,
        Err(error) => return error.into_response(),
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
    let access_token_id = match parse_path_object_id(&raw, "accessTokenId", "AccessToken") {
        Ok(value) => value,
        Err(error) => return error.into_response(),
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
    let access_token_id =
        match parse_path_object_id(&raw_access_token_id, "accessTokenId", "AccessToken") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
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
            access_token_id: parse_body_object_id(
                &self.access_token_id,
                "accessTokenId",
                "AccessToken",
            )?,
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
    fn should_parse_admin_access_token_query_with_clamped_cursor() -> Result<(), ApiError> {
        let user_id = UserId::new();
        let access_token_id = AccessTokenId::new();
        let request = parse_list_admin_access_tokens_query(
            user_id,
            Some(&format!(
                "size=200&searchAfter=[\"2026-09-04T12:00:00Z\",\"{access_token_id}\"]"
            )),
        )?;

        assert_eq!(user_id, request.user_id);
        assert_eq!(
            Some(Cursor {
                size: 100,
                search_after: Some(AccessTokenSearchCursor {
                    position: OffsetDateTime::parse("2026-09-04T12:00:00Z", &Rfc3339,).map_err(
                        |error| ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                            .with_detail(error.to_string())
                    )?,
                    access_token_id,
                }),
            }),
            request.cursor
        );
        Ok(())
    }

    #[test]
    fn should_reject_invalid_admin_access_token_query_values() {
        for query in [
            "size=not-a-number",
            "searchAfter=not-json",
            "searchAfter=%5B%22not-a-timestamp%22%2C%22not-an-object-id%22%5D",
        ] {
            assert!(
                parse_list_admin_access_tokens_query(UserId::new(), Some(query)).is_err(),
                "{query}"
            );
        }
    }

    #[test]
    fn should_serialize_admin_access_token_metadata_without_secrets() {
        let value = serde_json::to_value(TokenData::from(AccessTokenView {
            user_id: UserId::new(),
            access_token_id: AccessTokenId::new(),
            name: AccessTokenName::from("admin inspection"),
            scopes: HashSet::from([Scope::UsersRead]),
            origin: AccessTokenOrigin::User,
            expires: None,
        }))
        .unwrap_or_else(|error| panic!("admin access-token metadata serializes: {error}"));

        assert!(value.get("accessToken").is_none());
        assert!(value.get("token").is_none());
        assert!(value.get("tokenShort").is_none());
        assert!(value.get("tokenHash").is_none());
        assert!(value.get("hash").is_none());
        assert!(value.get("accessTokenId").is_some());
        assert!(value.get("scopes").is_some());
    }

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
