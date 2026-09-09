use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};
use product_listing_postgres::SqlxProductListingLifecycleGuardFactory;
use product_listing_service::ports::{
    ProductListingLifecycleGuard, ProductListingLifecycleGuardFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_lock_and_read_only_the_canonical_product_listing_lifecycle() {
    let result = lifecycle_guard_flow().await;
    assert!(
        result.is_ok(),
        "lifecycle guard integration test failed: {result:?}"
    );
}

async fn lifecycle_guard_flow() -> Result<(), Box<dyn std::error::Error>> {
    let pool = get_postgres_client().await;
    let active_listing_id = seed_product(&pool, ListingLifecycle::Active).await?;
    let withdrawn_listing_id = seed_product(&pool, ListingLifecycle::Withdrawn).await?;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let mut guard_transaction = unit_of_work.begin().await?;
    let factory = SqlxProductListingLifecycleGuardFactory::new();

    assert_eq!(
        Some(ListingLifecycle::Active),
        factory
            .in_transaction(&mut guard_transaction)
            .lock_and_find_lifecycle(active_listing_id)
            .await?
    );
    assert_eq!(
        Some(ListingLifecycle::Withdrawn),
        factory
            .in_transaction(&mut guard_transaction)
            .lock_and_find_lifecycle(withdrawn_listing_id)
            .await?
    );
    assert_eq!(
        None,
        factory
            .in_transaction(&mut guard_transaction)
            .lock_and_find_lifecycle(ProductListingId::new())
            .await?
    );

    let mut competing_transaction = pool.begin().await?;
    let competing_lock = sqlx::query(
        "SELECT product_listing_id FROM product_listings WHERE product_listing_id = $1 FOR NO KEY UPDATE NOWAIT",
    )
    .bind(active_listing_id.into_uuid())
    .execute(&mut *competing_transaction)
    .await;
    assert!(
        matches!(competing_lock, Err(sqlx::Error::Database(_))),
        "an exclusive ProductListing lock succeeded while the lifecycle guard share lock was held: {competing_lock:?}"
    );
    competing_transaction.rollback().await?;

    guard_transaction.commit().await?;

    let mut released_competing_transaction = pool.begin().await?;
    let released_competing_lock = sqlx::query(
        "SELECT product_listing_id FROM product_listings WHERE product_listing_id = $1 FOR NO KEY UPDATE NOWAIT",
    )
    .bind(active_listing_id.into_uuid())
    .execute(&mut *released_competing_transaction)
    .await;
    assert!(
        released_competing_lock.is_ok(),
        "a ProductListing no-key-update lock did not succeed after the lifecycle guard released its share lock: {released_competing_lock:?}"
    );
    released_competing_transaction.rollback().await?;

    Ok(())
}

async fn seed_product(
    pool: &sqlx::PgPool,
    lifecycle: ListingLifecycle,
) -> Result<ProductListingId, sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = uuid::Uuid::now_v7();
    let product_uuid = product_listing_id.into_uuid();
    let mut transaction = pool.begin().await?;

    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Lifecycle guard party')")
        .bind(party_id)
        .bind(format!("lifecycle-guard-party-{party_id}"))
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Lifecycle guard source', $3)")
        .bind(listing_source_id)
        .bind(format!("lifecycle-guard-source-{listing_source_id}"))
        .bind(party_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Lifecycle guard product', 'en', $6, $7, 'https://example.test/product', '[]')")
        .bind(product_uuid)
        .bind(format!(
                    "lifecycle-guard-{}",
                    &product_uuid.simple().to_string()[26..]
                ))
        .bind(event_id.into_uuid())
        .bind(listing_source_id)
        .bind(product_uuid.to_string())
        .bind(
            (lifecycle == ListingLifecycle::Active)
                .then_some("AVAILABLE"),
        )
        .bind(lifecycle.as_str())
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(event_id.into_uuid())
        .bind(product_uuid)
        .bind(serde_json::json!({
            "listingSourceId": listing_source_id.to_string(),
            "sourceListingId": product_uuid.to_string(),
            "title": {"language": "en", "text": "Lifecycle guard product"},
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": {"start": null, "end": null}
        }))
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;

    Ok(product_listing_id)
}
