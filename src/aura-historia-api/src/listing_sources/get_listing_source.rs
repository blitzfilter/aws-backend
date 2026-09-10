use crate::auth::protected_context;
use crate::error::ApiError;
use crate::listing_sources::types::ListingSourceData;
use crate::state::ListingSourcesState;
use crate::wire::parse_path_object_id;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use listing_source_core::ListingSourceId;
use listing_source_service::use_cases::queries::get_listing_source::GetListingSourceRequest;

pub async fn get_listing_source(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
) -> Response {
    let listing_source_id: ListingSourceId =
        match parse_path_object_id(&raw_listing_source_id, "listingSourceId", "ListingSource") {
            Ok(value) => value,
            Err(error) => return error.into_response(),
        };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };

    match state
        .get
        .execute(&context, GetListingSourceRequest::ById(listing_source_id))
        .await
    {
        Ok(result) => axum::Json(ListingSourceData::from(result)).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}
