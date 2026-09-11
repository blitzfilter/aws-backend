use application::error::BoxError;
use auction_core::AuctionId;
use domain_primitives::event_id::EventId;
use std::collections::BTreeSet;
use std::str::FromStr;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, strum_macros::EnumIter)]
pub enum AuctionMetadataField {
    Name,
    Description,
    CatalogueUrl,
    Format,
    ReportedStatus,
    ReportedLotCount,
    BiddingOpens,
    LiveStarts,
    LotsBeginClosing,
    ScheduledEnd,
}

impl AuctionMetadataField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Name => "NAME",
            Self::Description => "DESCRIPTION",
            Self::CatalogueUrl => "CATALOGUE_URL",
            Self::Format => "FORMAT",
            Self::ReportedStatus => "REPORTED_STATUS",
            Self::ReportedLotCount => "REPORTED_LOT_COUNT",
            Self::BiddingOpens => "BIDDING_OPENS",
            Self::LiveStarts => "LIVE_STARTS",
            Self::LotsBeginClosing => "LOTS_BEGIN_CLOSING",
            Self::ScheduledEnd => "SCHEDULED_END",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid auction metadata field `{value}`")]
pub struct InvalidAuctionMetadataField {
    value: String,
}

impl FromStr for AuctionMetadataField {
    type Err = InvalidAuctionMetadataField;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        use strum::IntoEnumIterator;

        Self::iter()
            .find(|field| field.as_str() == value)
            .ok_or_else(|| InvalidAuctionMetadataField {
                value: value.to_owned(),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionMetadataPolicyAudit {
    pub audit_id: EventId,
    pub auction_id: AuctionId,
    pub actor_label: String,
    pub recorded_at: OffsetDateTime,
    pub fields: BTreeSet<AuctionMetadataField>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuctionMetadataPolicyRepositoryError {
    #[error("auction metadata policy persistence failed")]
    PersistenceFailed {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted auction metadata policy")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AuctionMetadataPolicyRepository: Send {
    async fn find_protected_fields(
        &mut self,
        auction_id: AuctionId,
    ) -> Result<BTreeSet<AuctionMetadataField>, AuctionMetadataPolicyRepositoryError>;

    async fn protect(
        &mut self,
        audit: &AuctionMetadataPolicyAudit,
    ) -> Result<(), AuctionMetadataPolicyRepositoryError>;
}

pub trait AuctionMetadataPolicyRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl AuctionMetadataPolicyRepository + 'tx;
}
