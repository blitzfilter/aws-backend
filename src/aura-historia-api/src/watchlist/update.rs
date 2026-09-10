use super::types::{PatchWatchlistData, WatchlistEntryData, watchlist_state};
use super::util::parse_json;
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::patch_value::non_nullable_option;
use crate::state::WatchlistState;
use crate::wire::parse_path_object_id;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use product_listing_core::product_listing_id::ProductListingId;
use watchlist_service::use_cases::UpdateWatchlistProductListingCommand;

pub async fn patch_watchlist(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    Path(raw_product_listing_id): Path<String>,
    body: String,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let product_listing_id = match parse_path_object_id::<ProductListingId>(
        &raw_product_listing_id,
        "productListingId",
        "ProductListing",
    ) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let data: PatchWatchlistData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let notifications = match non_nullable_option(data.notifications, "notifications") {
        Ok(notifications) => notifications,
        Err(error) => return error.into_response(),
    };
    let state_field = match non_nullable_option(data.state, "state") {
        Ok(state_field) => state_field.map(watchlist_state),
        Err(error) => return error.into_response(),
    };
    match state
        .update_watchlist_product
        .execute(
            &ctx,
            UpdateWatchlistProductListingCommand {
                user_id,
                product_listing_id,
                notifications,
                state: state_field,
            },
        )
        .await
    {
        Ok(r) => Json(WatchlistEntryData::from(r.entry)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
