use application::error::BoxError;
use auction_core::{
    AuctionDescription, AuctionFormat, AuctionId, AuctionName, AuctionReportedStatus,
    AuctionSchedule, ReportedCatalogueLotCount,
};
use listing_source_core::{
    ListingSourceId, ListingSourceName, ListingSourceSlugId, ReferralConfiguration,
};
use localization::{Language, Localized};
use url::Url;

/// Safe source data needed to link a public Auction to its source and referral policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAuctionSourceSummary {
    pub listing_source_id: ListingSourceId,
    pub slug_id: ListingSourceSlugId,
    pub name: ListingSourceName,
    pub referral_configuration: Option<ReferralConfiguration>,
}

/// Authoritative public Auction detail data. It deliberately excludes the source Auction key,
/// policy/audit state, evidence, and storage version.
#[derive(Debug, Clone, PartialEq)]
pub struct PublicAuctionDetails {
    pub auction_id: AuctionId,
    pub source: PublicAuctionSourceSummary,
    pub name: Option<Localized<Language, AuctionName>>,
    pub description: Option<Localized<Language, AuctionDescription>>,
    pub catalogue_url: Option<Url>,
    pub format: Option<AuctionFormat>,
    pub schedule: AuctionSchedule,
    pub reported_status: Option<AuctionReportedStatus>,
    pub reported_lot_count: Option<ReportedCatalogueLotCount>,
    pub visible_active_assigned_listing_count: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum PublicAuctionDetailsReadError {
    #[error("public Auction details query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("public Auction details read model is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PublicAuctionDetailsReader: Send + Sync {
    async fn find_by_id(
        &self,
        auction_id: AuctionId,
    ) -> Result<Option<PublicAuctionDetails>, PublicAuctionDetailsReadError>;
}
