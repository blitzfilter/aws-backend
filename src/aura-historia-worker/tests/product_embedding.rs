use aura_historia_worker::{
    WorkerRunError, WorkerScope, product_embedding::consume_product_embedding_queue,
    serve_with_runtime,
};
use domain_primitives::event_id::EventId;
use embedding::{
    EMBEDDING_DIMENSIONS, EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText,
    EmbeddingVector,
};
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_postgres::{
    SqlxProductListingEmbeddingSourceReader, SqlxProductListingEmbeddingWriterFactory,
};
use product_listing_service::use_cases::{
    EmbedProductListingEventHandler, EmbedProductListingEventUseCase,
};
use std::{sync::Arc, time::Duration};
use test_api::{
    IntegrationTestService, Postgres, Sequin, aura_integration_test, get_postgres_client,
    get_sequin_worker_webhook_bind_addr,
};
use tokio::{sync::oneshot, task::JoinHandle};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
mod support;
const SCOPE: WorkerScope = WorkerScope::ProductListingEmbedding;
const WORKER_SQS: test_api::WorkerSqs = support::queues(SCOPE);
const WORKER_SEQUIN: Sequin = Sequin::worker_webhook_for_tables(&["public.product_listing_events"]);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const POLL_ATTEMPTS: usize = 80;
const NO_SIDE_EFFECT_OBSERVATION: Duration = Duration::from_secs(2);

struct FixedEmbeddingGenerator;

#[async_trait::async_trait]
impl EmbeddingGenerator for FixedEmbeddingGenerator {
    async fn embed_product(
        &self,
        _: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        EmbeddingVector::try_new(vec![1.0; EMBEDDING_DIMENSIONS])
    }

    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        Err(EmbeddingError::InvalidInput {
            reason: "expected product embedding",
        })
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_embed_committed_created_product_event_and_persist_canonical_target_shape() {
    let worker = EmbeddingWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, source_event_id) = insert_product_with_event(&worker.pool, "PRODUCT_LISTING_DISCOVERED", "DOMAIN").await?;
        let (embedding, current_event_id, embedding_source_event_id) =
            wait_for_embedding(&worker.pool, product_listing_id).await?;
        assert_eq!(EMBEDDING_DIMENSIONS, embedding.len());
        assert!((embedding[0] - (1.0 / (EMBEDDING_DIMENSIONS as f32).sqrt())).abs() < 0.000_001);
        assert_ne!(uuid::Uuid::from(source_event_id), current_event_id);
        assert_eq!(uuid::Uuid::from(source_event_id), embedding_source_event_id);
        let payload: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'",
        )
        .bind(uuid::Uuid::from(product_listing_id))
        .fetch_one(&worker.pool)
        .await?;
        assert_eq!(
            serde_json::json!({"sourceEventId": source_event_id.as_uuid().to_string()}),
            payload
        );
        Ok(())
    }.await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_ignore_non_created_product_event_without_embedding_side_effect() {
    let worker = EmbeddingWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, _) =
            insert_product_with_event(&worker.pool, "PRODUCT_LISTING_CHANGED", "DOMAIN").await?;
        assert_no_embedding(&worker.pool, product_listing_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_not_embed_rolled_back_created_product_event() {
    let worker = EmbeddingWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let product_listing_id = insert_product_with_event_then_rollback(&worker.pool).await?;
        assert_no_embedding(&worker.pool, product_listing_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_skip_stale_created_event_after_product_revision_advances() {
    let pool = get_postgres_client().await;
    let (product_listing_id, source_event_id) =
        insert_product_with_event(&pool, "PRODUCT_LISTING_DISCOVERED", "DOMAIN")
            .await
            .unwrap_or_else(|error| panic!("seed ProductListing: {error}"));
    advance_product_revision(&pool, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("advance ProductListing: {error}"));
    let worker = EmbeddingWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        worker
            .redeliver(
                product_listing_id,
                source_event_id,
                "PRODUCT_LISTING_DISCOVERED",
                "DOMAIN",
            )
            .await?;
        assert_no_embedding(&worker.pool, product_listing_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_reembed_when_committed_image_change_advances_source_marker() {
    let worker = EmbeddingWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, first_event_id) =
            insert_product_with_event(&worker.pool, "PRODUCT_LISTING_DISCOVERED", "DOMAIN").await?;
        let _ = wait_for_embedding(&worker.pool, product_listing_id).await?;
        let image_event_id = insert_image_change(&worker.pool, product_listing_id).await?;

        let (_, current_event_id, embedding_source_event_id) =
            wait_for_embedding(&worker.pool, product_listing_id).await?;
        assert_eq!(uuid::Uuid::from(image_event_id), embedding_source_event_id);
        assert_ne!(uuid::Uuid::from(image_event_id), current_event_id);
        for event_id in [
            image_event_id,
            first_event_id,
            image_event_id,
            first_event_id,
        ] {
            support::redeliver_product_event(&worker.pool, uuid::Uuid::from(event_id)).await?;
        }
        support::wait_until_empty(SCOPE).await?;
        let (_, persisted_current, persisted_source) =
            wait_for_embedding(&worker.pool, product_listing_id).await?;
        assert_eq!(current_event_id, persisted_current);
        assert_eq!(embedding_source_event_id, persisted_source);
        assert_embedding_event_count_for_duration(
            &worker.pool,
            product_listing_id,
            2,
            NO_SIDE_EFFECT_OBSERVATION,
        )
        .await
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_not_append_another_embedded_event_when_source_is_redelivered() {
    let worker = EmbeddingWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, event_id) =
            insert_product_with_event(&worker.pool, "PRODUCT_LISTING_DISCOVERED", "DOMAIN").await?;
        let _ = wait_for_embedding(&worker.pool, product_listing_id).await?;
        worker
            .redeliver(
                product_listing_id,
                event_id,
                "PRODUCT_LISTING_DISCOVERED",
                "DOMAIN",
            )
            .await?;
        assert_embedding_event_count_for_duration(
            &worker.pool,
            product_listing_id,
            1,
            NO_SIDE_EFFECT_OBSERVATION,
        )
        .await
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

struct EmbeddingWorker {
    pool: sqlx::PgPool,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
    consumer: JoinHandle<()>,
}

impl EmbeddingWorker {
    async fn start() -> Self {
        let pool = get_postgres_client().await;
        let handler: Arc<dyn EmbedProductListingEventUseCase> =
            Arc::new(EmbedProductListingEventHandler::new(
                SqlxProductListingEmbeddingSourceReader::new(pool.clone()),
                FixedEmbeddingGenerator,
                SqlxUnitOfWork::new(pool.clone()),
                SqlxProductListingEmbeddingWriterFactory::new(),
            ));
        let (runtime, receiver) = support::composition(SCOPE)
            .await
            .unwrap_or_else(|error| panic!("valid scoped SQS configuration: {error}"))
            .into_parts();
        let consumer = support::competing_consumers(SCOPE, receiver, move |receiver| {
            consume_product_embedding_queue(receiver, handler.clone())
        })
        .await
        .unwrap_or_else(|error| panic!("start competing consumers: {error}"));
        let listener = tokio::net::TcpListener::bind(get_sequin_worker_webhook_bind_addr())
            .await
            .unwrap_or_else(|error| panic!("worker webhook bind address is available: {error}"));
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));
        Self {
            pool,
            shutdown_tx,
            server,
            consumer,
        }
    }

    async fn redeliver(
        &self,
        product_listing_id: ProductListingId,
        event_id: EventId,
        event_type: &str,
        event_group: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let payload: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM product_listing_events WHERE event_id = $1 AND product_listing_id = $2",
        )
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .fetch_one(&self.pool)
        .await?;
        let response = reqwest::Client::new().post(format!("http://127.0.0.1:{}/cdc/sequin", get_sequin_worker_webhook_bind_addr().port()))
            .json(&serde_json::json!({"record":{"event_id":event_id.as_uuid().to_string(),"product_listing_id":product_listing_id.as_uuid().to_string(),"event_type":event_type,"event_group":event_group,"event_type_schema_version":1,"payload":payload},"action":"insert","metadata":{"table_schema":"public","table_name":"product_listing_events"}}))
            .send().await?;
        if response.status() != reqwest::StatusCode::ACCEPTED {
            return Err(std::io::Error::other("worker did not accept redelivery").into());
        }
        Ok(())
    }

    async fn finish(
        self,
        test_result: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let shutdown_result = self
            .shutdown_tx
            .send(())
            .map_err(|_| std::io::Error::other("worker shutdown channel closed"));
        let (server_result, consumer_result) = tokio::join!(self.server, self.consumer);
        shutdown_result?;
        server_result??;
        consumer_result?;
        test_result
    }
}

async fn insert_product_with_event(
    pool: &sqlx::PgPool,
    event_type: &str,
    event_group: &str,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let product_listing_uuid = uuid::Uuid::from(product_listing_id);
    let event_id = EventId::new();
    let title_slug_id = ProductListingSlugId::from_title_and_suffix(
        "embedding worker product",
        &product_listing_uuid.simple().to_string()[26..],
    )
    .map_err(|_| sqlx::Error::Protocol("invalid fixture title slug".to_owned()))?;
    let listing_source_id = ListingSourceId::new();
    let operator_party_id = uuid::Uuid::now_v7();
    let source_listing_id = "embedding-fixture-product";
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'Embedding worker source', party_id FROM operator")
        .bind(operator_party_id)
        .bind(format!("embedding-worker-source-{}", listing_source_id.as_uuid()))
        .bind(listing_source_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Eichenstuhl', 'de', 'Bemalter Stuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[{\"url\": \"https://example.test/image.jpg\"}]')")
        .bind(product_listing_uuid).bind(title_slug_id.as_ref()).bind(event_id.as_uuid()).bind(listing_source_id.as_uuid()).bind(source_listing_id)
        .execute(&mut *tx).await?;
    let payload = match event_type {
        "PRODUCT_LISTING_DISCOVERED" => serde_json::json!({
            "listingSourceId": listing_source_id.as_uuid().to_string(),
            "sourceListingId": source_listing_id,
            "title": {"language": "de", "text": "Antiker Eichenstuhl"},
            "description": {"language": "de", "text": "Bemalter Stuhl"},
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 1,
            "auction": null
        }),
        "PRODUCT_LISTING_CHANGED" => serde_json::json!({
            "availability": {"previous": null, "current": "AVAILABLE"}
        }),
        _ => serde_json::json!({
            "sourceEventId": event_id.as_uuid().to_string()
        }),
    };
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, $3, $4, 1, $5, now())")
        .bind(event_id.as_uuid()).bind(product_listing_uuid).bind(event_type).bind(event_group).bind(payload).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((product_listing_id, event_id))
}

async fn insert_product_with_event_then_rollback(
    pool: &sqlx::PgPool,
) -> Result<ProductListingId, sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let product_listing_uuid = uuid::Uuid::from(product_listing_id);
    let event_id = EventId::new();
    let title_slug_id = ProductListingSlugId::from_title_and_suffix(
        "rollback embedding worker product",
        &product_listing_uuid.simple().to_string()[26..],
    )
    .map_err(|_| sqlx::Error::Protocol("invalid fixture title slug".to_owned()))?;
    let listing_source_id = ListingSourceId::new();
    let operator_party_id = uuid::Uuid::now_v7();
    let source_listing_id = "rollback-embedding-fixture-product";
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'Rollback embedding source', party_id FROM operator")
        .bind(operator_party_id)
        .bind(format!(
            "rollback-embedding-source-{}",
            listing_source_id.as_uuid()
        ))
        .bind(listing_source_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Eichenstuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_listing_uuid).bind(title_slug_id.as_ref()).bind(event_id.as_uuid()).bind(listing_source_id.as_uuid()).bind(source_listing_id)
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(event_id.as_uuid())
        .bind(product_listing_uuid)
        .bind(serde_json::json!({
            "listingSourceId": listing_source_id.as_uuid().to_string(),
            "sourceListingId": source_listing_id,
            "title": {"language": "de", "text": "Antiker Eichenstuhl"},
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": null
        }))
        .execute(&mut *tx)
        .await?;
    tx.rollback().await?;
    Ok(product_listing_id)
}

async fn insert_image_change(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, '{\"images\": {\"previousCount\": 1, \"currentCount\": 1}}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE product_listings SET current_event_id = $1, embedding_source_event_id = $1, embedding = NULL, version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(event_id)
}

async fn advance_product_revision(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<(), sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(serde_json::json!({
            "availability": {"previous": "AVAILABLE", "current": "SOLD_OUT"}
        }))
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE product_listings SET current_event_id = $1, availability = 'SOLD_OUT', embedding_source_event_id = $1, embedding = NULL, version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

async fn wait_for_embedding(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<(Vec<f32>, uuid::Uuid, uuid::Uuid), Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        let row: (Option<Vec<f32>>, uuid::Uuid, uuid::Uuid) = sqlx::query_as(
            "SELECT embedding, current_event_id, embedding_source_event_id FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(uuid::Uuid::from(product_listing_id))
        .fetch_one(pool)
        .await?;
        if let Some(embedding) = row.0 {
            return Ok((embedding, row.1, row.2));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other("timed out waiting for product embedding").into())
}

async fn assert_no_embedding(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'").bind(uuid::Uuid::from(product_listing_id)).fetch_one(pool).await?;
        let embedding: Option<Vec<f32>> = sqlx::query_scalar(
            "SELECT embedding FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(uuid::Uuid::from(product_listing_id))
        .fetch_optional(pool)
        .await?
        .flatten();
        if count != 0 || embedding.is_some() {
            return Err(std::io::Error::other("unexpected product embedding persisted").into());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}

async fn assert_embedding_event_count_for_duration(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    expected_count: i64,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    loop {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'").bind(uuid::Uuid::from(product_listing_id)).fetch_one(pool).await?;
        if count == expected_count {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(
                std::io::Error::other(format!(
                    "timed out waiting for embedded events after source-marker redelivery: expected {expected_count}, got {count}"
                ))
                .into(),
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'").bind(uuid::Uuid::from(product_listing_id)).fetch_one(pool).await?;
        if count != expected_count {
            return Err(
                std::io::Error::other(format!(
                    "embedded event count changed after source-marker redelivery: expected {expected_count}, got {count}"
                ))
                .into(),
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}
