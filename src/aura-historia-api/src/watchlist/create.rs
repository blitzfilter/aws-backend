use super::types::{PostWatchlistData, WatchlistEntryData};
use super::util::parse_json;
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::WatchlistState;
use crate::wire::parse_body_object_id;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use watchlist_service::use_cases::WatchProductListingCommand;

pub async fn post_watchlist(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let data: PostWatchlistData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let product_listing_id = match parse_body_object_id(
        &data.product_listing_id,
        "productListingId",
        "ProductListing",
    ) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    match state
        .watch_product
        .execute(
            &ctx,
            WatchProductListingCommand {
                user_id,
                product_listing_id,
                notifications: data.notifications.unwrap_or(true),
            },
        )
        .await
    {
        Ok(r) => (StatusCode::CREATED, Json(WatchlistEntryData::from(r.entry))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
