use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE, INVALID_UUID};
use crate::product_listings::product_data::product_response;
use crate::state::ProductListingsState;
use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use localization::Language;
use money::Currency;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::use_cases::{GetProductListingRequest, ProductListingLookup};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ProductListingDetailsQuery {
    #[serde(default)]
    #[serde(with = "crate::wire::language")]
    language: Language,
    #[serde(default, with = "crate::wire::currency")]
    currency: Currency,
}

pub async fn get_product_by_id(
    State(state): State<ProductListingsState>,
    headers: HeaderMap,
    Path(raw_product_listing_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let query: ProductListingDetailsQuery =
        match serde_qs::from_str(raw_query.as_deref().unwrap_or_default()) {
            Ok(query) => query,
            Err(error) => {
                return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response();
            }
        };
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let product_listing_id = match ProductListingId::try_from(raw_product_listing_id.as_str()) {
        Ok(product_listing_id) => product_listing_id,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("productListingId")
                .with_detail("Path parameter 'productListingId' must be a UUID.")
                .into_response();
        }
    };

    let context = principal.operation_context(metadata);
    match state
        .get_product
        .execute(
            &context,
            GetProductListingRequest {
                lookup: ProductListingLookup::ById(product_listing_id),
                language: query.language,
                currency: query.currency,
            },
        )
        .await
    {
        Ok(view) => product_response(view, &context.principal),
        Err(error) => ApiError::from(error).into_response(),
    }
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
    use fxrate_core::FxRateId;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::{Language, Localized};
    use money::Currency;
    use money::{MonetaryAmount, Price};
    use notification_core::notification_id::NotificationId;
    use product_listing_core::listing_availability::ListingAvailability;
    use product_listing_core::listing_lifecycle::ListingLifecycle;
    use product_listing_core::product_listing::{ProductListingAuction, ProductListingPricing};
    use product_listing_core::product_listing_slug_id::ProductListingSlugId;
    use product_listing_core::source_listing_id::SourceListingId;
    use product_listing_core::title::Title;
    use product_listing_service::ports::ListingSourceSummary;
    use product_listing_service::use_cases::{
        DisplayProductListingPricing, GetProductListingError, GetProductListingUseCase,
        GetSimilarProductListingsError, GetSimilarProductListingsRequest,
        GetSimilarProductListingsResult, GetSimilarProductListingsUseCase,
        PersonalizedProductListingDetailsView, ProductListingDetailsView,
        ProductListingPricingPresentation, ProductListingPricingValuation,
        SearchProductListingsError, SearchProductListingsRequest, SearchProductListingsResult,
        SearchProductListingsUseCase,
    };
    use product_listing_service::user_state::{
        ContentVisibilityUserState, NotificationUserState, ProductListingUserState,
        SearchFilterUserState, WatchlistUserState,
    };
    use search_filter_core::enhanced_match_reason::EnhancedMatchReason;
    use search_filter_core::user_search_filter_id::UserSearchFilterId;
    use search_filter_core::user_search_filter_name::UserSearchFilterName;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::OffsetDateTime;
    use tower::ServiceExt;
    use url::Url;
    use user_core::user_id::UserId;

    type GetProductListingCalls = Arc<Mutex<Vec<(OperationContext, GetProductListingRequest)>>>;

    #[derive(Clone)]
    struct FakeGetProductListingUseCase {
        result: PersonalizedProductListingDetailsView,
        calls: GetProductListingCalls,
    }

    #[async_trait::async_trait]
    impl GetProductListingUseCase for FakeGetProductListingUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            request: GetProductListingRequest,
        ) -> Result<PersonalizedProductListingDetailsView, GetProductListingError> {
            lock(&self.calls).push((context.clone(), request));
            Ok(self.result.clone())
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
        reject: bool,
        user_id: Option<UserId>,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            if self.reject {
                Err(AuthError::InvalidCredentials)
            } else {
                Ok(match self.user_id {
                    Some(user_id) => TransportPrincipal::User {
                        user_id,
                        auth_method: AuthMethod::CognitoJwt,
                        capabilities: Default::default(),
                    },
                    None => TransportPrincipal::Anonymous,
                })
            }
        }
    }

    #[tokio::test]
    async fn should_return_product_details_headers_and_omit_audit_actors()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut view = product_details_view()?;
        view.item.availability = None;
        let product_listing_id = view.item.product_listing_id;
        let product_listing_title_slug_id = view.item.product_listing_title_slug_id.clone();
        let (app, calls) = app(view, false, None);

        let response = app
            .oneshot(
                Request::get(format!("/api/v1/product-listings/{product_listing_id}"))
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "public, max-age=180, s-maxage=900",
            response.headers()[header::CACHE_CONTROL]
        );
        assert_eq!("en", response.headers()[header::CONTENT_LANGUAGE]);
        assert!(response.headers().get(header::ETAG).is_none());
        assert!(response.headers().get(header::LAST_MODIFIED).is_none());
        let body = body_json(response).await?;
        assert_eq!(
            json!(product_listing_id.to_string()),
            body["item"]["productListingId"]
        );
        assert_eq!(
            json!(product_listing_title_slug_id.map(|slug| slug.to_string())),
            body["item"]["productListingTitleSlugId"]
        );
        assert!(body["item"].get("sourceListingSlugId").is_none());
        assert!(body["item"].get("createdBy").is_none());
        assert!(body["item"].get("updatedBy").is_none());
        assert_eq!(
            "EUR",
            body["item"]["pricing"]["source"]["price"]["currency"]
        );
        assert_eq!(
            "EUR",
            body["item"]["pricing"]["display"]["price"]["currency"]
        );
        assert_eq!("CURRENT", body["item"]["pricing"]["valuation"]["type"]);
        assert!(body["item"].get("price").is_none());
        assert!(body["item"].get("priceEstimateMin").is_none());
        assert!(body["item"].get("priceEstimateMax").is_none());
        assert!(body["item"].get("currency").is_none());
        assert!(body["item"].get("availability").is_some());
        assert!(body["item"]["availability"].is_null());
        assert!(body.get("userState").is_none());
        assert!(matches!(
            lock(&calls)[0].1,
            GetProductListingRequest {
                lookup: ProductListingLookup::ById(actual),
                language: Language::En,
                currency: Currency::Eur,
            } if actual == product_listing_id
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_pass_requested_language_to_use_case() -> Result<(), Box<dyn std::error::Error>>
    {
        let view = product_details_view()?;
        let product_listing_id = view.item.product_listing_id;
        let (app, calls) = app(view, false, None);

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/product-listings/{product_listing_id}?language=de&currency=USD"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert!(matches!(
            lock(&calls)[0].1,
            GetProductListingRequest {
                lookup: ProductListingLookup::ById(actual),
                language: Language::De,
                currency: Currency::Usd,
            } if actual == product_listing_id
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_map_title_slug_path_to_title_slug_lookup()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = product_details_view()?;
        let product_listing_title_slug_id = ProductListingSlugId::raw("cabinet-a1b2c3")
            .unwrap_or_else(|error| panic!("valid product listing title slug: {error}"));
        let (app, calls) = app(view, false, None);

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/product-listings/by-slug/{product_listing_title_slug_id}?language=de&currency=USD"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert!(matches!(
            &lock(&calls)[0].1,
            GetProductListingRequest {
                lookup: ProductListingLookup::ByTitleSlug(actual),
                language: Language::De,
                currency: Currency::Usd,
            } if actual == &product_listing_title_slug_id
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_language_before_calling_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let product_listing_id = ProductListingId::new();
        let (app, calls) = app(product_details_view()?, false, None);

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/product-listings/{product_listing_id}?language=invalid"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(
            "BAD_QUERY_PARAMETER_VALUE",
            body_json(response).await?["error"]
        );
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_product_listing_id_before_calling_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app(product_details_view()?, false, None);

        let response = app
            .oneshot(Request::get("/api/v1/product-listings/not-a-uuid").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_serialize_user_state_and_disable_cache_for_authenticated_user()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let notification_id = NotificationId::new();
        let search_filter_id = UserSearchFilterId::new();
        let mut view = product_details_view()?;
        let product_listing_id = view.item.product_listing_id;
        view.user_state = Some(ProductListingUserState {
            watchlist: WatchlistUserState {
                watching: true,
                notifications: false,
            },
            content_visibility: ContentVisibilityUserState {
                show_unassessed_or_sensitive_content: false,
            },
            notification: NotificationUserState {
                unseen_notification_ids: vec![notification_id],
            },
            search_filter: SearchFilterUserState {
                matched: true,
                hidden: false,
                user_search_filter_id: Some(search_filter_id),
                user_search_filter_name: Some(UserSearchFilterName::from("Vintage furniture")),
                match_reason: Some(EnhancedMatchReason::from("Matched the material.")),
                match_feedback: Some(false),
            },
        });
        let (app, calls) = app(view, false, Some(user_id));

        let response = app
            .oneshot(
                Request::get(format!("/api/v1/product-listings/{product_listing_id}"))
                    .header(header::AUTHORIZATION, "Bearer valid")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        let body = body_json(response).await?;
        assert_eq!(true, body["userState"]["watchlist"]["watching"]);
        assert_eq!(false, body["userState"]["watchlist"]["notifications"]);
        assert_eq!(
            false,
            body["userState"]["contentVisibility"]["showUnassessedOrSensitiveContent"]
        );
        assert_eq!(
            json!([notification_id.to_string()]),
            body["userState"]["notification"]["unseenNotificationIds"]
        );
        assert_eq!(true, body["userState"]["searchFilter"]["matched"]);
        assert_eq!(
            json!(search_filter_id.to_string()),
            body["userState"]["searchFilter"]["userSearchFilterId"]
        );
        assert_eq!(
            "Vintage furniture",
            body["userState"]["searchFilter"]["userSearchFilterName"]
        );
        assert_eq!(
            "Matched the material.",
            body["userState"]["searchFilter"]["matchReason"]
        );
        assert_eq!(false, body["userState"]["searchFilter"]["matchFeedback"]);
        assert!(matches!(
            lock(&calls)[0].0.principal,
            application::operation_context::Principal::User(actual) if actual == user_id
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_optional_token_before_calling_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = product_details_view()?;
        let product_listing_id = view.item.product_listing_id;
        let (app, calls) = app(view, true, None);

        let response = app
            .oneshot(
                Request::get(format!("/api/v1/product-listings/{product_listing_id}"))
                    .header(header::AUTHORIZATION, "Bearer invalid")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    fn app(
        view: PersonalizedProductListingDetailsView,
        reject_token: bool,
        user_id: Option<UserId>,
    ) -> (Router, GetProductListingCalls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let state = ProductListingsState::new(
            Arc::new(FakeGetProductListingUseCase {
                result: view,
                calls: Arc::clone(&calls),
            }),
            Arc::new(UnusedSimilarProductListingsUseCase),
            Arc::new(UnusedSearchProductListingsUseCase),
            Arc::new(FakeAuthenticator {
                reject: reject_token,
                user_id,
            }),
        );
        (
            Router::new()
                .route(
                    "/api/v1/product-listings/{product_listing_id}",
                    axum::routing::get(get_product_by_id),
                )
                .route(
                    "/api/v1/product-listings/by-slug/{product_listing_title_slug_id}",
                    axum::routing::get(
                        crate::product_listings::get_product_by_title_slug::get_product_by_title_slug,
                    ),
                )
                .with_state(state),
            calls,
        )
    }

    async fn body_json(response: Response) -> Result<Value, Box<dyn std::error::Error>> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn product_details_view() -> Result<PersonalizedProductListingDetailsView, url::ParseError> {
        Ok(Personalized {
            item: ProductListingDetailsView {
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
                product_title: None,
                product_description: None,
                title: Some(Localized {
                    localization: Language::En,
                    payload: Title::from("Cabinet"),
                }),
                description: None,
                pricing: ProductListingPricingPresentation {
                    source: ProductListingPricing {
                        price: Some(
                            Price::new(MonetaryAmount::from(100_u64), Currency::Eur).into(),
                        ),
                        ..Default::default()
                    },
                    display: DisplayProductListingPricing {
                        price: Some(
                            Price::new(MonetaryAmount::from(100_u64), Currency::Eur).into(),
                        ),
                        price_estimate_min: None,
                        price_estimate_max: None,
                    },
                    valuation: ProductListingPricingValuation::Current {
                        fx_rate_id: FxRateId::new(),
                        captured_at: OffsetDateTime::UNIX_EPOCH,
                    },
                },
                availability: Some(ListingAvailability::Available),
                lifecycle: ListingLifecycle::Active,
                url: Url::parse("https://source.example/product-listings/1")?,
                view_url: Url::parse("https://aura.example/product-listings/cabinet-abcdef")?,
                images: Default::default(),
                content_policy: None,
                auction: ProductListingAuction::default(),
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: None,
        })
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}
