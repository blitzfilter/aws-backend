use crate::mapping::{
    AuctionRow, AuctionSchedulePointRow, map_error, map_stored_auction, storage_version_to_i64,
};
use application::error::box_error;
use auction_core::{Auction, AuctionKey, AuctionTime};
use auction_service::ports::{
    AuctionRepository, AuctionRepositoryError, AuctionStorageVersion, StoredAuction,
};
use sqlx::PgConnection;

pub(crate) struct SqlxAuctionRepository<'tx> {
    pub(crate) connection: &'tx mut PgConnection,
}

#[async_trait::async_trait]
impl AuctionRepository for SqlxAuctionRepository<'_> {
    async fn find_by_id(
        &mut self,
        id: auction_core::AuctionId,
    ) -> Result<Option<StoredAuction>, AuctionRepositoryError> {
        let row = sqlx::query_as::<_, AuctionRow>("SELECT auction_id, listing_source_id, source_auction_id, name_text, name_language, description_text, description_language, catalogue_url, format, reported_status, reported_lot_count, version, created, updated FROM auctions WHERE auction_id = $1")
            .bind(id.as_uuid()).fetch_optional(&mut *self.connection).await.map_err(read_error)?;
        load_optional(self.connection, row).await
    }

    async fn find_by_key(
        &mut self,
        key: &AuctionKey,
    ) -> Result<Option<StoredAuction>, AuctionRepositoryError> {
        let row = sqlx::query_as::<_, AuctionRow>("SELECT auction_id, listing_source_id, source_auction_id, name_text, name_language, description_text, description_language, catalogue_url, format, reported_status, reported_lot_count, version, created, updated FROM auctions WHERE listing_source_id = $1 AND source_auction_id = $2")
            .bind(key.listing_source_id().as_uuid()).bind(key.source_auction_id().as_ref()).fetch_optional(&mut *self.connection).await.map_err(read_error)?;
        load_optional(self.connection, row).await
    }

    async fn insert(&mut self, auction: &Auction) -> Result<StoredAuction, AuctionRepositoryError> {
        let row = sqlx::query_as::<_, AuctionRow>("INSERT INTO auctions (auction_id, listing_source_id, source_auction_id, name_text, name_language, description_text, description_language, catalogue_url, format, reported_status, reported_lot_count) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING auction_id, listing_source_id, source_auction_id, name_text, name_language, description_text, description_language, catalogue_url, format, reported_status, reported_lot_count, version, created, updated")
            .bind(auction.id().as_uuid()).bind(auction.key().listing_source_id().as_uuid()).bind(auction.key().source_auction_id().as_ref())
            .bind(auction.name().map(|value| value.payload.as_ref())).bind(auction.name().map(|value| value.localization.as_str()))
            .bind(auction.description().map(|value| value.payload.as_ref())).bind(auction.description().map(|value| value.localization.as_str()))
            .bind(auction.catalogue_url().map(url::Url::as_str)).bind(auction.format().map(|value| value.as_str()))
            .bind(auction.reported_status().map(|value| value.as_str())).bind(auction.reported_lot_count().map(|value| i64::from(value.value())))
            .fetch_one(&mut *self.connection).await.map_err(write_error)?;
        replace_schedule(self.connection, auction).await?;
        load(self.connection, row).await
    }

    async fn update(
        &mut self,
        auction: &Auction,
        expected_version: AuctionStorageVersion,
    ) -> Result<StoredAuction, AuctionRepositoryError> {
        let expected_version = storage_version_to_i64(expected_version).map_err(|error| {
            AuctionRepositoryError::InvalidPersistedState {
                source: map_error(error),
            }
        })?;
        let row = sqlx::query_as::<_, AuctionRow>("UPDATE auctions SET name_text=$1, name_language=$2, description_text=$3, description_language=$4, catalogue_url=$5, format=$6, reported_status=$7, reported_lot_count=$8, version=version+1, updated=now() WHERE auction_id=$9 AND version=$10 RETURNING auction_id, listing_source_id, source_auction_id, name_text, name_language, description_text, description_language, catalogue_url, format, reported_status, reported_lot_count, version, created, updated")
            .bind(auction.name().map(|value| value.payload.as_ref())).bind(auction.name().map(|value| value.localization.as_str()))
            .bind(auction.description().map(|value| value.payload.as_ref())).bind(auction.description().map(|value| value.localization.as_str()))
            .bind(auction.catalogue_url().map(url::Url::as_str)).bind(auction.format().map(|value| value.as_str()))
            .bind(auction.reported_status().map(|value| value.as_str())).bind(auction.reported_lot_count().map(|value| i64::from(value.value())))
            .bind(auction.id().as_uuid()).bind(expected_version)
            .fetch_optional(&mut *self.connection).await.map_err(write_error)?.ok_or(AuctionRepositoryError::ConcurrencyConflict)?;
        replace_schedule(self.connection, auction).await?;
        load(self.connection, row).await
    }
}

async fn load_optional(
    connection: &mut PgConnection,
    row: Option<AuctionRow>,
) -> Result<Option<StoredAuction>, AuctionRepositoryError> {
    match row {
        Some(row) => load(connection, row).await.map(Some),
        None => Ok(None),
    }
}

pub(crate) async fn load(
    connection: &mut PgConnection,
    row: AuctionRow,
) -> Result<StoredAuction, AuctionRepositoryError> {
    let schedule = schedule_rows(connection, row.auction_id).await?;
    map_stored_auction(row, schedule).map_err(|error| {
        AuctionRepositoryError::InvalidPersistedState {
            source: map_error(error),
        }
    })
}

pub(crate) async fn schedule_rows(
    connection: &mut PgConnection,
    auction_id: uuid::Uuid,
) -> Result<Vec<AuctionSchedulePointRow>, AuctionRepositoryError> {
    sqlx::query_as::<_, AuctionSchedulePointRow>("SELECT role, precision, instant_at, date_on, source_timezone FROM auction_schedule_points WHERE auction_id=$1")
        .bind(auction_id).fetch_all(&mut *connection).await.map_err(read_error)
}

async fn replace_schedule(
    connection: &mut PgConnection,
    auction: &Auction,
) -> Result<(), AuctionRepositoryError> {
    sqlx::query("DELETE FROM auction_schedule_points WHERE auction_id=$1")
        .bind(auction.id().as_uuid())
        .execute(&mut *connection)
        .await
        .map_err(write_error)?;
    for (role, value) in [
        ("BIDDING_OPENS", auction.schedule().bidding_opens()),
        ("LIVE_STARTS", auction.schedule().live_starts()),
        (
            "LOTS_BEGIN_CLOSING",
            auction.schedule().lots_begin_closing(),
        ),
        ("SCHEDULED_END", auction.schedule().scheduled_end()),
    ] {
        if let Some(value) = value {
            let (precision, instant_at, date_on) = match value {
                AuctionTime::Instant { at, .. } => ("INSTANT", Some(*at), None),
                AuctionTime::Date { on, .. } => ("DATE", None, Some(*on)),
            };
            sqlx::query("INSERT INTO auction_schedule_points (auction_id, role, precision, instant_at, date_on, source_timezone) VALUES ($1,$2,$3,$4,$5,$6)")
                .bind(auction.id().as_uuid()).bind(role).bind(precision).bind(instant_at).bind(date_on).bind(value.source_timezone().map(|zone| zone.as_str()))
                .execute(&mut *connection).await.map_err(write_error)?;
        }
    }
    Ok(())
}

fn read_error(error: sqlx::Error) -> AuctionRepositoryError {
    AuctionRepositoryError::TemporarilyUnavailable {
        source: box_error(error),
    }
}
fn write_error(error: sqlx::Error) -> AuctionRepositoryError {
    match &error {
        sqlx::Error::Database(database)
            if database.constraint() == Some("auctions_source_key_unique") =>
        {
            AuctionRepositoryError::SourceAuctionAlreadyExists {
                source: box_error(error),
            }
        }
        sqlx::Error::Database(database)
            if database.constraint() == Some("auctions_listing_source_id_fkey") =>
        {
            AuctionRepositoryError::ListingSourceNotFound {
                source: box_error(error),
            }
        }
        _ => AuctionRepositoryError::Internal {
            source: box_error(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        SqlxAuctionEventAppenderFactory, SqlxAuctionMetadataPolicyRepositoryFactory,
        SqlxAuctionRepositoryFactory,
    };
    use application::transaction::{Transaction, UnitOfWork};
    use auction_core::{
        AuctionFormat, AuctionId, AuctionKey, AuctionSchedule, AuctionTime, NewAuction,
        SourceAuctionId,
    };
    use auction_service::ports::{
        AuctionEventAppender, AuctionEventAppenderFactory, AuctionMetadataField,
        AuctionMetadataPolicyAudit, AuctionMetadataPolicyRepository,
        AuctionMetadataPolicyRepositoryFactory, AuctionRepositoryFactory, stamp_auction_event,
    };
    use domain_primitives::event_id::EventId;
    use listing_source_core::ListingSourceId;
    use std::collections::BTreeSet;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
    use time::macros::datetime;

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    async fn source(pool: &sqlx::PgPool) -> ListingSourceId {
        let source_id = ListingSourceId::new();
        let party_id = uuid::Uuid::now_v7();
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1,$2,$3)")
            .bind(party_id)
            .bind(format!("party-{party_id}"))
            .bind("Auction source operator")
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert party: {error}"));
        sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1,$2,$3,$4)")
            .bind(source_id.as_uuid()).bind(format!("source-{}", source_id.as_uuid())).bind("Auction source").bind(party_id)
            .execute(pool).await.unwrap_or_else(|error| panic!("insert source: {error}"));
        source_id
    }

    fn auction(source_id: ListingSourceId) -> Auction {
        Auction::create(NewAuction {
            id: AuctionId::new(),
            key: AuctionKey::new(
                source_id,
                SourceAuctionId::try_from("catalogue-42")
                    .unwrap_or_else(|error| panic!("source key: {error}")),
            ),
            name: None,
            description: None,
            catalogue_url: None,
            format: Some(AuctionFormat::Timed),
            schedule: AuctionSchedule::new(
                Some(AuctionTime::instant(datetime!(2026-10-18 16:00 UTC), None)),
                None,
                None,
                None,
            )
            .unwrap_or_else(|error| panic!("schedule: {error}")),
            reported_status: None,
            reported_lot_count: None,
        })
        .unwrap_or_else(|error| panic!("auction: {error}"))
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_persist_schedule_event_and_field_protection_atomically() {
        let pool = get_postgres_client().await;
        let source_id = source(&pool).await;
        let mut auction = auction(source_id);
        let payload = auction
            .take_pending_event_payload()
            .unwrap_or_else(|| panic!("discovery event"));
        let event = stamp_auction_event(auction.id(), datetime!(2026-01-01 00:00 UTC), payload);
        let unit_of_work = platform_postgres::SqlxUnitOfWork::new(pool.clone());
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin: {error}"));
        let stored = SqlxAuctionRepositoryFactory::new()
            .in_transaction(&mut tx)
            .insert(&auction)
            .await
            .unwrap_or_else(|error| panic!("insert: {error}"));
        SqlxAuctionEventAppenderFactory::new()
            .in_transaction(&mut tx)
            .append(&event)
            .await
            .unwrap_or_else(|error| panic!("event: {error}"));
        SqlxAuctionMetadataPolicyRepositoryFactory::new()
            .in_transaction(&mut tx)
            .protect(&AuctionMetadataPolicyAudit {
                audit_id: EventId::new(),
                auction_id: auction.id(),
                actor_label: "SYSTEM".to_owned(),
                recorded_at: datetime!(2026-01-01 00:00 UTC),
                fields: BTreeSet::from([AuctionMetadataField::Format]),
            })
            .await
            .unwrap_or_else(|error| panic!("policy: {error}"));
        tx.commit()
            .await
            .unwrap_or_else(|error| panic!("commit: {error}"));
        assert_eq!(Some(AuctionFormat::Timed), stored.auction.format());
        assert_eq!(
            Some(datetime!(2026-10-18 16:00 UTC)),
            stored
                .auction
                .schedule()
                .bidding_opens()
                .and_then(AuctionTime::exact_instant)
        );
        let events: i64 =
            sqlx::query_scalar("SELECT count(*) FROM auction_events WHERE auction_id=$1")
                .bind(auction.id().as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|error| panic!("events: {error}"));
        let protected: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM auction_metadata_field_protections WHERE auction_id=$1",
        )
        .bind(auction.id().as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("protection: {error}"));
        assert_eq!(1, events);
        assert_eq!(1, protected);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_replace_schedule_with_root_update_and_roll_back_uncommitted_write() {
        let pool = get_postgres_client().await;
        let source_id = source(&pool).await;
        let persisted_auction = auction(source_id);
        let unit_of_work = platform_postgres::SqlxUnitOfWork::new(pool.clone());

        let mut insert_tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin insert: {error}"));
        let stored = SqlxAuctionRepositoryFactory::new()
            .in_transaction(&mut insert_tx)
            .insert(&persisted_auction)
            .await
            .unwrap_or_else(|error| panic!("insert: {error}"));
        insert_tx
            .commit()
            .await
            .unwrap_or_else(|error| panic!("commit insert: {error}"));

        let mut changed = stored.auction.clone();
        changed.clear_format();
        changed
            .replace_schedule(AuctionSchedule::default())
            .unwrap_or_else(|error| panic!("replace schedule: {error}"));
        let mut update_tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin update: {error}"));
        let updated = SqlxAuctionRepositoryFactory::new()
            .in_transaction(&mut update_tx)
            .update(&changed, stored.version)
            .await
            .unwrap_or_else(|error| panic!("update: {error}"));
        update_tx
            .commit()
            .await
            .unwrap_or_else(|error| panic!("commit update: {error}"));
        assert_eq!(stored.version.next(), updated.version);
        assert_eq!(None, updated.auction.format());
        let schedule_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM auction_schedule_points WHERE auction_id=$1")
                .bind(updated.auction.id().as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|error| panic!("count replaced schedule: {error}"));
        assert_eq!(0, schedule_count);

        let mut rolled_back = auction(source(&pool).await);
        let rollback_payload = rolled_back
            .take_pending_event_payload()
            .unwrap_or_else(|| panic!("rollback discovery event"));
        let rollback_event = stamp_auction_event(
            rolled_back.id(),
            datetime!(2026-01-01 00:00 UTC),
            rollback_payload,
        );
        let mut rollback_tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin rollback: {error}"));
        SqlxAuctionRepositoryFactory::new()
            .in_transaction(&mut rollback_tx)
            .insert(&rolled_back)
            .await
            .unwrap_or_else(|error| panic!("insert rollback auction: {error}"));
        SqlxAuctionEventAppenderFactory::new()
            .in_transaction(&mut rollback_tx)
            .append(&rollback_event)
            .await
            .unwrap_or_else(|error| panic!("append rollback event: {error}"));
        SqlxAuctionMetadataPolicyRepositoryFactory::new()
            .in_transaction(&mut rollback_tx)
            .protect(&AuctionMetadataPolicyAudit {
                audit_id: EventId::new(),
                auction_id: rolled_back.id(),
                actor_label: "SYSTEM".to_owned(),
                recorded_at: datetime!(2026-01-01 00:00 UTC),
                fields: BTreeSet::from([AuctionMetadataField::Format]),
            })
            .await
            .unwrap_or_else(|error| panic!("protect rollback policy: {error}"));
        drop(rollback_tx);
        let rolled_back_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM auctions WHERE auction_id=$1")
                .bind(rolled_back.id().as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|error| panic!("count rolled back auction: {error}"));
        assert_eq!(0, rolled_back_count);
        let rolled_back_events: i64 =
            sqlx::query_scalar("SELECT count(*) FROM auction_events WHERE auction_id=$1")
                .bind(rolled_back.id().as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|error| panic!("count rolled back events: {error}"));
        let rolled_back_protections: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM auction_metadata_field_protections WHERE auction_id=$1",
        )
        .bind(rolled_back.id().as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("count rolled back protections: {error}"));
        assert_eq!(0, rolled_back_events);
        assert_eq!(0, rolled_back_protections);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_enforce_source_key_uniqueness_and_root_cas() {
        let pool = get_postgres_client().await;
        let source_id = source(&pool).await;
        let auction = auction(source_id);
        let unit_of_work = platform_postgres::SqlxUnitOfWork::new(pool.clone());
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin: {error}"));
        let stored = SqlxAuctionRepositoryFactory::new()
            .in_transaction(&mut tx)
            .insert(&auction)
            .await
            .unwrap_or_else(|error| panic!("insert: {error}"));
        tx.commit()
            .await
            .unwrap_or_else(|error| panic!("commit: {error}"));
        let duplicate = Auction::create(NewAuction {
            id: AuctionId::new(),
            key: auction.key().clone(),
            name: None,
            description: None,
            catalogue_url: None,
            format: None,
            schedule: AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
        })
        .unwrap_or_else(|error| panic!("duplicate: {error}"));
        let mut duplicate_tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin duplicate: {error}"));
        assert!(matches!(
            SqlxAuctionRepositoryFactory::new()
                .in_transaction(&mut duplicate_tx)
                .insert(&duplicate)
                .await,
            Err(AuctionRepositoryError::SourceAuctionAlreadyExists { .. })
        ));
        drop(duplicate_tx);
        let mut stale_tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin stale: {error}"));
        assert!(matches!(
            SqlxAuctionRepositoryFactory::new()
                .in_transaction(&mut stale_tx)
                .update(
                    &stored.auction,
                    AuctionStorageVersion::try_from(99_i64)
                        .unwrap_or_else(|error| panic!("version: {error}"))
                )
                .await,
            Err(AuctionRepositoryError::ConcurrencyConflict)
        ));
    }
}
