use super::types::PartyCollectionData;
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_ORDER_VALUE, BAD_QUERY_PARAMETER_VALUE, BAD_SORT_VALUE};
use crate::state::PartiesState;
use crate::wire::parse_query_object_id;
use application::pagination::Cursor;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;
use domain_primitives::sort::{Sort, SortOrder};
use party_core::party_id::PartyId;
use party_core::party_search::PartySearch;
use party_core::sort_party_field::SortPartyField;
use party_service::use_cases::queries::search_parties::SearchPartiesRequest;
use serde::Deserialize;
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchPartiesQuery {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    phone: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(
        default,
        with = "domain_primitives::query::range_query::range_rfc3339::option"
    )]
    created: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        default,
        with = "domain_primitives::query::range_query::range_rfc3339::option"
    )]
    updated: Option<RangeQuery<OffsetDateTime>>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    search_after: Option<String>,
}

pub async fn search_parties(
    State(state): State<PartiesState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let request = match parse_search_parties_query(raw_query.as_deref()) {
        Ok(request) => request,
        Err(error) => return no_store(error.into_response()),
    };

    match state.search_parties.execute(&context, request).await {
        Ok(result) => no_store(axum::Json(PartyCollectionData::from(result)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_search_parties_query(raw_query: Option<&str>) -> Result<SearchPartiesRequest, ApiError> {
    let query: SearchPartiesQuery = serde_qs::Config::new()
        .use_form_encoding(true)
        .deserialize_str(raw_query.unwrap_or_default())
        .map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_detail(error.to_string())
        })?;

    let search = PartySearch {
        query: parse_text_query(query.query, "query")?,
        name_query: parse_text_query(query.name, "name")?,
        phone_query: parse_text_query(query.phone, "phone")?,
        email_query: parse_text_query(query.email, "email")?,
        created: query.created,
        updated: query.updated,
    };
    let sort = parse_sort(query.sort.as_deref(), query.order.as_deref())?;
    let cursor = parse_cursor(query.size.as_deref(), query.search_after.as_deref())?;

    Ok(SearchPartiesRequest {
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

fn parse_sort(
    sort: Option<&str>,
    order: Option<&str>,
) -> Result<Option<Sort<SortPartyField>>, ApiError> {
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

fn parse_sort_field(value: &str) -> Result<SortPartyField, ApiError> {
    match value {
        "name" => Ok(SortPartyField::Name),
        "email" => Ok(SortPartyField::Email),
        "phone" => Ok(SortPartyField::Phone),
        "created" => Ok(SortPartyField::Created),
        "updated" => Ok(SortPartyField::Updated),
        value => Err(ApiError::bad_request(BAD_SORT_VALUE)
            .with_query_field("sort")
            .with_detail(format!(
                "Expected any of: 'name', 'email', 'phone', 'created', 'updated'. Got: '{value}'"
            ))),
    }
}

fn parse_cursor(
    size: Option<&str>,
    search_after: Option<&str>,
) -> Result<Option<Cursor<PartyId>>, ApiError> {
    let size = size
        .map(|value| value.parse::<u64>().map(|size| size.clamp(1, 100)))
        .transpose()
        .map_err(|error| bad_query("size", error))?;
    let search_after = search_after.map(parse_search_after).transpose()?;

    if size.is_some() || search_after.is_some() {
        Ok(Some(Cursor {
            size: size.unwrap_or_else(|| Cursor::<PartyId>::default().size),
            search_after,
        }))
    } else {
        Ok(None)
    }
}

fn parse_search_after(value: &str) -> Result<PartyId, ApiError> {
    parse_query_object_id(value, "searchAfter", "Party")
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
    use time::macros::datetime;

    #[test]
    fn should_map_party_search_query_to_service_request() -> Result<(), ApiError> {
        let party_id = PartyId::new();
        let request = parse_search_parties_query(Some(&format!(
            "query=operator&name=Antik&phone=%2B49&email=example.com&created%5Bmin%5D=2026-01-01T00%3A00%3A00Z&created%5Bmax%5D=2026-12-31T23%3A59%3A59Z&updated%5Bmin%5D=2026-02-01T00%3A00%3A00Z&sort=email&order=desc&size=200&searchAfter={party_id}",
        )))?;

        assert_eq!(Some("operator"), request.search.query.as_deref());
        assert_eq!(Some("Antik"), request.search.name_query.as_deref());
        assert_eq!(Some("+49"), request.search.phone_query.as_deref());
        assert_eq!(Some("example.com"), request.search.email_query.as_deref());
        assert_eq!(
            Some(RangeQuery {
                min: Some(datetime!(2026-01-01 00:00 UTC)),
                max: Some(datetime!(2026-12-31 23:59:59 UTC)),
            }),
            request.search.created
        );
        assert_eq!(
            Some(RangeQuery {
                min: Some(datetime!(2026-02-01 00:00 UTC)),
                max: None,
            }),
            request.search.updated
        );
        assert_eq!(
            Some(Sort {
                sort: SortPartyField::Email,
                order: SortOrder::Desc,
            }),
            request.sort
        );
        assert_eq!(
            Some(Cursor {
                size: 100,
                search_after: Some(party_id),
            }),
            request.cursor
        );
        Ok(())
    }

    #[test]
    fn should_reject_legacy_json_array_party_cursor() {
        let query = format!("searchAfter=%5B%22name%22%2C%22{}%22%5D", PartyId::new());

        let error = parse_search_parties_query(Some(&query))
            .expect_err("legacy JSON-array cursor must be rejected");

        assert_eq!(crate::error::INVALID_OBJECT_ID, error.code());
    }

    #[test]
    fn should_clamp_party_search_page_size() -> Result<(), ApiError> {
        let request = parse_search_parties_query(Some("size=0"))?;
        assert_eq!(1, request.cursor.as_ref().map_or(0, |cursor| cursor.size));

        let request = parse_search_parties_query(Some("size=1000"))?;
        assert_eq!(100, request.cursor.as_ref().map_or(0, |cursor| cursor.size));
        Ok(())
    }

    #[test]
    fn should_reject_invalid_party_search_query_values() {
        assert!(parse_search_parties_query(Some("sort=invalid&order=asc")).is_err());
        assert!(parse_search_parties_query(Some("sort=email&order=sideways")).is_err());
        assert!(parse_search_parties_query(Some("size=not-a-number")).is_err());
        assert!(parse_search_parties_query(Some("searchAfter=not-an-object-id")).is_err());
        assert!(parse_search_parties_query(Some("created[min]=not-a-timestamp")).is_err());
    }
}
