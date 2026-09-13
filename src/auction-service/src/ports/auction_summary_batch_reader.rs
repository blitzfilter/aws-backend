use application::error::BoxError;
use auction_core::{AuctionFormat, AuctionId, AuctionName, AuctionReportedStatus, AuctionSchedule};
use localization::{Language, Localized};
use std::collections::HashMap;

/// Safe current Auction facts used to enrich ProductListing presentation reads.
///
/// This is intentionally a read model, not an aggregate or an administrative view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionSummary {
    pub auction_id: AuctionId,
    pub name: Option<Localized<Language, AuctionName>>,
    pub format: Option<AuctionFormat>,
    pub reported_status: Option<AuctionReportedStatus>,
    pub schedule: AuctionSchedule,
}

#[derive(Debug, thiserror::Error)]
pub enum AuctionSummaryBatchReadError {
    #[error("Auction summary query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("Auction summary read model is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AuctionSummaryBatchReader: Send + Sync {
    /// Returns all found summaries keyed by Auction ID. A requested resolved ID absent from this
    /// authoritative read is an integrity error for the caller, never an unresolved context.
    async fn find_summaries(
        &self,
        auction_ids: &[AuctionId],
    ) -> Result<HashMap<AuctionId, AuctionSummary>, AuctionSummaryBatchReadError>;
}
