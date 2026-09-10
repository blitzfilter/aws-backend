use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::wire::parse_path_object_id;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use partnership_core::partnership_application_id::PartnershipApplicationId;
use serde::Deserialize;

pub(super) fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(super) fn parse_id(raw: &str) -> Result<PartnershipApplicationId, ApiError> {
    parse_path_object_id(raw, "partnershipApplicationId", "PartnershipApplication")
}

pub(super) fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Request body is required."));
    }
    serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}
