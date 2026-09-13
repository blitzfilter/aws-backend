use aura_historia_worker::{
    WorkerRunError, WorkerScope,
    product_content_assessment::consume_product_content_assessment_queue, serve_with_runtime,
};
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_postgres::{
    SqlxProductListingContentAssessmentSourceReader,
    SqlxProductListingContentAssessmentWriterFactory,
};
use product_listing_service::use_cases::{
    AssessProductListingContentEventHandler, AssessProductListingContentEventUseCase,
};
use std::{sync::Arc, time::Duration};
use test_api::{
    IntegrationTestService, Postgres, Sequin, aura_integration_test, get_postgres_client,
    get_sequin_worker_webhook_bind_addr,
};
use tokio::{sync::oneshot, task::JoinHandle};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
mod support;
const SCOPE: WorkerScope = WorkerScope::ProductListingContentAssessment;
const WORKER_SQS: test_api::WorkerSqs = support::queues(SCOPE);
const WORKER_SEQUIN: Sequin = Sequin::worker_webhook_for_tables(&["public.product_listing_events"]);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const POLL_ATTEMPTS: usize = 80;
const NO_SIDE_EFFECT_OBSERVATION: Duration = Duration::from_secs(2);

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_assess_committed_created_product_event_as_allowed() {
    let worker = ContentAssessmentWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, content_source_event_id) = insert_product_with_event(
            &worker.pool,
            "PRODUCT_LISTING_DISCOVERED",
            "DOMAIN",
            "Antiker Eichenstuhl",
            "Bemalter Stuhl",
        )
        .await?;

        let assessment = wait_for_assessment(&worker.pool, product_listing_id).await?;

        assert_eq!(uuid::Uuid::from(content_source_event_id), assessment.0);
        assert_eq!("ALLOWED", assessment.1);
        assert_eq!(None, assessment.2);
        Ok(())
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_assess_committed_created_product_event_as_requires_consent() {
    let worker = ContentAssessmentWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, content_source_event_id) = insert_product_with_event(
            &worker.pool,
            "PRODUCT_LISTING_DISCOVERED",
            "DOMAIN",
            "Hakenkreuz-Abzeichen",
            "Historisches Abzeichen.",
        )
        .await?;

        let assessment = wait_for_assessment(&worker.pool, product_listing_id).await?;

        assert_eq!(uuid::Uuid::from(content_source_event_id), assessment.0);
        assert_eq!("REQUIRES_CONSENT", assessment.1);
        assert_eq!(Some("NAZI_GERMANY".to_owned()), assessment.2);
        Ok(())
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_not_assess_committed_price_event() {
    let worker = ContentAssessmentWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, _) = insert_product_with_event(
            &worker.pool,
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
            "Hakenkreuz-Abzeichen",
            "This event must not route to content assessment.",
        )
        .await?;

        assert_no_assessment(&worker.pool, product_listing_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_not_assess_rolled_back_discovery() {
    let worker = ContentAssessmentWorker::start().await;
    let result: support::TestResult = async {
        let mut tx = worker.pool.begin().await?;
        let (id, _) = insert_product_in_transaction(
            &mut tx,
            "PRODUCT_LISTING_DISCOVERED",
            "DOMAIN",
            "Hakenkreuz-Abzeichen",
            "Historisches Abzeichen.",
        )
        .await?;
        tx.rollback().await?;
        assert_no_assessment(&worker.pool, id, NO_SIDE_EFFECT_OBSERVATION).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM product_listings WHERE product_listing_id = $1)",
        )
        .bind(uuid::Uuid::from(id))
        .fetch_one(&worker.pool)
        .await?;
        assert!(!exists);
        Ok(())
    }
    .await;
    worker
        .finish(result)
        .await
        .expect("rollback acceptance and cleanup");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_preserve_exact_assessments_after_reversed_duplicate_sqs_deliveries() {
    let worker = ContentAssessmentWorker::start().await;
    let result: support::TestResult = async {
        let first = insert_product_with_event(
            &worker.pool,
            "PRODUCT_LISTING_DISCOVERED",
            "DOMAIN",
            "Antiker Eichenstuhl",
            "Bemalter Stuhl",
        )
        .await?;
        let second = insert_product_with_event(
            &worker.pool,
            "PRODUCT_LISTING_DISCOVERED",
            "DOMAIN",
            "Hakenkreuz-Abzeichen",
            "Historisches Abzeichen.",
        )
        .await?;
        let first_assessment = wait_for_assessment(&worker.pool, first.0).await?;
        let second_assessment = wait_for_assessment(&worker.pool, second.0).await?;
        for event in [second.1, first.1, second.1, first.1] {
            support::redeliver_product_event(&worker.pool, uuid::Uuid::from(event)).await?;
        }
        support::wait_until_empty(SCOPE).await?;
        assert_eq!(
            (uuid::Uuid::from(first.1), "ALLOWED".to_owned(), None),
            first_assessment
        );
        assert_eq!(
            (
                uuid::Uuid::from(second.1),
                "REQUIRES_CONSENT".to_owned(),
                Some("NAZI_GERMANY".to_owned())
            ),
            second_assessment
        );
        assert_eq!(
            Some(first_assessment),
            assessment(&worker.pool, first.0).await?
        );
        assert_eq!(
            Some(second_assessment),
            assessment(&worker.pool, second.0).await?
        );
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM product_listing_content_assessments")
                .fetch_one(&worker.pool)
                .await?;
        assert_eq!(2, count);
        Ok(())
    }
    .await;
    worker
        .finish(result)
        .await
        .expect("duplicate acceptance and cleanup");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SQS, WORKER_SEQUIN])]
async fn should_skip_discovery_after_content_source_advances() {
    let pool = get_postgres_client().await;
    let (id, event) = insert_product_with_event(
        &pool,
        "PRODUCT_LISTING_DISCOVERED",
        "DOMAIN",
        "Hakenkreuz-Abzeichen",
        "Historisches Abzeichen.",
    )
    .await
    .expect("seed discovery");
    let newer = EventId::new();
    let mut tx = pool.begin().await.expect("begin revision");
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
        .bind(newer.as_uuid()).bind(uuid::Uuid::from(id)).bind(serde_json::json!({"images": {"previousCount": 0, "currentCount": 0}}))
        .execute(&mut *tx).await.expect("new event");
    sqlx::query("UPDATE product_listings SET current_event_id = $1, content_source_event_id = $1, version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2")
        .bind(newer.as_uuid()).bind(uuid::Uuid::from(id)).execute(&mut *tx).await.expect("advance source");
    tx.commit().await.expect("commit revision");
    let worker = ContentAssessmentWorker::start().await;
    let result: support::TestResult = async {
        support::redeliver_product_event(&pool, uuid::Uuid::from(event)).await?;
        support::wait_until_empty(SCOPE).await?;
        assert_no_assessment(&pool, id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .expect("stale acceptance and cleanup");
}

struct ContentAssessmentWorker {
    pool: sqlx::PgPool,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
    consumer: JoinHandle<()>,
}

impl ContentAssessmentWorker {
    async fn start() -> Self {
        let pool = get_postgres_client().await;
        let handler: Arc<dyn AssessProductListingContentEventUseCase> =
            Arc::new(AssessProductListingContentEventHandler::new(
                SqlxProductListingContentAssessmentSourceReader::new(pool.clone()),
                SqlxUnitOfWork::new(pool.clone()),
                SqlxProductListingContentAssessmentWriterFactory::new(),
            ));
        let (runtime, receiver) = support::composition(SCOPE)
            .await
            .unwrap_or_else(|error| panic!("valid scoped SQS configuration: {error}"))
            .into_parts();

        let consumer = support::competing_consumers(SCOPE, receiver, move |receiver| {
            consume_product_content_assessment_queue(receiver, handler.clone())
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
    title: &str,
    description: &str,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let result =
        insert_product_in_transaction(&mut tx, event_type, event_group, title, description).await?;
    tx.commit().await?;
    Ok(result)
}

async fn insert_product_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_type: &str,
    event_group: &str,
    title: &str,
    description: &str,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let product_listing_uuid = uuid::Uuid::from(product_listing_id);
    let event_id = EventId::new();
    let title_slug_id = ProductListingSlugId::from_title_and_suffix(
        "content assessment worker product",
        &product_listing_uuid.simple().to_string()[26..],
    )
    .map_err(|_| sqlx::Error::Protocol("invalid fixture title slug".to_owned()))?;
    let listing_source_id = ListingSourceId::new();
    let operator_party_id = uuid::Uuid::now_v7();
    let source_listing_id = "content-assessment-fixture-product";
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'Content assessment worker source', party_id FROM operator")
        .bind(operator_party_id)
        .bind(format!(
            "content-assessment-worker-source-{}",
            listing_source_id.as_uuid()
        ))
        .bind(listing_source_id.as_uuid())
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, 'de', $7, 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_listing_uuid)
        .bind(title_slug_id.as_ref())
        .bind(event_id.as_uuid())
        .bind(listing_source_id.as_uuid())
        .bind(source_listing_id)
        .bind(title)
        .bind(description)

        .execute(&mut **tx)
        .await?;
    let payload = if event_type == "PRODUCT_LISTING_CHANGED" {
        serde_json::json!({"pricing": {"price": {"previous": null, "current": {"type": "MONETARY", "amount": 1200, "currency": "EUR"}}}})
    } else {
        serde_json::json!({
            "listingSourceId": listing_source_id.as_uuid().to_string(),
            "sourceListingId": source_listing_id,
            "title": {"language": "de", "text": title},
            "description": {"language": "de", "text": description},
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": null
        })
    };
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, $3, $4, 1, $5, now())")
        .bind(event_id.as_uuid())
        .bind(product_listing_uuid)
        .bind(event_type)
        .bind(event_group)
        .bind(payload)
        .execute(&mut **tx)
        .await?;
    Ok((product_listing_id, event_id))
}

async fn wait_for_assessment(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<(uuid::Uuid, String, Option<String>), Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        if let Some(assessment) = assessment(pool, product_listing_id).await? {
            return Ok(assessment);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other("timed out waiting for product content assessment").into())
}

async fn assert_no_assessment(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        if assessment(pool, product_listing_id).await?.is_some() {
            return Err(
                std::io::Error::other("unexpected product content assessment persisted").into(),
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}

async fn assessment(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<Option<(uuid::Uuid, String, Option<String>)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT source_event_id, decision, category FROM product_listing_content_assessments WHERE product_listing_id = $1",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_optional(pool)
    .await
}
