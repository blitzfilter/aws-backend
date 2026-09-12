use auction_core::{AuctionId, AuctionTime, AuctionTimeZone};
use product_listing_core::product_listing_auction::{
    AuctionMembership, CataloguePosition, LotAuctionTiming, LotNumber, ProductListingAuction,
};
use time::{Date, OffsetDateTime};

#[derive(Debug, thiserror::Error)]
#[error("persisted product-listing auction context is invalid")]
pub(crate) struct ProductListingAuctionMappingError;

#[derive(Debug, Clone)]
pub(crate) struct ProductListingAuctionParts {
    pub(crate) context_product_listing_id: Option<uuid::Uuid>,
    pub(crate) auction_id: Option<uuid::Uuid>,
    pub(crate) lot_number: Option<String>,
    pub(crate) catalogue_position: Option<i64>,
    pub(crate) timing_product_listing_id: Option<uuid::Uuid>,
    pub(crate) bidding_opens_precision: Option<String>,
    pub(crate) bidding_opens_instant_at: Option<OffsetDateTime>,
    pub(crate) bidding_opens_date_on: Option<Date>,
    pub(crate) bidding_opens_source_timezone: Option<String>,
    pub(crate) scheduled_closes_precision: Option<String>,
    pub(crate) scheduled_closes_instant_at: Option<OffsetDateTime>,
    pub(crate) scheduled_closes_date_on: Option<Date>,
    pub(crate) scheduled_closes_source_timezone: Option<String>,
    pub(crate) reported_closed_at: Option<OffsetDateTime>,
}

pub(crate) fn auction_from_parts(
    parts: ProductListingAuctionParts,
) -> Result<Option<ProductListingAuction>, ProductListingAuctionMappingError> {
    if parts.context_product_listing_id.is_none() {
        return if parts.lot_number.is_none()
            && parts.catalogue_position.is_none()
            && parts.timing_product_listing_id.is_none()
            && parts.bidding_opens_precision.is_none()
            && parts.bidding_opens_instant_at.is_none()
            && parts.bidding_opens_date_on.is_none()
            && parts.bidding_opens_source_timezone.is_none()
            && parts.scheduled_closes_precision.is_none()
            && parts.scheduled_closes_instant_at.is_none()
            && parts.scheduled_closes_date_on.is_none()
            && parts.scheduled_closes_source_timezone.is_none()
            && parts.reported_closed_at.is_none()
        {
            Ok(None)
        } else {
            Err(ProductListingAuctionMappingError)
        };
    }

    let membership = parts
        .auction_id
        .map(|value| AuctionId::try_from(value).map(AuctionMembership::new))
        .transpose()
        .map_err(|_| ProductListingAuctionMappingError)?;
    let lot_number = parts
        .lot_number
        .map(|value| {
            let parsed = LotNumber::try_from(value.as_str())
                .map_err(|_| ProductListingAuctionMappingError)?;
            if parsed.as_str() != value {
                return Err(ProductListingAuctionMappingError);
            }
            Ok(parsed)
        })
        .transpose()?;
    let catalogue_position = parts
        .catalogue_position
        .map(|value| {
            let value = u64::try_from(value).map_err(|_| ProductListingAuctionMappingError)?;
            CataloguePosition::try_from(value).map_err(|_| ProductListingAuctionMappingError)
        })
        .transpose()?;

    let timing = match parts.timing_product_listing_id {
        None => {
            if parts.bidding_opens_precision.is_none()
                && parts.bidding_opens_instant_at.is_none()
                && parts.bidding_opens_date_on.is_none()
                && parts.bidding_opens_source_timezone.is_none()
                && parts.scheduled_closes_precision.is_none()
                && parts.scheduled_closes_instant_at.is_none()
                && parts.scheduled_closes_date_on.is_none()
                && parts.scheduled_closes_source_timezone.is_none()
                && parts.reported_closed_at.is_none()
            {
                None
            } else {
                return Err(ProductListingAuctionMappingError);
            }
        }
        Some(_) => Some(
            LotAuctionTiming::new(
                auction_time_from_parts(
                    parts.bidding_opens_precision,
                    parts.bidding_opens_instant_at,
                    parts.bidding_opens_date_on,
                    parts.bidding_opens_source_timezone,
                )?,
                auction_time_from_parts(
                    parts.scheduled_closes_precision,
                    parts.scheduled_closes_instant_at,
                    parts.scheduled_closes_date_on,
                    parts.scheduled_closes_source_timezone,
                )?,
                parts.reported_closed_at,
            )
            .map_err(|_| ProductListingAuctionMappingError)?,
        ),
    };

    Ok(Some(ProductListingAuction::new(
        membership,
        lot_number,
        catalogue_position,
        timing,
    )))
}

fn auction_time_from_parts(
    precision: Option<String>,
    instant_at: Option<OffsetDateTime>,
    date_on: Option<Date>,
    source_timezone: Option<String>,
) -> Result<Option<AuctionTime>, ProductListingAuctionMappingError> {
    let source_timezone = source_timezone
        .map(|value| {
            AuctionTimeZone::try_from(value).map_err(|_| ProductListingAuctionMappingError)
        })
        .transpose()?;

    match (precision.as_deref(), instant_at, date_on) {
        (None, None, None) if source_timezone.is_none() => Ok(None),
        (Some("INSTANT"), Some(instant_at), None) => {
            Ok(Some(AuctionTime::instant(instant_at, source_timezone)))
        }
        (Some("DATE"), None, Some(date_on)) => {
            Ok(Some(AuctionTime::date(date_on, source_timezone)))
        }
        _ => Err(ProductListingAuctionMappingError),
    }
}

pub(crate) struct ProductListingAuctionWriteParts {
    pub(crate) auction_id: Option<uuid::Uuid>,
    pub(crate) lot_number: Option<String>,
    pub(crate) catalogue_position: Option<i64>,
    pub(crate) timing: Option<LotAuctionTimingWriteParts>,
}

pub(crate) struct LotAuctionTimingWriteParts {
    pub(crate) bidding_opens: AuctionTimeWriteParts,
    pub(crate) scheduled_closes: AuctionTimeWriteParts,
    pub(crate) reported_closed_at: Option<OffsetDateTime>,
}

pub(crate) struct AuctionTimeWriteParts {
    pub(crate) precision: Option<&'static str>,
    pub(crate) instant_at: Option<OffsetDateTime>,
    pub(crate) date_on: Option<Date>,
    pub(crate) source_timezone: Option<String>,
}

pub(crate) fn auction_write_parts(
    value: &ProductListingAuction,
) -> ProductListingAuctionWriteParts {
    ProductListingAuctionWriteParts {
        auction_id: value
            .membership()
            .map(|value| *value.auction_id().as_uuid()),
        lot_number: value.lot_number().map(|value| value.as_str().to_owned()),
        catalogue_position: value
            .catalogue_position()
            .map(|value| i64::from(value.value())),
        timing: value.timing().map(|timing| LotAuctionTimingWriteParts {
            bidding_opens: auction_time_write_parts(timing.bidding_opens()),
            scheduled_closes: auction_time_write_parts(timing.scheduled_closes()),
            reported_closed_at: timing.reported_closed_at(),
        }),
    }
}

fn auction_time_write_parts(value: Option<&AuctionTime>) -> AuctionTimeWriteParts {
    match value {
        None => AuctionTimeWriteParts {
            precision: None,
            instant_at: None,
            date_on: None,
            source_timezone: None,
        },
        Some(AuctionTime::Instant {
            at,
            source_timezone,
        }) => AuctionTimeWriteParts {
            precision: Some("INSTANT"),
            instant_at: Some(*at),
            date_on: None,
            source_timezone: source_timezone
                .as_ref()
                .map(|value| value.as_str().to_owned()),
        },
        Some(AuctionTime::Date {
            on,
            source_timezone,
        }) => AuctionTimeWriteParts {
            precision: Some("DATE"),
            instant_at: None,
            date_on: Some(*on),
            source_timezone: source_timezone
                .as_ref()
                .map(|value| value.as_str().to_owned()),
        },
    }
}
