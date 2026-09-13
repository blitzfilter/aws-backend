use crate::mapping::{AuctionRow, AuctionSchedulePointRow, map_error, map_stored_auction};
use application::{
    error::{BoxError, box_error},
    pagination::{Cursor, CursoredResult},
};
use auction_core::AuctionSchedulePoint;
use auction_service::ports::{
    AuctionDirectoryCursor, AuctionDirectoryReadError, AuctionDirectoryReader,
    ListAuctionsDirectoryRequest, ListAuctionsDirectoryResult, PublicAuctionDirectoryItem,
    PublicAuctionDirectorySourceSummary,
};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use sqlx::{PgPool, Postgres, QueryBuilder};
use std::collections::HashMap;

#[derive(Clone)]
pub struct SqlxAuctionDirectoryReader {
    pool: PgPool,
}

impl SqlxAuctionDirectoryReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct AuctionDirectoryRow {
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

#[async_trait::async_trait]
impl AuctionDirectoryReader for SqlxAuctionDirectoryReader {
    async fn list(
        &self,
        request: &ListAuctionsDirectoryRequest,
    ) -> Result<ListAuctionsDirectoryResult, AuctionDirectoryReadError> {
        let cursor = request.cursor.clone().unwrap_or_default();
        let size = cursor.size.clamp(1, 100);
        let size_usize = usize::try_from(size).map_err(invalid_read_model)?;
        let limit = i64::try_from(size + 1).map_err(invalid_read_model)?;

        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT a.auction_id, a.listing_source_id, a.source_auction_id, a.name_text, a.name_language, a.description_text, a.description_language, a.catalogue_url, a.format, a.reported_status, a.reported_lot_count, a.version, a.created, a.updated, s.listing_source_slug_id, s.name AS listing_source_name FROM auctions a JOIN listing_sources s ON s.listing_source_id = a.listing_source_id",
        );
        if request.schedule.is_some() {
            builder.push(" JOIN auction_schedule_points schedule_filter ON schedule_filter.auction_id = a.auction_id");
        }
        builder.push(" WHERE TRUE");
        push_filters(&mut builder, request)?;
        if let Some(search_after) = cursor.search_after {
            builder
                .push(" AND (a.created, a.auction_id) < (")
                .push_bind(search_after.created)
                .push(", ")
                .push_bind(search_after.auction_id.as_uuid())
                .push(")");
        }
        builder
            .push(" ORDER BY a.created DESC, a.auction_id DESC LIMIT ")
            .push_bind(limit);

        let mut connection = self.pool.acquire().await.map_err(query_error)?;
        let mut rows = builder
            .build_query_as::<AuctionDirectoryRow>()
            .fetch_all(&mut *connection)
            .await
            .map_err(query_error)?;
        let has_more = rows.len() > size_usize;
        if has_more {
            rows.truncate(size_usize);
        }
        if rows.is_empty() {
            return Ok(CursoredResult {
                items: Vec::new(),
                cursor: Cursor {
                    size,
                    search_after: None,
                },
                total: None,
            });
        }

        let auction_ids = rows.iter().map(|row| row.auction_id).collect::<Vec<_>>();
        let schedule_rows = sqlx::query_as::<_, ScheduleRow>(
            "SELECT auction_id, role, precision, instant_at, date_on, source_timezone FROM auction_schedule_points WHERE auction_id = ANY($1)",
        )
        .bind(&auction_ids)
        .fetch_all(&mut *connection)
        .await
        .map_err(query_error)?;
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

        let items = rows
            .into_iter()
            .map(|row| {
                let schedule_rows = schedules.remove(&row.auction_id).unwrap_or_default();
                map_directory_item(row, schedule_rows)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| AuctionDirectoryReadError::InvalidReadModel { source })?;
        let search_after = has_more.then(|| {
            let item = &items[items.len() - 1];
            AuctionDirectoryCursor {
                created: item.created,
                auction_id: item.auction_id,
                scope: request.scope(),
            }
        });

        Ok(CursoredResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn push_filters(
    builder: &mut QueryBuilder<Postgres>,
    request: &ListAuctionsDirectoryRequest,
) -> Result<(), AuctionDirectoryReadError> {
    if let Some(listing_source_id) = request.listing_source_id {
        builder
            .push(" AND a.listing_source_id = ")
            .push_bind(listing_source_id.as_uuid());
    }
    if let Some(format) = request.format {
        builder.push(" AND a.format = ").push_bind(format.as_str());
    }
    if let Some(reported_status) = request.reported_status {
        builder
            .push(" AND a.reported_status = ")
            .push_bind(reported_status.as_str());
    }
    if let Some(schedule) = request.schedule.as_ref() {
        let (Some(min), Some(max)) = (schedule.range.min, schedule.range.max) else {
            return Err(AuctionDirectoryReadError::InvalidReadModel {
                source: box_error(std::io::Error::other(
                    "Auction directory schedule filter has incomplete exact-instant range",
                )),
            });
        };
        builder
            .push(" AND schedule_filter.role = ")
            .push_bind(schedule_role_code(schedule.role))
            .push(" AND schedule_filter.precision = 'INSTANT' AND schedule_filter.instant_at >= ")
            .push_bind(min)
            .push(" AND schedule_filter.instant_at < ")
            .push_bind(max);
    }
    Ok(())
}

fn schedule_role_code(role: AuctionSchedulePoint) -> &'static str {
    match role {
        AuctionSchedulePoint::BiddingOpens => "BIDDING_OPENS",
        AuctionSchedulePoint::LiveStarts => "LIVE_STARTS",
        AuctionSchedulePoint::LotsBeginClosing => "LOTS_BEGIN_CLOSING",
        AuctionSchedulePoint::ScheduledEnd => "SCHEDULED_END",
    }
}

fn map_directory_item(
    row: AuctionDirectoryRow,
    schedule_rows: Vec<AuctionSchedulePointRow>,
) -> Result<PublicAuctionDirectoryItem, BoxError> {
    let source_id = ListingSourceId::try_from(row.listing_source_id).map_err(box_error)?;
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
    .map_err(map_error)?;
    if stored.auction.key().listing_source_id() != source_id {
        return Err(box_error(std::io::Error::other(
            "persisted Auction source does not match its joined ListingSource",
        )));
    }
    let source = PublicAuctionDirectorySourceSummary {
        listing_source_id: source_id,
        slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id).map_err(box_error)?,
        name: ListingSourceName::try_from(row.listing_source_name).map_err(box_error)?,
    };
    let auction = stored.auction;
    Ok(PublicAuctionDirectoryItem {
        auction_id: auction.id(),
        source,
        name: auction.name().cloned(),
        format: auction.format(),
        schedule: auction.schedule().clone(),
        reported_status: auction.reported_status(),
        created: stored.created,
    })
}

fn query_error(error: sqlx::Error) -> AuctionDirectoryReadError {
    AuctionDirectoryReadError::QueryFailed {
        source: box_error(error),
    }
}

fn invalid_read_model(
    error: impl std::error::Error + Send + Sync + 'static,
) -> AuctionDirectoryReadError {
    AuctionDirectoryReadError::InvalidReadModel {
        source: box_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_exact_persisted_schedule_role_codes() {
        assert_eq!(
            "BIDDING_OPENS",
            schedule_role_code(AuctionSchedulePoint::BiddingOpens)
        );
        assert_eq!(
            "LIVE_STARTS",
            schedule_role_code(AuctionSchedulePoint::LiveStarts)
        );
        assert_eq!(
            "LOTS_BEGIN_CLOSING",
            schedule_role_code(AuctionSchedulePoint::LotsBeginClosing)
        );
        assert_eq!(
            "SCHEDULED_END",
            schedule_role_code(AuctionSchedulePoint::ScheduledEnd)
        );
    }
}
