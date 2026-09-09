use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::wire::parse_path_object_id;

use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use user_core::user_id::UserId;

pub(crate) fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
#[allow(clippy::result_large_err)]
pub(crate) fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, Response> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("Request body is required.")
            .into_response());
    }
    serde_json::from_str(body).map_err(|e| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail(e.to_string())
            .into_response()
    })
}

#[allow(clippy::result_large_err)]
pub(crate) fn parse_user_id(raw: &str, field: &'static str) -> Result<UserId, Response> {
    parse_path_object_id(raw, field, "User").map_err(IntoResponse::into_response)
}
