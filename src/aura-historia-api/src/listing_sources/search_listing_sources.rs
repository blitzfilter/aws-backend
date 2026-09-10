use super::types::ListingSourceSearchCollectionData;
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_ORDER_VALUE, BAD_QUERY_PARAMETER_VALUE, BAD_SORT_VALUE};
use crate::state::ListingSourcesState;
use crate::wire::parse_query_object_id;
use application::pagination::Cursor;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use domain_primitives::query::text_query::TextQuery;
use domain_primitives::sort::{Sort, SortOrder};
use listing_source_core::{
    ListingIngestionMethod, ListingSourceId, ListingSourceSearch, ListingSourceSlugId,
    SortListingSourceField,
};
use listing_source_service::use_cases::queries::search_listing_sources::SearchListingSourcesRequest;
use party_core::party_id::PartyId;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchListingSourcesQuery {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    listing_source_id: Option<String>,
    #[serde(default)]
    listing_source_slug_id: Option<String>,
    #[serde(default)]
    operator_party_id: Option<String>,
    #[serde(default)]
    ingestion_method: Option<String>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    search_after: Option<String>,
}

pub async fn search_listing_sources(
    State(state): State<ListingSourcesState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let request = match parse_search_listing_sources_query(raw_query.as_deref()) {
        Ok(request) => request,
        Err(error) => return no_store(error.into_response()),
    };

    match state.search.execute(&context, request).await {
        Ok(result) => {
            no_store(axum::Json(ListingSourceSearchCollectionData::from(result)).into_response())
        }
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_search_listing_sources_query(
    raw_query: Option<&str>,
) -> Result<SearchListingSourcesRequest, ApiError> {
    let query: SearchListingSourcesQuery = serde_qs::Config::new()
        .use_form_encoding(true)
        .deserialize_str(raw_query.unwrap_or_default())
        .map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_detail(error.to_string())
        })?;

    let search = ListingSourceSearch {
        query: parse_text_query(query.query, "query")?,
        name_query: parse_text_query(query.name, "name")?,
        listing_source_id: parse_listing_source_id(query.listing_source_id, "listingSourceId")?,
        listing_source_slug_id: parse_listing_source_slug_id(
            query.listing_source_slug_id,
            "listingSourceSlugId",
        )?,
        operator_party_id: parse_party_id(query.operator_party_id, "operatorPartyId")?,
        ingestion_method: parse_ingestion_method(query.ingestion_method)?,
    };
    let sort = parse_sort(query.sort.as_deref(), query.order.as_deref())?;
    let cursor = parse_cursor(query.size.as_deref(), query.search_after.as_deref())?;

    Ok(SearchListingSourcesRequest {
        search,
        sort,
        cursor,
    })
}

fn parse_text_query(
    value: Option<String>,
    field: &'static str,
) -> Result<Option<TextQuery<0>>, ApiError> {
    value
        .map(|value| TextQuery::<0>::try_from(value).map_err(|error| bad_query(field, error)))
        .transpose()
}

fn parse_listing_source_id(
    value: Option<String>,
    field: &'static str,
) -> Result<Option<ListingSourceId>, ApiError> {
    value
        .map(|value| parse_query_object_id(&value, field, "ListingSource"))
        .transpose()
}

fn parse_listing_source_slug_id(
    value: Option<String>,
    field: &'static str,
) -> Result<Option<ListingSourceSlugId>, ApiError> {
    value
        .map(|value| ListingSourceSlugId::raw(value).map_err(|error| bad_query(field, error)))
        .transpose()
}

fn parse_party_id(value: Option<String>, field: &'static str) -> Result<Option<PartyId>, ApiError> {
    value
        .map(|value| parse_query_object_id(&value, field, "Party"))
        .transpose()
}

fn parse_ingestion_method(
    value: Option<String>,
) -> Result<Option<ListingIngestionMethod>, ApiError> {
    value
        .map(|value| {
            value
                .parse()
                .map_err(|error| bad_query("ingestionMethod", error))
        })
        .transpose()
}

fn parse_sort(
    sort: Option<&str>,
    order: Option<&str>,
) -> Result<Option<Sort<SortListingSourceField>>, ApiError> {
    match (sort, order) {
        (Some(sort), Some(order)) => {
            let sort = parse_sort_field(sort)?;
            let order = SortOrder::try_from(order).map_err(|detail| {
                ApiError::bad_request(BAD_ORDER_VALUE)
                    .with_query_field("order")
                    .with_detail(detail)
            })?;
            Ok(Some(Sort { sort, order }))
        }
        _ => Ok(None),
    }
}

fn parse_sort_field(value: &str) -> Result<SortListingSourceField, ApiError> {
    match value {
        "name" => Ok(SortListingSourceField::Name),
        "slug" => Ok(SortListingSourceField::Slug),
        "created" => Ok(SortListingSourceField::Created),
        "updated" => Ok(SortListingSourceField::Updated),
        value => Err(ApiError::bad_request(BAD_SORT_VALUE)
            .with_query_field("sort")
            .with_detail(format!(
                "Expected any of: 'name', 'slug', 'created', 'updated'. Got: '{value}'"
            ))),
    }
}

fn parse_cursor(
    size: Option<&str>,
    search_after: Option<&str>,
) -> Result<Option<Cursor<ListingSourceId>>, ApiError> {
    let size = size
        .map(|value| value.parse::<u64>().map(|size| size.clamp(1, 100)))
        .transpose()
        .map_err(|error| bad_query("size", error))?;
    let search_after = search_after.map(parse_search_after).transpose()?;

    if size.is_some() || search_after.is_some() {
        Ok(Some(Cursor {
            size: size.unwrap_or_else(|| Cursor::<ListingSourceId>::default().size),
            search_after,
        }))
    } else {
        Ok(None)
    }
}

fn parse_search_after(value: &str) -> Result<ListingSourceId, ApiError> {
    parse_query_object_id(value, "searchAfter", "ListingSource")
}

fn bad_query(field: &'static str, detail: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail.to_string())
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
    use domain_primitives::sort::SortOrder;

    #[test]
    fn should_map_listing_source_search_query_to_service_request() -> Result<(), ApiError> {
        let listing_source_id = ListingSourceId::new();
        let operator_party_id = PartyId::new();
        let request = parse_search_listing_sources_query(Some(&format!(
            "query=operator&name=Antik&listingSourceId={listing_source_id}&listingSourceSlugId=antik-source&operatorPartyId={operator_party_id}&ingestionMethod=SHOPIFY&sort=created&order=desc&size=200&searchAfter={listing_source_id}"
        )))?;

        assert_eq!(Some("operator"), request.search.query.as_deref());
        assert_eq!(Some("Antik"), request.search.name_query.as_deref());
        assert_eq!(Some(listing_source_id), request.search.listing_source_id);
        assert_eq!(
            Some(
                ListingSourceSlugId::raw("antik-source")
                    .unwrap_or_else(|error| panic!("test slug: {error}"))
            ),
            request.search.listing_source_slug_id
        );
        assert_eq!(Some(operator_party_id), request.search.operator_party_id);
        assert_eq!(
            Some(ListingIngestionMethod::Shopify),
            request.search.ingestion_method
        );
        assert_eq!(
            Some(Sort {
                sort: SortListingSourceField::Created,
                order: SortOrder::Desc,
            }),
            request.sort
        );
        assert_eq!(
            Some(Cursor {
                size: 100,
                search_after: Some(listing_source_id),
            }),
            request.cursor
        );
        Ok(())
    }

    #[test]
    fn should_clamp_listing_source_search_page_size() -> Result<(), ApiError> {
        let request = parse_search_listing_sources_query(Some("size=0"))?;
        assert_eq!(1, request.cursor.as_ref().map_or(0, |cursor| cursor.size));

        let request = parse_search_listing_sources_query(Some("size=1000"))?;
        assert_eq!(100, request.cursor.as_ref().map_or(0, |cursor| cursor.size));
        Ok(())
    }

    #[test]
    fn should_reject_invalid_listing_source_search_query_values() {
        for query in [
            "sort=invalid&order=asc",
            "sort=name&order=sideways",
            "size=not-a-number",
            "searchAfter=not-an-object-id",
            "listingSourceId=not-an-object-id",
            "listingSourceSlugId=Not-A-Slug",
            "operatorPartyId=not-an-object-id",
            "ingestionMethod=UNKNOWN",
        ] {
            assert!(
                parse_search_listing_sources_query(Some(query)).is_err(),
                "{query}"
            );
        }
    }
}
