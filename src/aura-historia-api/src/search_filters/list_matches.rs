use super::util::{no_store, parse_json_query, parse_search_filter_id};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE, SEARCH_FILTER_INTERNAL_ERROR};
use crate::product_listings::product_data::{
    PersonalizedProductListingDetailsData, personalized_product_details_data,
};
use crate::state::SearchFiltersState;
use crate::wire::parse_query_object_id;
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use localization::Language;
use money::Currency;

use crate::pagination_data::JsonCursoredData;
use application::pagination::{Cursor, CursoredResult};
use domain_primitives::sort::SortOrder;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_service::ports::SearchFilterMatchCursor;
use search_filter_service::use_cases::ListSearchFilterMatchesRequest;
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSearchFilterMatchesQuery {
    #[serde(default)]
    #[serde(with = "crate::wire::language")]
    language: Language,
    #[serde(default, with = "crate::wire::currency")]
    currency: Currency,
    #[serde(default)]
    size: Option<SearchFilterMatchPageSize>,
    #[serde(default)]
    search_after: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(transparent)]
struct SearchFilterMatchPageSize(u64);

pub(super) async fn list_search_filter_matches(
    State(state): State<SearchFiltersState>,
    headers: HeaderMap,
    Path(raw_search_filter_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let search_filter_id = match parse_search_filter_id(&raw_search_filter_id) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query: ListSearchFilterMatchesQuery =
        match serde_qs::from_str(raw_query.as_deref().unwrap_or_default()) {
            Ok(value) => value,
            Err(error) => {
                return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response();
            }
        };
    let cursor = match matches_cursor(query.size, query.search_after.as_deref()) {
        Ok(cursor) => cursor,
        Err(error) => return error.into_response(),
    };
    let (context, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    match state
        .list_search_filter_matches
        .execute(
            &context,
            ListSearchFilterMatchesRequest {
                user_id,
                search_filter_id,
                language: query.language,
                currency: query.currency,
                cursor: Some(cursor),
                order: SortOrder::Asc,
            },
        )
        .await
    {
        Ok(result) => {
            let search_after = match result
                .cursor
                .search_after
                .map(matches_cursor_value)
                .transpose()
            {
                Ok(value) => value,
                Err(error) => return error.into_response(),
            };
            no_store(
                Json(
                    JsonCursoredData::<PersonalizedProductListingDetailsData>::from(
                        CursoredResult {
                            items: result
                                .items
                                .into_iter()
                                .map(personalized_product_details_data)
                                .collect(),
                            cursor: Cursor {
                                size: result.cursor.size,
                                search_after,
                            },
                            total: result.total,
                        },
                    ),
                )
                .into_response(),
            )
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn matches_cursor(
    size: Option<SearchFilterMatchPageSize>,
    search_after: Option<&str>,
) -> Result<Cursor<SearchFilterMatchCursor>, ApiError> {
    let size = size.map_or(21, |value| value.0).clamp(1, 100);
    let search_after = search_after.map(parse_matches_cursor).transpose()?;
    Ok(Cursor { size, search_after })
}

fn parse_matches_cursor(raw: &str) -> Result<SearchFilterMatchCursor, ApiError> {
    let value: Value = parse_json_query(raw, "searchAfter")?;
    let Value::Array(values) = value else {
        return Err(ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail("searchAfter must be a JSON array containing timestamp and product ID."));
    };
    let [Value::String(created), Value::String(product_listing_id)] = values.as_slice() else {
        return Err(ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail("searchAfter must contain an RFC3339 timestamp and ProductListing ID."));
    };
    let created = OffsetDateTime::parse(created, &Rfc3339).map_err(|error| {
        ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail(error.to_string())
    })?;
    let product_listing_id = parse_query_object_id::<ProductListingId>(
        product_listing_id,
        "searchAfter",
        "ProductListing",
    )?;
    Ok(SearchFilterMatchCursor {
        created,
        product_listing_id,
    })
}

fn matches_cursor_value(cursor: SearchFilterMatchCursor) -> Result<Value, ApiError> {
    cursor
        .created
        .format(&Rfc3339)
        .map(|created| json!([created, cursor.product_listing_id]))
        .map_err(|_| {
            ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                .with_detail("Search-filter match cursor failed internally.")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use listing_source_core::ListingSourceId;

    #[test]
    fn should_default_match_currency_to_eur_and_parse_requested_currency()
    -> Result<(), Box<dyn std::error::Error>> {
        let default_query: ListSearchFilterMatchesQuery = serde_qs::from_str("")?;
        let requested_query: ListSearchFilterMatchesQuery = serde_qs::from_str("currency=USD")?;

        assert_eq!(Currency::Eur, default_query.currency);
        assert_eq!(Currency::Usd, requested_query.currency);
        Ok(())
    }

    #[test]
    fn should_clamp_match_page_size_after_defaulting() -> Result<(), Box<dyn std::error::Error>> {
        for (size, expected) in [
            (None, 21),
            (Some(SearchFilterMatchPageSize(0)), 1),
            (Some(SearchFilterMatchPageSize(1)), 1),
            (Some(SearchFilterMatchPageSize(21)), 21),
            (Some(SearchFilterMatchPageSize(100)), 100),
            (Some(SearchFilterMatchPageSize(101)), 100),
        ] {
            assert_eq!(expected, matches_cursor(size, None)?.size);
        }
        Ok(())
    }

    #[test]
    fn should_reject_malformed_or_negative_match_page_size_query() {
        for raw_query in ["size=not-an-integer", "size=-1"] {
            assert!(serde_qs::from_str::<ListSearchFilterMatchesQuery>(raw_query).is_err());
        }
    }

    #[test]
    fn should_parse_tie_safe_match_cursor() -> Result<(), Box<dyn std::error::Error>> {
        let product_listing_id = ProductListingId::new();
        let raw_cursor =
            serde_json::to_string(&json!(["2026-08-05T12:30:00Z", product_listing_id]))?;
        let cursor = parse_matches_cursor(&raw_cursor)?;

        assert_eq!(
            cursor.created,
            OffsetDateTime::parse("2026-08-05T12:30:00Z", &Rfc3339)?
        );
        assert_eq!(cursor.product_listing_id, product_listing_id);
        assert_eq!(
            matches_cursor_value(cursor)?,
            json!(["2026-08-05T12:30:00Z", product_listing_id])
        );
        for raw in [
            "not-json".to_owned(),
            "{}".to_owned(),
            "[]".to_owned(),
            json!(["only-one-value"]).to_string(),
            json!(["bad-date", product_listing_id]).to_string(),
        ] {
            let Err(error) = parse_matches_cursor(&raw) else {
                return Err("malformed match cursor was accepted".into());
            };
            assert_eq!(BAD_QUERY_PARAMETER_VALUE, error.code());
        }
        for invalid_id in [
            ListingSourceId::new().to_string(),
            product_listing_id.as_uuid().to_string(),
            "pl_not-a-typeid".to_owned(),
        ] {
            let raw = json!(["2026-08-05T12:30:00Z", invalid_id]).to_string();
            let Err(error) = parse_matches_cursor(&raw) else {
                return Err("noncanonical ProductListing cursor ID was accepted".into());
            };
            assert_eq!(crate::error::INVALID_OBJECT_ID, error.code());
        }
        Ok(())
    }

    #[test]
    fn should_document_only_persisted_match_route() {
        let swagger = include_str!("../../../../docs/swagger.yaml");

        assert!(swagger.contains("/api/v1/me/search-filters/{userSearchFilterId}/matches:"));
        assert!(swagger.contains("operationId: listSearchFilterMatches"));
        assert!(
            !swagger.contains("/api/v1/me/search-filters/{userSearchFilterId}/product-listings:")
        );
        assert!(!swagger.contains("getSearchFilterPreviewProductListings"));
    }
}
