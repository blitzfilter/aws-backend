use application::error::BoxError;
use fxrate_core::{FxRateId, FxRateSnapshot};
use time::OffsetDateTime;

/// Ordinary read capability for immutable FX snapshots.
///
/// Unlike `FxRateSnapshotRepository`, this reader is not transaction-bound and
/// is intended for presentation reads that do not require a caller transaction.
#[derive(Debug, thiserror::Error)]
pub enum FxRateSnapshotReadError {
    #[error("FX rate snapshot read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted FX rate snapshot is invalid")]
    InvalidPersistedSnapshot {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait FxRateSnapshotReader: Send + Sync {
    async fn find_by_id(
        &self,
        id: FxRateId,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError>;

    async fn find_latest_at_or_before(
        &self,
        at: OffsetDateTime,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError>;
}
