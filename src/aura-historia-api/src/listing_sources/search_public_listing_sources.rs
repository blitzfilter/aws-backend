use super::types::PublicListingSourceSearchCollectionData;
use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE, LISTING_SOURCE_INTERNAL_ERROR};
use crate::state::ListingSourcesState;
use crate::wire::parse_query_object_id;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use listing_source_core::ListingSourceId;
use listing_source_service::use_cases::queries::search_public_listing_sources::{
    DEFAULT_PUBLIC_LISTING_SOURCE_SEARCH_PAGE_SIZE, PublicListingSourceSearchContinuation,
    PublicListingSourceSearchPosition, PublicListingSourceSearchQuery,
    SearchPublicListingSourcesRequest,
};
use serde::{Deserialize, Serialize};

const MAX_RAW_QUERY_BYTES: usize = 8 * 1024;
const MAX_CURSOR_ENCODED_BYTES: usize = 4 * 1024;
const MAX_CURSOR_DECODED_BYTES: usize = 3 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublicListingSourceSearchCursorData {
    binding: String,
    match_tier: u8,
    name_search: String,
    listing_source_id: String,
}

pub async fn search_public_listing_sources(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request = match parse_request(raw_query.as_deref()) {
        Ok(request) => request,
        Err(error) => return no_store(error.into_response()),
    };
    let Some(budget) = state.public_read_budget.as_ref() else {
        return no_store(
            ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                .with_detail("Public listing source search is not configured.")
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
                    .with_detail("Public listing source search timed out.")
                    .into_response(),
            );
        }
    };
    let Some(search_public) = state.search_public.as_ref() else {
        return no_store(
            ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                .with_detail("Public listing source search is not configured.")
                .into_response(),
        );
    };

    let remaining = budget.request_timeout().saturating_sub(started.elapsed());
    match tokio::time::timeout(
        remaining,
        search_public.execute(&principal.operation_context(metadata), request),
    )
    .await
    {
        Err(_) => no_store(
            ApiError::service_unavailable(crate::error::LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
                .with_detail("Public listing source search timed out.")
                .into_response(),
        ),
        Ok(result) => match result {
            Ok(result) => {
                let search_after = result
                    .continuation
                    .as_ref()
                    .map(encode_continuation)
                    .transpose();
                match search_after {
                    Ok(search_after) => no_store(
                        axum::Json(PublicListingSourceSearchCollectionData::new(
                            result,
                            search_after,
                        ))
                        .into_response(),
                    ),
                    Err(error) => no_store(error.into_response()),
                }
            }
            Err(error) => no_store(ApiError::from(error).into_response()),
        },
    }
}

fn parse_request(raw_query: Option<&str>) -> Result<SearchPublicListingSourcesRequest, ApiError> {
    let raw_query = raw_query.unwrap_or_default();
    if raw_query.len() > MAX_RAW_QUERY_BYTES {
        return Err(bad_query("query", "Query string exceeds 8 KiB."));
    }

    let mut query = None;
    let mut size = None;
    let mut search_after = None;
    for (field, value) in url::form_urlencoded::parse(raw_query.as_bytes()) {
        let (field, target): (&'static str, &mut Option<String>) = match field.as_ref() {
            "query" => ("query", &mut query),
            "size" => ("size", &mut size),
            "searchAfter" => ("searchAfter", &mut search_after),
            _ => return Err(bad_query("query", "Unknown query parameter.")),
        };
        if target.replace(value.into_owned()).is_some() {
            return Err(bad_query(field, "Repeated query parameter."));
        }
    }

    let page_size = parse_page_size(size.as_deref())?;
    let query = PublicListingSourceSearchQuery::new(query)
        .map_err(|_| bad_query("query", "Query parameter is invalid."))?;
    let continuation = search_after
        .as_deref()
        .map(parse_continuation)
        .transpose()?;
    SearchPublicListingSourcesRequest::new(query, page_size, continuation).map_err(|_| {
        bad_query(
            "searchAfter",
            "Search continuation is invalid for this request.",
        )
    })
}

fn parse_page_size(value: Option<&str>) -> Result<u8, ApiError> {
    let Some(value) = value else {
        return Ok(DEFAULT_PUBLIC_LISTING_SOURCE_SEARCH_PAGE_SIZE);
    };
    let value = value
        .parse::<u16>()
        .map_err(|_| bad_query("size", "Size must be an integer from 1 through 50."))?;
    u8::try_from(value)
        .ok()
        .filter(|value| (1..=50).contains(value))
        .ok_or_else(|| bad_query("size", "Size must be an integer from 1 through 50."))
}

fn parse_continuation(value: &str) -> Result<PublicListingSourceSearchContinuation, ApiError> {
    if value.len() > MAX_CURSOR_ENCODED_BYTES {
        return Err(bad_query(
            "searchAfter",
            "Search continuation exceeds the size limit.",
        ));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| bad_query("searchAfter", "Search continuation is invalid."))?;
    if bytes.len() > MAX_CURSOR_DECODED_BYTES {
        return Err(bad_query(
            "searchAfter",
            "Search continuation exceeds the size limit.",
        ));
    }
    let data: PublicListingSourceSearchCursorData = serde_json::from_slice(&bytes)
        .map_err(|_| bad_query("searchAfter", "Search continuation is invalid."))?;
    let binding = URL_SAFE_NO_PAD
        .decode(data.binding)
        .map_err(|_| bad_query("searchAfter", "Search continuation is invalid."))?
        .try_into()
        .map_err(|_: Vec<u8>| bad_query("searchAfter", "Search continuation is invalid."))?;
    let listing_source_id: ListingSourceId =
        parse_query_object_id(&data.listing_source_id, "searchAfter", "ListingSource")?;
    let position = PublicListingSourceSearchPosition::new(
        data.match_tier,
        data.name_search,
        listing_source_id,
    )
    .map_err(|_| bad_query("searchAfter", "Search continuation is invalid."))?;
    Ok(PublicListingSourceSearchContinuation::new(
        binding, position,
    ))
}

fn encode_continuation(
    continuation: &PublicListingSourceSearchContinuation,
) -> Result<String, ApiError> {
    let position = continuation.position();
    let payload = PublicListingSourceSearchCursorData {
        binding: URL_SAFE_NO_PAD.encode(continuation.binding()),
        match_tier: position.match_tier(),
        name_search: position.name_search().to_owned(),
        listing_source_id: position.listing_source_id().to_string(),
    };
    let bytes = serde_json::to_vec(&payload).map_err(|_| {
        ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
            .with_detail("Public listing source continuation failed internally.")
    })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn bad_query(field: &'static str, detail: &str) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail)
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
    fn should_parse_public_query_with_default_size() -> Result<(), ApiError> {
        let request = parse_request(Some("query=mul"))?;
        assert_eq!(21, request.page_size());
        assert_eq!(Some("mul"), request.query().canonical_text());
        Ok(())
    }

    #[test]
    fn should_reject_unknown_repeated_and_out_of_range_query_parameters() {
        for query in ["sort=name", "query=mu&query=mul", "size=0", "size=51"] {
            assert!(parse_request(Some(query)).is_err(), "{query}");
        }
    }

    #[test]
    fn should_reject_oversized_or_malformed_continuations() {
        assert!(parse_request(Some("searchAfter=not-base64")).is_err());
        let oversized = "a".repeat(MAX_CURSOR_ENCODED_BYTES + 1);
        assert!(parse_request(Some(&format!("searchAfter={oversized}"))).is_err());
    }
}
