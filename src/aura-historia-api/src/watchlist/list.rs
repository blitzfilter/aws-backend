use super::util::no_store;
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE, WATCHLIST_INTERNAL_ERROR};
use crate::pagination_data::JsonCursoredData;
use crate::product_listings::product_data::{
    PersonalizedProductListingDetailsData, personalized_product_details_data,
};
use crate::state::WatchlistState;
use crate::wire::parse_query_object_id;
use application::pagination::{Cursor, CursoredResult};
use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use localization::Language;
use money::Currency;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::ProductListingWatchlistDetailsCursor;
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use watchlist_service::use_cases::ListWatchlistRequest;

#[derive(Debug, Deserialize)]
struct ListWatchlistQuery {
    #[serde(default)]
    #[serde(with = "crate::wire::language")]
    language: Language,
    #[serde(default, with = "crate::wire::currency")]
    currency: Currency,
    size: Option<u64>,
    #[serde(rename = "searchAfter")]
    search_after: Option<String>,
}

pub async fn list_watchlist(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let query: ListWatchlistQuery =
        match serde_qs::from_str(raw_query.as_deref().unwrap_or_default()) {
            Ok(query) => query,
            Err(error) => {
                return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response();
            }
        };
    let cursor = match watchlist_cursor(query.size, query.search_after.as_deref()) {
        Ok(cursor) => cursor,
        Err(error) => return error.into_response(),
    };
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match state
        .list_watchlist
        .execute(
            &ctx,
            ListWatchlistRequest {
                user_id,
                language: query.language,
                currency: query.currency,
                cursor,
            },
        )
        .await
    {
        Ok(result) => {
            let search_after = match result
                .cursor
                .search_after
                .map(watchlist_cursor_value)
                .transpose()
            {
                Ok(search_after) => search_after,
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

fn watchlist_cursor(
    size: Option<u64>,
    search_after: Option<&str>,
) -> Result<Cursor<ProductListingWatchlistDetailsCursor>, ApiError> {
    let size = size.unwrap_or(21).clamp(1, 100);
    let search_after = search_after
        .map(|value| {
            serde_json::from_str::<Value>(value)
                .map_err(|error| {
                    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                        .with_query_field("searchAfter")
                        .with_detail(error.to_string())
                })
                .and_then(parse_watchlist_cursor)
        })
        .transpose()?;
    Ok(Cursor { size, search_after })
}

fn parse_watchlist_cursor(value: Value) -> Result<ProductListingWatchlistDetailsCursor, ApiError> {
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
    let watchlist_created = OffsetDateTime::parse(created, &Rfc3339).map_err(|error| {
        ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail(error.to_string())
    })?;
    let product_listing_id = parse_query_object_id::<ProductListingId>(
        product_listing_id,
        "searchAfter",
        "ProductListing",
    )?;
    Ok(ProductListingWatchlistDetailsCursor {
        watchlist_created,
        product_listing_id,
    })
}

fn watchlist_cursor_value(cursor: ProductListingWatchlistDetailsCursor) -> Result<Value, ApiError> {
    cursor
        .watchlist_created
        .format(&Rfc3339)
        .map(|created| json!([created, cursor.product_listing_id]))
        .map_err(|_| {
            ApiError::internal_server_error(WATCHLIST_INTERNAL_ERROR)
                .with_detail("Watchlist cursor failed internally.")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::error::static_error;
    use application::operation_context::OperationContext;
    use application::personalized::Personalized;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use domain_primitives::event_id::EventId;
    use fxrate_core::FxRateId;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::Language;
    use money::Currency;
    use product_listing_core::listing_availability::ListingAvailability;
    use product_listing_core::listing_lifecycle::ListingLifecycle;
    use product_listing_core::product_listing::{ProductListingAuction, ProductListingPricing};
    use product_listing_core::product_listing_id::ProductListingId;
    use product_listing_core::product_listing_slug_id::ProductListingSlugId;
    use product_listing_core::source_listing_id::SourceListingId;
    use product_listing_service::ports::ListingSourceSummary;
    use product_listing_service::user_state::ProductListingUserState;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::OffsetDateTime;
    use tower::ServiceExt;
    use url::Url;
    use user_core::user_id::UserId;
    use watchlist_service::use_cases::{
        ListWatchlistError, ListWatchlistResult, ListWatchlistUseCase,
        UnwatchProductListingCommand, UnwatchProductListingError, UnwatchProductListingUseCase,
        UpdateWatchlistProductListingCommand, UpdateWatchlistProductListingError,
        UpdateWatchlistProductListingUseCase, WatchProductListingCommand, WatchProductListingError,
        WatchProductListingUseCase,
    };
    use watchlist_service::use_cases::{
        UnwatchProductListingResult, UpdateWatchlistProductListingResult, WatchProductListingResult,
    };

    type ListRequests = Arc<Mutex<Vec<(OperationContext, ListWatchlistRequest)>>>;

    #[derive(Clone)]
    struct FakeListWatchlistUseCase {
        result: ListWatchlistResult,
        requests: ListRequests,
    }

    #[async_trait::async_trait]
    impl ListWatchlistUseCase for FakeListWatchlistUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            request: ListWatchlistRequest,
        ) -> Result<ListWatchlistResult, ListWatchlistError> {
            lock(&self.requests).push((context.clone(), request));
            Ok(self.result.clone())
        }
    }

    struct UnusedWatchProductListingUseCase;
    #[async_trait::async_trait]
    impl WatchProductListingUseCase for UnusedWatchProductListingUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: WatchProductListingCommand,
        ) -> Result<WatchProductListingResult, WatchProductListingError> {
            Err(WatchProductListingError::TemporarilyUnavailable {
                source: static_error("unused watch product use case"),
            })
        }
    }

    struct UnusedUpdateWatchlistProductListingUseCase;
    #[async_trait::async_trait]
    impl UpdateWatchlistProductListingUseCase for UnusedUpdateWatchlistProductListingUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: UpdateWatchlistProductListingCommand,
        ) -> Result<UpdateWatchlistProductListingResult, UpdateWatchlistProductListingError>
        {
            Err(UpdateWatchlistProductListingError::TemporarilyUnavailable {
                source: static_error("unused update watchlist product use case"),
            })
        }
    }

    struct UnusedUnwatchProductListingUseCase;
    #[async_trait::async_trait]
    impl UnwatchProductListingUseCase for UnusedUnwatchProductListingUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: UnwatchProductListingCommand,
        ) -> Result<UnwatchProductListingResult, UnwatchProductListingError> {
            Err(UnwatchProductListingError::TemporarilyUnavailable {
                source: static_error("unused unwatch product use case"),
            })
        }
    }

    struct FakeAuthenticator {
        user_id: UserId,
        reject: bool,
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
                Ok(TransportPrincipal::User {
                    user_id: self.user_id,
                    auth_method: AuthMethod::CognitoJwt,
                    capabilities: Default::default(),
                })
            }
        }
    }

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        match value.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn product(
        product_listing_id: ProductListingId,
    ) -> Result<
        product_listing_service::use_cases::PersonalizedProductListingDetailsView,
        url::ParseError,
    > {
        let url = Url::parse("https://example.test/product")?;
        Ok(Personalized {
            item: product_listing_service::use_cases::ProductListingDetailsView {
                product_listing_id,
                product_listing_title_slug_id: Some(ProductListingSlugId::raw("product-a1b2c3")
                                    .unwrap_or_else(|error| panic!("valid product listing title slug: {error}"))),
                event_id: EventId::new(),
                source: ListingSourceSummary {
                    listing_source_id: ListingSourceId::new(),
                    name: ListingSourceName::try_from("Source")
                                            .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
                    slug_id: ListingSourceSlugId::raw("source").unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
                },
                source_listing_id: SourceListingId::try_from("product")
                                    .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
                product_title: None,
                product_description: None,
                title: None,
                description: None,
                pricing: product_listing_service::use_cases::ProductListingPricingPresentation {
                    source: ProductListingPricing::default(),
                    display: product_listing_service::use_cases::DisplayProductListingPricing {
                        price: None,
                        price_estimate_min: None,
                        price_estimate_max: None,
                    },
                    valuation:
                        product_listing_service::use_cases::ProductListingPricingValuation::Current {
                            fx_rate_id: FxRateId::new(),
                            captured_at: OffsetDateTime::UNIX_EPOCH,
                        },
                },
                availability: Some(ListingAvailability::Available),
                lifecycle: ListingLifecycle::Active,
                url: url.clone(),
                view_url: url,
                images: Default::default(),
                content_policy: None,
                auction: ProductListingAuction::default(),
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: Some(ProductListingUserState::default()),
        })
    }

    fn app(
        user_id: UserId,
        products: Vec<product_listing_service::use_cases::PersonalizedProductListingDetailsView>,
        reject_auth: bool,
    ) -> (Router, ListRequests) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = WatchlistState {
            list_watchlist: Arc::new(FakeListWatchlistUseCase {
                result: ListWatchlistResult {
                    items: products,
                    cursor: Cursor::default(),
                    total: None,
                },
                requests: Arc::clone(&requests),
            }),
            watch_product: Arc::new(UnusedWatchProductListingUseCase),
            update_watchlist_product: Arc::new(UnusedUpdateWatchlistProductListingUseCase),
            unwatch_product: Arc::new(UnusedUnwatchProductListingUseCase),
            authenticator: Arc::new(FakeAuthenticator {
                user_id,
                reject: reject_auth,
            }),
        };
        (
            Router::new()
                .route("/api/v1/me/watchlist", axum::routing::get(list_watchlist))
                .with_state(state),
            requests,
        )
    }

    async fn json(response: Response) -> Result<serde_json::Value, axum::Error> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&bytes).unwrap_or_default())
    }

    #[tokio::test]
    async fn should_return_personalized_products_without_cache_and_forward_language()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let (app, requests) = app(user_id, vec![product(product_listing_id)?], false);

        let response = app
            .oneshot(
                Request::get("/api/v1/me/watchlist?language=de")
                    .header(header::AUTHORIZATION, "Bearer valid")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        let body = json(response).await?;
        assert_eq!(21, body["size"]);
        assert!(body["searchAfter"].is_null());
        assert_eq!(
            product_listing_id.to_string(),
            body["items"][0]["item"]["productListingId"]
        );
        assert_eq!(
            serde_json::json!([]),
            body["items"][0]["userState"]["notification"]["unseenNotificationIds"]
        );
        assert!(matches!(
            lock(&requests)[0].1,
            ListWatchlistRequest {
                user_id: actual_user_id,
                language: Language::De,
                currency: Currency::Eur,
                cursor: Cursor { size: 21, search_after: None },
            } if actual_user_id == user_id
        ));
        Ok(())
    }

    #[test]
    fn should_parse_tie_safe_watchlist_cursor_and_clamp_size()
    -> Result<(), Box<dyn std::error::Error>> {
        let product_listing_id = ProductListingId::new();
        let raw_cursor =
            serde_json::to_string(&json!(["1970-01-01T00:00:00Z", product_listing_id]))?;
        let cursor = watchlist_cursor(Some(500), Some(&raw_cursor))?;

        assert_eq!(100, cursor.size);
        let search_after = cursor.search_after.ok_or("cursor was missing")?;
        assert_eq!(product_listing_id, search_after.product_listing_id);
        assert_eq!(
            search_after,
            parse_watchlist_cursor(watchlist_cursor_value(search_after)?)?
        );
        assert!(watchlist_cursor(Some(1), Some("invalid")).is_err());
        for invalid_id in [
            ListingSourceId::new().to_string(),
            product_listing_id.as_uuid().to_string(),
            "pl_not-a-typeid".to_owned(),
        ] {
            let Err(error) = parse_watchlist_cursor(json!(["1970-01-01T00:00:00Z", invalid_id]))
            else {
                return Err("noncanonical ProductListing cursor ID was accepted".into());
            };
            assert_eq!(crate::error::INVALID_OBJECT_ID, error.code());
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_language_before_authentication()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let (app, requests) = app(user_id, Vec::new(), false);

        let response = app
            .oneshot(Request::get("/api/v1/me/watchlist?language=invalid").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!("BAD_QUERY_PARAMETER_VALUE", json(response).await?["error"]);
        assert!(lock(&requests).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_bearer_token() -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let (app, requests) = app(user_id, Vec::new(), true);

        let response = app
            .oneshot(
                Request::get("/api/v1/me/watchlist")
                    .header(header::AUTHORIZATION, "Bearer invalid")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert!(lock(&requests).is_empty());
        Ok(())
    }
}
