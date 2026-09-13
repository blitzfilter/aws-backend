use application::{
    error::BoxError,
    pagination::{Cursor, CursoredResult},
};
use auction_core::{
    AuctionFormat, AuctionId, AuctionName, AuctionReportedStatus, AuctionSchedule,
    AuctionSchedulePoint,
};
use domain_primitives::query::range_query::RangeQuery;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use localization::{Language, Localized};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct AuctionDirectoryScope {
    pub listing_source_id: Option<ListingSourceId>,
    pub format: Option<AuctionFormat>,
    pub reported_status: Option<AuctionReportedStatus>,
    pub schedule: Option<AuctionInstantScheduleFilter>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuctionDirectoryCursor {
    pub created: OffsetDateTime,
    pub auction_id: AuctionId,
    pub scope: AuctionDirectoryScope,
}

/// An exact-instant schedule constraint. Both range bounds are required and the upper bound is
/// exclusive so adjacent windows do not overlap.
#[derive(Debug, Clone, PartialEq)]
pub struct AuctionInstantScheduleFilter {
    pub role: AuctionSchedulePoint,
    pub range: RangeQuery<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListAuctionsDirectoryRequest {
    pub listing_source_id: Option<ListingSourceId>,
    pub format: Option<AuctionFormat>,
    pub reported_status: Option<AuctionReportedStatus>,
    pub schedule: Option<AuctionInstantScheduleFilter>,
    pub cursor: Option<Cursor<AuctionDirectoryCursor>>,
}

impl ListAuctionsDirectoryRequest {
    pub fn scope(&self) -> AuctionDirectoryScope {
        AuctionDirectoryScope {
            listing_source_id: self.listing_source_id,
            format: self.format,
            reported_status: self.reported_status,
            schedule: self.schedule.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAuctionDirectorySourceSummary {
    pub listing_source_id: ListingSourceId,
    pub slug_id: ListingSourceSlugId,
    pub name: ListingSourceName,
}

/// Compact public Auction data for the fixed newest-first directory.
#[derive(Debug, Clone, PartialEq)]
pub struct PublicAuctionDirectoryItem {
    pub auction_id: AuctionId,
    pub source: PublicAuctionDirectorySourceSummary,
    pub name: Option<Localized<Language, AuctionName>>,
    pub format: Option<AuctionFormat>,
    pub schedule: AuctionSchedule,
    pub reported_status: Option<AuctionReportedStatus>,
    pub created: OffsetDateTime,
}

pub type ListAuctionsDirectoryResult =
    CursoredResult<PublicAuctionDirectoryItem, AuctionDirectoryCursor>;

#[derive(Debug, thiserror::Error)]
pub enum AuctionDirectoryReadError {
    #[error("Auction directory query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("Auction directory read model is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AuctionDirectoryReader: Send + Sync {
    async fn list(
        &self,
        request: &ListAuctionsDirectoryRequest,
    ) -> Result<ListAuctionsDirectoryResult, AuctionDirectoryReadError>;
}
