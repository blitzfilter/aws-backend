use crate::mapping::{AuctionRow, AuctionSchedulePointRow, map_error, map_stored_auction};
use application::error::box_error;
use auction_core::AuctionId;
use auction_service::ports::{
    AuctionSummary, AuctionSummaryBatchReadError, AuctionSummaryBatchReader,
};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
pub struct SqlxAuctionSummaryBatchReader {
    pool: PgPool,
}

impl SqlxAuctionSummaryBatchReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl AuctionSummaryBatchReader for SqlxAuctionSummaryBatchReader {
    async fn find_summaries(
        &self,
        auction_ids: &[AuctionId],
    ) -> Result<HashMap<AuctionId, AuctionSummary>, AuctionSummaryBatchReadError> {
        let auction_ids = auction_ids
            .iter()
            .map(|id| id.as_uuid())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if auction_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut connection = self.pool.acquire().await.map_err(|error| {
            AuctionSummaryBatchReadError::QueryFailed {
                source: box_error(error),
            }
        })?;
        let rows = sqlx::query_as::<_, AuctionRow>(
            "SELECT auction_id, listing_source_id, source_auction_id, name_text, name_language, description_text, description_language, catalogue_url, format, reported_status, reported_lot_count, version, created, updated FROM auctions WHERE auction_id = ANY($1)",
        )
        .bind(&auction_ids)
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| AuctionSummaryBatchReadError::QueryFailed {
            source: box_error(error),
        })?;
        let schedule_rows = sqlx::query_as::<_, ScheduleRow>(
            "SELECT auction_id, role, precision, instant_at, date_on, source_timezone FROM auction_schedule_points WHERE auction_id = ANY($1)",
        )
        .bind(&auction_ids)
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| AuctionSummaryBatchReadError::QueryFailed {
            source: box_error(error),
        })?;

        let mut schedules = HashMap::<uuid::Uuid, Vec<AuctionSchedulePointRow>>::new();
        for row in schedule_rows {
            schedules
                .entry(row.auction_id)
                .or_default()
                .push(AuctionSchedulePointRow {
                    role: row.role,
                    precision: row.precision,
                    instant_at: row.instant_at,
                    date_on: row.date_on,
                    source_timezone: row.source_timezone,
                });
        }

        rows.into_iter()
            .map(|row| {
                let schedule = schedules.remove(&row.auction_id).unwrap_or_default();
                let stored = map_stored_auction(row, schedule).map_err(|error| {
                    AuctionSummaryBatchReadError::InvalidReadModel {
                        source: map_error(error),
                    }
                })?;
                let auction = stored.auction;
                Ok((
                    auction.id(),
                    AuctionSummary {
                        auction_id: auction.id(),
                        name: auction.name().cloned(),
                        format: auction.format(),
                        reported_status: auction.reported_status(),
                        schedule: auction.schedule().clone(),
                    },
                ))
            })
            .collect()
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ScheduleRow {
    auction_id: uuid::Uuid,
    role: String,
    precision: String,
    instant_at: Option<time::OffsetDateTime>,
    date_on: Option<time::Date>,
    source_timezone: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use auction_core::AuctionTime;
    use auction_service::ports::AuctionSummaryBatchReader;
    use listing_source_core::ListingSourceId;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
    use time::macros::{date, datetime};

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    async fn source(pool: &PgPool) -> ListingSourceId {
        let source_id = ListingSourceId::new();
        let party_id = uuid::Uuid::now_v7();
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(party_id)
            .bind(format!("party-{party_id}"))
            .bind("Auction source operator")
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert party: {error}"));
        sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
            .bind(source_id.as_uuid())
            .bind(format!("source-{}", source_id.as_uuid()))
            .bind("Auction source")
            .bind(party_id)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert listing source: {error}"));
        source_id
    }

    async fn insert_auction(pool: &PgPool, auction_id: AuctionId, source_id: ListingSourceId) {
        sqlx::query("INSERT INTO auctions (auction_id, listing_source_id, source_auction_id, format, reported_status) VALUES ($1, $2, $3, $4, $5)")
            .bind(auction_id.as_uuid())
            .bind(source_id.as_uuid())
            .bind("catalogue-42")
            .bind("TIMED")
            .bind("SCHEDULED")
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert auction: {error}"));
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_batch_unique_auction_ids_and_reconstruct_exact_and_date_schedule_points() {
        let pool = get_postgres_client().await;
        let source_id = source(&pool).await;
        let auction_id = AuctionId::new();
        insert_auction(&pool, auction_id, source_id).await;
        sqlx::query("INSERT INTO auction_schedule_points (auction_id, role, precision, instant_at, date_on, source_timezone) VALUES ($1, $2, $3, $4, $5, $6), ($1, $7, $8, $9, $10, $11)")
            .bind(auction_id.as_uuid())
            .bind("BIDDING_OPENS")
            .bind("INSTANT")
            .bind(datetime!(2026-10-18 16:00 UTC))
            .bind(Option::<time::Date>::None)
            .bind("Europe/Berlin")
            .bind("SCHEDULED_END")
            .bind("DATE")
            .bind(Option::<time::OffsetDateTime>::None)
            .bind(date!(2026-10-19))
            .bind("Europe/Berlin")
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert schedule: {error}"));

        let summaries = SqlxAuctionSummaryBatchReader::new(pool)
            .find_summaries(&[auction_id, auction_id, AuctionId::new()])
            .await
            .unwrap_or_else(|error| panic!("read summaries: {error}"));

        assert_eq!(1, summaries.len());
        let summary = summaries
            .get(&auction_id)
            .unwrap_or_else(|| panic!("summary must exist"));
        assert_eq!(Some(auction_core::AuctionFormat::Timed), summary.format);
        assert_eq!(
            Some(auction_core::AuctionReportedStatus::Scheduled),
            summary.reported_status
        );
        assert_eq!(
            Some(datetime!(2026-10-18 16:00 UTC)),
            summary
                .schedule
                .bidding_opens()
                .and_then(AuctionTime::exact_instant)
        );
        assert!(matches!(
            summary.schedule.scheduled_end(),
            Some(AuctionTime::Date { on, source_timezone: Some(timezone) })
                if *on == date!(2026-10-19) && timezone.as_str() == "Europe/Berlin"
        ));
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_reject_invalid_persisted_schedule_timezone() {
        let pool = get_postgres_client().await;
        let source_id = source(&pool).await;
        let auction_id = AuctionId::new();
        insert_auction(&pool, auction_id, source_id).await;
        sqlx::query("INSERT INTO auction_schedule_points (auction_id, role, precision, instant_at, source_timezone) VALUES ($1, $2, $3, $4, $5)")
            .bind(auction_id.as_uuid())
            .bind("BIDDING_OPENS")
            .bind("INSTANT")
            .bind(datetime!(2026-10-18 16:00 UTC))
            .bind("Not/AZone")
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert corrupt schedule: {error}"));

        let result = SqlxAuctionSummaryBatchReader::new(pool)
            .find_summaries(&[auction_id])
            .await;

        assert!(matches!(
            result,
            Err(AuctionSummaryBatchReadError::InvalidReadModel { .. })
        ));
    }
}
