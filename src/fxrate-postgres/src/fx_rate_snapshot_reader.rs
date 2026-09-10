use crate::fx_rate_snapshot_repository::{QuoteRow, SnapshotRow, map_snapshots};
use application::error::{box_error, static_error};
use fxrate_core::{FxRateId, FxRateSnapshot};
use fxrate_service::ports::{FxRateSnapshotReadError, FxRateSnapshotReader};
use sqlx::PgPool;
use time::OffsetDateTime;

/// Pooled, one-statement reader for immutable FX snapshots used by presentation reads.
#[derive(Debug, Clone)]
pub struct SqlxFxRateSnapshotReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct SnapshotWithQuoteRow {
    fx_rate_id: uuid::Uuid,
    generation: i64,
    captured_at: OffsetDateTime,
    source: String,
    currency: Option<String>,
    units_per_eur: Option<i64>,
}

impl SqlxFxRateSnapshotReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl FxRateSnapshotReader for SqlxFxRateSnapshotReader {
    async fn find_by_id(
        &self,
        id: FxRateId,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
        let rows = sqlx::query_as::<_, SnapshotWithQuoteRow>(
            r#"
            SELECT
                s.fx_rate_id,
                s.generation,
                s.captured_at,
                s.source,
                q.currency,
                q.units_per_eur
            FROM fx_rates s
            LEFT JOIN fx_rate_quotes q ON q.fx_rate_id = s.fx_rate_id
            WHERE s.fx_rate_id = $1
            ORDER BY q.currency
            "#,
        )
        .bind(id.into_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(|source| FxRateSnapshotReadError::ReadFailed {
            source: box_error(source),
        })?;

        decode_snapshot_rows(rows)
    }

    async fn find_latest_at_or_before(
        &self,
        at: OffsetDateTime,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
        let rows = sqlx::query_as::<_, SnapshotWithQuoteRow>(
            r#"
            WITH selected_snapshot AS (
                SELECT fx_rate_id, generation, captured_at, source
                FROM fx_rates
                WHERE captured_at <= $1
                ORDER BY captured_at DESC, generation DESC
                LIMIT 1
            )
            SELECT
                s.fx_rate_id,
                s.generation,
                s.captured_at,
                s.source,
                q.currency,
                q.units_per_eur
            FROM selected_snapshot s
            LEFT JOIN fx_rate_quotes q ON q.fx_rate_id = s.fx_rate_id
            ORDER BY q.currency
            "#,
        )
        .bind(at)
        .fetch_all(&self.pool)
        .await
        .map_err(|source| FxRateSnapshotReadError::ReadFailed {
            source: box_error(source),
        })?;

        decode_snapshot_rows(rows)
    }
}

fn decode_snapshot_rows(
    rows: Vec<SnapshotWithQuoteRow>,
) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
    let Some(first) = rows.first() else {
        return Ok(None);
    };
    let snapshot = SnapshotRow {
        fx_rate_id: first.fx_rate_id,
        generation: first.generation,
        captured_at: first.captured_at,
        source: first.source.clone(),
    };
    let mut quotes = Vec::new();
    for row in rows {
        if row.fx_rate_id != snapshot.fx_rate_id
            || row.generation != snapshot.generation
            || row.captured_at != snapshot.captured_at
            || row.source != snapshot.source
        {
            return Err(FxRateSnapshotReadError::InvalidPersistedSnapshot {
                source: static_error("FX snapshot statement returned inconsistent snapshot rows"),
            });
        }
        match (row.currency, row.units_per_eur) {
            (Some(currency), Some(units_per_eur)) => quotes.push(QuoteRow {
                fx_rate_id: row.fx_rate_id,
                currency,
                units_per_eur,
            }),
            (None, None) => {}
            _ => {
                return Err(FxRateSnapshotReadError::InvalidPersistedSnapshot {
                    source: static_error("persisted FX quote is partially null"),
                });
            }
        }
    }

    let mapped = map_snapshots(vec![snapshot], quotes).map_err(map_decode_error)?;
    Ok(mapped.into_iter().next())
}

fn map_decode_error(
    error: fxrate_service::ports::FxRateSnapshotRepositoryError,
) -> FxRateSnapshotReadError {
    match error {
        fxrate_service::ports::FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
            source,
        } => FxRateSnapshotReadError::InvalidPersistedSnapshot { source },
        fxrate_service::ports::FxRateSnapshotRepositoryError::ReadFailed { source }
        | fxrate_service::ports::FxRateSnapshotRepositoryError::InsertFailed { source } => {
            FxRateSnapshotReadError::ReadFailed { source }
        }
        fxrate_service::ports::FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => {
            FxRateSnapshotReadError::InvalidPersistedSnapshot {
                source: static_error("unexpected FX snapshot capture monotonicity error"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxrate_core::FxRateSource;

    fn row(
        fx_rate_id: uuid::Uuid,
        currency: Option<&str>,
        units_per_eur: Option<i64>,
    ) -> SnapshotWithQuoteRow {
        SnapshotWithQuoteRow {
            fx_rate_id,
            generation: 1,
            captured_at: OffsetDateTime::UNIX_EPOCH,
            source: FxRateSource::FxRatesApi.as_str().to_owned(),
            currency: currency.map(ToOwned::to_owned),
            units_per_eur,
        }
    }

    #[test]
    fn should_reject_snapshot_header_without_quotes() {
        let result = decode_snapshot_rows(vec![row(FxRateId::new().into_uuid(), None, None)]);

        assert!(matches!(
            result,
            Err(FxRateSnapshotReadError::InvalidPersistedSnapshot { .. })
        ));
    }

    #[test]
    fn should_reject_partial_quote_row() {
        let result =
            decode_snapshot_rows(vec![row(FxRateId::new().into_uuid(), Some("EUR"), None)]);

        assert!(matches!(
            result,
            Err(FxRateSnapshotReadError::InvalidPersistedSnapshot { .. })
        ));
    }
}
