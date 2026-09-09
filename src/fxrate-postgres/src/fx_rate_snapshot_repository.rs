use application::error::{box_error, static_error};
use fxrate_core::{
    FxRateGeneration, FxRateId, FxRateQuote, FxRateSnapshot, FxRateSource, NewFxRateSnapshot,
};
use fxrate_service::ports::{
    FxRateSnapshotInsertOutcome, FxRateSnapshotRepository, FxRateSnapshotRepositoryError,
    FxRateSnapshotRepositoryFactory,
};
use money::Currency;
use platform_postgres::SqlxTransaction;
use sqlx::{PgConnection, Postgres, QueryBuilder};
use std::collections::HashMap;
use strum::IntoEnumIterator;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxFxRateSnapshotRepositoryFactory;

struct SqlxFxRateSnapshotRepository<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct SnapshotRow {
    pub(crate) fx_rate_id: uuid::Uuid,
    generation: i64,
    captured_at: OffsetDateTime,
    source: String,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct QuoteRow {
    fx_rate_id: uuid::Uuid,
    currency: String,
    units_per_eur: i64,
}

#[derive(Debug, thiserror::Error)]
#[error("FX rate snapshot SQL insert failed")]
struct FxRateSnapshotInsertSqlxError(#[source] sqlx::Error);

impl SqlxFxRateSnapshotRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl FxRateSnapshotRepositoryFactory<SqlxTransaction> for SqlxFxRateSnapshotRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl FxRateSnapshotRepository + 'tx {
        SqlxFxRateSnapshotRepository {
            connection: tx.connection(),
        }
    }
}

impl SqlxFxRateSnapshotRepository<'_> {
    async fn rehydrate(
        &mut self,
        snapshots: Vec<SnapshotRow>,
    ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        if snapshots.is_empty() {
            return Ok(Vec::new());
        }

        let ids = snapshots
            .iter()
            .map(|snapshot| snapshot.fx_rate_id)
            .collect::<Vec<_>>();
        let quotes = sqlx::query_as::<_, QuoteRow>(
            "SELECT fx_rate_id, currency, units_per_eur FROM fx_rate_quotes WHERE fx_rate_id = ANY($1)",
        )
        .bind(ids)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(|source| FxRateSnapshotRepositoryError::ReadFailed {
            source: box_error(source),
        })?;

        map_snapshots(snapshots, quotes)
    }
}

#[async_trait::async_trait]
impl FxRateSnapshotRepository for SqlxFxRateSnapshotRepository<'_> {
    async fn find_latest(
        &mut self,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        let row = sqlx::query_as::<_, SnapshotRow>(
            "SELECT fx_rate_id, generation, captured_at, source FROM fx_rates ORDER BY captured_at DESC, generation DESC LIMIT 1",
        )
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(|source| FxRateSnapshotRepositoryError::ReadFailed {
            source: box_error(source),
        })?;

        Ok(self.rehydrate(row.into_iter().collect()).await?.pop())
    }

    async fn find_latest_at_or_before(
        &mut self,
        timestamp: OffsetDateTime,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        let row = sqlx::query_as::<_, SnapshotRow>(
            "SELECT fx_rate_id, generation, captured_at, source FROM fx_rates WHERE captured_at <= $1 ORDER BY captured_at DESC, generation DESC LIMIT 1",
        )
        .bind(timestamp)
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(|source| FxRateSnapshotRepositoryError::ReadFailed {
            source: box_error(source),
        })?;

        Ok(self.rehydrate(row.into_iter().collect()).await?.pop())
    }

    async fn find_by_id(
        &mut self,
        id: FxRateId,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        let row = sqlx::query_as::<_, SnapshotRow>(
            "SELECT fx_rate_id, generation, captured_at, source FROM fx_rates WHERE fx_rate_id = $1",
        )
        .bind(id.into_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(|source| FxRateSnapshotRepositoryError::ReadFailed {
            source: box_error(source),
        })?;

        Ok(self.rehydrate(row.into_iter().collect()).await?.pop())
    }

    async fn find_by_ids(
        &mut self,
        ids: &[FxRateId],
    ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let ids = ids
            .iter()
            .copied()
            .map(FxRateId::into_uuid)
            .collect::<Vec<_>>();
        let snapshots = sqlx::query_as::<_, SnapshotRow>(
            "SELECT fx_rate_id, generation, captured_at, source FROM fx_rates WHERE fx_rate_id = ANY($1) ORDER BY generation ASC",
        )
        .bind(ids)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(|source| FxRateSnapshotRepositoryError::ReadFailed {
            source: box_error(source),
        })?;

        self.rehydrate(snapshots).await
    }

    async fn insert(
        &mut self,
        snapshot: &NewFxRateSnapshot,
        source_event_id: &str,
    ) -> Result<FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError> {
        sqlx::query("LOCK TABLE fx_rates IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *self.connection)
            .await
            .map_err(FxRateSnapshotInsertSqlxError)?;

        let duplicate = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM fx_rates WHERE source_event_id = $1)",
        )
        .bind(source_event_id)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(FxRateSnapshotInsertSqlxError)?;
        if duplicate {
            return Ok(FxRateSnapshotInsertOutcome::Duplicate);
        }

        let latest_captured_at = sqlx::query_scalar::<_, OffsetDateTime>(
            "SELECT captured_at FROM fx_rates ORDER BY captured_at DESC, generation DESC LIMIT 1",
        )
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(FxRateSnapshotInsertSqlxError)?;
        if latest_captured_at.is_some_and(|latest| snapshot.captured_at() <= latest) {
            return Err(FxRateSnapshotRepositoryError::CapturedAtNotMonotonic);
        }

        let generation = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id)
            VALUES ($1, $2, $3, $4)
            RETURNING generation
            "#,
        )
        .bind(snapshot.id().into_uuid())
        .bind(snapshot.captured_at())
        .bind(snapshot.source().as_str())
        .bind(source_event_id)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(FxRateSnapshotInsertSqlxError)?;
        let generation = FxRateGeneration::try_from(generation).map_err(|source| {
            FxRateSnapshotRepositoryError::InsertFailed {
                source: box_error(source),
            }
        })?;

        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO fx_rate_quotes (fx_rate_id, currency, units_per_eur) ",
        );
        query.push_values(snapshot.quotes(), |mut row, quote| {
            row.push_bind(snapshot.id().into_uuid())
                .push_bind(quote.currency().as_str())
                .push_bind(quote.units_per_eur() as i64);
        });
        query
            .build()
            .execute(&mut *self.connection)
            .await
            .map_err(FxRateSnapshotInsertSqlxError)?;

        Ok(FxRateSnapshotInsertOutcome::Inserted(
            snapshot.clone().into_persisted(generation),
        ))
    }
}

pub(crate) fn map_snapshots(
    snapshots: Vec<SnapshotRow>,
    quotes: Vec<QuoteRow>,
) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
    let mut quotes_by_snapshot = HashMap::<uuid::Uuid, Vec<FxRateQuote>>::new();
    for quote in quotes {
        let currency = Currency::from_code(&quote.currency).ok_or_else(|| {
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                source: static_error("persisted FX quote currency is unsupported"),
            }
        })?;
        let units_per_eur = u64::try_from(quote.units_per_eur).map_err(|_| {
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                source: static_error("persisted FX quote is negative"),
            }
        })?;
        quotes_by_snapshot
            .entry(quote.fx_rate_id)
            .or_default()
            .push(FxRateQuote::new(currency, units_per_eur));
    }

    snapshots
        .into_iter()
        .map(|snapshot| {
            let generation = FxRateGeneration::try_from(snapshot.generation).map_err(|source| {
                FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                    source: box_error(source),
                }
            })?;
            let source = parse_source(&snapshot.source).map_err(|source| {
                FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                    source: box_error(source),
                }
            })?;
            let fx_rate_id = FxRateId::try_from(snapshot.fx_rate_id).map_err(|source| {
                FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                    source: box_error(source),
                }
            })?;
            FxRateSnapshot::rehydrate(
                fx_rate_id,
                generation,
                snapshot.captured_at,
                source,
                quotes_by_snapshot
                    .remove(&snapshot.fx_rate_id)
                    .unwrap_or_default(),
            )
            .map_err(|source| {
                FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                    source: box_error(source),
                }
            })
        })
        .collect()
}

fn parse_source(value: &str) -> Result<FxRateSource, fxrate_core::FxRateSnapshotError> {
    FxRateSource::iter()
        .find(|source| source.as_str() == value)
        .ok_or(fxrate_core::FxRateSnapshotError::InvalidSource)
}

impl From<FxRateSnapshotInsertSqlxError> for FxRateSnapshotRepositoryError {
    fn from(source: FxRateSnapshotInsertSqlxError) -> Self {
        Self::InsertFailed {
            source: box_error(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_round_trip_v7_fx_rate_id_from_storage_rows() {
        let fx_rate_id = FxRateId::new();
        let snapshots = vec![SnapshotRow {
            fx_rate_id: fx_rate_id.into_uuid(),
            generation: 1,
            captured_at: OffsetDateTime::UNIX_EPOCH,
            source: FxRateSource::FxRatesApi.as_str().to_owned(),
        }];
        let quotes = Currency::iter()
            .map(|currency| QuoteRow {
                fx_rate_id: fx_rate_id.into_uuid(),
                currency: currency.as_str().to_owned(),
                units_per_eur: 1_000_000,
            })
            .collect();

        let mapped = map_snapshots(snapshots, quotes)
            .unwrap_or_else(|error| panic!("valid storage rows failed: {error}"));

        assert_eq!(
            vec![fx_rate_id],
            mapped.iter().map(FxRateSnapshot::id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn should_reject_v4_fx_rate_id_from_storage_row() {
        let result = map_snapshots(
            vec![SnapshotRow {
                fx_rate_id: uuid::Uuid::new_v4(),
                generation: 1,
                captured_at: OffsetDateTime::UNIX_EPOCH,
                source: FxRateSource::FxRatesApi.as_str().to_owned(),
            }],
            Vec::new(),
        );

        assert!(matches!(
            result,
            Err(FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { .. })
        ));
    }

    #[test]
    fn should_parse_each_canonical_persisted_source() {
        for expected in FxRateSource::iter() {
            assert_eq!(Ok(expected), parse_source(expected.as_str()));
        }
    }

    #[test]
    fn should_reject_unknown_and_noncanonical_persisted_source() {
        assert!(parse_source("FXRATESAPI").is_err());
        assert!(parse_source("unknown").is_err());
    }
}
