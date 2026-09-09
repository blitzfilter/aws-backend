use super::types::{
    AdminUserData, AdminUserSummaryData, CursorData, PatchAdminUserData, PatchOwnUserData,
};
use super::util::{no_store, parse_json, parse_user_id};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::patch_value::{clearable, non_nullable_option, non_nullable_patch};
use crate::state::UsersState;
use crate::wire::parse_query_object_id;
use application::pagination::Cursor;
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use domain_primitives::query::any_of_query::AnyOfQuery;
use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;
use domain_primitives::sort::{Sort, SortOrder};
use serde::Deserialize;
use time::OffsetDateTime;
use user_core::role::UserRole;
use user_core::sort_user_field::SortUserField;
use user_core::tier::UserTier;
use user_core::user_id::UserId;
use user_core::user_search::UserSearch;
use user_service::use_cases::commands::change_user_role::ChangeUserRoleCommand;
use user_service::use_cases::commands::change_user_tier::ChangeUserTierCommand;
use user_service::use_cases::commands::delete_user::DeleteUserCommand;
use user_service::use_cases::commands::update_user_profile::UpdateUserProfileCommand;
use user_service::use_cases::queries::admin_get_user::AdminGetUserRequest;
use user_service::use_cases::queries::search_users::SearchUsersRequest;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchUsersQuery {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
    #[serde(default)]
    tier: Vec<String>,
    #[serde(default)]
    role: Vec<String>,
    #[serde(
        default,
        with = "domain_primitives::query::range_query::range_rfc3339::option"
    )]
    created: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        default,
        with = "domain_primitives::query::range_query::range_rfc3339::option"
    )]
    updated: Option<RangeQuery<OffsetDateTime>>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    search_after: Option<String>,
}

pub async fn search_users(
    State(state): State<UsersState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return no_store(*r),
    };
    let query = match parse_search_users_query(raw_query.as_deref()) {
        Ok(query) => query,
        Err(error) => return no_store(error.into_response()),
    };
    match state.search_users.execute(&ctx, query).await {
        Ok(result) => no_store(
            Json(CursorData {
                items: result
                    .items
                    .into_iter()
                    .map(AdminUserSummaryData::from)
                    .collect(),
                size: result.cursor.size,
                search_after: result.cursor.search_after,
                total: result.total,
            })
            .into_response(),
        ),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_search_users_query(raw_query: Option<&str>) -> Result<SearchUsersRequest, ApiError> {
    let query: SearchUsersQuery = serde_qs::Config::new()
        .use_form_encoding(true)
        .deserialize_str(raw_query.unwrap_or_default())
        .map_err(|error| {
            ApiError::bad_request(crate::error::BAD_QUERY_PARAMETER_VALUE)
                .with_detail(error.to_string())
        })?;

    let search = UserSearch {
        query: parse_text_query(query.query, "query")?,
        email_query: parse_text_query(query.email, "email")?,
        first_name_query: parse_text_query(query.first_name, "firstName")?,
        last_name_query: parse_text_query(query.last_name, "lastName")?,
        tier_query: parse_user_tiers(query.tier)?,
        role_query: parse_user_roles(query.role)?,
        created: query.created,
        updated: query.updated,
    };
    let sort = parse_sort(query.sort.as_deref(), query.order.as_deref())?;
    let cursor = parse_cursor(query.size.as_deref(), query.search_after.as_deref())?;

    Ok(SearchUsersRequest {
        search,
        sort,
        cursor,
    })
}

fn parse_text_query(
    value: Option<String>,
    field: &'static str,
) -> Result<Option<TextQuery<0>>, ApiError> {
    value
        .map(|value| TextQuery::<0>::try_from(value).map_err(|error| bad_query(field, error)))
        .transpose()
}

fn parse_user_tiers(values: Vec<String>) -> Result<AnyOfQuery<UserTier>, ApiError> {
    values
        .into_iter()
        .map(|value| {
            UserTier::from_code(&value).ok_or_else(|| {
                bad_query(
                    "tier",
                    format!("Expected any of: 'FREE', 'PRO', 'ULTIMATE'. Got: '{value}'"),
                )
            })
        })
        .collect()
}

fn parse_user_roles(values: Vec<String>) -> Result<AnyOfQuery<UserRole>, ApiError> {
    values
        .into_iter()
        .map(|value| {
            UserRole::from_code(&value).ok_or_else(|| {
                bad_query(
                    "role",
                    format!("Expected any of: 'USER', 'ADMIN'. Got: '{value}'"),
                )
            })
        })
        .collect()
}

fn parse_sort(
    sort: Option<&str>,
    order: Option<&str>,
) -> Result<Option<Sort<SortUserField>>, ApiError> {
    match (sort, order) {
        (Some(sort), Some(order)) => {
            let sort = parse_sort_field(sort)?;
            let order = SortOrder::try_from(order).map_err(|detail| {
                ApiError::bad_request(crate::error::BAD_ORDER_VALUE)
                    .with_query_field("order")
                    .with_detail(detail)
            })?;
            Ok(Some(Sort { sort, order }))
        }
        _ => Ok(None),
    }
}

fn parse_sort_field(value: &str) -> Result<SortUserField, ApiError> {
    match value {
        "name" => Ok(SortUserField::Name),
        "email" => Ok(SortUserField::Email),
        "firstName" => Ok(SortUserField::FirstName),
        "lastName" => Ok(SortUserField::LastName),
        "tier" => Ok(SortUserField::Tier),
        "role" => Ok(SortUserField::Role),
        "created" => Ok(SortUserField::Created),
        "updated" => Ok(SortUserField::Updated),
        value => Err(ApiError::bad_request(crate::error::BAD_SORT_VALUE)
            .with_query_field("sort")
            .with_detail(format!(
                "Expected any of: 'name', 'email', 'firstName', 'lastName', 'tier', 'role', 'updated', 'created'. Got: '{value}'"
            ))),
    }
}

fn parse_cursor(
    size: Option<&str>,
    search_after: Option<&str>,
) -> Result<Option<Cursor<UserId>>, ApiError> {
    let size = size
        .map(|value| value.parse::<u64>().map(|size| size.clamp(1, 100)))
        .transpose()
        .map_err(|error| bad_query("size", error))?;
    let search_after = search_after.map(parse_search_after).transpose()?;

    if size.is_some() || search_after.is_some() {
        Ok(Some(Cursor {
            size: size.unwrap_or_else(|| Cursor::<UserId>::default().size),
            search_after,
        }))
    } else {
        Ok(None)
    }
}

fn parse_search_after(value: &str) -> Result<UserId, ApiError> {
    let candidate = match serde_json::from_str::<serde_json::Value>(value) {
        Ok(serde_json::Value::String(value)) => value,
        Ok(serde_json::Value::Array(values)) => values
            .last()
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| bad_query("searchAfter", "searchAfter must contain a User ID."))?,
        Ok(_) => {
            return Err(bad_query(
                "searchAfter",
                "searchAfter must contain a User ID.",
            ));
        }
        Err(_) => value.to_owned(),
    };

    parse_query_object_id(&candidate, "searchAfter", "User")
}

fn bad_query(field: &'static str, detail: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(crate::error::BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail.to_string())
}

pub async fn get_user(
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
        .admin_get_user
        .execute(&ctx, AdminGetUserRequest { user_id })
        .await
    {
        Ok(view) => no_store(Json(AdminUserData::from(view)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

pub async fn patch_admin_user(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw_user_id): Path<String>,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return no_store(*r),
    };
    let user_id = match parse_user_id(&raw_user_id, "userId") {
        Ok(v) => v,
        Err(r) => return no_store(r),
    };
    let data: PatchAdminUserData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return no_store(r),
    };
    no_store(admin_patch_user(state, ctx, user_id, data).await)
}

async fn admin_patch_user(
    state: UsersState,
    ctx: application::operation_context::OperationContext,
    user_id: user_core::user_id::UserId,
    data: PatchAdminUserData,
) -> Response {
    let profile_changed = data.email.is_present()
        || data.first_name.is_present()
        || data.last_name.is_present()
        || data.language.is_present()
        || data.currency.is_present()
        || data.measurement_unit.is_present()
        || data.show_unassessed_or_sensitive_content.is_present();
    let role_changed = data.role.is_present();
    let tier_changed = data.tier.is_present();
    let change_count = u8::from(profile_changed) + u8::from(role_changed) + u8::from(tier_changed);
    if change_count > 1 {
        return ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("Patch only one user change category per request.")
            .into_response();
    }

    let PatchAdminUserData {
        email,
        first_name,
        last_name,
        language,
        currency,
        measurement_unit,
        show_unassessed_or_sensitive_content,
        tier,
        role,
    } = data;
    let role = match non_nullable_option(role, "role") {
        Ok(role) => role,
        Err(error) => return error.into_response(),
    };
    let tier = match non_nullable_option(tier, "tier") {
        Ok(tier) => tier,
        Err(error) => return error.into_response(),
    };

    if let Some(role) = role {
        return match state
            .change_user_role
            .execute(&ctx, ChangeUserRoleCommand { user_id, role })
            .await
        {
            Ok(result) => no_store(Json(AdminUserData::from(result.view)).into_response()),
            Err(error) => ApiError::from(error).into_response(),
        };
    }

    if let Some(tier) = tier {
        return match state
            .change_user_tier
            .execute(&ctx, ChangeUserTierCommand { user_id, tier })
            .await
        {
            Ok(result) => no_store(Json(AdminUserData::from(result.view)).into_response()),
            Err(error) => ApiError::from(error).into_response(),
        };
    }

    let command = match profile_command(
        PatchOwnUserData {
            email,
            first_name,
            last_name,
            language,
            currency,
            measurement_unit,
            show_unassessed_or_sensitive_content,
        },
        user_id,
    ) {
        Ok(command) => command,
        Err(error) => return error.into_response(),
    };
    match state.admin_update_user_profile.execute(&ctx, command).await {
        Ok(result) => no_store(Json(AdminUserData::from(result.view)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn profile_command(
    data: PatchOwnUserData,
    user_id: user_core::user_id::UserId,
) -> Result<UpdateUserProfileCommand, ApiError> {
    Ok(UpdateUserProfileCommand {
        user_id,
        email: non_nullable_patch(data.email, "email")?,
        first_name: clearable(data.first_name),
        last_name: clearable(data.last_name),
        language: clearable(data.language),
        currency: clearable(data.currency),
        measurement_unit: clearable(data.measurement_unit),
        show_unassessed_or_sensitive_content: non_nullable_patch(
            data.show_unassessed_or_sensitive_content,
            "showUnassessedOrSensitiveContent",
        )?,
    })
}

pub async fn delete_admin_user(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw_user_id): Path<String>,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let user_id = match parse_user_id(&raw_user_id, "userId") {
        Ok(v) => v,
        Err(r) => return r,
    };
    match state
        .admin_delete_user
        .execute(&ctx, DeleteUserCommand { user_id })
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::sort::SortOrder;
    use time::macros::datetime;

    #[test]
    fn should_map_user_search_query_to_service_request() -> Result<(), ApiError> {
        let request = parse_search_users_query(Some(&format!(
            "query=ada&email=example.com&firstName=Ada&lastName=Lovelace&tier=PRO&tier=ULTIMATE&role=ADMIN&created%5Bmin%5D=2026-01-01T00%3A00%3A00Z&created%5Bmax%5D=2026-12-31T23%3A59%3A59Z&updated%5Bmin%5D=2026-02-01T00%3A00%3A00Z&sort=email&order=desc&size=200&searchAfter={}",
            UserId::new()
        )))?;

        assert_eq!(Some("ada"), request.search.query.as_deref());
        assert_eq!(Some("example.com"), request.search.email_query.as_deref());
        assert_eq!(Some("Ada"), request.search.first_name_query.as_deref());
        assert_eq!(Some("Lovelace"), request.search.last_name_query.as_deref());
        assert_eq!(
            std::collections::HashSet::from([UserTier::Pro, UserTier::Ultimate]),
            request.search.tier_query.into()
        );
        assert_eq!(
            std::collections::HashSet::from([UserRole::Admin]),
            request.search.role_query.into()
        );
        assert_eq!(
            Some(RangeQuery {
                min: Some(datetime!(2026-01-01 00:00 UTC)),
                max: Some(datetime!(2026-12-31 23:59:59 UTC)),
            }),
            request.search.created
        );
        assert_eq!(
            Some(RangeQuery {
                min: Some(datetime!(2026-02-01 00:00 UTC)),
                max: None,
            }),
            request.search.updated
        );
        assert_eq!(
            Some(Sort {
                sort: SortUserField::Email,
                order: SortOrder::Desc,
            }),
            request.sort
        );
        let cursor_user_id = request
            .cursor
            .as_ref()
            .and_then(|cursor| cursor.search_after)
            .ok_or_else(|| ApiError::internal_server_error(crate::error::USER_INTERNAL_ERROR))?;
        assert_eq!(
            Some(Cursor {
                size: 100,
                search_after: Some(cursor_user_id),
            }),
            request.cursor
        );
        Ok(())
    }

    #[test]
    fn should_accept_json_array_user_cursor() -> Result<(), ApiError> {
        let request = parse_search_users_query(Some(&format!(
            "searchAfter=[\"email\",\"{}\"]",
            UserId::new()
        )))?;

        assert!(request.search.query.is_none());
        assert_eq!(21, request.cursor.as_ref().map_or(0, |cursor| cursor.size));
        assert!(
            request
                .cursor
                .is_some_and(|cursor| cursor.search_after.is_some())
        );
        Ok(())
    }

    #[test]
    fn should_clamp_user_search_page_size() -> Result<(), ApiError> {
        let request = parse_search_users_query(Some("size=0"))?;
        assert_eq!(1, request.cursor.as_ref().map_or(0, |cursor| cursor.size));

        let request = parse_search_users_query(Some("size=1000"))?;
        assert_eq!(100, request.cursor.as_ref().map_or(0, |cursor| cursor.size));
        Ok(())
    }

    #[test]
    fn should_reject_invalid_user_search_query_values() {
        assert!(parse_search_users_query(Some("tier=pro")).is_err());
        assert!(parse_search_users_query(Some("role=administrator")).is_err());
        assert!(parse_search_users_query(Some("sort=invalid&order=asc")).is_err());
        assert!(parse_search_users_query(Some("sort=email&order=sideways")).is_err());
        assert!(parse_search_users_query(Some("size=not-a-number")).is_err());
        assert!(parse_search_users_query(Some("searchAfter=not-an-object-id")).is_err());
        assert!(parse_search_users_query(Some("created[min]=not-a-timestamp")).is_err());
    }
}
