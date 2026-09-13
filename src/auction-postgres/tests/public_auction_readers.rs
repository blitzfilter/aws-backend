use application::pagination::Cursor;
use auction_core::{AuctionId, AuctionSchedulePoint};
use auction_postgres::{SqlxAuctionDirectoryReader, SqlxPublicAuctionDetailsReader};
use auction_service::ports::{
    AuctionDirectoryReader, AuctionInstantScheduleFilter, ListAuctionsDirectoryRequest,
    PublicAuctionDetailsReader,
};
use domain_primitives::query::range_query::RangeQuery;
use listing_source_core::ListingSourceId;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Date, Month, OffsetDateTime, macros::datetime};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_filter_directory_by_exact_schedule_role_with_half_open_bounds_and_exclude_dates() {
    let pool = get_postgres_client().await;
    let source_id = seed_listing_source(&pool, "directory-schedule-source").await;
    let from = datetime!(2026-10-18 16:00 UTC);
    let to = datetime!(2026-10-18 17:00 UTC);
    let included = seed_auction(&pool, source_id, "directory-included", 0).await;
    let excluded_upper = seed_auction(&pool, source_id, "directory-upper", 0).await;
    let wrong_role = seed_auction(&pool, source_id, "directory-wrong-role", 0).await;
    let date_only = seed_auction(&pool, source_id, "directory-date-only", 0).await;
    seed_instant_schedule(&pool, included, "LIVE_STARTS", from).await;
    seed_instant_schedule(&pool, excluded_upper, "LIVE_STARTS", to).await;
    seed_instant_schedule(&pool, wrong_role, "SCHEDULED_END", from).await;
    seed_date_schedule(&pool, date_only, "LIVE_STARTS").await;

    let reader = SqlxAuctionDirectoryReader::new(pool);
    let page = reader
        .list(&ListAuctionsDirectoryRequest {
            schedule: Some(AuctionInstantScheduleFilter {
                role: AuctionSchedulePoint::LiveStarts,
                range: RangeQuery {
                    min: Some(from),
                    max: Some(to),
                },
            }),
            cursor: Some(Cursor {
                size: 10,
                search_after: None,
            }),
            ..Default::default()
        })
        .await
        .unwrap_or_else(|error| panic!("failed to list auction directory: {error:?}"));

    assert_eq!(
        vec![included],
        page.items
            .into_iter()
            .map(|item| item.auction_id)
            .collect::<Vec<_>>()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_reported_lot_count_separately_from_visible_active_assigned_listing_count() {
    let pool = get_postgres_client().await;
    let source_id = seed_listing_source(&pool, "details-count-source").await;
    let auction_id = seed_auction(&pool, source_id, "details-count", 7).await;
    seed_assigned_listing(&pool, source_id, auction_id, "details-active", "ACTIVE").await;
    seed_assigned_listing(
        &pool,
        source_id,
        auction_id,
        "details-withdrawn",
        "WITHDRAWN",
    )
    .await;

    let details = SqlxPublicAuctionDetailsReader::new(pool)
        .find_by_id(auction_id)
        .await
        .unwrap_or_else(|error| panic!("failed to read public auction details: {error:?}"))
        .unwrap_or_else(|| panic!("missing seeded auction"));

    assert_eq!(
        Some(7),
        details.reported_lot_count.map(|count| count.value())
    );
    assert_eq!(1, details.visible_active_assigned_listing_count);
    assert_eq!(auction_id, details.auction_id);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_none_for_missing_public_auction_details() {
    let pool = get_postgres_client().await;

    let details = SqlxPublicAuctionDetailsReader::new(pool)
        .find_by_id(AuctionId::new())
        .await
        .unwrap_or_else(|error| panic!("failed to query missing public auction: {error:?}"));

    assert!(details.is_none());
}

async fn seed_listing_source(pool: &sqlx::PgPool, slug: &str) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    let party_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed listing-source party: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(listing_source_id.into_uuid())
        .bind(slug)
        .bind(slug)
        .bind(party_id)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed listing source: {error}"));
    listing_source_id
}

async fn seed_auction(
    pool: &sqlx::PgPool,
    source_id: ListingSourceId,
    source_auction_id: &str,
    reported_lot_count: i64,
) -> AuctionId {
    let auction_id = AuctionId::new();
    sqlx::query("INSERT INTO auctions (auction_id, listing_source_id, source_auction_id, name_text, name_language, format, reported_status, reported_lot_count) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)")
        .bind(auction_id.as_uuid())
        .bind(source_id.into_uuid())
        .bind(source_auction_id)
        .bind("Public auction")
        .bind("en")
        .bind("TIMED")
        .bind("SCHEDULED")
        .bind(reported_lot_count)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed auction: {error}"));
    auction_id
}

async fn seed_instant_schedule(
    pool: &sqlx::PgPool,
    auction_id: AuctionId,
    role: &str,
    instant: OffsetDateTime,
) {
    sqlx::query("INSERT INTO auction_schedule_points (auction_id, role, precision, instant_at) VALUES ($1, $2, 'INSTANT', $3)")
        .bind(auction_id.as_uuid())
        .bind(role)
        .bind(instant)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed instant schedule: {error}"));
}

async fn seed_date_schedule(pool: &sqlx::PgPool, auction_id: AuctionId, role: &str) {
    let date = Date::from_calendar_date(2026, Month::October, 18)
        .unwrap_or_else(|error| panic!("invalid fixture date: {error}"));
    sqlx::query("INSERT INTO auction_schedule_points (auction_id, role, precision, date_on) VALUES ($1, $2, 'DATE', $3)")
        .bind(auction_id.as_uuid())
        .bind(role)
        .bind(date)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed date schedule: {error}"));
}

async fn seed_assigned_listing(
    pool: &sqlx::PgPool,
    source_id: ListingSourceId,
    auction_id: AuctionId,
    source_listing_id: &str,
    lifecycle: &str,
) {
    let listing_id = uuid::Uuid::now_v7();
    let event_id = uuid::Uuid::now_v7();
    let mut transaction = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin listing fixture transaction: {error}"));
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, $7, $8, '[]')",
    )
    .bind(listing_id)
    .bind(format!("{source_listing_id}-000001"))
    .bind(event_id)
    .bind(source_id.into_uuid())
    .bind(source_listing_id)
    .bind((lifecycle == "ACTIVE").then_some("AVAILABLE"))
    .bind(lifecycle)
    .bind(format!("https://example.com/{source_listing_id}"))
    .execute(&mut *transaction)
    .await
    .unwrap_or_else(|error| panic!("failed to seed assigned listing: {error}"));
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, '{}', now())")
        .bind(event_id)
        .bind(listing_id)
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to seed listing event: {error}"));
    sqlx::query("INSERT INTO product_listing_auction_contexts (product_listing_id, listing_source_id, auction_id) VALUES ($1, $2, $3)")
        .bind(listing_id)
        .bind(source_id.into_uuid())
        .bind(auction_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to attach listing to auction: {error}"));
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("failed to commit listing fixture transaction: {error}"));
}
