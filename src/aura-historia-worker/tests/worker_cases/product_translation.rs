use crate::{BUSINESS_SCHEMA, WORKER_ACCEPTANCE, WORKER_SEQUIN, support};
use aura_historia_worker::{
    WorkerRunError, WorkerScope, product_translation::consume_product_translation_queue,
    serve_with_runtime,
};
use domain_primitives::event_id::EventId;
use large_language_model::{
    LargeLanguageModel, LargeLanguageModelError, StructuredGenerationRequest,
};
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_postgres::{
    SqlxProductListingTranslationSourceReader, SqlxProductListingTranslationWriterFactory,
};
use product_listing_service::use_cases::{
    TranslateProductListingEventHandler, TranslateProductListingEventUseCase,
};
use product_listing_translation_llm::LargeLanguageModelProductListingTitleTranslator;
use std::{sync::Arc, time::Duration};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use tokio::{sync::oneshot, task::JoinHandle};

const SCOPE: WorkerScope = WorkerScope::ProductListingTranslation;
const WORKER_SQS: test_api::WorkerSqs = support::queues(SCOPE);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const POLL_ATTEMPTS: usize = 80;
const NO_SIDE_EFFECT_OBSERVATION: Duration = Duration::from_secs(2);

struct FixedTranslationLlm;

#[async_trait::async_trait]
impl LargeLanguageModel for FixedTranslationLlm {
    async fn generate<Output>(
        &self,
        _request: StructuredGenerationRequest,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: serde::de::DeserializeOwned + Send,
    {
        serde_json::from_str(
            r#"{"titles":{"en":"Antique oak chair","fr":"Chaise ancienne en chêne","es":"Silla antigua de roble","it":"Sedia antica in rovere"}}"#,
        )
        .map_err(|source| LargeLanguageModelError::InvalidResponse {
            source: application::error::box_error(source),
        })
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, WORKER_SQS, WORKER_SEQUIN])]
async fn should_translate_committed_discovered_product_event_and_persist_canonical_target_shape() {
    let worker = TranslationWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, source_event_id) =
            insert_product_with_event(&worker.pool, "PRODUCT_LISTING_DISCOVERED", "DOMAIN").await?;

        let rows = wait_for_translations(&worker.pool, product_listing_id, 4).await?;

        assert_eq!(
            vec![
                ("en".to_owned(), "Antique oak chair".to_owned()),
                ("es".to_owned(), "Silla antigua de roble".to_owned()),
                ("fr".to_owned(), "Chaise ancienne en chêne".to_owned()),
                ("it".to_owned(), "Sedia antica in rovere".to_owned()),
            ],
            rows
        );
        let count = enrichment_event_count(&worker.pool, product_listing_id).await?;
        assert_eq!(
            1, count,
            "one translated-titles enrichment event"
        );
        let (content_source_event_id, payload): (uuid::Uuid, serde_json::Value) =
            sqlx::query_as(
                "SELECT product.content_source_event_id, event.payload FROM product_listings product JOIN product_listing_events event ON event.product_listing_id = product.product_listing_id WHERE product.product_listing_id = $1 AND event.event_type = 'ENRICHMENT_TRANSLATED_TITLES'",
            )
            .bind(uuid::Uuid::from(product_listing_id))
            .fetch_one(&worker.pool)
            .await?;
        assert_eq!(uuid::Uuid::from(source_event_id), content_source_event_id);
        assert_eq!(
            serde_json::json!({
                "sourceEventId": source_event_id.as_uuid().to_string(),
                "sourceLanguage": "de",
                "targetLanguages": ["en", "fr", "es", "it"],
            }),
            payload
        );
        Ok(())
    }
    .await;
    worker
        .finish(result)
        .await
        .expect("worker cleanup or test failed");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, WORKER_SQS, WORKER_SEQUIN])]
async fn should_ignore_non_discovered_product_event_without_translation_side_effect() {
    let worker = TranslationWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, _) =
            insert_product_with_event(&worker.pool, "PRODUCT_LISTING_CHANGED", "DOMAIN").await?;

        assert_no_translations(&worker.pool, product_listing_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .expect("worker cleanup or test failed");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, WORKER_SQS, WORKER_SEQUIN])]
async fn should_not_translate_rolled_back_discovered_product_event() {
    let worker = TranslationWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let product_listing_id = insert_product_with_event_then_rollback(&worker.pool).await?;

        assert_no_translations(&worker.pool, product_listing_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .expect("worker cleanup or test failed");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, WORKER_SQS, WORKER_SEQUIN])]
async fn should_skip_stale_discovered_event_after_content_source_revision_advances() {
    let worker = TranslationWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, source_event_id) =
            insert_product_with_event(&worker.pool, "PRODUCT_LISTING_DISCOVERED", "DOMAIN").await?;
        let _newer_event_id = advance_product_revision(&worker.pool, product_listing_id).await?;

        worker.redeliver(source_event_id).await?;
        assert_no_translations(&worker.pool, product_listing_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .expect("worker cleanup or test failed");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, WORKER_SQS, WORKER_SEQUIN])]
async fn should_not_append_another_translation_event_when_source_is_redelivered() {
    let worker = TranslationWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, event_id) =
            insert_product_with_event(&worker.pool, "PRODUCT_LISTING_DISCOVERED", "DOMAIN").await?;
        let _rows = wait_for_translations(&worker.pool, product_listing_id, 4).await?;

        worker.redeliver(event_id).await?;
        assert_translation_count_for_duration(
            &worker.pool,
            product_listing_id,
            4,
            NO_SIDE_EFFECT_OBSERVATION,
        )
        .await?;
        assert_eq!(
            1,
            enrichment_event_count(&worker.pool, product_listing_id).await?
        );
        Ok(())
    }
    .await;
    worker
        .finish(result)
        .await
        .expect("worker cleanup or test failed");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, WORKER_SQS, WORKER_SEQUIN])]
async fn should_preserve_translation_rows_and_events_after_reversed_duplicate_sqs_deliveries() {
    let worker = TranslationWorker::start().await;
    let result: support::TestResult = async {
        let first = insert_product_with_event(&worker.pool, "PRODUCT_LISTING_DISCOVERED", "DOMAIN").await?;
        let second = insert_product_with_event(&worker.pool, "PRODUCT_LISTING_DISCOVERED", "DOMAIN").await?;
        let first_rows = wait_for_translations(&worker.pool, first.0, 4).await?;
        let second_rows = wait_for_translations(&worker.pool, second.0, 4).await?;
        for event in [second.1, first.1, second.1, first.1] {
            support::redeliver_product_event(&worker.pool, uuid::Uuid::from(event)).await?;
        }
        support::wait_until_empty(SCOPE).await?;
        assert_eq!(first_rows, wait_for_translations(&worker.pool, first.0, 4).await?);
        assert_eq!(second_rows, wait_for_translations(&worker.pool, second.0, 4).await?);
        for (id, event) in [first, second] {
            assert_eq!(1, enrichment_event_count(&worker.pool, id).await?);
            let payload: serde_json::Value = sqlx::query_scalar("SELECT payload FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_TRANSLATED_TITLES'")
                .bind(uuid::Uuid::from(id)).fetch_one(&worker.pool).await?;
            assert_eq!(serde_json::json!(event.as_uuid()), payload["sourceEventId"]);
        }
        Ok(())
    }.await;
    worker
        .finish(result)
        .await
        .expect("reversed SQS translation acceptance and cleanup");
}

struct TranslationWorker {
    pool: sqlx::PgPool,
    _route_lease: support::sequin_router::RouteLease,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
    consumer: JoinHandle<()>,
}

impl TranslationWorker {
    async fn start() -> Self {
        let pool = get_postgres_client().await;
        let handler: Arc<dyn TranslateProductListingEventUseCase> =
            Arc::new(TranslateProductListingEventHandler::new(
                SqlxProductListingTranslationSourceReader::new(pool.clone()),
                LargeLanguageModelProductListingTitleTranslator::new(FixedTranslationLlm),
                SqlxUnitOfWork::new(pool.clone()),
                SqlxProductListingTranslationWriterFactory::new(),
            ));
        let (runtime, receiver) = support::composition(SCOPE)
            .await
            .unwrap_or_else(|error| panic!("valid scoped SQS configuration: {error}"))
            .into_parts();
        let consumer = support::competing_consumers(SCOPE, receiver, move |receiver| {
            consume_product_translation_queue(receiver, handler.clone())
        })
        .await
        .unwrap_or_else(|error| panic!("start competing consumers: {error}"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("worker webhook bind address is available");
        let worker_addr = listener
            .local_addr()
            .expect("worker webhook has a local address");
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));
        let route_lease = support::sequin_router::activate_scope(SCOPE, worker_addr)
            .await
            .unwrap_or_else(|error| panic!("activate worker Sequin route: {error}"));
        Self {
            pool,
            _route_lease: route_lease,
            shutdown_tx,
            server,
            consumer,
        }
    }

    async fn redeliver(&self, event_id: EventId) -> Result<(), Box<dyn std::error::Error>> {
        support::redeliver_product_event(&self.pool, uuid::Uuid::from(event_id)).await
    }

    async fn finish(
        self,
        test_result: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let TranslationWorker {
            shutdown_tx,
            server,
            consumer,
            ..
        } = self;
        let shutdown_result = shutdown_tx
            .send(())
            .map_err(|_| std::io::Error::other("worker shutdown channel closed"));
        let (server_result, consumer_result) = tokio::join!(server, consumer);
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
    let product_uuid = uuid::Uuid::from(product_listing_id);
    let event_id = EventId::new();
    let title_slug_id = ProductListingSlugId::from_title_and_suffix(
        "translation worker product",
        &product_uuid.simple().to_string()[26..],
    )
    .map_err(|_| sqlx::Error::Protocol("invalid fixture title slug".to_owned()))?;
    let listing_source_id = ListingSourceId::new();
    let operator_party_id = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'Translation worker source', party_id FROM operator")
        .bind(operator_party_id)
        .bind(format!("translation-worker-source-{}", listing_source_id.as_uuid()))
        .bind(listing_source_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Eichenstuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_uuid)
        .bind(title_slug_id.as_ref())
        .bind(uuid::Uuid::from(event_id))
        .bind(listing_source_id.as_uuid())
        .bind(product_uuid.to_string())

        .execute(&mut *tx)
        .await?;
    let payload = match event_type {
        "PRODUCT_LISTING_DISCOVERED" => serde_json::json!({
            "listingSourceId": listing_source_id.as_uuid().to_string(),
            "sourceListingId": product_uuid.to_string(),
            "title": {"language": "de", "text": "Antiker Eichenstuhl"},
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": {"start": null, "end": null}
        }),
        "PRODUCT_LISTING_CHANGED" => serde_json::json!({
            "images": {"previousCount": 0, "currentCount": 0}
        }),
        _ => serde_json::json!({"sourceEventId": event_id.as_uuid().to_string()}),
    };
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, $3, $4, 1, $5, now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(event_type)
        .bind(event_group)
        .bind(payload)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((product_listing_id, event_id))
}

async fn advance_product_revision(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(serde_json::json!({
            "images": {"previousCount": 0, "currentCount": 0}
        }))
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE product_listings SET current_event_id = $1, content_source_event_id = $1, version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(event_id)
}

async fn insert_product_with_event_then_rollback(
    pool: &sqlx::PgPool,
) -> Result<ProductListingId, sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let product_uuid = uuid::Uuid::from(product_listing_id);
    let event_id = EventId::new();
    let title_slug_id = ProductListingSlugId::from_title_and_suffix(
        "rollback translation worker product",
        &product_uuid.simple().to_string()[26..],
    )
    .map_err(|_| sqlx::Error::Protocol("invalid fixture title slug".to_owned()))?;
    let listing_source_id = ListingSourceId::new();
    let operator_party_id = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'Rollback translation source', party_id FROM operator")
        .bind(operator_party_id)
        .bind(format!("rollback-translation-source-{}", listing_source_id.as_uuid()))
        .bind(listing_source_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Eichenstuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_uuid)
        .bind(title_slug_id.as_ref())
        .bind(uuid::Uuid::from(event_id))
        .bind(listing_source_id.as_uuid())
        .bind(product_uuid.to_string())

        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(serde_json::json!({
            "listingSourceId": listing_source_id.as_uuid().to_string(),
            "sourceListingId": product_uuid.to_string(),
            "title": {"language": "de", "text": "Antiker Eichenstuhl"},
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": {"start": null, "end": null}
        }))
        .execute(&mut *tx)
        .await?;
    tx.rollback().await?;
    Ok(product_listing_id)
}

async fn wait_for_translations(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    expected_count: i64,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        let rows = translations(pool, product_listing_id).await?;
        if i64::try_from(rows.len())? == expected_count {
            return Ok(rows);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other("timed out waiting for product translations").into())
}

async fn assert_no_translations(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        if !translations(pool, product_listing_id).await?.is_empty() {
            return Err(std::io::Error::other("unexpected product translation persisted").into());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}

async fn assert_translation_count_for_duration(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    expected_count: usize,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        if translations(pool, product_listing_id).await?.len() != expected_count {
            return Err(std::io::Error::other(
                "product translation count changed after redelivery",
            )
            .into());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}

async fn translations(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT language, title FROM product_listing_translations WHERE product_listing_id = $1 ORDER BY language",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_all(pool)
    .await
}

async fn enrichment_event_count(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_group = 'ENRICHMENT'",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_one(pool)
    .await
}
