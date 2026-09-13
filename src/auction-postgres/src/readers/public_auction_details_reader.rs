use crate::mapping::{AuctionRow, AuctionSchedulePointRow, map_stored_auction};
use application::error::box_error;
use auction_service::ports::{
    PublicAuctionDetails, PublicAuctionDetailsReadError, PublicAuctionDetailsReader,
    PublicAuctionSourceSummary,
};
use listing_source_core::{
    ListingSourceId, ListingSourceName, ListingSourceSlugId, PartnerizeCamref,
    ReferralConfiguration,
};
use sqlx::PgPool;

#[derive(Clone)]
pub struct SqlxPublicAuctionDetailsReader {
    pool: PgPool,
}

impl SqlxPublicAuctionDetailsReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct PublicAuctionDetailsRow {
    auction_id: uuid::Uuid,
    listing_source_id: uuid::Uuid,
    source_auction_id: String,
    name_text: Option<String>,
    name_language: Option<String>,
    description_text: Option<String>,
    description_language: Option<String>,
    catalogue_url: Option<String>,
    format: Option<String>,
    reported_status: Option<String>,
    reported_lot_count: Option<i64>,
    version: i64,
    created: time::OffsetDateTime,
    updated: time::OffsetDateTime,
    listing_source_slug_id: String,
    listing_source_name: String,
    referral_configuration: Option<serde_json::Value>,
    visible_active_assigned_listing_count: i64,
}

#[derive(Debug, thiserror::Error)]
enum PublicAuctionDetailsRowMappingError {
    #[error("invalid persisted Auction")]
    Auction,
    #[error("invalid persisted ListingSource slug")]
    ListingSourceSlug(#[source] listing_source_core::InvalidListingSourceSlug),
    #[error("invalid persisted ListingSource name")]
    ListingSourceName(#[source] listing_source_core::ListingSourceNameError),
    #[error("invalid persisted referral configuration")]
    ReferralConfiguration,
    #[error("invalid persisted Partnerize camref")]
    PartnerizeCamref(#[source] listing_source_core::PartnerizeCamrefError),
    #[error("invalid persisted visible assigned listing count")]
    VisibleActiveAssignedListingCount(#[source] std::num::TryFromIntError),
    #[error("persisted Auction source does not match its joined ListingSource")]
    SourceMismatch,
}

#[async_trait::async_trait]
impl PublicAuctionDetailsReader for SqlxPublicAuctionDetailsReader {
    async fn find_by_id(
        &self,
        auction_id: auction_core::AuctionId,
    ) -> Result<Option<PublicAuctionDetails>, PublicAuctionDetailsReadError> {
        let mut connection = self.pool.acquire().await.map_err(query_error)?;
        let row = sqlx::query_as::<_, PublicAuctionDetailsRow>(
            "SELECT a.auction_id, a.listing_source_id, a.source_auction_id, a.name_text, a.name_language, a.description_text, a.description_language, a.catalogue_url, a.format, a.reported_status, a.reported_lot_count, a.version, a.created, a.updated, s.listing_source_slug_id, s.name AS listing_source_name, s.referral_configuration, (SELECT COUNT(*) FROM product_listing_auction_contexts context JOIN product_listings listing ON listing.product_listing_id = context.product_listing_id WHERE context.auction_id = a.auction_id AND listing.lifecycle = 'ACTIVE') AS visible_active_assigned_listing_count FROM auctions a JOIN listing_sources s ON s.listing_source_id = a.listing_source_id WHERE a.auction_id = $1",
        )
        .bind(auction_id.as_uuid())
        .fetch_optional(&mut *connection)
        .await
        .map_err(query_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let schedule_rows = sqlx::query_as::<_, AuctionSchedulePointRow>(
            "SELECT role, precision, instant_at, date_on, source_timezone FROM auction_schedule_points WHERE auction_id = $1",
        )
        .bind(auction_id.as_uuid())
        .fetch_all(&mut *connection)
        .await
        .map_err(query_error)?;

        map_details(row, schedule_rows)
            .map(Some)
            .map_err(read_model_error)
    }
}

fn map_details(
    row: PublicAuctionDetailsRow,
    schedule_rows: Vec<AuctionSchedulePointRow>,
) -> Result<PublicAuctionDetails, PublicAuctionDetailsRowMappingError> {
    let source_id = ListingSourceId::try_from(row.listing_source_id)
        .map_err(|_| PublicAuctionDetailsRowMappingError::Auction)?;
    let stored = map_stored_auction(
        AuctionRow {
            auction_id: row.auction_id,
            listing_source_id: row.listing_source_id,
            source_auction_id: row.source_auction_id,
            name_text: row.name_text,
            name_language: row.name_language,
            description_text: row.description_text,
            description_language: row.description_language,
            catalogue_url: row.catalogue_url,
            format: row.format,
            reported_status: row.reported_status,
            reported_lot_count: row.reported_lot_count,
            version: row.version,
            created: row.created,
            updated: row.updated,
        },
        schedule_rows,
    )
    .map_err(|_| PublicAuctionDetailsRowMappingError::Auction)?;
    if stored.auction.key().listing_source_id() != source_id {
        return Err(PublicAuctionDetailsRowMappingError::SourceMismatch);
    }
    let source = PublicAuctionSourceSummary {
        listing_source_id: source_id,
        slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id)
            .map_err(PublicAuctionDetailsRowMappingError::ListingSourceSlug)?,
        name: ListingSourceName::try_from(row.listing_source_name)
            .map_err(PublicAuctionDetailsRowMappingError::ListingSourceName)?,
        referral_configuration: parse_referral_configuration(row.referral_configuration)?,
    };
    let auction = stored.auction;
    Ok(PublicAuctionDetails {
        auction_id: auction.id(),
        source,
        name: auction.name().cloned(),
        description: auction.description().cloned(),
        catalogue_url: auction.catalogue_url().cloned(),
        format: auction.format(),
        schedule: auction.schedule().clone(),
        reported_status: auction.reported_status(),
        reported_lot_count: auction.reported_lot_count(),
        visible_active_assigned_listing_count: u64::try_from(
            row.visible_active_assigned_listing_count,
        )
        .map_err(PublicAuctionDetailsRowMappingError::VisibleActiveAssignedListingCount)?,
    })
}

fn parse_referral_configuration(
    value: Option<serde_json::Value>,
) -> Result<Option<ReferralConfiguration>, PublicAuctionDetailsRowMappingError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.get("kind").and_then(serde_json::Value::as_str) != Some("PARTNERIZE") {
        return Err(PublicAuctionDetailsRowMappingError::ReferralConfiguration);
    }
    let camref = value
        .get("camref")
        .and_then(serde_json::Value::as_str)
        .ok_or(PublicAuctionDetailsRowMappingError::ReferralConfiguration)?;
    Ok(Some(ReferralConfiguration::Partnerize {
        camref: PartnerizeCamref::try_from(camref)
            .map_err(PublicAuctionDetailsRowMappingError::PartnerizeCamref)?,
    }))
}

fn query_error(error: sqlx::Error) -> PublicAuctionDetailsReadError {
    PublicAuctionDetailsReadError::QueryFailed {
        source: box_error(error),
    }
}

fn read_model_error(error: PublicAuctionDetailsRowMappingError) -> PublicAuctionDetailsReadError {
    PublicAuctionDetailsReadError::InvalidReadModel {
        source: box_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn row(referral_configuration: Option<serde_json::Value>) -> PublicAuctionDetailsRow {
        PublicAuctionDetailsRow {
            auction_id: uuid::Uuid::now_v7(),
            listing_source_id: uuid::Uuid::now_v7(),
            source_auction_id: "sale-42".to_owned(),
            name_text: None,
            name_language: None,
            description_text: None,
            description_language: None,
            catalogue_url: None,
            format: Some("TIMED".to_owned()),
            reported_status: Some("SCHEDULED".to_owned()),
            reported_lot_count: Some(4),
            version: 1,
            created: datetime!(2026-01-01 00:00 UTC),
            updated: datetime!(2026-01-01 00:00 UTC),
            listing_source_slug_id: "auction-house".to_owned(),
            listing_source_name: "Auction House".to_owned(),
            referral_configuration,
            visible_active_assigned_listing_count: 3,
        }
    }

    #[test]
    fn should_map_safe_partnerize_source_configuration_and_auction_facts() {
        let result = map_details(
            row(Some(serde_json::json!({
                "kind": "PARTNERIZE",
                "camref": "auctioncampaign",
            }))),
            Vec::new(),
        );

        assert!(matches!(
            result,
            Ok(PublicAuctionDetails {
                format: Some(auction_core::AuctionFormat::Timed),
                reported_status: Some(auction_core::AuctionReportedStatus::Scheduled),
                visible_active_assigned_listing_count: 3,
                source: PublicAuctionSourceSummary {
                    referral_configuration: Some(ReferralConfiguration::Partnerize { camref }),
                    ..
                },
                ..
            }) if camref.as_ref() == "auctioncampaign"
        ));
    }

    #[test]
    fn should_reject_noncanonical_persisted_auction_enum_values() {
        let mut invalid = row(None);
        invalid.format = Some("timed".to_owned());

        assert!(map_details(invalid, Vec::new()).is_err());
    }
}
