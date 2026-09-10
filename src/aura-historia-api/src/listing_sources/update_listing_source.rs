use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::listing_sources::types::{ListingSourceReferenceData, UpdateListingSourceData};
use crate::state::ListingSourcesState;
use crate::wire::parse_path_object_id;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use listing_source_core::ListingSourceId;
use listing_source_service::use_cases::commands::update_listing_source::UpdateListingSourceCommand;

pub async fn update_listing_source(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
    body: String,
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
    let data = match parse_body(&body) {
        Ok(data) => data,
        Err(error) => return error.into_response(),
    };
    let parts = match data.into_parts() {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let command = UpdateListingSourceCommand {
        listing_source_id,
        name: parts.name,
        ingestion_configuration: parts.ingestion_configuration,
        woocommerce_webhook_secret: parts.woocommerce_webhook_secret,
        url: parts.url,
        image: parts.image,
        referral_configuration: parts.referral_configuration,
    };

    match state.update.execute(&context, command).await {
        Ok(result) => axum::Json(ListingSourceReferenceData::from((
            result.listing_source_id,
            result.slug_id,
        )))
        .into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn parse_body(body: &str) -> Result<UpdateListingSourceData, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }
    serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}
