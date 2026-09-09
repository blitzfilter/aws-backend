mod support;

use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use std::time::Duration;

use platform_postgres::SqlxUnitOfWork;
const EMBEDDING_DIMENSIONS: usize = 768;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_postgres::SqlxProductListingEmbeddingWriterFactory;
use product_listing_service::ports::{
    ProductListingEmbeddingWrite, ProductListingEmbeddingWriteOutcome,
    ProductListingEmbeddingWriter, ProductListingEmbeddingWriterFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_store_embedding_append_enrichment_event_and_advance_current_event_without_aggregate_version()
 {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_listing_id, source_event_id) =
            insert_product_with_created_event(&pool).await?;
        let embedding_write = new_write(product_listing_id, source_event_id, EventId::new());
        let outcome = apply(&pool, &embedding_write).await?;
        assert_eq!(ProductListingEmbeddingWriteOutcome::Applied, outcome);
        let (embedding, current_event, version, projection_version): (Option<Vec<f32>>, uuid::Uuid, i64, i64) = sqlx::query_as(
            "SELECT embedding, current_event_id, version, projection_version FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(product_listing_id.into_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!(Some(vec![0.25; EMBEDDING_DIMENSIONS]), embedding);
        assert_eq!(
            embedding_write.enrichment_event_id.into_uuid(),
            current_event
        );
        assert_eq!(1, version);
        assert_eq!(2, projection_version);
        let payload: serde_json::Value =
            sqlx::query_scalar("SELECT payload FROM product_listing_events WHERE event_id = $1")
                .bind(embedding_write.enrichment_event_id.into_uuid())
                .fetch_one(&pool)
                .await?;
        assert_eq!(
            serde_json::json!({
                "sourceEventId": source_event_id.as_uuid().to_string(),
            }),
            payload
        );
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "embedding write acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_duplicate_and_stale_without_second_embedding_event() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
    let pool = get_postgres_client().await;
    let (product_listing_id, source_event_id) = insert_product_with_created_event(&pool).await?;
    let embedding_write = new_write(product_listing_id, source_event_id, EventId::new());
    apply(&pool, &embedding_write).await?;
    assert_eq!(
        ProductListingEmbeddingWriteOutcome::Duplicate,
        apply(
            &pool,
            &ProductListingEmbeddingWrite {
                enrichment_event_id: EventId::new(),
                ..embedding_write.clone()
            }
        )
        .await?
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'").bind(product_listing_id.into_uuid()).fetch_one(&pool).await?;
    assert_eq!(1, count);
    let (stale_product_listing_id, stale_event_id) = insert_product_with_created_event(&pool).await?;
    advance_product_current_event(&pool, stale_product_listing_id).await?;
    assert_eq!(
        ProductListingEmbeddingWriteOutcome::Stale,
        apply(
            &pool,
            &new_write(stale_product_listing_id, stale_event_id, EventId::new())
        )
        .await?
    );
    Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "embedding duplicate/stale acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_keep_first_embedding_when_duplicate_completions_overlap() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_id, source_event_id) = insert_product_with_created_event(&pool).await?;
        let first_write = new_write(product_id, source_event_id, EventId::new());
        let mut duplicate_write = new_write(product_id, source_event_id, EventId::new());
        duplicate_write.embedding = vec![0.75; EMBEDDING_DIMENSIONS];
        let mut first = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(first.connection()).await?;
        assert_eq!(ProductListingEmbeddingWriteOutcome::Applied,
            SqlxProductListingEmbeddingWriterFactory::new().in_transaction(&mut first)
                .apply(&first_write).await?);
        let duplicate = apply(&pool, &duplicate_write);
        tokio::pin!(duplicate);
        support::assert_blocked(&pool, blocker_pid, 1, duplicate.as_mut()).await?;
        first.commit().await?;
        assert_eq!(ProductListingEmbeddingWriteOutcome::Duplicate,
            tokio::time::timeout(Duration::from_secs(10), duplicate).await??);
        let stored: (Vec<f32>, uuid::Uuid, i64, i64) = sqlx::query_as(
            "SELECT embedding, current_event_id, version, projection_version FROM product_listings WHERE product_listing_id = $1"
        ).bind(product_id.into_uuid()).fetch_one(&pool).await?;
        assert_eq!((first_write.embedding, first_write.enrichment_event_id.into_uuid(), 1, 2), stored);
        let events: Vec<uuid::Uuid> = sqlx::query_scalar(
            "SELECT event_id FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'"
        ).bind(product_id.into_uuid()).fetch_all(&pool).await?;
        assert_eq!(vec![first_write.enrichment_event_id.into_uuid()], events);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "concurrent embedding completion: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_late_embedding_completion_after_new_image_revision_commits() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_id, old_source) = insert_product_with_created_event(&pool).await?;
        let old_write = new_write(product_id, old_source, EventId::new());
        advance_product_current_event(&pool, product_id).await?;
        let new_source: uuid::Uuid = sqlx::query_scalar(
            "SELECT embedding_source_event_id FROM product_listings WHERE product_listing_id = $1"
        ).bind(product_id.into_uuid()).fetch_one(&pool).await?;
        let new_source = EventId::try_from(new_source)?;
        let new_write = new_write(product_id, new_source, EventId::new());
        let mut newer = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(newer.connection()).await?;
        SqlxProductListingEmbeddingWriterFactory::new().in_transaction(&mut newer)
            .apply(&new_write).await?;
        let late = apply(&pool, &old_write);
        tokio::pin!(late);
        support::assert_blocked(&pool, blocker_pid, 1, late.as_mut()).await?;
        newer.commit().await?;
        assert_eq!(ProductListingEmbeddingWriteOutcome::Stale,
            tokio::time::timeout(Duration::from_secs(10), late).await??);
        let stored: (Vec<f32>, uuid::Uuid, i64) = sqlx::query_as(
            "SELECT embedding, current_event_id, projection_version FROM product_listings WHERE product_listing_id = $1"
        ).bind(product_id.into_uuid()).fetch_one(&pool).await?;
        assert_eq!((new_write.embedding, new_write.enrichment_event_id.into_uuid(), 3), stored);
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'"
        ).bind(product_id.into_uuid()).fetch_one(&pool).await?;
        assert_eq!(1, count);
        Ok(())
    }.await;
    assert!(result.is_ok(), "reversed embedding completion: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_apply_embedding_after_unrelated_newer_event_without_invalidating_image_source() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_id, source) = insert_product_with_created_event(&pool).await?;
        let mut update = pool.begin().await?;
        let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *update).await?;
        let unrelated = EventId::new();
        sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
            .bind(unrelated.into_uuid()).bind(product_id.into_uuid())
            .bind(serde_json::json!({"availability": {"previous": "AVAILABLE", "current": "RESERVED"}})).execute(&mut *update).await?;
        sqlx::query("UPDATE product_listings SET current_event_id = $1, availability = 'RESERVED', version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2")
            .bind(unrelated.into_uuid()).bind(product_id.into_uuid()).execute(&mut *update).await?;
        let write = new_write(product_id, source, EventId::new());
        let completion = apply(&pool, &write);
        tokio::pin!(completion);
        support::assert_blocked(&pool, blocker_pid, 1, completion.as_mut()).await?;
        update.commit().await?;
        assert_eq!(ProductListingEmbeddingWriteOutcome::Applied,
            tokio::time::timeout(Duration::from_secs(10), completion).await??);
        Ok(())
    }.await;
    assert!(result.is_ok(), "independent image source: {result:?}");
}

async fn apply(
    pool: &sqlx::PgPool,
    write: &ProductListingEmbeddingWrite,
) -> Result<ProductListingEmbeddingWriteOutcome, Box<dyn std::error::Error>> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let outcome = SqlxProductListingEmbeddingWriterFactory::new()
        .in_transaction(&mut tx)
        .apply(write)
        .await?;
    tx.commit().await?;
    Ok(outcome)
}
fn new_write(
    product_listing_id: ProductListingId,
    source_event_id: EventId,
    enrichment_event_id: EventId,
) -> ProductListingEmbeddingWrite {
    ProductListingEmbeddingWrite {
        product_listing_id,
        source_event_id,
        enrichment_event_id,
        embedding: vec![0.25; EMBEDDING_DIMENSIONS],
    }
}
async fn insert_product_with_created_event(
    pool: &sqlx::PgPool,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Embedding party')",
    )
    .bind(party_id)
    .bind(format!("embedding-party-{party_id}"))
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Embedding source', $3)").bind(listing_source_id).bind(format!("embedding-source-{listing_source_id}")).bind(party_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Stuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')").bind(product_listing_id.into_uuid()).bind(format!(
                "embedding-product-{}",
                &product_listing_id.as_uuid().simple().to_string()[26..]
            )).bind(event_id.into_uuid()).bind(listing_source_id).bind(product_listing_id.to_string())
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(event_id.into_uuid())
        .bind(product_listing_id.into_uuid())
        .bind(serde_json::json!({
            "listingSourceId": listing_source_id.to_string(),
            "sourceListingId": product_listing_id.to_string(),
            "title": {"language": "de", "text": "Antiker Stuhl"},
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": {"start": null, "end": null}
        }))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((product_listing_id, event_id))
}
async fn advance_product_current_event(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<(), sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
        .bind(event_id.into_uuid())
        .bind(product_listing_id.into_uuid())
        .bind(serde_json::json!({
            "images": {"previousCount": 0, "currentCount": 0}
        }))
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE product_listings SET current_event_id = $1, embedding_source_event_id = $1, embedding = NULL, version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2")
        .bind(event_id.into_uuid())
        .bind(product_listing_id.into_uuid())
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}
