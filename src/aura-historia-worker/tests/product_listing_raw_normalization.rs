use application::transaction::{Transaction, UnitOfWork};
use aura_historia_worker::{
    WorkerRunError, WorkerScope,
    product_listing_raw_normalization::consume_product_listing_raw_normalization_queue,
    serve_with_runtime,
};
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_normalization::{
    NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingProvenance, RawProductListingValues,
    SourcePayload,
};
use product_listing_postgres::{
    SqlxPendingProductListingRawStreamReader, SqlxProductListingEventAppenderFactory,
    SqlxProductListingRawCaptureWriterFactory, SqlxProductListingRawNormalizationWriterFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::ports::{
    ProductListingRawCaptureWrite, ProductListingRawCaptureWriteOutcome,
    ProductListingRawCaptureWriter, ProductListingRawCaptureWriterFactory,
    ProductListingRawIngestionMethod, ProductListingRawRevisionId, ProductListingRawStreamId,
    SourceRecordKeySha256,
};
use product_service::use_cases::{
    NormalizeProductListingRawRevisionHandler, NormalizeProductListingRawRevisionUseCase,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use test_api::{
    IntegrationTestService, Postgres, Sequin, aura_integration_test, get_postgres_client,
    get_sequin_worker_webhook_bind_addr, get_sqs_client,
};
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
mod support;
const SCOPE: WorkerScope = WorkerScope::ProductListingRawNormalization;
const WORKER_SQS: test_api::WorkerSqs = support::queues(SCOPE);
const WORKER_SEQUIN: Sequin =
    Sequin::worker_webhook_for_tables(&["public.product_listing_raw_revisions"]);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const POLL_ATTEMPTS: usize = 80;
const DIRECT_CDC_POLL_ATTEMPTS: usize = 20;
const DIRECT_CDC_TIMEOUT: Duration = Duration::from_secs(4);

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_reconcile_preexisting_revisions_and_process_sequin_redelivery_idempotently() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let listing_source_id = seed_listing_source(&pool, "raw-normalization-worker").await?;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();

        // This insert happens before the runtime starts. The immediate reconciliation tick must
        // repair work committed before ingress startup even if Sequin could not deliver it yet.
        let first = capture(
            &unit_of_work,
            &capture_writer,
            raw_write(listing_source_id, "pre-start", 1, "EUR 100"),
        )
        .await?;
        let (stream_id, revision_id, revision) = changed_parts(first)?;

        let worker = RawNormalizationWorker::start(pool.clone()).await?;
        let work_result: Result<(), Box<dyn std::error::Error>> = async {
            wait_for_normalization(&pool, *revision_id.as_uuid(), 1).await?;

            let listing_count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listings")
                .fetch_one(&pool)
                .await?;
            let event_count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events")
                .fetch_one(&pool)
                .await?;
            assert_eq!(1, listing_count);
            assert_eq!(1, event_count);

            redeliver_raw_revision(
                *stream_id.as_uuid(),
                *revision_id.as_uuid(),
                revision,
            )
            .await?;
            tokio::time::sleep(POLL_INTERVAL).await;

            let normalization_count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM product_listing_raw_normalizations WHERE product_listing_raw_revision_id = $1",
            )
            .bind(revision_id.as_uuid())
            .fetch_one(&pool)
            .await?;
            let duplicate_event_count: i64 =
                sqlx::query_scalar("SELECT count(*) FROM product_listing_events")
                    .fetch_one(&pool)
                    .await?;
            assert_eq!(1, normalization_count);
            assert_eq!(1, duplicate_event_count);
            Ok(())
        }
        .await;
        worker.finish(work_result).await
    }
    .await;

    if let Err(error) = result {
        panic!("raw normalization worker test failed: {error}");
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_normalize_direct_cdc_wakeup_without_waiting_for_reconciliation() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let listing_source_id = seed_listing_source(&pool, "raw-normalization-direct-cdc").await?;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();
        let barrier = capture(
            &unit_of_work,
            &capture_writer,
            raw_write(listing_source_id, "direct-cdc-barrier", 1, "EUR 100"),
        )
        .await?;
        let (_, barrier_revision_id, _) = changed_parts(barrier)?;

        let worker = RawNormalizationWorker::start(pool.clone()).await?;
        let work_result: Result<(), Box<dyn std::error::Error>> = async {
            wait_for_normalization(&pool, *barrier_revision_id.as_uuid(), 1).await?;

            let captured = capture(
                &unit_of_work,
                &capture_writer,
                raw_write(listing_source_id, "direct-cdc", 2, "EUR 120"),
            )
            .await?;
            let (stream_id, revision_id, revision) = changed_parts(captured)?;

            // The barrier completed before this distinct row existed. Its explicit CDC delivery
            // must normalize within four seconds, well below the 30-second reconciliation cadence.
            redeliver_raw_revision(
                *stream_id.as_uuid(),
                *revision_id.as_uuid(),
                revision,
            )
            .await?;
            tokio::time::timeout(
                DIRECT_CDC_TIMEOUT,
                wait_for_normalization_with_attempts(
                    &pool,
                    *revision_id.as_uuid(),
                    1,
                    DIRECT_CDC_POLL_ATTEMPTS,
                ),
            )
            .await
            .map_err(|_| "direct CDC wake-up did not normalize within four seconds")??;

            redeliver_raw_revision(
                *stream_id.as_uuid(),
                *revision_id.as_uuid(),
                revision,
            )
            .await?;
            tokio::time::sleep(POLL_INTERVAL).await;
            let normalization_count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM product_listing_raw_normalizations WHERE product_listing_raw_revision_id = $1",
            )
            .bind(revision_id.as_uuid())
            .fetch_one(&pool)
            .await?;
            assert_eq!(1, normalization_count);
            Ok(())
        }
        .await;
        worker.finish(work_result).await
    }
    .await;

    if let Err(error) = result {
        panic!("raw normalization direct CDC worker test failed: {error}");
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_normalize_committed_sequin_revisions_in_order_despite_reversed_duplicate_sqs_wakeups()
 {
    let result: support::TestResult = async {
        let pool = get_postgres_client().await;
        let source = seed_listing_source(&pool, "raw-sqs-order").await?;
        let uow = SqlxUnitOfWork::new(pool.clone());
        let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();
        let worker = RawNormalizationWorker::start(pool.clone()).await?;
        let result = async {
            // Startup repair has no rows. This first committed write must arrive through Sequin/SQS.
            tokio::time::sleep(Duration::from_millis(300)).await;
            let first = changed_parts(capture(&uow, &capture_writer, raw_write(source, "ordered", 7, "EUR 100")).await?)?;
            let normalized = tokio::time::timeout(
                DIRECT_CDC_TIMEOUT,
                wait_for_normalization(&pool, *first.1.as_uuid(), 1),
            )
            .await;
            if normalized.is_err() {
                use aws_sdk_sqs::types::QueueAttributeName as A;
                let counts = test_api::get_sqs_client().await.get_queue_attributes()
                    .queue_url(WORKER_SQS.queue_url())
                    .attribute_names(A::ApproximateNumberOfMessages)
                    .attribute_names(A::ApproximateNumberOfMessagesNotVisible)
                    .send().await?;
                return Err(format!("Sequin wake-up did not normalize within four seconds; SQS counts: {:?}", counts.attributes()).into());
            }
            normalized??;
            let second = changed_parts(capture(&uow, &capture_writer, raw_write(source, "ordered", 7, "EUR 120")).await?)?;
            assert_eq!(first.0, second.0);
            assert_eq!(1, first.2);
            assert_eq!(2, second.2);
            wait_for_normalization(&pool, *second.1.as_uuid(), 1).await?;
            for (stream, revision, number) in [second, first, second, first] {
                redeliver_raw_revision_sqs(
                    *stream.as_uuid(),
                    *revision.as_uuid(),
                    number,
                )
                .await?;
            }
            support::wait_until_empty(SCOPE).await?;
            let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as("SELECT revision, outcome, error_code FROM product_listing_raw_normalizations WHERE product_listing_raw_stream_id = $1 ORDER BY revision")
                .bind(first.0.as_uuid()).fetch_all(&pool).await?;
            assert_eq!(vec![(1, "APPLIED".to_owned(), None), (2, "APPLIED".to_owned(), None)], rows);
            let head: (i64, uuid::Uuid) = sqlx::query_as("SELECT last_processed_revision, product_listing_id FROM product_listing_raw_normalization_heads WHERE product_listing_raw_stream_id = $1")
                .bind(first.0.as_uuid()).fetch_one(&pool).await?;
            assert_eq!(2, head.0);
            let product: (i64, String, String) = sqlx::query_as("SELECT price_amount, price_currency, lifecycle FROM product_listings WHERE product_listing_id = $1")
                .bind(head.1).fetch_one(&pool).await?;
            assert_eq!((12000, "EUR".to_owned(), "ACTIVE".to_owned()), product);
            let events: Vec<String> = sqlx::query_scalar("SELECT event_type FROM product_listing_events WHERE product_listing_id = $1 ORDER BY event_time")
                .bind(head.1).fetch_all(&pool).await?;
            assert_eq!(vec!["PRODUCT_LISTING_DISCOVERED", "PRODUCT_LISTING_CHANGED"], events);
            Ok(())
        }.await;
        worker.finish(result).await
    }.await;
    result.expect("ordered Sequin/SQS raw normalization and cleanup");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_not_normalize_rollback_or_accept_unrouted_raw_changes() {
    let result: support::TestResult = async {
        let pool = get_postgres_client().await;
        let source = seed_listing_source(&pool, "raw-sqs-rollback").await?;
        let worker = RawNormalizationWorker::start(pool.clone()).await?;
        let result = async {
            let uow = SqlxUnitOfWork::new(pool.clone());
            let mut tx = uow.begin().await?;
            let outcome = SqlxProductListingRawCaptureWriterFactory::new()
                .in_transaction(&mut tx)
                .capture(raw_write(source, "rollback", 9, "EUR 100"))
                .await?;
            let (stream, revision, number) = changed_parts(outcome)?;
            drop(tx);
            let client = reqwest::Client::new();
            for (table, operation) in [
                ("product_listing_raw_revisions", "update"),
                ("product_listing_raw_streams", "insert"),
            ] {
                let response = client
                    .post(format!(
                        "http://127.0.0.1:{}/cdc/sequin",
                        get_sequin_worker_webhook_bind_addr().port()
                    ))
                    .json(
                        &json!({"changes": [{"table": table, "operation": operation, "record": {
                            "product_listing_raw_stream_id": stream.as_uuid().to_string(),
                            "product_listing_raw_revision_id": revision.as_uuid().to_string(),
                            "revision": number,
                        }}]}),
                    )
                    .send()
                    .await?;
                assert_eq!(reqwest::StatusCode::SERVICE_UNAVAILABLE, response.status());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            support::wait_until_empty(SCOPE).await?;
            for query in [
                "SELECT count(*) FROM product_listing_raw_revisions",
                "SELECT count(*) FROM product_listing_raw_normalizations",
                "SELECT count(*) FROM product_listings",
                "SELECT count(*) FROM product_listing_events",
            ] {
                assert_eq!(
                    0,
                    sqlx::query_scalar::<_, i64>(query).fetch_one(&pool).await?
                );
            }
            Ok(())
        }
        .await;
        worker.finish(result).await
    }
    .await;
    result.expect("rollback/filter raw SQS acceptance and cleanup");
}

struct RawNormalizationWorker {
    shutdown_tx: oneshot::Sender<()>,
    consumer_shutdown: watch::Sender<bool>,
    server: JoinHandle<Result<(), WorkerRunError>>,
    consumer: JoinHandle<()>,
}

impl RawNormalizationWorker {
    async fn start(pool: sqlx::PgPool) -> Result<Self, Box<dyn std::error::Error>> {
        let handler: Arc<dyn NormalizeProductListingRawRevisionUseCase> =
            Arc::new(NormalizeProductListingRawRevisionHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxProductListingRawNormalizationWriterFactory::new(),
                SqlxProductListingRepositoryFactory::new(),
                SqlxProductListingEventAppenderFactory::new(),
                SqlxPendingProductListingRawStreamReader::new(pool),
            ));
        let (runtime, receiver) = support::composition(SCOPE).await?.into_parts();

        let (consumer_shutdown, consumer_shutdown_rx) = watch::channel(false);
        let consumer = support::competing_consumers(SCOPE, receiver, move |receiver| {
            consume_product_listing_raw_normalization_queue(
                receiver,
                handler.clone(),
                consumer_shutdown_rx.clone(),
            )
        })
        .await?;
        let listener = tokio::net::TcpListener::bind(get_sequin_worker_webhook_bind_addr()).await?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));
        Ok(Self {
            shutdown_tx,
            consumer_shutdown,
            server,
            consumer,
        })
    }

    async fn finish(
        self,
        result: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let _send_result = self.shutdown_tx.send(());
        let _previous_shutdown = self.consumer_shutdown.send_replace(true);
        let server_result = self.server.await;
        self.consumer.await?;
        server_result??;
        result
    }
}

async fn redeliver_raw_revision(
    stream_id: uuid::Uuid,
    revision_id: uuid::Uuid,
    revision: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let body = json!({
        "changes": [{
            "table": "product_listing_raw_revisions",
            "operation": "insert",
            "record": {
                "product_listing_raw_stream_id": stream_id.to_string(),
                "product_listing_raw_revision_id": revision_id.to_string(),
                "revision": revision,
            }
        }]
    });
    let url = format!(
        "http://127.0.0.1:{}/cdc/sequin",
        get_sequin_worker_webhook_bind_addr().port()
    );
    let response = reqwest::Client::new().post(url).json(&body).send().await?;
    assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());
    Ok(())
}

async fn redeliver_raw_revision_sqs(
    stream_id: uuid::Uuid,
    revision_id: uuid::Uuid,
    revision: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream_id = ProductListingRawStreamId::try_from(stream_id)?;
    let revision_id = ProductListingRawRevisionId::try_from(revision_id)?;
    let body = json!({
        "schema_version": 2,
        "scope": SCOPE.as_str(),
        "job_type": "PRODUCT_LISTING_RAW_REVISION",
        "idempotency_key": format!("product-listing-raw-revision:{revision_id}"),
        "ordering_key": format!("product-listing-raw-stream:{stream_id}"),
        "payload": {
            "product_listing_raw_stream_id": stream_id,
            "product_listing_raw_revision_id": revision_id,
            "revision": revision,
        }
    })
    .to_string();
    let response = get_sqs_client()
        .await
        .send_message()
        .queue_url(WORKER_SQS.queue_url())
        .message_body(body)
        .send()
        .await?;
    response.message_id().ok_or("SQS send missing message ID")?;
    Ok(())
}

async fn wait_for_normalization(
    pool: &sqlx::PgPool,
    revision_id: uuid::Uuid,
    expected_count: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    wait_for_normalization_with_attempts(pool, revision_id, expected_count, POLL_ATTEMPTS).await
}

async fn wait_for_normalization_with_attempts(
    pool: &sqlx::PgPool,
    revision_id: uuid::Uuid,
    expected_count: i64,
    attempts: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..attempts {
        let actual_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM product_listing_raw_normalizations WHERE product_listing_raw_revision_id = $1",
        )
        .bind(revision_id)
        .fetch_one(pool)
        .await?;
        if actual_count == expected_count {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err("timed out waiting for raw revision normalization".into())
}

fn raw_write(
    listing_source_id: ListingSourceId,
    record_key: &str,
    hash_byte: u8,
    price: &str,
) -> ProductListingRawCaptureWrite {
    let input = ProductListingNormalizationInput::new(
        RawProductListingOperation::Upsert,
        RawProductListingPayloadFormat::WoocommerceProduct,
        1,
        1,
        SourcePayload::new(json!({"retainedUnknown": record_key}))
            .unwrap_or_else(|error| panic!("source payload: {error}")),
        RawProductListingValues::new(json!({
            "sourceListingId": "worker-source-123",
            "priceFormat": "DISPLAY_TEXT",
            "title": {"action": "SET", "value": "An antique ceramic vase from an English collection"},
            "description": {"action": "SET", "value": ["This antique ceramic vase has documented provenance and careful restoration history."]},
            "price": {"action": "SET", "value": price},
            "priceEstimateMin": {"action": "CLEAR"},
            "priceEstimateMax": {"action": "CLEAR"},
            "availability": {"action": "SET", "value": "in stock"},
            "url": {"action": "SET", "value": "https://example.test/listings/worker-source-123"},
            "images": {"action": "SET", "value": ["/images/worker-source-123.jpg"]},
            "auctionStart": {"action": "UNCHANGED"},
            "auctionEnd": {"action": "UNCHANGED"},
            "attributes": {}
        }))
        .unwrap_or_else(|error| panic!("raw values: {error}")),
        NormalizationContext::new(json!({
            "baseUrl": "https://example.test/listings/worker-source-123",
            "fallbackCurrency": "EUR"
        }))
        .unwrap_or_else(|error| panic!("normalization context: {error}")),
    )
    .unwrap_or_else(|error| panic!("normalization input: {error}"));
    let input_sha256 = input
        .hash()
        .unwrap_or_else(|error| panic!("normalization input hash: {error}"));
    ProductListingRawCaptureWrite {
        listing_source_id,
        ingestion_method: ProductListingRawIngestionMethod::Woocommerce,
        source_record_key: record_key.to_owned(),
        source_record_key_sha256: SourceRecordKeySha256::new([hash_byte; 32]),
        input,
        input_sha256,
        provenance: RawProductListingProvenance::new(json!({"deliveryId": record_key}))
            .unwrap_or_else(|error| panic!("provenance: {error}")),
        source_event_id: Some(record_key.to_owned()),
        source_occurred_at: None,
        provider_receipt: None,
    }
}

async fn capture(
    unit_of_work: &SqlxUnitOfWork,
    factory: &SqlxProductListingRawCaptureWriterFactory,
    write: ProductListingRawCaptureWrite,
) -> Result<ProductListingRawCaptureWriteOutcome, Box<dyn std::error::Error>> {
    let mut tx = unit_of_work.begin().await?;
    let outcome = factory.in_transaction(&mut tx).capture(write).await?;
    tx.commit().await?;
    Ok(outcome)
}

fn changed_parts(
    outcome: ProductListingRawCaptureWriteOutcome,
) -> Result<
    (
        product_listing_service::ports::ProductListingRawStreamId,
        product_listing_service::ports::ProductListingRawRevisionId,
        u64,
    ),
    Box<dyn std::error::Error>,
> {
    match outcome {
        ProductListingRawCaptureWriteOutcome::Changed {
            product_listing_raw_stream_id,
            product_listing_raw_revision_id,
            revision,
        } => Ok((
            product_listing_raw_stream_id,
            product_listing_raw_revision_id,
            revision,
        )),
        ProductListingRawCaptureWriteOutcome::Unchanged { .. }
        | ProductListingRawCaptureWriteOutcome::Duplicate { .. }
        | ProductListingRawCaptureWriteOutcome::Stale { .. } => {
            Err("test input must create a raw revision".into())
        }
    }
}

async fn seed_listing_source(
    pool: &sqlx::PgPool,
    slug: &str,
) -> Result<ListingSourceId, sqlx::Error> {
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(*listing_source_id.as_uuid())
        .bind(slug)
        .bind(slug)
        .bind(party_id)
        .execute(pool)
        .await?;
    Ok(listing_source_id)
}
