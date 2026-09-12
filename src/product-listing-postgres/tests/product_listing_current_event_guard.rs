mod support;

use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_postgres::SqlxProductListingCurrentEventGuardFactory;
use product_listing_service::ports::{
    ProductListingCurrentEventCheck, ProductListingCurrentEventGuard,
    ProductListingCurrentEventGuardFactory, ProductListingCurrentEventRef,
};
use std::time::Duration;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tokio::sync::oneshot;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_block_product_current_event_update_until_current_event_guard_transaction_commits() {
    let result = current_event_guard_lock_flow().await;
    assert!(
        result.is_ok(),
        "current event guard lock integration test failed: {result:?}"
    );
}

async fn current_event_guard_lock_flow() -> Result<(), Box<dyn std::error::Error>> {
    let pool = get_postgres_client().await;
    let (product_listing_id, current_event_id) = seed_product(&pool).await?;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let mut guard_transaction = unit_of_work.begin().await?;

    let current_ref = ProductListingCurrentEventRef {
        product_listing_id,
        expected_event_id: current_event_id,
    };
    let stale_ref = ProductListingCurrentEventRef {
        product_listing_id,
        expected_event_id: EventId::new(),
    };
    let current_events = SqlxProductListingCurrentEventGuardFactory::new()
        .in_transaction(&mut guard_transaction)
        .lock_and_check_all(&[current_ref, stale_ref])
        .await?;
    assert_eq!(
        Some(&ProductListingCurrentEventCheck::Current),
        current_events.get(&current_ref)
    );
    assert_eq!(
        Some(&ProductListingCurrentEventCheck::Stale),
        current_events.get(&stale_ref)
    );

    let next_event_id = EventId::new();
    sqlx::query(
        "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())",
    )
    .bind(next_event_id.into_uuid())
    .bind(product_listing_id.into_uuid())
    .bind(serde_json::json!({
        "availability": {"previous": "AVAILABLE", "current": "SOLD_OUT"}
    }))
    .execute(&pool)
    .await?;
    let (update_started_tx, update_started_rx) = oneshot::channel();
    let update_pool = pool.clone();
    let mut update = tokio::spawn(async move {
        let _ = update_started_tx.send(());
        sqlx::query(
            "UPDATE product_listings SET current_event_id = $1, availability = 'SOLD_OUT', version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2",
        )
        .bind(next_event_id.into_uuid())
        .bind(product_listing_id.into_uuid())
        .execute(&update_pool)
        .await
    });
    update_started_rx.await?;

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut update)
            .await
            .is_err(),
        "a ProductListing update committed while the current event guard share lock was held"
    );

    guard_transaction.commit().await?;
    let update_result: Result<sqlx::postgres::PgQueryResult, sqlx::Error> = update.await?;
    update_result?;
    Ok(())
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_recheck_current_event_after_waiting_for_concurrent_product_commit() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_id, original) = seed_product(&pool).await?;
        let newer = EventId::new();
        let mut update = pool.begin().await?;
        let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *update).await?;
        sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
            .bind(newer.into_uuid()).bind(product_id.into_uuid())
            .bind(serde_json::json!({"availability": {"previous": "AVAILABLE", "current": "SOLD_OUT"}}))
            .execute(&mut *update).await?;
        sqlx::query("UPDATE product_listings SET current_event_id = $1, availability = 'SOLD_OUT', version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2")
            .bind(newer.into_uuid()).bind(product_id.into_uuid())
            .execute(&mut *update).await?;
        let final_guard = async {
            let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
            let result = SqlxProductListingCurrentEventGuardFactory::new().in_transaction(&mut tx)
                .lock_and_check(product_id, original).await?;
            tx.commit().await?;
            Ok::<_, Box<dyn std::error::Error>>(result)
        };
        tokio::pin!(final_guard);
        support::assert_blocked(&pool, blocker_pid, 1, final_guard.as_mut()).await?;
        update.commit().await?;
        assert_eq!(ProductListingCurrentEventCheck::Stale,
            tokio::time::timeout(Duration::from_secs(10), final_guard).await??);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "final current-event revalidation: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_reversed_historical_watchlist_and_match_facts_after_newer_event() {
    use product_listing_postgres::{
        SqlxProductListingSearchFilterMatchSourceReaderFactory,
        SqlxProductListingWatchlistNotificationSourceReaderFactory,
    };
    use product_listing_service::ports::{
        ProductListingSearchFilterMatchSourceReader,
        ProductListingSearchFilterMatchSourceReaderFactory,
        ProductListingWatchlistNotificationSourceReadOutcome,
        ProductListingWatchlistNotificationSourceReader,
        ProductListingWatchlistNotificationSourceReaderFactory,
    };
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_id, discovery) = seed_product(&pool).await?;
        let first = EventId::new();
        let second = EventId::new();
        for (event, previous, current) in [(first, "AVAILABLE", "RESERVED"), (second, "RESERVED", "SOLD_OUT")] {
            sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
                .bind(event.into_uuid()).bind(product_id.into_uuid())
                .bind(serde_json::json!({"availability": {"previous": previous, "current": current}}))
                .execute(&pool).await?;
        }
        let newer = EventId::new();
        let mut update = pool.begin().await?;
        sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'ENRICHMENT_EMBEDDED', 'ENRICHMENT', 1, $3, now())")
            .bind(newer.into_uuid()).bind(product_id.into_uuid())
            .bind(serde_json::json!({"sourceEventId": discovery.as_uuid().to_string()})).execute(&mut *update).await?;
        sqlx::query("UPDATE product_listings SET current_event_id = $1, projection_version = projection_version + 1 WHERE product_listing_id = $2")
            .bind(newer.into_uuid()).bind(product_id.into_uuid()).execute(&mut *update).await?;
        update.commit().await?;
        let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(tx.connection()).await?;
        for event in [second, first, first] {
            let source = SqlxProductListingWatchlistNotificationSourceReaderFactory::new().in_transaction(&mut tx)
                .find_source(event, product_id).await?;
            let ProductListingWatchlistNotificationSourceReadOutcome::Found(source) = source else {
                return Err(std::io::Error::other("historical watchlist fact suppressed").into());
            };
            assert_eq!(event, source.event_id);
            assert_eq!(1, source.changes.len());
            let match_source = SqlxProductListingSearchFilterMatchSourceReaderFactory::new().in_transaction(&mut tx)
                .find_source(event, product_id).await?.ok_or_else(|| std::io::Error::other("historical match product source suppressed"))?;
            assert_eq!(event, match_source.event_id);
            assert_eq!(newer, match_source.current_event_id);
        }
        let withdraw = sqlx::query("UPDATE product_listings SET lifecycle = 'WITHDRAWN', availability = NULL WHERE product_listing_id = $1")
            .bind(product_id.into_uuid()).execute(&pool);
        tokio::pin!(withdraw);
        support::assert_blocked(&pool, blocker_pid, 1, withdraw.as_mut()).await?;
        tx.commit().await?;
        tokio::time::timeout(Duration::from_secs(10), withdraw).await??;
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "historical facts and lifecycle lock: {result:?}"
    );
}

async fn seed_product(pool: &sqlx::PgPool) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = uuid::Uuid::now_v7();
    let product_uuid = product_listing_id.into_uuid();
    let slug_suffix = product_uuid.simple().to_string()[26..].to_owned();
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Current event guard party')",
    )
    .bind(party_id)
    .bind(format!("current-event-guard-party-{party_id}"))
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Current event guard source', $3)",
    )
    .bind(listing_source_id)
    .bind(format!("current-event-guard-source-{listing_source_id}"))
    .bind(party_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, 'en', 'Current event guard description', 'en', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')",
    )
    .bind(product_uuid)
    .bind(format!("current-event-guard-{slug_suffix}"))
    .bind(event_id.into_uuid())
    .bind(listing_source_id)
    .bind(product_uuid.to_string())
    .bind("Current event guard product")
        .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())",
    )
    .bind(event_id.into_uuid())
    .bind(product_uuid)
    .bind(serde_json::json!({
        "listingSourceId": listing_source_id.to_string(),
        "sourceListingId": product_uuid.to_string(),
        "title": {"language": "en", "text": "Current event guard product"},
        "description": {"language": "en", "text": "Current event guard description"},
        "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
        "availability": "AVAILABLE",
        "url": "https://example.test/product",
        "imageCount": 0,
        "auction": null
    }))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((product_listing_id, event_id))
}
