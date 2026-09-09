use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::WatchlistState;
use crate::wire::parse_path_object_id;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use product_listing_core::product_listing_id::ProductListingId;
use watchlist_service::use_cases::UnwatchProductListingCommand;

pub async fn delete_watchlist(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    Path(raw_product_listing_id): Path<String>,
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
    match state
        .unwatch_product
        .execute(
            &ctx,
            UnwatchProductListingCommand {
                user_id,
                product_listing_id,
            },
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
