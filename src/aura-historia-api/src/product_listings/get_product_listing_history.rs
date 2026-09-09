use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, PRODUCT_LISTING_INTERNAL_ERROR};
use crate::product_listings::product_listing_history_entry_data::ProductListingHistoryEntryData;
use crate::state::ProductListingsState;
use crate::wire::parse_path_object_id;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::use_cases::{
    GetProductListingHistoryRequest, ProductListingHistoryLookup,
};

const HISTORY_CACHE_CONTROL: &str = "public, max-age=180, s-maxage=900";

pub async fn get_product_listing_history_by_id(
    State(state): State<ProductListingsState>,
    headers: HeaderMap,
    Path(raw_product_listing_id): Path<String>,
) -> Response {
    let product_listing_id = match parse_path_object_id::<ProductListingId>(
        &raw_product_listing_id,
        "productListingId",
        "ProductListing",
    ) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    history_response(
        state,
        headers,
        ProductListingHistoryLookup::ById(product_listing_id),
    )
    .await
}

async fn history_response(
    state: ProductListingsState,
    headers: HeaderMap,
    lookup: ProductListingHistoryLookup,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(value) => value,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let Some(use_case) = state.get_product_listing_history.as_ref() else {
        return ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
            .with_detail("ProductListing history is not configured.")
            .into_response();
    };
    let context = principal.operation_context(metadata);
    match use_case
        .execute(&context, GetProductListingHistoryRequest { lookup })
        .await
    {
        Ok(history) => {
            let mut response = Json(
                history
                    .into_iter()
                    .map(ProductListingHistoryEntryData::from)
                    .collect::<Vec<_>>(),
            )
            .into_response();
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(HISTORY_CACHE_CONTROL),
            );
            response
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}
