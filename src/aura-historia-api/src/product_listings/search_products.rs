use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_ORDER_VALUE, BAD_QUERY_PARAMETER_VALUE, BAD_SORT_VALUE};
use crate::product_listings::product_data::{
    PersonalizedProductListingSummaryData, personalized_product_summary_data,
};
use crate::state::ProductListingsState;

use application::operation_context::Principal;
use application::pagination::Cursor;
use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;
use domain_primitives::sort::{Sort, SortOrder};
use fxrate_core::FxRateId;

use localization::Language;
use money::Currency;
use money::MonetaryAmount;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_orderability::ListingOrderability;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_search::{
    EnhancedSearchDescription, ListingAvailabilityQuery, ProductListingSearch,
};

use listing_source_core::ListingSourceId;
use product_listing_core::sort_product_listing_field::SortProductListingField;
use product_listing_service::use_cases::{
    ProductListingSearchCursor, SearchProductListingsRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::hash::Hash;
use std::str::FromStr;
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingSearchData {
    #[serde(default)]
    #[serde(with = "crate::wire::language")]
    language: Language,
    #[serde(default, with = "crate::wire::currency")]
    currency: Currency,
    #[serde(default, rename = "productQuery")]
    product_listing_query: Vec<TextQuery<1>>,
    #[serde(default)]
    enhanced_search_description: Option<String>,
    #[serde(default)]
    exclude_product_listing_id: HashSet<String>,
    #[serde(default)]
    listing_source_id: HashSet<String>,
    #[serde(default)]
    exclude_listing_source_id: HashSet<String>,
    #[serde(default)]
    price: Option<RangeQuery<u64>>,
    #[serde(
        default,
        deserialize_with = "crate::wire::listing_availability::set_option::deserialize"
    )]
    availability: Option<HashSet<ListingAvailability>>,
    #[serde(
        default,
        deserialize_with = "crate::wire::listing_orderability::set_option::deserialize"
    )]
    orderability: Option<HashSet<ListingOrderability>>,
    #[serde(default)]
    include_unspecified_availability: Option<bool>,
    #[serde(
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default
    )]
    created: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default
    )]
    updated: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default
    )]
    lot_bidding_opens: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default
    )]
    lot_scheduled_closes: Option<RangeQuery<OffsetDateTime>>,
}

#[derive(Debug, Clone, Copy)]
enum SortProductListingFieldData {
    Score,
    Updated,
    Created,
}

impl TryFrom<&str> for SortProductListingFieldData {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "score" => Ok(Self::Score),
            "updated" => Ok(Self::Updated),
            "created" => Ok(Self::Created),
            _ => Err("Expected any of: 'score', 'updated', 'created'.".to_owned()),
        }
    }
}

impl From<SortProductListingFieldData> for SortProductListingField {
    fn from(value: SortProductListingFieldData) -> Self {
        match value {
            SortProductListingFieldData::Score => Self::Score,
            SortProductListingFieldData::Updated => Self::Updated,
            SortProductListingFieldData::Created => Self::Created,
        }
    }
}

impl TryFrom<ProductListingSearchData> for ProductListingSearch {
    type Error = ApiError;

    fn try_from(data: ProductListingSearchData) -> Result<Self, Self::Error> {
        Ok(Self {
            language: data.language,
            currency: data.currency,
            product_listing_query: data.product_listing_query,
            enhanced_search_description: data
                .enhanced_search_description
                .map(EnhancedSearchDescription::try_from)
                .transpose()
                .map_err(|error| {
                    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                        .with_query_field("enhancedSearchDescription")
                        .with_detail(error.to_string())
                })?,
            exclude_product_listing_id_query: parse_query_object_ids::<ProductListingId>(
                data.exclude_product_listing_id,
                "excludeProductListingId",
                "ProductListing",
            )?
            .into(),
            listing_source_id_query: parse_query_object_ids::<ListingSourceId>(
                data.listing_source_id,
                "listingSourceId",
                "ListingSource",
            )?
            .into(),
            exclude_listing_source_id_query: parse_query_object_ids::<ListingSourceId>(
                data.exclude_listing_source_id,
                "excludeListingSourceId",
                "ListingSource",
            )?
            .into(),
            price_query: data.price.map(|range| range.map(MonetaryAmount::from)),
            availability_query: availability_query_from_parts(
                data.availability,
                data.orderability,
                data.include_unspecified_availability,
            ),
            created_query: data.created,
            updated_query: data.updated,
            lot_bidding_opens_query: data.lot_bidding_opens,
            lot_scheduled_closes_query: data.lot_scheduled_closes,
        })
    }
}

fn parse_query_object_ids<T>(
    values: HashSet<String>,
    field: &'static str,
    semantic_type: &'static str,
) -> Result<HashSet<T>, ApiError>
where
    T: FromStr + Eq + Hash,
{
    values
        .into_iter()
        .map(|value| crate::wire::parse_query_object_id(&value, field, semantic_type))
        .collect()
}

fn availability_query_from_parts(
    availability: Option<HashSet<ListingAvailability>>,
    orderability: Option<HashSet<ListingOrderability>>,
    include_unspecified: Option<bool>,
) -> Option<ListingAvailabilityQuery> {
    if availability.is_none() && orderability.is_none() && include_unspecified.is_none() {
        None
    } else {
        Some(ListingAvailabilityQuery {
            any_of: availability.unwrap_or_default().into(),
            orderability: orderability.unwrap_or_default().into(),
            include_unspecified: include_unspecified.unwrap_or(false),
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CursoredProductListingsData {
    items: Vec<PersonalizedProductListingSummaryData>,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<ProductListingSearchCursorData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<u64>,
}

/// HTTP encoding of the ProductListing-owned opaque continuation cursor.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingSearchCursorData {
    fx_rate_id: FxRateId,
    search_after: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingSearchCursorInputData {
    fx_rate_id: String,
    search_after: Value,
}

impl From<ProductListingSearchCursor> for ProductListingSearchCursorData {
    fn from(cursor: ProductListingSearchCursor) -> Self {
        Self {
            fx_rate_id: cursor.fx_rate_id,
            search_after: cursor.search_after,
        }
    }
}

pub async fn get_products(
    State(state): State<ProductListingsState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let data = match serde_qs::from_str(raw_query.as_deref().unwrap_or_default()) {
        Ok(data) => data,
        Err(error) => {
            return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_detail(error.to_string())
                .into_response();
        }
    };
    handle_search(state, headers, raw_query.as_deref(), data).await
}

async fn handle_search(
    state: ProductListingsState,
    headers: HeaderMap,
    raw_query: Option<&str>,
    data: ProductListingSearchData,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let sort = match parse_sort(raw_query) {
        Ok(sort) => sort,
        Err(error) => return error.into_response(),
    };
    let cursor = match parse_cursor(raw_query) {
        Ok(cursor) => cursor,
        Err(error) => return error.into_response(),
    };

    let search = match ProductListingSearch::try_from(data) {
        Ok(search) => search,
        Err(error) => return error.into_response(),
    };
    let context = principal.operation_context(metadata);
    match state
        .search_products
        .execute(
            &context,
            SearchProductListingsRequest {
                search,
                sort,
                cursor,
            },
        )
        .await
    {
        Ok(result) => {
            let mut response = Json(CursoredProductListingsData {
                items: result
                    .items
                    .into_iter()
                    .map(personalized_product_summary_data)
                    .collect(),
                size: result.cursor.size,
                search_after: result.cursor.search_after.map(Into::into),
                total: result.total,
            })
            .into_response();
            let value = match context.principal {
                Principal::Anonymous => "public, max-age=60, s-maxage=300",
                Principal::User(_)
                | Principal::DelegatedUser { .. }
                | Principal::Service(_)
                | Principal::System => "no-store",
            };
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
            response
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn parse_sort(raw_query: Option<&str>) -> Result<Option<Sort<SortProductListingField>>, ApiError> {
    match (
        query_value(raw_query, "sort"),
        query_value(raw_query, "order"),
    ) {
        (Some(sort), Some(order)) => {
            let sort = SortProductListingFieldData::try_from(sort.as_str()).map_err(|detail| {
                ApiError::bad_request(BAD_SORT_VALUE)
                    .with_query_field("sort")
                    .with_detail(detail)
            })?;
            let order = SortOrder::try_from(order.as_str()).map_err(|detail| {
                ApiError::bad_request(BAD_ORDER_VALUE)
                    .with_query_field("order")
                    .with_detail(detail)
            })?;
            Ok(Some(Sort {
                sort: sort.into(),
                order,
            }))
        }
        _ => Ok(None),
    }
}

fn parse_cursor(
    raw_query: Option<&str>,
) -> Result<Option<Cursor<ProductListingSearchCursor>>, ApiError> {
    let size = query_value(raw_query, "size")
        .map(|value| value.parse::<u64>())
        .transpose()
        .map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field("size")
                .with_detail(error.to_string())
        })?
        .map(|size| size.clamp(1, 100));
    let values = query_values(raw_query, "searchAfter");
    let search_after = match values.as_slice() {
        [] => None,
        [value] => Some(parse_search_after(value)?),
        _ => {
            return Err(ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field("searchAfter")
                .with_detail("ProductListing search cursor must be supplied once."));
        }
    };

    if size.is_some() || search_after.is_some() {
        Ok(Some(Cursor {
            size: size.unwrap_or_else(|| Cursor::<ProductListingSearchCursor>::default().size),
            search_after,
        }))
    } else {
        Ok(None)
    }
}

fn parse_search_after(value: &str) -> Result<ProductListingSearchCursor, ApiError> {
    let cursor =
        serde_json::from_str::<ProductListingSearchCursorInputData>(value).map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field("searchAfter")
                .with_detail(error.to_string())
        })?;
    Ok(ProductListingSearchCursor {
        fx_rate_id: crate::wire::parse_query_object_id(
            &cursor.fx_rate_id,
            "searchAfter",
            "FxRate",
        )?,
        search_after: cursor.search_after,
    })
}

fn query_value(raw_query: Option<&str>, key: &str) -> Option<String> {
    query_values(raw_query, key).into_iter().next()
}

fn query_values(raw_query: Option<&str>, key: &str) -> Vec<String> {
    raw_query
        .map(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .filter(|(name, _)| name == key)
                .map(|(_, value)| value.into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthError, RequestMetadata, TokenAuthenticator, TransportPrincipal};
    use application::operation_context::OperationContext;
    use application::pagination::Cursor;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use domain_primitives::sort::SortOrder;
    use localization::Language;
    use money::Currency;
    use product_listing_service::use_cases::{
        GetProductListingError, GetProductListingRequest, GetProductListingUseCase,
        GetSimilarProductListingsError, GetSimilarProductListingsRequest,
        GetSimilarProductListingsResult, GetSimilarProductListingsUseCase,
        SearchProductListingsError, SearchProductListingsResult, SearchProductListingsUseCase,
    };
    use serde_json::Value;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;

    type SearchProductListingsCalls =
        Arc<Mutex<Vec<(OperationContext, SearchProductListingsRequest)>>>;

    struct UnusedGetProductListingUseCase;

    #[async_trait::async_trait]
    impl GetProductListingUseCase for UnusedGetProductListingUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetProductListingRequest,
        ) -> Result<
            product_listing_service::use_cases::PersonalizedProductListingDetailsView,
            GetProductListingError,
        > {
            Err(GetProductListingError::NotFound)
        }
    }

    struct UnusedSimilarProductListingsUseCase;

    #[async_trait::async_trait]
    impl GetSimilarProductListingsUseCase for UnusedSimilarProductListingsUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetSimilarProductListingsRequest,
        ) -> Result<GetSimilarProductListingsResult, GetSimilarProductListingsError> {
            Err(GetSimilarProductListingsError::SimilaritySearchUnavailable)
        }
    }

    #[derive(Clone)]
    struct FakeSearchProductListingsUseCase {
        calls: SearchProductListingsCalls,
    }

    #[async_trait::async_trait]
    impl SearchProductListingsUseCase for FakeSearchProductListingsUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            request: SearchProductListingsRequest,
        ) -> Result<SearchProductListingsResult, SearchProductListingsError> {
            lock(&self.calls).push((context.clone(), request));
            Ok(SearchProductListingsResult::default())
        }
    }

    struct AnonymousAuthenticator;

    #[async_trait::async_trait]
    impl TokenAuthenticator for AnonymousAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Ok(TransportPrincipal::Anonymous)
        }
    }

    #[tokio::test]
    async fn should_map_get_search_request_and_add_public_cache_header()
    -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();
        let fx_rate_id = FxRateId::new();
        let cursor = serde_json::json!({
            "fxRateId": fx_rate_id,
            "searchAfter": ["next"]
        })
        .to_string();
        let encoded_cursor: String =
            url::form_urlencoded::byte_serialize(cursor.as_bytes()).collect();

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/product-listings?language=de&currency=USD&productQuery=cabinet&sort=updated&order=desc&size=200&searchAfter={encoded_cursor}"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "public, max-age=60, s-maxage=300",
            response.headers()[header::CACHE_CONTROL]
        );
        let calls = lock(&calls);
        assert_eq!(1, calls.len());
        let request = &calls[0].1;
        assert_eq!(Language::De, request.search.language);
        assert_eq!(Currency::Usd, request.search.currency);
        assert_eq!("cabinet", request.search.product_listing_query[0].as_ref());
        assert!(matches!(
            request.sort,
            Some(Sort {
                sort: SortProductListingField::Updated,
                order: SortOrder::Desc,
            })
        ));
        assert_eq!(
            Some(Cursor {
                size: 100,
                search_after: Some(ProductListingSearchCursor {
                    fx_rate_id,
                    search_after: Value::Array(vec![Value::String("next".to_owned())]),
                }),
            }),
            request.cursor
        );
        Ok(())
    }

    #[test]
    fn should_reject_noncanonical_query_object_ids() -> Result<(), Box<dyn std::error::Error>> {
        let listing_source_id = ListingSourceId::new();
        for invalid in [
            ProductListingId::new().to_string(),
            listing_source_id.as_uuid().to_string(),
            "ls_not-a-typeid".to_owned(),
        ] {
            let Err(error) = parse_query_object_ids::<ListingSourceId>(
                HashSet::from([invalid]),
                "listingSourceId",
                "ListingSource",
            ) else {
                return Err("noncanonical ListingSource ID was accepted".into());
            };
            assert_eq!(crate::error::INVALID_OBJECT_ID, error.code());
        }

        let fx_rate_id = FxRateId::new();
        for invalid in [
            ProductListingId::new().to_string(),
            fx_rate_id.as_uuid().to_string(),
            "fx_not-a-typeid".to_owned(),
        ] {
            let raw = serde_json::json!({
                "fxRateId": invalid,
                "searchAfter": ["next"]
            })
            .to_string();
            let Err(error) = parse_search_after(&raw) else {
                return Err("noncanonical FxRate ID was accepted".into());
            };
            assert_eq!(crate::error::INVALID_OBJECT_ID, error.code());
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_map_exact_orderability_and_unspecified_availability_query()
    -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();

        let response = app
            .oneshot(
                Request::get(
                    "/api/v1/product-listings?availability=IN_STOCK&orderability=ORDERABLE_NOW&includeUnspecifiedAvailability=true",
                )
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        let calls = lock(&calls);
        assert_eq!(
            Some(&ListingAvailabilityQuery {
                any_of: HashSet::from([ListingAvailability::InStock]).into(),
                orderability: HashSet::from([ListingOrderability::OrderableNow]).into(),
                include_unspecified: true,
            }),
            calls[0].1.search.availability_query.as_ref()
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_price_sort() -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();

        let response = app
            .oneshot(
                Request::get("/api/v1/product-listings?sort=price&order=asc").body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    fn app() -> (Router, SearchProductListingsCalls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let state = ProductListingsState::new(
            Arc::new(UnusedGetProductListingUseCase),
            Arc::new(UnusedSimilarProductListingsUseCase),
            Arc::new(FakeSearchProductListingsUseCase {
                calls: Arc::clone(&calls),
            }),
            Arc::new(AnonymousAuthenticator),
        );
        (
            Router::new()
                .route("/api/v1/product-listings", axum::routing::get(get_products))
                .with_state(state),
            calls,
        )
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}
