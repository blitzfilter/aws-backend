use application::error::BoxError;
use async_trait::async_trait;
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_normalization::ProductListingNormalizationInput;
use product_listing_service::ports::{ProductListingRawRevisionId, ProductListingRawStreamId};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingRawRevision {
    pub product_listing_raw_revision_id: ProductListingRawRevisionId,
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
    pub revision: u64,
    pub input: ProductListingNormalizationInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingRawNormalizationHead {
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
    pub listing_source_id: ListingSourceId,
    pub last_processed_revision: u64,
    pub product_listing_id: Option<ProductListingId>,
    pub source_listing_id: Option<SourceListingId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingRawNormalizationWork {
    pub head: ProductListingRawNormalizationHead,
    /// The exact next revision while the stream head is locked. `None` is an idempotent wake-up.
    pub next_revision: Option<ProductListingRawRevision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingRawNormalizationOutcome {
    Applied,
    NoChange,
    Ignored,
    Rejected,
}

impl ProductListingRawNormalizationOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "APPLIED",
            Self::NoChange => "NO_CHANGE",
            Self::Ignored => "IGNORED",
            Self::Rejected => "REJECTED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingRawNormalizationCompletion {
    pub product_listing_raw_revision_id: ProductListingRawRevisionId,
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
    pub revision: u64,
    pub normalizer_version: u16,
    pub outcome: ProductListingRawNormalizationOutcome,
    pub product_listing_id: Option<ProductListingId>,
    pub product_listing_event_id: Option<EventId>,
    pub error_code: Option<&'static str>,
    pub next_product_listing_id: Option<ProductListingId>,
    pub next_source_listing_id: Option<SourceListingId>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingRawNormalizationPortError {
    #[error("raw normalization persistence failed")]
    Persistence {
        #[source]
        source: BoxError,
    },
    #[error("raw normalization persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

/// Locks one raw stream and returns only its immediate next revision.
#[async_trait]
pub trait ProductListingRawNormalizationWriter: Send {
    async fn lock_next(
        &mut self,
        product_listing_raw_stream_id: ProductListingRawStreamId,
    ) -> Result<ProductListingRawNormalizationWork, ProductListingRawNormalizationPortError>;

    async fn complete(
        &mut self,
        completion: ProductListingRawNormalizationCompletion,
    ) -> Result<(), ProductListingRawNormalizationPortError>;
}

pub trait ProductListingRawNormalizationWriterFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingRawNormalizationWriter + 'tx;
}

/// Reads a possible next revision before the short write transaction. The handler locks and
/// revalidates it before any canonical mutation.
#[async_trait]
pub trait ProductListingRawRevisionReader: Send + Sync {
    async fn find_next_revision(
        &self,
        product_listing_raw_stream_id: ProductListingRawStreamId,
    ) -> Result<Option<ProductListingRawRevision>, ProductListingRawNormalizationPortError>;
}

/// One bounded recovery candidate. It carries only scheduling metadata, never source payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingProductListingRawStream {
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
    pub oldest_pending_at: OffsetDateTime,
}

/// Bounded recovery query. It returns stream IDs and oldest pending time only.
#[async_trait]
pub trait PendingProductListingRawStreamReader: Send + Sync {
    async fn list_pending_streams(
        &self,
        limit: u32,
    ) -> Result<Vec<PendingProductListingRawStream>, ProductListingRawNormalizationPortError>;
}
