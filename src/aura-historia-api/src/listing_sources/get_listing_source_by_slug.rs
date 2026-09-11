use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{
    ApiError, BAD_PATH_PARAMETER_VALUE, BAD_QUERY_PARAMETER_VALUE, LISTING_SOURCE_INTERNAL_ERROR,
};
use crate::listing_sources::types::PublicListingSourceData;
use crate::state::ListingSourcesState;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use listing_source_core::ListingSourceSlugId;
use listing_source_service::use_cases::queries::get_public_listing_source_by_slug::GetPublicListingSourceBySlugRequest;

const MAX_RAW_QUERY_BYTES: usize = 8 * 1024;

pub async fn get_listing_source_by_slug(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    Path(raw_listing_source_slug_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Err(error) = reject_query(raw_query.as_deref()) {
        return no_store(error.into_response());
    }
    let listing_source_slug_id = match ListingSourceSlugId::raw(&raw_listing_source_slug_id) {
        Ok(value) => value,
        Err(_) => {
            return no_store(
                ApiError::bad_request(BAD_PATH_PARAMETER_VALUE)
                    .with_path_field("listingSourceSlugId")
                    .with_detail("Path parameter 'listingSourceSlugId' is invalid.")
                    .into_response(),
            );
        }
    };
    let Some(budget) = state.public_read_budget.as_ref() else {
        return no_store(
            ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                .with_detail("Public listing source details are not configured.")
                .into_response(),
        );
    };
    let Some(_permit) = budget.try_acquire() else {
        return overloaded_response();
    };
    let started = std::time::Instant::now();
    let metadata = request_metadata(&headers);
    let principal = match tokio::time::timeout(
        budget.request_timeout(),
        OptionalAuthExtractor::new(state.authenticator.as_ref()).extract(&headers, &metadata),
    )
    .await
    {
        Ok(Ok(principal)) => principal,
        Ok(Err(error)) => return no_store(ApiError::from(error).into_response()),
        Err(_) => {
            return no_store(
                ApiError::service_unavailable(crate::error::LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Public listing source details timed out.")
                    .into_response(),
            );
        }
    };
    let Some(get_public_by_slug) = state.get_public_by_slug.as_ref() else {
        return no_store(
            ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                .with_detail("Public listing source details are not configured.")
                .into_response(),
        );
    };

    let remaining = budget.request_timeout().saturating_sub(started.elapsed());
    match tokio::time::timeout(
        remaining,
        get_public_by_slug.execute(
            &principal.operation_context(metadata),
            GetPublicListingSourceBySlugRequest {
                slug_id: listing_source_slug_id,
            },
        ),
    )
    .await
    {
        Err(_) => no_store(
            ApiError::service_unavailable(crate::error::LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
                .with_detail("Public listing source details timed out.")
                .into_response(),
        ),
        Ok(result) => match result {
            Ok(result) => {
                no_store(axum::Json(PublicListingSourceData::from(result)).into_response())
            }
            Err(error) => no_store(ApiError::from(error).into_response()),
        },
    }
}

fn reject_query(raw_query: Option<&str>) -> Result<(), ApiError> {
    let raw_query = raw_query.unwrap_or_default();
    if raw_query.len() > MAX_RAW_QUERY_BYTES {
        return Err(ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("query")
            .with_detail("Query string exceeds 8 KiB."));
    }
    if raw_query.is_empty() {
        Ok(())
    } else {
        Err(ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_detail("This endpoint does not accept query parameters."))
    }
}

fn overloaded_response() -> Response {
    let mut response =
        ApiError::service_unavailable(crate::error::LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
            .with_detail("Public listing source reads are at capacity.")
            .into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    no_store(response)
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_accept_absent_slug_detail_query() {
        assert!(reject_query(None).is_ok());
        assert!(reject_query(Some("")).is_ok());
    }

    #[test]
    fn should_reject_slug_detail_query_parameters() {
        assert!(reject_query(Some("query=source")).is_err());
        assert!(reject_query(Some("a".repeat(MAX_RAW_QUERY_BYTES + 1).as_str())).is_err());
    }
}
