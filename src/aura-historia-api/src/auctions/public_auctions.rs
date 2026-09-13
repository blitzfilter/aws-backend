use crate::{
    auth::{OptionalAuthExtractor, request_metadata},
    error::{
        AUCTION_INTERNAL_ERROR, AUCTION_NOT_FOUND, AUCTION_TEMPORARILY_UNAVAILABLE, ApiError,
        BAD_QUERY_PARAMETER_VALUE,
    },
    state::PublicAuctionsState,
    wire::{parse_path_object_id, parse_query_object_id},
};
use application::pagination::Cursor;
use auction_core::{AuctionFormat, AuctionId, AuctionReportedStatus, AuctionSchedulePoint};
use auction_service::{
    ports::{
        AuctionDirectoryCursor, AuctionDirectoryScope, AuctionInstantScheduleFilter,
        ListAuctionsDirectoryRequest, PublicAuctionDetails,
    },
    use_cases::{GetPublicAuctionError, ListAuctionsError, ListAuctionsRequest},
};
use axum::{
    Json,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use listing_source_core::ListingSourceId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

pub async fn get_public_auction(
    State(state): State<PublicAuctionsState>,
    headers: HeaderMap,
    Path(raw_auction_id): Path<String>,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return no_store(ApiError::from(error).into_response()),
    };
    let auction_id =
        match parse_path_object_id::<AuctionId>(&raw_auction_id, "auctionId", "Auction") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
        };
    match state
        .get
        .execute(&principal.operation_context(metadata), auction_id)
        .await
    {
        Ok(view) => match PublicAuctionData::try_from(view) {
            Ok(data) => no_store(Json(data).into_response()),
            Err(error) => no_store(error.into_response()),
        },
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

pub async fn get_auction_catalogue(
    State(state): State<PublicAuctionsState>,
    headers: HeaderMap,
    Path(raw_auction_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let auction_id =
        match parse_path_object_id::<AuctionId>(&raw_auction_id, "auctionId", "Auction") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
        };
    let query = match serde_qs::from_str::<CatalogueQuery>(raw_query.as_deref().unwrap_or_default())
    {
        Ok(value) => value,
        Err(error) => {
            return no_store(
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response(),
            );
        }
    };
    let cursor = match query
        .search_after
        .map(CatalogueCursorData::try_into_cursor)
        .transpose()
    {
        Ok(value) => application::pagination::Cursor {
            size: query.page_size.unwrap_or(21),
            search_after: value,
        },
        Err(error) => return no_store(error.into_response()),
    };
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(value) => value,
        Err(error) => return no_store(ApiError::from(error).into_response()),
    };
    match state
        .catalogue
        .execute(
            &principal.operation_context(metadata),
            product_listing_service::use_cases::GetAuctionCatalogueRequest {
                auction_id,
                language: query.language,
                currency: query.currency,
                cursor: Some(cursor),
            },
        )
        .await
    {
        Ok(page) => no_store(Json(CatalogueData::from(page)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

pub async fn list_public_auctions(
    State(state): State<PublicAuctionsState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let data = match serde_qs::from_str::<DirectoryQuery>(raw_query.as_deref().unwrap_or_default())
    {
        Ok(value) => value,
        Err(error) => {
            return no_store(
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response(),
            );
        }
    };
    let request = match data.try_into_request() {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return no_store(ApiError::from(error).into_response()),
    };
    match state
        .list
        .execute(&principal.operation_context(metadata), request)
        .await
    {
        Ok(result) => no_store(Json(PublicAuctionDirectoryData::from(result)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogueQuery {
    #[serde(default, with = "crate::wire::language")]
    language: localization::Language,
    #[serde(default, with = "crate::wire::currency")]
    currency: money::Currency,
    page_size: Option<u64>,
    search_after: Option<CatalogueCursorData>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogueCursorData {
    auction_id: String,
    catalogue_position: Option<u32>,
    product_listing_id: String,
}
impl CatalogueCursorData {
    fn try_into_cursor(
        self,
    ) -> Result<product_listing_service::ports::AuctionCatalogueCursor, ApiError> {
        Ok(product_listing_service::ports::AuctionCatalogueCursor {
            auction_id: parse_query_object_id(
                &self.auction_id,
                "searchAfter.auctionId",
                "Auction",
            )?,
            catalogue_position: self.catalogue_position,
            product_listing_id: parse_query_object_id(
                &self.product_listing_id,
                "searchAfter.productListingId",
                "ProductListing",
            )?,
        })
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogueData {
    items: Vec<crate::product_listings::product_data::PersonalizedProductListingDetailsData>,
    page_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<CatalogueCursorData>,
}
impl
    From<
        application::pagination::CursoredResult<
            product_listing_service::use_cases::PersonalizedProductListingDetailsView,
            product_listing_service::ports::AuctionCatalogueCursor,
        >,
    > for CatalogueData
{
    fn from(
        value: application::pagination::CursoredResult<
            product_listing_service::use_cases::PersonalizedProductListingDetailsView,
            product_listing_service::ports::AuctionCatalogueCursor,
        >,
    ) -> Self {
        Self {
            page_size: value.cursor.size,
            search_after: value.cursor.search_after.map(|cursor| CatalogueCursorData {
                auction_id: cursor.auction_id.to_string(),
                catalogue_position: cursor.catalogue_position,
                product_listing_id: cursor.product_listing_id.to_string(),
            }),
            items: value
                .items
                .into_iter()
                .map(crate::product_listings::product_data::personalized_product_details_data)
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DirectoryQuery {
    listing_source_id: Option<String>,
    format: Option<String>,
    reported_status: Option<String>,
    time_role: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    from: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    to: Option<OffsetDateTime>,
    page_size: Option<u64>,
    search_after: Option<DirectoryCursorData>,
}

impl DirectoryQuery {
    fn try_into_request(self) -> Result<ListAuctionsRequest, ApiError> {
        let listing_source_id = self
            .listing_source_id
            .map(|value| parse_query_object_id(&value, "listingSourceId", "ListingSource"))
            .transpose()?;
        let format = parse_auction_format(self.format)?;
        let reported_status = parse_auction_status(self.reported_status)?;
        let schedule = parse_schedule_filter(self.time_role, self.from, self.to)?;
        let cursor = self
            .search_after
            .map(DirectoryCursorData::try_into_cursor)
            .transpose()?;
        Ok(ListAuctionsDirectoryRequest {
            listing_source_id,
            format,
            reported_status,
            schedule,
            cursor: Some(Cursor {
                size: self.page_size.unwrap_or(21),
                search_after: cursor,
            }),
        })
    }
}

fn schedule_point(value: &str) -> Result<AuctionSchedulePoint, ApiError> {
    match value {
        "BIDDING_OPENS" => Ok(AuctionSchedulePoint::BiddingOpens),
        "LIVE_STARTS" => Ok(AuctionSchedulePoint::LiveStarts),
        "LOTS_BEGIN_CLOSING" => Ok(AuctionSchedulePoint::LotsBeginClosing),
        "SCHEDULED_END" => Ok(AuctionSchedulePoint::ScheduledEnd),
        _ => Err(query_error("timeRole is invalid.")),
    }
}

fn query_error(detail: &'static str) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_detail(detail)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DirectoryCursorData {
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    auction_id: String,
    scope: DirectoryCursorScopeData,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DirectoryCursorScopeData {
    listing_source_id: Option<String>,
    format: Option<String>,
    reported_status: Option<String>,
    time_role: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    from: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    to: Option<OffsetDateTime>,
}

impl DirectoryCursorData {
    fn try_into_cursor(self) -> Result<AuctionDirectoryCursor, ApiError> {
        Ok(AuctionDirectoryCursor {
            created: self.created,
            auction_id: parse_query_object_id(
                &self.auction_id,
                "searchAfter.auctionId",
                "Auction",
            )?,
            scope: self.scope.try_into_scope()?,
        })
    }
}

impl DirectoryCursorScopeData {
    fn try_into_scope(self) -> Result<AuctionDirectoryScope, ApiError> {
        Ok(AuctionDirectoryScope {
            listing_source_id: self
                .listing_source_id
                .map(|value| {
                    parse_query_object_id(
                        &value,
                        "searchAfter.scope.listingSourceId",
                        "ListingSource",
                    )
                })
                .transpose()?,
            format: parse_auction_format(self.format)?,
            reported_status: parse_auction_status(self.reported_status)?,
            schedule: parse_schedule_filter(self.time_role, self.from, self.to)?,
        })
    }
}

impl From<AuctionDirectoryScope> for DirectoryCursorScopeData {
    fn from(value: AuctionDirectoryScope) -> Self {
        let (time_role, from, to) = match value.schedule {
            Some(schedule) => (
                Some(schedule_role_code(schedule.role).to_owned()),
                schedule.range.min,
                schedule.range.max,
            ),
            None => (None, None, None),
        };
        Self {
            listing_source_id: value.listing_source_id.map(|value| value.to_string()),
            format: value.format.map(|value| value.as_str().to_owned()),
            reported_status: value.reported_status.map(|value| value.as_str().to_owned()),
            time_role,
            from,
            to,
        }
    }
}

fn parse_auction_format(value: Option<String>) -> Result<Option<AuctionFormat>, ApiError> {
    value
        .map(|value| {
            value
                .parse()
                .map_err(|_| query_error("format must be LIVE or TIMED."))
        })
        .transpose()
}

fn parse_auction_status(value: Option<String>) -> Result<Option<AuctionReportedStatus>, ApiError> {
    value
        .map(|value| {
            value
                .parse()
                .map_err(|_| query_error("reportedStatus is invalid."))
        })
        .transpose()
}

fn parse_schedule_filter(
    time_role: Option<String>,
    from: Option<OffsetDateTime>,
    to: Option<OffsetDateTime>,
) -> Result<Option<AuctionInstantScheduleFilter>, ApiError> {
    match (time_role, from, to) {
        (None, None, None) => Ok(None),
        (Some(role), Some(from), Some(to)) => Ok(Some(AuctionInstantScheduleFilter {
            role: schedule_point(&role)?,
            range: domain_primitives::query::range_query::RangeQuery {
                min: Some(from),
                max: Some(to),
            },
        })),
        _ => Err(query_error(
            "timeRole, from, and to must be supplied together.",
        )),
    }
}

fn schedule_role_code(value: AuctionSchedulePoint) -> &'static str {
    match value {
        AuctionSchedulePoint::BiddingOpens => "BIDDING_OPENS",
        AuctionSchedulePoint::LiveStarts => "LIVE_STARTS",
        AuctionSchedulePoint::LotsBeginClosing => "LOTS_BEGIN_CLOSING",
        AuctionSchedulePoint::ScheduledEnd => "SCHEDULED_END",
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicAuctionData {
    auction_id: AuctionId,
    listing_source: PublicSourceData,
    name: Option<crate::values::LocalizedTextData>,
    description: Option<crate::values::LocalizedTextData>,
    catalogue_url: Option<url::Url>,
    view_url: Option<url::Url>,
    format: Option<&'static str>,
    schedule: crate::auctions::types::AuctionScheduleResponseData,
    reported_status: Option<&'static str>,
    reported_lot_count: Option<u32>,
    visible_listing_count: u64,
}
impl TryFrom<PublicAuctionDetails> for PublicAuctionData {
    type Error = ApiError;

    fn try_from(value: PublicAuctionDetails) -> Result<Self, Self::Error> {
        let view_url = value
            .catalogue_url
            .as_ref()
            .map(|url| {
                listing_source_core::outbound_url(value.source.referral_configuration.as_ref(), url)
                    .map_err(|_| ApiError::internal_server_error(AUCTION_INTERNAL_ERROR))
            })
            .transpose()?;
        Ok(Self {
            auction_id: value.auction_id,
            listing_source: PublicSourceData {
                listing_source_id: value.source.listing_source_id,
                name: value.source.name.as_ref().to_owned(),
                slug_id: value.source.slug_id.to_string(),
            },
            name: value.name.map(Into::into),
            description: value.description.map(Into::into),
            catalogue_url: value.catalogue_url,
            view_url,
            format: value.format.map(AuctionFormat::as_str),
            schedule: value.schedule.into(),
            reported_status: value.reported_status.map(AuctionReportedStatus::as_str),
            reported_lot_count: value.reported_lot_count.map(|count| count.value()),
            visible_listing_count: value.visible_active_assigned_listing_count,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicSourceData {
    listing_source_id: ListingSourceId,
    name: String,
    slug_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicAuctionDirectoryData {
    items: Vec<PublicAuctionDirectoryItemData>,
    page_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<DirectoryCursorData>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicAuctionDirectoryItemData {
    auction_id: AuctionId,
    listing_source: PublicSourceData,
    name: Option<crate::values::LocalizedTextData>,
    format: Option<&'static str>,
    schedule: crate::auctions::types::AuctionScheduleResponseData,
    reported_status: Option<&'static str>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
}
impl From<auction_service::ports::ListAuctionsDirectoryResult> for PublicAuctionDirectoryData {
    fn from(value: auction_service::ports::ListAuctionsDirectoryResult) -> Self {
        Self {
            page_size: value.cursor.size,
            search_after: value.cursor.search_after.map(|cursor| DirectoryCursorData {
                created: cursor.created,
                auction_id: cursor.auction_id.to_string(),
                scope: cursor.scope.into(),
            }),
            items: value
                .items
                .into_iter()
                .map(|item| PublicAuctionDirectoryItemData {
                    auction_id: item.auction_id,
                    listing_source: PublicSourceData {
                        listing_source_id: item.source.listing_source_id,
                        name: item.source.name.as_ref().to_owned(),
                        slug_id: item.source.slug_id.to_string(),
                    },
                    name: item.name.map(Into::into),
                    format: item.format.map(AuctionFormat::as_str),
                    schedule: item.schedule.into(),
                    reported_status: item.reported_status.map(AuctionReportedStatus::as_str),
                    created: item.created,
                })
                .collect(),
        }
    }
}

impl From<GetPublicAuctionError> for ApiError {
    fn from(error: GetPublicAuctionError) -> Self {
        match error {
            GetPublicAuctionError::NotFound => ApiError::not_found(AUCTION_NOT_FOUND),
            GetPublicAuctionError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(AUCTION_TEMPORARILY_UNAVAILABLE)
            }
            GetPublicAuctionError::InvalidReadModel { .. } => {
                ApiError::internal_server_error(AUCTION_INTERNAL_ERROR)
            }
        }
    }
}
impl From<product_listing_service::use_cases::GetAuctionCatalogueError> for ApiError {
    fn from(error: product_listing_service::use_cases::GetAuctionCatalogueError) -> Self {
        match error {
            product_listing_service::use_cases::GetAuctionCatalogueError::AuctionNotFound => {
                ApiError::not_found(AUCTION_NOT_FOUND)
            }
            product_listing_service::use_cases::GetAuctionCatalogueError::CursorScopeMismatch => {
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            }
            product_listing_service::use_cases::GetAuctionCatalogueError::TemporarilyUnavailable { .. }
            | product_listing_service::use_cases::GetAuctionCatalogueError::PricingFxSnapshotMissing => {
                ApiError::service_unavailable(AUCTION_TEMPORARILY_UNAVAILABLE)
            }
            product_listing_service::use_cases::GetAuctionCatalogueError::InvalidReadModel { .. }
            | product_listing_service::use_cases::GetAuctionCatalogueError::PricingPresentationFailed { .. } => {
                ApiError::internal_server_error(AUCTION_INTERNAL_ERROR)
            }
        }
    }
}
impl From<ListAuctionsError> for ApiError {
    fn from(error: ListAuctionsError) -> Self {
        match error {
            ListAuctionsError::IncompleteScheduleInstantRange
            | ListAuctionsError::InvalidScheduleInstantRange
            | ListAuctionsError::CursorScopeMismatch => {
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            }
            ListAuctionsError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(AUCTION_TEMPORARILY_UNAVAILABLE)
            }
            ListAuctionsError::InvalidReadModel { .. } => {
                ApiError::internal_server_error(AUCTION_INTERNAL_ERROR)
            }
        }
    }
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
    use crate::auth::{AuthError, RequestMetadata, TokenAuthenticator, TransportPrincipal};
    use application::{operation_context::OperationContext, pagination::CursoredResult};
    use auction_service::{
        ports::PublicAuctionSourceSummary,
        use_cases::{
            GetPublicAuctionResult, GetPublicAuctionUseCase, ListAuctionsResult,
            ListAuctionsUseCase,
        },
    };
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode, header},
        routing::get,
    };
    use money::Currency;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;

    #[derive(Clone)]
    struct FakeGet {
        result: GetPublicAuctionResult,
    }

    #[async_trait::async_trait]
    impl GetPublicAuctionUseCase for FakeGet {
        async fn execute(
            &self,
            _context: &OperationContext,
            _auction_id: AuctionId,
        ) -> Result<GetPublicAuctionResult, GetPublicAuctionError> {
            Ok(self.result.clone())
        }
    }

    #[derive(Clone, Copy)]
    struct FakeList;

    #[async_trait::async_trait]
    impl ListAuctionsUseCase for FakeList {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: ListAuctionsRequest,
        ) -> Result<ListAuctionsResult, ListAuctionsError> {
            Ok(CursoredResult::default())
        }
    }

    #[derive(Clone, Default)]
    struct FakeCatalogue {
        requests: Arc<Mutex<Vec<product_listing_service::use_cases::GetAuctionCatalogueRequest>>>,
    }

    #[async_trait::async_trait]
    impl product_listing_service::use_cases::GetAuctionCatalogueUseCase for FakeCatalogue {
        async fn execute(
            &self,
            _context: &OperationContext,
            request: product_listing_service::use_cases::GetAuctionCatalogueRequest,
        ) -> Result<
            CursoredResult<
                product_listing_service::use_cases::PersonalizedProductListingDetailsView,
                product_listing_service::ports::AuctionCatalogueCursor,
            >,
            product_listing_service::use_cases::GetAuctionCatalogueError,
        > {
            lock(&self.requests).push(request);
            Ok(CursoredResult::default())
        }
    }

    #[derive(Clone, Copy)]
    struct FakeAuthenticator;

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Err(AuthError::InvalidCredentials)
        }
    }

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        match value.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn detail() -> PublicAuctionDetails {
        PublicAuctionDetails {
            auction_id: AuctionId::new(),
            source: PublicAuctionSourceSummary {
                listing_source_id: ListingSourceId::new(),
                slug_id: listing_source_core::ListingSourceSlugId::raw("source")
                    .unwrap_or_else(|error| panic!("valid source slug: {error}")),
                name: listing_source_core::ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("valid source name: {error}")),
                referral_configuration: None,
            },
            name: None,
            description: None,
            catalogue_url: None,
            format: None,
            schedule: auction_core::AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
            visible_active_assigned_listing_count: 0,
        }
    }

    fn app(catalogue: FakeCatalogue) -> Router {
        Router::new()
            .route("/api/v1/auctions", get(list_public_auctions))
            .route("/api/v1/auctions/{auction_id}", get(get_public_auction))
            .route(
                "/api/v1/auctions/{auction_id}/product-listings",
                get(get_auction_catalogue),
            )
            .with_state(PublicAuctionsState::new(
                Arc::new(FakeGet { result: detail() }),
                Arc::new(FakeList),
                Arc::new(catalogue),
                Arc::new(FakeAuthenticator),
            ))
    }

    #[tokio::test]
    async fn should_return_safe_public_auction_detail_without_store_for_anonymous_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = detail().auction_id;
        let response = app(FakeCatalogue::default())
            .oneshot(Request::get(format!("/api/v1/auctions/{auction_id}")).body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body: serde_json::Value = serde_json::from_slice(&body)?;
        assert!(body.get("sourceAuctionId").is_none());
        assert!(body.get("evidence").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_public_auction_id_without_store()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = app(FakeCatalogue::default())
            .oneshot(Request::get("/api/v1/auctions/not-an-auction").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_catalogue_currency_and_page_size_for_anonymous_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let catalogue = FakeCatalogue::default();
        let auction_id = detail().auction_id;
        let response = app(catalogue.clone())
            .oneshot(
                Request::get(format!(
                    "/api/v1/auctions/{auction_id}/product-listings?currency=USD&pageSize=25"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        assert!(matches!(
            lock(&catalogue.requests).as_slice(),
            [request] if request.auction_id == auction_id
                && request.currency == Currency::Usd
                && request.cursor.as_ref().map(|cursor| cursor.size) == Some(25)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_an_invalid_optional_public_credential()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = app(FakeCatalogue::default())
            .oneshot(
                Request::get("/api/v1/auctions")
                    .header(header::AUTHORIZATION, "Bearer invalid")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        Ok(())
    }
}
