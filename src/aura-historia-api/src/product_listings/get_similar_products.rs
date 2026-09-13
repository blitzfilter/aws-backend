use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, PRODUCT_LISTING_INTERNAL_ERROR};
use crate::product_listings::product_data::personalized_product_summary_data;
use crate::state::ProductListingsState;
use crate::wire::parse_path_object_id;
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use localization::Language;
use money::Currency;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::ProductListingEmbeddingLookup;
use product_listing_service::use_cases::{
    GetSimilarProductListingsRequest, GetSimilarProductListingsResult,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SimilarProductListingsQuery {
    #[serde(default)]
    #[serde(with = "crate::wire::language")]
    language: Language,
    #[serde(default, with = "crate::wire::currency")]
    currency: Currency,
}

const READY_CACHE_CONTROL: &str = "public, max-age=180, s-maxage=900";
const PENDING_CACHE_CONTROL: &str = "public, max-age=300, s-maxage=900";

pub async fn get_similar_products_by_id(
    State(state): State<ProductListingsState>,
    headers: HeaderMap,
    Path(raw_product_listing_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let product_listing_id = match parse_path_object_id::<ProductListingId>(
        &raw_product_listing_id,
        "productListingId",
        "ProductListing",
    ) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let query = match parse_query(raw_query.as_deref()) {
        Ok(query) => query,
        Err(error) => return error.into_response(),
    };
    similar_response(
        state,
        headers,
        ProductListingEmbeddingLookup::ById(product_listing_id),
        query,
    )
    .await
}

fn parse_query(raw_query: Option<&str>) -> Result<SimilarProductListingsQuery, ApiError> {
    serde_qs::from_str(raw_query.unwrap_or_default()).map_err(|error| {
        ApiError::bad_request(crate::error::BAD_QUERY_PARAMETER_VALUE)
            .with_detail(error.to_string())
    })
}

async fn similar_response(
    state: ProductListingsState,
    headers: HeaderMap,
    lookup: ProductListingEmbeddingLookup,
    query: SimilarProductListingsQuery,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let context = principal.operation_context(metadata);
    match state
        .get_similar_products
        .execute(
            &context,
            GetSimilarProductListingsRequest {
                lookup: lookup.clone(),
                language: query.language,
                currency: query.currency,
            },
        )
        .await
    {
        Ok(GetSimilarProductListingsResult::Ready(products)) => {
            ready_response(products, &principal)
        }
        Ok(GetSimilarProductListingsResult::EmbeddingPending) => pending_response(lookup),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn ready_response(
    product_listings: Vec<product_listing_service::use_cases::PersonalizedProductListingSummary>,
    principal: &crate::auth::TransportPrincipal,
) -> Response {
    let mut response = Json(
        product_listings
            .into_iter()
            .map(personalized_product_summary_data)
            .collect::<Vec<_>>(),
    )
    .into_response();
    let cache_control = match principal {
        crate::auth::TransportPrincipal::Anonymous => READY_CACHE_CONTROL,
        crate::auth::TransportPrincipal::User { .. } => "no-store",
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    response
}

fn pending_response(lookup: ProductListingEmbeddingLookup) -> Response {
    let location_path = match lookup {
        ProductListingEmbeddingLookup::ById(product_listing_id) => {
            format!("/api/v1/product-listings/{product_listing_id}/similar")
        }
        ProductListingEmbeddingLookup::ByTitleSlug(_) => {
            return ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
                .with_detail(
                    "Similar product polling location is unavailable for a title-slug lookup.",
                )
                .into_response();
        }
    };
    let location = match HeaderValue::from_str(&location_path) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
                .with_detail("Similar product polling location failed internally.")
                .into_response();
        }
    };
    let mut response = StatusCode::ACCEPTED.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(PENDING_CACHE_CONTROL),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::operation_context::OperationContext;
    use application::personalized::Personalized;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use domain_primitives::event_id::EventId;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::{Language, Localized};
    use money::Currency;
    use money::{MonetaryAmount, Price};
    use product_listing_core::listing_availability::ListingAvailability;
    use product_listing_core::listing_lifecycle::ListingLifecycle;
    use product_listing_core::product_listing_slug_id::ProductListingSlugId;
    use product_listing_core::source_listing_id::SourceListingId;
    use product_listing_core::title::Title;
    use product_listing_service::ports::ListingSourceSummary;
    use product_listing_service::use_cases::{
        GetProductListingError, GetProductListingRequest, GetProductListingUseCase,
        GetSimilarProductListingsError, GetSimilarProductListingsRequest,
        GetSimilarProductListingsResult, GetSimilarProductListingsUseCase,
        PersonalizedProductListingDetailsView, PersonalizedProductListingSummary,
        ProductListingSummary, ProductListingSummaryPriceValuation, SearchProductListingsError,
        SearchProductListingsRequest, SearchProductListingsResult, SearchProductListingsUseCase,
    };
    use product_listing_service::user_state::ProductListingUserState;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use time::OffsetDateTime;
    use tower::ServiceExt;
    use url::Url;
    use user_core::user_id::UserId;

    #[derive(Clone)]
    enum FakeSimilarProductListingsResult {
        Ready(Vec<PersonalizedProductListingSummary>),
        Pending,
        NotFound,
        Unavailable,
    }

    struct FakeSimilarProductListingsUseCase {
        result: FakeSimilarProductListingsResult,
    }

    #[async_trait::async_trait]
    impl GetSimilarProductListingsUseCase for FakeSimilarProductListingsUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetSimilarProductListingsRequest,
        ) -> Result<GetSimilarProductListingsResult, GetSimilarProductListingsError> {
            match &self.result {
                FakeSimilarProductListingsResult::Ready(products) => {
                    Ok(GetSimilarProductListingsResult::Ready(products.clone()))
                }
                FakeSimilarProductListingsResult::Pending => {
                    Ok(GetSimilarProductListingsResult::EmbeddingPending)
                }
                FakeSimilarProductListingsResult::NotFound => {
                    Err(GetSimilarProductListingsError::NotFound)
                }
                FakeSimilarProductListingsResult::Unavailable => {
                    Err(GetSimilarProductListingsError::SimilaritySearchUnavailable)
                }
            }
        }
    }

    struct UnusedGetProductListingUseCase;

    #[async_trait::async_trait]
    impl GetProductListingUseCase for UnusedGetProductListingUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetProductListingRequest,
        ) -> Result<PersonalizedProductListingDetailsView, GetProductListingError> {
            Err(GetProductListingError::NotFound)
        }
    }

    struct UnusedSearchProductListingsUseCase;

    #[async_trait::async_trait]
    impl SearchProductListingsUseCase for UnusedSearchProductListingsUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: SearchProductListingsRequest,
        ) -> Result<SearchProductListingsResult, SearchProductListingsError> {
            Ok(SearchProductListingsResult::default())
        }
    }

    struct FakeAuthenticator {
        principal: TransportPrincipal,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Ok(self.principal.clone())
        }
    }

    #[test]
    fn should_parse_similar_products_currency_and_language() {
        let query = parse_query(Some("language=de&currency=USD"));
        assert!(
            matches!(query, Ok(query) if query.language == Language::De && query.currency == Currency::Usd)
        );

        let default_query = parse_query(None);
        assert!(
            matches!(default_query, Ok(query) if query.language == Language::En && query.currency == Currency::Eur)
        );
    }

    #[test]
    fn should_reject_invalid_similar_products_query() {
        assert!(parse_query(Some("language=invalid")).is_err());
        assert!(parse_query(Some("currency=invalid")).is_err());
    }

    #[tokio::test]
    async fn should_return_ready_similar_products_as_json_with_public_cache_header()
    -> Result<(), Box<dyn std::error::Error>> {
        let product = product_summary()?;
        let product_listing_id = product.item.product_listing_id;
        let app = app(
            FakeSimilarProductListingsResult::Ready(vec![product]),
            TransportPrincipal::Anonymous,
        );

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/product-listings/{product_listing_id}/similar"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            READY_CACHE_CONTROL,
            response.headers()[header::CACHE_CONTROL]
        );
        let body = body_json(response).await?;
        assert_eq!(
            product_listing_id.to_string(),
            body[0]["item"]["productListingId"]
        );
        assert_eq!("Cabinet", body[0]["item"]["title"]["text"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_cache_ready_similar_products_for_authenticated_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let product_listing_id = ProductListingId::new();
        let mut product = product_summary()?;
        product.user_state = Some(ProductListingUserState::default());
        let app = app(
            FakeSimilarProductListingsResult::Ready(vec![product]),
            TransportPrincipal::User {
                user_id: UserId::new(),
                auth_method: AuthMethod::CognitoJwt,
                capabilities: BTreeSet::new(),
            },
        );

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/product-listings/{product_listing_id}/similar"
                ))
                .header(header::AUTHORIZATION, "Bearer token")
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        assert_eq!(
            serde_json::json!([]),
            body_json(response).await?[0]["userState"]["notification"]["unseenNotificationIds"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_return_pending_response_with_id_location_and_cache_header()
    -> Result<(), Box<dyn std::error::Error>> {
        let product_listing_id = ProductListingId::new();
        let app = app(
            FakeSimilarProductListingsResult::Pending,
            TransportPrincipal::Anonymous,
        );

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/product-listings/{product_listing_id}/similar"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::ACCEPTED, response.status());
        assert_eq!(
            format!("/api/v1/product-listings/{product_listing_id}/similar"),
            response.headers()[header::LOCATION]
        );
        assert_eq!(
            PENDING_CACHE_CONTROL,
            response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_similar_product_not_found_error() -> Result<(), Box<dyn std::error::Error>>
    {
        let app = app(
            FakeSimilarProductListingsResult::NotFound,
            TransportPrincipal::Anonymous,
        );

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/product-listings/{}/similar",
                    ProductListingId::new()
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        assert_eq!(
            "PRODUCT_LISTING_NOT_FOUND",
            body_json(response).await?["error"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_similarity_service_unavailable_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let app = app(
            FakeSimilarProductListingsResult::Unavailable,
            TransportPrincipal::Anonymous,
        );

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/product-listings/{}/similar",
                    ProductListingId::new()
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
        assert_eq!(
            "PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE",
            body_json(response).await?["error"]
        );
        Ok(())
    }

    fn app(result: FakeSimilarProductListingsResult, principal: TransportPrincipal) -> Router {
        let state = ProductListingsState::new(
            Arc::new(UnusedGetProductListingUseCase),
            Arc::new(FakeSimilarProductListingsUseCase { result }),
            Arc::new(UnusedSearchProductListingsUseCase),
            Arc::new(FakeAuthenticator { principal }),
        );
        Router::new()
            .route(
                "/api/v1/product-listings/{product_listing_id}/similar",
                axum::routing::get(get_similar_products_by_id),
            )
            .with_state(state)
    }

    async fn body_json(
        response: Response,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn product_summary() -> Result<PersonalizedProductListingSummary, url::ParseError> {
        Ok(Personalized {
            item: ProductListingSummary {
                product_listing_id: ProductListingId::new(),
                product_listing_title_slug_id: Some(
                    ProductListingSlugId::raw("cabinet-abcdef").unwrap_or_else(|error| {
                        panic!("valid product listing title slug: {error}")
                    }),
                ),
                event_id: EventId::new(),
                source: ListingSourceSummary {
                    listing_source_id: ListingSourceId::new(),
                    name: ListingSourceName::try_from("Source").unwrap_or_else(|error| {
                        panic!("invalid test listing source name: {error}")
                    }),
                    slug_id: ListingSourceSlugId::raw("source")
                        .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
                },
                source_listing_id: SourceListingId::try_from("source-listing-id")
                    .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
                auction_id: None,
                has_auction_context: false,
                auction_summary: None,
                title: Some(Localized {
                    localization: Language::En,
                    payload: Title::from("Cabinet"),
                }),
                display_price: Some(
                    Price::new(MonetaryAmount::from(100_u64), Currency::Eur).into(),
                ),
                price_valuation: ProductListingSummaryPriceValuation::Current {
                    fx_rate_id: fxrate_core::FxRateId::new(),
                    captured_at: OffsetDateTime::UNIX_EPOCH,
                },
                availability: Some(ListingAvailability::Available),
                lifecycle: ListingLifecycle::Active,
                url: Url::parse("https://source.example/product-listings/1")?,
                view_url: Url::parse("https://aura.example/product-listings/cabinet-abcdef")?,
                images: Default::default(),
                content_policy: None,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: None,
        })
    }
}
