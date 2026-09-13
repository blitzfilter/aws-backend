use application::{
    pagination::Cursor,
    transaction::{Transaction, UnitOfWork},
};
use auction_core::AuctionId;
use listing_source_core::ListingSourceId;
use localization::Language;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_postgres::SqlxAuctionCatalogueReaderFactory;
use product_listing_service::ports::{
    AuctionCatalogueCursor, AuctionCatalogueReadRequest, AuctionCatalogueReader,
    AuctionCatalogueReaderFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{OffsetDateTime, macros::datetime};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_only_active_assigned_listings_in_catalogue_position_order_across_pages() {
    let pool = get_postgres_client().await;
    let source_id = seed_listing_source(&pool, "catalogue-order-source").await;
    let auction_id = seed_auction(&pool, source_id, "catalogue-order").await;
    let first = seed_listing(&pool, source_id, "catalogue-first", "First", "ACTIVE").await;
    let duplicate_one = seed_listing(
        &pool,
        source_id,
        "catalogue-duplicate-one",
        "Duplicate one",
        "ACTIVE",
    )
    .await;
    let duplicate_two = seed_listing(
        &pool,
        source_id,
        "catalogue-duplicate-two",
        "Duplicate two",
        "ACTIVE",
    )
    .await;
    let unpositioned = seed_listing(
        &pool,
        source_id,
        "catalogue-unpositioned",
        "Unpositioned",
        "ACTIVE",
    )
    .await;
    let withdrawn = seed_listing(
        &pool,
        source_id,
        "catalogue-withdrawn",
        "Withdrawn",
        "WITHDRAWN",
    )
    .await;

    attach_listing_to_auction(&pool, first, source_id, auction_id, Some(1), None).await;
    attach_listing_to_auction(&pool, duplicate_one, source_id, auction_id, Some(2), None).await;
    attach_listing_to_auction(&pool, duplicate_two, source_id, auction_id, Some(2), None).await;
    attach_listing_to_auction(&pool, unpositioned, source_id, auction_id, None, None).await;
    attach_listing_to_auction(&pool, withdrawn, source_id, auction_id, Some(3), None).await;

    let first_page = read_catalogue(
        &pool,
        auction_id,
        Cursor {
            size: 2,
            search_after: None,
        },
    )
    .await;
    let first_ids = first_page
        .items
        .iter()
        .map(|item| item.item.product_listing_id)
        .collect::<Vec<_>>();
    let mut duplicates = [duplicate_one, duplicate_two];
    duplicates.sort_by_key(|id| *id.as_uuid());
    assert_eq!(vec![first, duplicates[0]], first_ids);

    let second_page = read_catalogue(
        &pool,
        auction_id,
        Cursor {
            size: 2,
            search_after: first_page.cursor.search_after,
        },
    )
    .await;
    let second_ids = second_page
        .items
        .iter()
        .map(|item| item.item.product_listing_id)
        .collect::<Vec<_>>();
    assert_eq!(vec![duplicates[1], unpositioned], second_ids);
    assert!(second_page.cursor.search_after.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_map_full_listing_auction_context_and_referral_url_for_catalogue_item() {
    let pool = get_postgres_client().await;
    let source_id = seed_listing_source(&pool, "catalogue-full-source").await;
    let auction_id = seed_auction(&pool, source_id, "catalogue-full").await;
    sqlx::query(
        "UPDATE listing_sources SET referral_configuration = $1 WHERE listing_source_id = $2",
    )
    .bind(serde_json::json!({"kind": "PARTNERIZE", "camref": "catalogue"}))
    .bind(source_id.into_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to configure referral: {error}"));
    let listing_id = seed_listing(
        &pool,
        source_id,
        "catalogue-full-listing",
        "Selected title",
        "ACTIVE",
    )
    .await;
    let scheduled_close = datetime!(2026-10-18 18:03:00 +02:00);
    attach_listing_to_auction(
        &pool,
        listing_id,
        source_id,
        auction_id,
        Some(43),
        Some(scheduled_close),
    )
    .await;

    let page = read_catalogue(
        &pool,
        auction_id,
        Cursor {
            size: 10,
            search_after: None,
        },
    )
    .await;
    let item = page
        .items
        .first()
        .unwrap_or_else(|| panic!("missing catalogue item"));
    assert_eq!(
        Some("Selected title"),
        item.item.title.as_ref().map(|value| value.payload.as_ref())
    );
    assert_eq!(
        "https://prf.hn/click/camref:catalogue/pubref:aurahistoria/destination:https%3A%2F%2Fexample.com%2Fcatalogue-full-listing",
        item.item.view_url.as_str(),
    );
    let context = item
        .item
        .auction
        .as_ref()
        .unwrap_or_else(|| panic!("missing auction context"));
    assert_eq!(
        Some(auction_id),
        context
            .membership()
            .map(|membership| membership.auction_id())
    );
    assert_eq!(
        Some(43),
        context
            .catalogue_position()
            .map(|position| position.value())
    );
    assert_eq!(
        Some(scheduled_close),
        context
            .timing()
            .and_then(|timing| timing.scheduled_closes())
            .and_then(auction_core::AuctionTime::exact_instant),
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_an_empty_page_for_an_existing_auction_without_active_assigned_listings() {
    let pool = get_postgres_client().await;
    let source_id = seed_listing_source(&pool, "catalogue-empty-source").await;
    let auction_id = seed_auction(&pool, source_id, "catalogue-empty").await;

    let page = read_catalogue(
        &pool,
        auction_id,
        Cursor {
            size: 10,
            search_after: None,
        },
    )
    .await;

    assert!(page.items.is_empty());
    assert!(page.cursor.search_after.is_none());
}

async fn read_catalogue(
    pool: &sqlx::PgPool,
    auction_id: AuctionId,
    cursor: Cursor<AuctionCatalogueCursor>,
) -> product_listing_service::ports::AuctionCataloguePage {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxAuctionCatalogueReaderFactory::new();
    let mut transaction = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin catalogue transaction: {error}"));
    let result = factory
        .in_transaction(&mut transaction)
        .list(&AuctionCatalogueReadRequest {
            auction_id,
            language: Language::En,
            user_id: None,
            cursor,
        })
        .await
        .unwrap_or_else(|error| panic!("failed to read catalogue: {error:?}"));
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("failed to commit catalogue transaction: {error}"));
    result
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
) -> AuctionId {
    let auction_id = AuctionId::new();
    sqlx::query("INSERT INTO auctions (auction_id, listing_source_id, source_auction_id, name_text, name_language, format, reported_status, reported_lot_count) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)")
        .bind(auction_id.as_uuid())
        .bind(source_id.into_uuid())
        .bind(source_auction_id)
        .bind("Auction catalogue")
        .bind("en")
        .bind("TIMED")
        .bind("SCHEDULED")
        .bind(0_i64)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed auction: {error}"));
    auction_id
}

async fn seed_listing(
    pool: &sqlx::PgPool,
    source_id: ListingSourceId,
    source_listing_id: &str,
    title: &str,
    lifecycle: &str,
) -> ProductListingId {
    let listing_id = ProductListingId::new();
    let event_id = uuid::Uuid::now_v7();
    let mut transaction = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin listing fixture transaction: {error}"));
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, price_kind, price_amount, price_currency, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, 'en', 'MONETARY', 100, 'EUR', $7, $8, $9, '[]')",
    )
    .bind(listing_id.as_uuid())
    .bind(format!("{source_listing_id}-000001"))
    .bind(event_id)
    .bind(source_id.into_uuid())
    .bind(source_listing_id)
    .bind(title)
    .bind((lifecycle == "ACTIVE").then_some("AVAILABLE"))
    .bind(lifecycle)
    .bind(format!("https://example.com/{source_listing_id}"))
    .execute(&mut *transaction)
    .await
    .unwrap_or_else(|error| panic!("failed to seed listing: {error}"));
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, '{}', now())")
        .bind(event_id)
        .bind(listing_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to seed listing event: {error}"));
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("failed to commit listing fixture transaction: {error}"));
    listing_id
}

async fn attach_listing_to_auction(
    pool: &sqlx::PgPool,
    listing_id: ProductListingId,
    source_id: ListingSourceId,
    auction_id: AuctionId,
    position: Option<i64>,
    scheduled_close: Option<OffsetDateTime>,
) {
    sqlx::query("INSERT INTO product_listing_auction_contexts (product_listing_id, listing_source_id, auction_id, lot_number, catalogue_position) VALUES ($1, $2, $3, $4, $5)")
        .bind(listing_id.as_uuid())
        .bind(source_id.into_uuid())
        .bind(auction_id.as_uuid())
        .bind("Lot")
        .bind(position)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to attach listing to auction: {error}"));
    if let Some(scheduled_close) = scheduled_close {
        sqlx::query("INSERT INTO product_listing_lot_auction_timings (product_listing_id, scheduled_closes_precision, scheduled_closes_instant_at, scheduled_closes_source_timezone) VALUES ($1, 'INSTANT', $2, 'Europe/Berlin')")
            .bind(listing_id.as_uuid())
            .bind(scheduled_close)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("failed to seed lot timing: {error}"));
    }
}
