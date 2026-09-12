use crate::{
    auth::protected_context,
    error::{ApiError, BAD_BODY_VALUE, BAD_HEADER_VALUE, PRODUCT_LISTING_INTERNAL_ERROR},
    state::AdminProductListingAuctionsState,
    wire::parse_path_object_id,
};

use auction_core::{AuctionId, AuctionTime, AuctionTimeZone};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use product_listing_core::product_listing_auction::{
    AuctionMembership, CataloguePosition, LotAuctionTiming, LotNumber, ProductListingAuction,
};
use product_listing_service::ports::{
    ProductListingAuctionPolicyVersion, ProductListingStorageVersion,
};
use product_listing_service::use_cases::{
    CorrectProductListingAuctionContextCommand, CorrectProductListingAuctionContextResult,
    ProductListingAuctionContextAdminView, ReleaseProductListingAuctionOverrideCommand,
    ReleaseProductListingAuctionOverrideResult,
};
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime, format_description::well_known::Iso8601};

pub async fn get_context(
    State(state): State<AdminProductListingAuctionsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let product_listing_id =
        match parse_path_object_id(&raw_id, "productListingId", "ProductListing") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
        };
    match state.get.execute(&context, product_listing_id).await {
        Ok(view) => {
            let version = view.version;
            let policy_version = view.auction_policy_version;
            response_with_etag(
                axum::Json(ContextData::from(view)).into_response(),
                version,
                policy_version,
            )
        }
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

pub async fn correct_context(
    State(state): State<AdminProductListingAuctionsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let product_listing_id =
        match parse_path_object_id(&raw_id, "productListingId", "ProductListing") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
        };
    let (expected_version, expected_auction_policy_version) = match parse_etag(&headers) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let command = match serde_json::from_str::<CorrectionData>(&body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
        .and_then(|data| {
            data.into_command(
                product_listing_id,
                expected_version,
                expected_auction_policy_version,
            )
        }) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    match state.correct.execute(&context, command).await {
        Ok(result) => {
            let version = result.version;
            let policy_version = result.auction_policy_version;
            response_with_etag(
                axum::Json(WriteResultData::from(result)).into_response(),
                version,
                policy_version,
            )
        }
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

pub async fn release_context(
    State(state): State<AdminProductListingAuctionsState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let product_listing_id =
        match parse_path_object_id(&raw_id, "productListingId", "ProductListing") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
        };
    let (expected_version, expected_auction_policy_version) = match parse_etag(&headers) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    match state
        .release
        .execute(
            &context,
            ReleaseProductListingAuctionOverrideCommand {
                product_listing_id,
                expected_version,
                expected_auction_policy_version,
            },
        )
        .await
    {
        Ok(result) => {
            let version = result.version;
            let policy_version = result.auction_policy_version;
            response_with_etag(
                axum::Json(ReleaseResultData::from(result)).into_response(),
                version,
                policy_version,
            )
        }
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CorrectionData {
    expected_current_auction_id: Option<String>,
    replacement: Option<AuctionContextData>,
    reason: String,
}
impl CorrectionData {
    fn into_command(
        self,
        product_listing_id: product_listing_core::product_listing_id::ProductListingId,
        expected_version: ProductListingStorageVersion,
        expected_auction_policy_version: ProductListingAuctionPolicyVersion,
    ) -> Result<CorrectProductListingAuctionContextCommand, ApiError> {
        Ok(CorrectProductListingAuctionContextCommand {
            product_listing_id,
            expected_version,
            expected_auction_policy_version,
            expected_current_auction_id: self
                .expected_current_auction_id
                .map(|value| parse_path_object_id(&value, "expectedCurrentAuctionId", "Auction"))
                .transpose()?,
            replacement: self
                .replacement
                .map(AuctionContextData::into_core)
                .transpose()?,
            reason: self.reason,
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuctionContextData {
    #[serde(default)]
    auction_id: Option<String>,
    #[serde(default)]
    lot_number: Option<String>,
    #[serde(default)]
    catalogue_position: Option<u32>,
    #[serde(default)]
    timing: Option<LotTimingData>,
}
impl AuctionContextData {
    fn into_core(self) -> Result<ProductListingAuction, ApiError> {
        Ok(ProductListingAuction::new(
            self.auction_id
                .map(|value| parse_path_object_id(&value, "replacement.auctionId", "Auction"))
                .transpose()?
                .map(AuctionMembership::new),
            self.lot_number
                .map(|value| {
                    LotNumber::try_from(value)
                        .map_err(|_| invalid("replacement.lotNumber is invalid."))
                })
                .transpose()?,
            self.catalogue_position
                .map(|value| {
                    CataloguePosition::new(value)
                        .map_err(|_| invalid("replacement.cataloguePosition must be positive."))
                })
                .transpose()?,
            self.timing.map(LotTimingData::into_core).transpose()?,
        ))
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LotTimingData {
    #[serde(default)]
    bidding_opens: Option<TimeData>,
    #[serde(default)]
    scheduled_closes: Option<TimeData>,
    #[serde(default)]
    reported_closed_at: Option<OffsetDateTime>,
}
impl LotTimingData {
    fn into_core(self) -> Result<LotAuctionTiming, ApiError> {
        LotAuctionTiming::new(
            self.bidding_opens.map(TimeData::into_core).transpose()?,
            self.scheduled_closes.map(TimeData::into_core).transpose()?,
            self.reported_closed_at,
        )
        .map_err(|_| invalid("replacement.timing has invalid comparable bounds."))
    }
}
#[derive(Deserialize)]
#[serde(
    tag = "precision",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
enum TimeData {
    Instant {
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
        #[serde(default, rename = "sourceTimezone")]
        source_timezone: Option<String>,
    },
    Date {
        on: String,
        #[serde(default, rename = "sourceTimezone")]
        source_timezone: Option<String>,
    },
}
impl TimeData {
    fn into_core(self) -> Result<AuctionTime, ApiError> {
        match self {
            Self::Instant {
                at,
                source_timezone,
            } => Ok(AuctionTime::instant(
                at,
                source_timezone
                    .map(AuctionTimeZone::try_from)
                    .transpose()
                    .map_err(|_| invalid("sourceTimezone must be an IANA timezone."))?,
            )),
            Self::Date {
                on,
                source_timezone,
            } => Ok(AuctionTime::date(
                Date::parse(&on, &Iso8601::DATE)
                    .map_err(|_| invalid("date must use YYYY-MM-DD."))?,
                source_timezone
                    .map(AuctionTimeZone::try_from)
                    .transpose()
                    .map_err(|_| invalid("sourceTimezone must be an IANA timezone."))?,
            )),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextData {
    product_listing_id: product_listing_core::product_listing_id::ProductListingId,
    listing_source_id: listing_source_core::ListingSourceId,
    auction: Option<ContextResponseData>,
    expected_version: u64,
    expected_auction_policy_version: u64,
    auction_context_override_active: bool,
}
impl From<ProductListingAuctionContextAdminView> for ContextData {
    fn from(value: ProductListingAuctionContextAdminView) -> Self {
        Self {
            product_listing_id: value.product_listing_id,
            listing_source_id: value.listing_source_id,
            auction: value.auction.map(ContextResponseData::from),
            expected_version: value.version.into_inner(),
            expected_auction_policy_version: value.auction_policy_version.into_inner(),
            auction_context_override_active: value.auction_context_override_active,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextResponseData {
    auction_id: Option<AuctionId>,
    lot_number: Option<String>,
    catalogue_position: Option<u32>,
    timing: LotTimingResponseData,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LotTimingResponseData {
    bidding_opens: Option<TimeResponseData>,
    scheduled_closes: Option<TimeResponseData>,
    #[serde(with = "time::serde::rfc3339::option")]
    reported_closed_at: Option<OffsetDateTime>,
}

#[derive(Serialize)]
#[serde(tag = "precision", rename_all = "SCREAMING_SNAKE_CASE")]
enum TimeResponseData {
    Instant {
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
        source_timezone: Option<String>,
    },
    Date {
        on: String,
        source_timezone: Option<String>,
    },
}

impl From<AuctionTime> for TimeResponseData {
    fn from(value: AuctionTime) -> Self {
        match value {
            AuctionTime::Instant {
                at,
                source_timezone,
            } => Self::Instant {
                at,
                source_timezone: source_timezone.map(|value| value.to_string()),
            },
            AuctionTime::Date {
                on,
                source_timezone,
            } => Self::Date {
                on: on.to_string(),
                source_timezone: source_timezone.map(|value| value.to_string()),
            },
        }
    }
}

impl From<LotAuctionTiming> for LotTimingResponseData {
    fn from(value: LotAuctionTiming) -> Self {
        Self {
            bidding_opens: value.bidding_opens().cloned().map(TimeResponseData::from),
            scheduled_closes: value
                .scheduled_closes()
                .cloned()
                .map(TimeResponseData::from),
            reported_closed_at: value.reported_closed_at(),
        }
    }
}
impl From<ProductListingAuction> for ContextResponseData {
    fn from(value: ProductListingAuction) -> Self {
        Self {
            auction_id: value.membership().map(|v| v.auction_id()),
            lot_number: value.lot_number().map(ToString::to_string),
            catalogue_position: value.catalogue_position().map(|v| v.value()),
            timing: value
                .timing()
                .cloned()
                .map(LotTimingResponseData::from)
                .unwrap_or(LotTimingResponseData {
                    bidding_opens: None,
                    scheduled_closes: None,
                    reported_closed_at: None,
                }),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WriteResultData {
    product_listing_id: product_listing_core::product_listing_id::ProductListingId,
    expected_version: u64,
    expected_auction_policy_version: u64,
}
impl From<CorrectProductListingAuctionContextResult> for WriteResultData {
    fn from(value: CorrectProductListingAuctionContextResult) -> Self {
        Self {
            product_listing_id: value.product_listing_id,
            expected_version: value.version.into_inner(),
            expected_auction_policy_version: value.auction_policy_version.into_inner(),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseResultData {
    product_listing_id: product_listing_core::product_listing_id::ProductListingId,
    expected_version: u64,
    expected_auction_policy_version: u64,
}
impl From<ReleaseProductListingAuctionOverrideResult> for ReleaseResultData {
    fn from(value: ReleaseProductListingAuctionOverrideResult) -> Self {
        Self {
            product_listing_id: value.product_listing_id,
            expected_version: value.version.into_inner(),
            expected_auction_policy_version: value.auction_policy_version.into_inner(),
        }
    }
}
fn parse_etag(
    headers: &HeaderMap,
) -> Result<
    (
        ProductListingStorageVersion,
        ProductListingAuctionPolicyVersion,
    ),
    ApiError,
> {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return Err(invalid_header("If-Match is required."));
    };
    let raw = value
        .to_str()
        .map_err(|_| invalid_header("If-Match must be valid ASCII."))?;
    let raw = raw
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .ok_or_else(|| invalid_header("If-Match must be a strong ETag."))?;
    let Some(values) = raw.strip_prefix("plv-").and_then(|v| v.split_once("-apv-")) else {
        return Err(invalid_header(
            "If-Match must use plv-{listing}-apv-{policy}.",
        ));
    };
    Ok((
        ProductListingStorageVersion::try_from(
            values
                .0
                .parse::<u64>()
                .map_err(|_| invalid_header("If-Match is invalid."))?,
        )
        .map_err(|_| invalid_header("If-Match is invalid."))?,
        ProductListingAuctionPolicyVersion::from(
            values
                .1
                .parse::<u64>()
                .map_err(|_| invalid_header("If-Match is invalid."))?,
        ),
    ))
}
fn response_with_etag(
    mut response: Response,
    version: ProductListingStorageVersion,
    policy_version: ProductListingAuctionPolicyVersion,
) -> Response {
    let etag = format!(
        "\"plv-{}-apv-{}\"",
        version.into_inner(),
        policy_version.into_inner(),
    );
    let header_value = match HeaderValue::from_str(&etag) {
        Ok(value) => value,
        Err(_) => {
            return no_store(
                ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR).into_response(),
            );
        }
    };
    response.headers_mut().insert(header::ETAG, header_value);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
fn invalid(detail: &str) -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail(detail)
}

fn invalid_header(detail: &str) -> ApiError {
    ApiError::bad_request(BAD_HEADER_VALUE)
        .with_header_field("If-Match")
        .with_detail(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_a_strong_listing_and_policy_etag() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_MATCH,
            HeaderValue::from_static("\"plv-12-apv-3\""),
        );

        let parsed = parse_etag(&headers);

        assert!(matches!(
            parsed,
            Ok((version, policy_version))
                if version.into_inner() == 12 && policy_version.into_inner() == 3
        ));
    }

    #[test]
    fn should_reject_missing_weak_wildcard_and_malformed_etags() {
        for value in [
            None,
            Some("W/\"plv-12-apv-3\""),
            Some("*"),
            Some("plv-12-apv-3"),
        ] {
            let mut headers = HeaderMap::new();
            if let Some(value) = value {
                headers.insert(header::IF_MATCH, HeaderValue::from_static(value));
            }

            let error = parse_etag(&headers).err();

            assert!(error.is_some());
            assert_eq!(Some(BAD_HEADER_VALUE), error.map(|value| value.code()));
        }
    }
}
