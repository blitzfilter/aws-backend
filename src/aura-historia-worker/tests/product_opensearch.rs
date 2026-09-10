use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aura_historia_worker::{
    WorkerRunError, WorkerScope,
    product_listing_opensearch::consume_product_listing_opensearch_queue, serve_with_runtime,
};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use listing_source_core::ListingSourceId;
use listing_source_postgres::SqlxListingSourceRepositoryFactory;
use listing_source_service::use_cases::commands::delete_listing_source::{
    DeleteListingSourceCommand, DeleteListingSourceError, DeleteListingSourceHandler,
    DeleteListingSourceUseCase,
};
use opensearch::GetParts;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_opensearch::OpenSearchProductListingSearchProjection;
use product_listing_postgres::SqlxProductListingSearchFilterMatchSourceReaderFactory;
use product_listing_service::use_cases::{
    ProjectProductListingHandler, ProjectProductListingUseCase,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use test_api::{
    IntegrationTestService, OpenSearch, Postgres, Sequin, aura_integration_test,
    get_opensearch_client, get_postgres_client, get_sequin_worker_webhook_bind_addr, refresh_index,
};
use tokio::{sync::oneshot, task::JoinHandle};
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminResult, CheckUserAdminUseCase,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
mod support;
const SCOPE: WorkerScope = WorkerScope::ProductListingOpenSearch;
const WORKER_SQS: test_api::WorkerSqs = support::queues(SCOPE);
const WORKER_SEQUIN: Sequin = Sequin::worker_webhook_for_tables(&["public.product_listing_events"]);
const PRODUCT_LISTINGS_INDEX: &str = "product-listings";
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const POLL_ATTEMPTS: usize = 80;
const NO_PROJECTION_OBSERVATION: Duration = Duration::from_secs(2);

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_project_committed_active_product_with_native_source_price_and_no_estimates() {
    let worker = ProductListingOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let fixture = insert_active_product_with_event(&worker.pool, 7).await?;

        let response = wait_for_product_response(fixture.product_listing_id).await?;
        let document = response.get("_source").ok_or_else(|| {
            std::io::Error::other("projected ProductListing response has no _source")
        })?;

        assert_eq!(Some(7), response.get("_version").and_then(Value::as_i64));
        assert_eq!(
            Some(fixture.product_listing_id.to_string().as_str()),
            document.get("productListingId").and_then(Value::as_str)
        );
        assert_eq!(
            Some(fixture.listing_source_id.to_string().as_str()),
            document.get("listingSourceId").and_then(Value::as_str)
        );
        assert_eq!(
            Some(fixture.event_id.to_string().as_str()),
            document.get("eventId").and_then(Value::as_str)
        );
        assert_eq!(
            Some("Active ProductListing OpenSearch chair"),
            document.pointer("/title/text").and_then(Value::as_str)
        );
        assert_eq!(
            Some("EN"),
            document.pointer("/title/language").and_then(Value::as_str)
        );
        assert_eq!(
            Some(&json!({ "type": "MONETARY", "amount": 12_345, "currency": "USD" })),
            document.get("sourcePrice")
        );
        assert!(document.get("priceEstimateMin").is_none());
        assert!(document.get("priceEstimateMax").is_none());
        assert!(document.get("salePrices").is_none());
        assert!(document.get("saleObservationFxRateId").is_none());
        assert!(document.get("saleObservedAt").is_none());
        assert_eq!(
            Some("AVAILABLE"),
            document.get("availability").and_then(Value::as_str)
        );
        assert!(document.get("lifecycle").is_none());
        Ok(())
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_not_project_rolled_back_product_event() {
    let worker = ProductListingOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let product_listing_id = insert_product_with_event_then_rollback(&worker.pool).await?;

        assert_no_product_projection(product_listing_id, NO_PROJECTION_OBSERVATION).await
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_keep_product_projection_unchanged_when_event_is_redelivered() {
    let worker = ProductListingOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let fixture = insert_active_product_with_event(&worker.pool, 9).await?;
        let expected = wait_for_product_response(fixture.product_listing_id).await?;

        worker
            .redeliver(
                fixture.product_listing_id,
                fixture.event_id,
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
            )
            .await?;
        assert_product_response_unchanged_for(
            fixture.product_listing_id,
            &expected,
            NO_PROJECTION_OBSERVATION,
        )
        .await
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_skip_stale_product_event_trigger() {
    let worker = ProductListingOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let fixture = insert_active_product_with_event(&worker.pool, 11).await?;
        let _ = wait_for_product_response(fixture.product_listing_id).await?;
        let current_event_id =
            advance_product_revision(&worker.pool, fixture.product_listing_id).await?;
        let expected = wait_for_product_event(fixture.product_listing_id, current_event_id).await?;

        assert_eq!(
            Some(current_event_id.to_string().as_str()),
            expected.pointer("/_source/eventId").and_then(Value::as_str)
        );
        worker
            .redeliver(
                fixture.product_listing_id,
                fixture.event_id,
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
            )
            .await?;
        assert_product_response_unchanged_for(
            fixture.product_listing_id,
            &expected,
            NO_PROJECTION_OBSERVATION,
        )
        .await
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_preserve_live_product_projection_and_withdrawn_tombstone_when_source_delete_is_blocked()
 {
    let worker = ProductListingOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let fixture = insert_active_product_with_event(&worker.pool, 21).await?;
        let live_document = wait_for_product_response(fixture.product_listing_id).await?;

        let active_delete = delete_listing_source_as_system(&worker.pool, fixture.listing_source_id).await;
        assert!(matches!(
            active_delete,
            Err(DeleteListingSourceError::DependencyConflict { .. })
        ));
        assert_product_response_unchanged_for(
            fixture.product_listing_id,
            &live_document,
            NO_PROJECTION_OBSERVATION,
        )
        .await?;

        let _withdrawn_event_id = withdraw_product_listing(&worker.pool, fixture.product_listing_id).await?;
        let tombstone = wait_for_product_tombstone(fixture.product_listing_id, 22).await?;
        let withdrawn_delete = delete_listing_source_as_system(&worker.pool, fixture.listing_source_id).await;
        assert!(matches!(
            withdrawn_delete,
            Err(DeleteListingSourceError::DependencyConflict { .. })
        ));
        assert_product_response_unchanged_for(
            fixture.product_listing_id,
            &tombstone,
            NO_PROJECTION_OBSERVATION,
        )
        .await?;

        let source_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM listing_sources WHERE listing_source_id = $1)",
        )
        .bind(uuid::Uuid::from(fixture.listing_source_id))
        .fetch_one(&worker.pool)
        .await?;
        let withdrawn_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM product_listings WHERE product_listing_id = $1 AND lifecycle = 'WITHDRAWN')",
        )
        .bind(uuid::Uuid::from(fixture.product_listing_id))
        .fetch_one(&worker.pool)
        .await?;
        assert!(source_exists);
        assert!(withdrawn_exists);
        Ok(())
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_delete_withdrawn_listing_then_reproject_restored_listing_without_stale_removal() {
    let worker = ProductListingOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let fixture = insert_active_product_with_event(&worker.pool, 15).await?;
        let response = wait_for_product_response(fixture.product_listing_id).await?;
        assert_eq!(Some(15), response.get("_version").and_then(Value::as_i64));

        let withdrawn_event_id = withdraw_product_listing(&worker.pool, fixture.product_listing_id).await?;
        let tombstone = wait_for_product_tombstone(fixture.product_listing_id, 16).await?;
        assert_product_readers_empty().await?;
        for event_id in [withdrawn_event_id, fixture.event_id, withdrawn_event_id, fixture.event_id] {
            support::redeliver_product_event(&worker.pool, uuid::Uuid::from(event_id)).await?;
        }
        support::wait_until_empty(SCOPE).await?;
        assert_product_response_unchanged_for(fixture.product_listing_id, &tombstone, NO_PROJECTION_OBSERVATION).await?;
        assert_product_readers_empty().await?;

        let restored_event_id = restore_product_listing(&worker.pool, fixture.product_listing_id).await?;
        let restored = wait_for_product_event(fixture.product_listing_id, restored_event_id).await?;
        assert_eq!(Some(17), restored.get("_version").and_then(Value::as_i64));
        let document = restored
            .get("_source")
            .ok_or_else(|| std::io::Error::other("restored ProductListing response has no _source"))?;
        assert!(document.get("availability").is_none());
        assert!(document.get("lifecycle").is_none());

        worker
            .redeliver(
                fixture.product_listing_id,
                withdrawn_event_id,
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
            )
            .await?;
        assert_product_response_unchanged_for(
            fixture.product_listing_id,
            &restored,
            NO_PROJECTION_OBSERVATION,
        )
        .await?;

        worker
            .redeliver(
                fixture.product_listing_id,
                restored_event_id,
                "PRODUCT_LISTING_CHANGED",
                "DOMAIN",
            )
            .await?;
        assert_product_response_unchanged_for(
            fixture.product_listing_id,
            &restored,
            NO_PROJECTION_OBSERVATION,
        )
        .await?;

        let (current_event_id, version, projection_version, lifecycle, availability):
            (uuid::Uuid, i64, i64, String, Option<String>) = sqlx::query_as(
            "SELECT current_event_id, version, projection_version, lifecycle, availability FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(uuid::Uuid::from(fixture.product_listing_id))
        .fetch_one(&worker.pool)
        .await?;
        assert_eq!(uuid::Uuid::from(restored_event_id), current_event_id);
        assert_eq!(3, version);
        assert_eq!(17, projection_version);
        assert_eq!("ACTIVE", lifecycle);
        assert_eq!(None, availability);
        Ok(())
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_project_sold_product_with_all_sale_price_currencies() {
    let worker = ProductListingOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let fx_rate_id = insert_equal_rate_snapshot(&worker.pool).await?;
        let fixture = insert_sold_product_with_event(&worker.pool, fx_rate_id).await?;

        let response = wait_for_product_response(fixture.product_listing_id).await?;
        let document = response.get("_source").ok_or_else(|| {
            std::io::Error::other("projected ProductListing response has no _source")
        })?;
        let sale_prices = document.get("salePrices").ok_or_else(|| {
            std::io::Error::other("sold ProductListing projection has no salePrices")
        })?;

        assert_eq!(
            Some(&json!({ "type": "MONETARY", "amount": 12_345, "currency": "EUR" })),
            document.get("sourcePrice")
        );
        assert_eq!(
            Some(fx_rate_id.to_string().as_str()),
            document
                .get("saleObservationFxRateId")
                .and_then(Value::as_str)
        );
        assert!(
            document
                .get("saleObservedAt")
                .and_then(Value::as_str)
                .is_some()
        );
        for currency in [
            "eur", "gbp", "usd", "aud", "cad", "nzd", "cny", "brl", "pln", "try", "jpy", "czk",
            "rub", "aed", "sar", "hkd", "sgd", "chf", "zar",
        ] {
            let expected = if currency == "jpy" { 123 } else { 12_345 };
            assert_eq!(
                Some(expected),
                sale_prices.get(currency).and_then(Value::as_i64),
                "sale price for {currency}"
            );
        }
        assert_eq!(
            Some("SOLD_OUT"),
            document.get("availability").and_then(Value::as_str)
        );
        Ok(())
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_project_sold_product_without_main_price_then_add_sale_prices_when_corrected() {
    let worker = ProductListingOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let fx_rate_id = insert_equal_rate_snapshot(&worker.pool).await?;
        let fixture =
            insert_sold_product_without_main_price_with_event(&worker.pool, fx_rate_id).await?;

        let initial = wait_for_product_response(fixture.product_listing_id).await?;
        let initial_document = initial.get("_source").ok_or_else(|| {
            std::io::Error::other("projected ProductListing response has no _source")
        })?;
        assert!(initial_document.get("sourcePrice").is_none());
        assert!(initial_document.get("salePrices").is_none());
        assert_eq!(
            Some(fx_rate_id.to_string().as_str()),
            initial_document
                .get("saleObservationFxRateId")
                .and_then(Value::as_str)
        );
        assert!(
            initial_document
                .get("saleObservedAt")
                .and_then(Value::as_str)
                .is_some()
        );
        assert!(initial_document.get("priceEstimateMin").is_none());
        assert!(initial_document.get("priceEstimateMax").is_none());

        let corrected_event_id =
            correct_sold_product_main_price(&worker.pool, fixture.product_listing_id).await?;
        let corrected =
            wait_for_product_event(fixture.product_listing_id, corrected_event_id).await?;
        let corrected_document = corrected.get("_source").ok_or_else(|| {
            std::io::Error::other("corrected ProductListing response has no _source")
        })?;
        assert_eq!(
            Some(&json!({ "type": "MONETARY", "amount": 12_345, "currency": "EUR" })),
            corrected_document.get("sourcePrice")
        );
        let sale_prices = corrected_document.get("salePrices").ok_or_else(|| {
            std::io::Error::other("corrected sold ProductListing has no salePrices")
        })?;
        for currency in [
            "eur", "gbp", "usd", "aud", "cad", "nzd", "cny", "brl", "pln", "try", "jpy", "czk",
            "rub", "aed", "sar", "hkd", "sgd", "chf", "zar",
        ] {
            let expected = if currency == "jpy" { 123 } else { 12_345 };
            assert_eq!(
                Some(expected),
                sale_prices.get(currency).and_then(Value::as_i64)
            );
        }
        Ok(())
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_reject_unrouted_product_cdc_without_creating_a_projection() {
    let worker = ProductListingOpenSearchWorker::start().await;
    let result: support::TestResult = async {
        let id = ProductListingId::new();
        for (table, operation) in [("product_listing_events", "update"), ("product_listings", "insert")] {
            let response = reqwest::Client::new()
                .post(format!("http://127.0.0.1:{}/cdc/sequin", get_sequin_worker_webhook_bind_addr().port()))
                .json(&json!({"changes": [{"table": table, "operation": operation, "record": {"product_listing_id": id.as_uuid()}}]}))
                .send().await?;
            assert_eq!(reqwest::StatusCode::SERVICE_UNAVAILABLE, response.status());
        }
        support::wait_until_empty(SCOPE).await?;
        assert_no_product_projection(id, NO_PROJECTION_OBSERVATION).await
    }.await;
    worker
        .finish(result)
        .await
        .expect("unrouted projection acceptance and cleanup");
}

struct ProductListingFixture {
    product_listing_id: ProductListingId,
    listing_source_id: ListingSourceId,
    event_id: EventId,
}

struct SystemOnlyAdminCheck;

#[async_trait::async_trait]
impl CheckUserAdminUseCase for SystemOnlyAdminCheck {
    async fn execute(
        &self,
        _: &OperationContext,
        _: CheckUserAdminRequest,
    ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
        Err(CheckUserAdminError::Forbidden)
    }
}

async fn delete_listing_source_as_system(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
) -> Result<(), DeleteListingSourceError> {
    DeleteListingSourceHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxListingSourceRepositoryFactory::new(),
        SystemOnlyAdminCheck,
    )
    .execute(
        &OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("product-opensearch-listing-source-delete-test"),
            correlation_id: CorrelationId::new(listing_source_id.to_string()),
        },
        DeleteListingSourceCommand { listing_source_id },
    )
    .await
}

struct ProductListingOpenSearchWorker {
    pool: sqlx::PgPool,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
    consumer: JoinHandle<()>,
}

impl ProductListingOpenSearchWorker {
    async fn start() -> Self {
        let pool = get_postgres_client().await;
        let handler: Arc<dyn ProjectProductListingUseCase> =
            Arc::new(ProjectProductListingHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
                SqlxFxRateSnapshotRepositoryFactory,
                OpenSearchProductListingSearchProjection::new(
                    get_opensearch_client().await.clone(),
                ),
            ));
        let (runtime, receiver) = support::composition(SCOPE)
            .await
            .unwrap_or_else(|error| panic!("valid scoped SQS configuration: {error}"))
            .into_parts();
        let consumer = support::competing_consumers(SCOPE, receiver, move |receiver| {
            consume_product_listing_opensearch_queue(receiver, handler.clone())
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
        let response = reqwest::Client::new()
            .post(format!(
                "http://127.0.0.1:{}/cdc/sequin",
                get_sequin_worker_webhook_bind_addr().port()
            ))
            .json(&json!({
                "record": {
                    "event_id": event_id.as_uuid(),
                    "product_listing_id": product_listing_id.as_uuid(),
                    "event_type": event_type,
                    "event_group": event_group,
                    "event_type_schema_version": 1,
                    "payload": payload,
                },
                "action": "insert",
                "metadata": {
                    "table_schema": "public",
                    "table_name": "product_listing_events",
                }
            }))
            .send()
            .await?;
        if response.status() != reqwest::StatusCode::ACCEPTED {
            return Err(std::io::Error::other(
                "worker did not accept ProductListing event redelivery",
            )
            .into());
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

async fn insert_active_product_with_event(
    pool: &sqlx::PgPool,
    projection_version: i64,
) -> Result<ProductListingFixture, sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let listing_source_id = ListingSourceId::new();
    let mut tx = pool.begin().await?;
    insert_listing_source(&mut tx, listing_source_id, "active-os").await?;
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, price_kind, price_amount, price_currency, price_estimate_min_amount, price_estimate_min_currency, price_estimate_max_amount, price_estimate_max_currency, availability, lifecycle, url, product_images, projection_version) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Active ProductListing OpenSearch chair', 'en', 'Native source price only', 'en', 'MONETARY', 12345, 'USD', 10000, 'USD', 15000, 'USD', 'AVAILABLE', 'ACTIVE', 'https://example.test/product_listings/active', '[{\"url\": \"https://example.test/images/active.jpg\"}]', $6)",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(product_slug("active-os", product_listing_id))
    .bind(uuid::Uuid::from(event_id))
    .bind(listing_source_id.as_uuid())
    .bind(product_listing_id.to_string())
    .bind(projection_version)

        .execute(&mut *tx)
    .await?;
    insert_product_event(&mut tx, product_listing_id, event_id).await?;
    tx.commit().await?;
    Ok(ProductListingFixture {
        product_listing_id,
        listing_source_id,
        event_id,
    })
}

async fn insert_product_with_event_then_rollback(
    pool: &sqlx::PgPool,
) -> Result<ProductListingId, sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let listing_source_id = ListingSourceId::new();
    let mut tx = pool.begin().await?;
    insert_listing_source(&mut tx, listing_source_id, "rollback-os").await?;
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Rolled back ProductListing OpenSearch chair', 'en', 'AVAILABLE', 'ACTIVE', 'https://example.test/product_listings/rolled-back', '[]')",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(product_slug("rollback-os", product_listing_id))
    .bind(uuid::Uuid::from(event_id))
    .bind(listing_source_id.as_uuid())
    .bind(product_listing_id.to_string())

        .execute(&mut *tx)
    .await?;
    insert_product_event(&mut tx, product_listing_id, event_id).await?;
    tx.rollback().await?;
    Ok(product_listing_id)
}

async fn advance_product_revision(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    insert_product_event(&mut tx, product_listing_id, event_id).await?;
    sqlx::query(
        "UPDATE product_listings SET current_event_id = $1, title_text = 'Current ProductListing OpenSearch chair', version = version + 1, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $2",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(event_id)
}

async fn withdraw_product_listing(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    insert_product_event_with_type(
        &mut tx,
        product_listing_id,
        event_id,
        "PRODUCT_LISTING_CHANGED",
        "DOMAIN",
        json!({"lifecycle": {"transition": "WITHDRAWN", "previousAvailability": "AVAILABLE"}}),
    )
    .await?;
    sqlx::query(
        "UPDATE product_listings SET current_event_id = $1, lifecycle = 'WITHDRAWN', availability = NULL, version = version + 1, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $2",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(event_id)
}

async fn restore_product_listing(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    insert_product_event_with_type(
        &mut tx,
        product_listing_id,
        event_id,
        "PRODUCT_LISTING_CHANGED",
        "DOMAIN",
        json!({"lifecycle": {"transition": "RESTORED"}}),
    )
    .await?;
    sqlx::query(
        "UPDATE product_listings SET current_event_id = $1, lifecycle = 'ACTIVE', availability = NULL, version = version + 1, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $2",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(event_id)
}

async fn insert_sold_product_with_event(
    pool: &sqlx::PgPool,
    fx_rate_id: FxRateId,
) -> Result<ProductListingFixture, sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let listing_source_id = ListingSourceId::new();
    let mut tx = pool.begin().await?;
    insert_listing_source(&mut tx, listing_source_id, "sold-os").await?;
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, price_kind, price_amount, price_currency, sale_observation_fx_rate_id, sale_observed_at, availability, lifecycle, url, product_images, projection_version) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Sold ProductListing OpenSearch chair', 'en', 'MONETARY', 12345, 'EUR', $6, now(), 'SOLD_OUT', 'ACTIVE', 'https://example.test/product_listings/sold', '[]', 13)",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(product_slug("sold-os", product_listing_id))
    .bind(uuid::Uuid::from(event_id))
    .bind(listing_source_id.as_uuid())
    .bind(product_listing_id.to_string())
    .bind(fx_rate_id.as_uuid())

        .execute(&mut *tx)
    .await?;
    insert_product_event(&mut tx, product_listing_id, event_id).await?;
    tx.commit().await?;
    Ok(ProductListingFixture {
        product_listing_id,
        listing_source_id,
        event_id,
    })
}

async fn insert_sold_product_without_main_price_with_event(
    pool: &sqlx::PgPool,
    fx_rate_id: FxRateId,
) -> Result<ProductListingFixture, sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let listing_source_id = ListingSourceId::new();
    let mut tx = pool.begin().await?;
    insert_listing_source(&mut tx, listing_source_id, "sold-no-price-os").await?;
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, price_estimate_min_amount, price_estimate_min_currency, price_estimate_max_amount, price_estimate_max_currency, sale_observation_fx_rate_id, sale_observed_at, availability, lifecycle, url, product_images, projection_version) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Sold ProductListing OpenSearch chair without main price', 'en', 10000, 'EUR', 15000, 'EUR', $6, now(), 'SOLD_OUT', 'ACTIVE', 'https://example.test/product_listings/sold-no-price', '[]', 17)",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(product_slug("sold-no-price-os", product_listing_id))
    .bind(uuid::Uuid::from(event_id))
    .bind(listing_source_id.as_uuid())
    .bind(product_listing_id.to_string())
    .bind(fx_rate_id.as_uuid())

        .execute(&mut *tx)
    .await?;
    insert_product_event(&mut tx, product_listing_id, event_id).await?;
    tx.commit().await?;
    Ok(ProductListingFixture {
        product_listing_id,
        listing_source_id,
        event_id,
    })
}

async fn correct_sold_product_main_price(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    insert_product_event(&mut tx, product_listing_id, event_id).await?;
    sqlx::query(
        "UPDATE product_listings SET current_event_id = $1, price_kind = 'MONETARY', price_amount = 12345, price_currency = 'EUR', version = version + 1, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $2",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(event_id)
}

fn product_slug(prefix: &str, product_listing_id: ProductListingId) -> String {
    let suffix = product_listing_id.as_uuid().simple().to_string();
    format!("{prefix}-{}", &suffix[26..])
}

async fn insert_listing_source(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    listing_source_id: ListingSourceId,
    slug_prefix: &str,
) -> Result<(), sqlx::Error> {
    let operator_party_id = uuid::Uuid::now_v7();
    sqlx::query(
        "WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'ProductListing OpenSearch worker source', party_id FROM operator",
    )
    .bind(operator_party_id)
    .bind(format!("{slug_prefix}-{}", listing_source_id.as_uuid()))
    .bind(listing_source_id.as_uuid())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_product_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    product_listing_id: ProductListingId,
    event_id: EventId,
) -> Result<(), sqlx::Error> {
    insert_product_event_with_type(
        tx,
        product_listing_id,
        event_id,
        "PRODUCT_LISTING_CHANGED",
        "DOMAIN",
        json!({"availability": {"previous": null, "current": "AVAILABLE"}}),
    )
    .await
}

async fn insert_product_event_with_type(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    product_listing_id: ProductListingId,
    event_id: EventId,
    event_type: &str,
    event_group: &str,
    payload: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, $3, $4, 1, $5, now())",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(event_type)
    .bind(event_group)
    .bind(payload)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_equal_rate_snapshot(pool: &sqlx::PgPool) -> Result<FxRateId, sqlx::Error> {
    let fx_rate_id = FxRateId::new();
    sqlx::query(
        "INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id) VALUES ($1, now(), 'fxratesapi', $2)",
    )
    .bind(fx_rate_id.as_uuid())
    .bind(fx_rate_id.as_uuid().to_string())
    .execute(pool)
    .await?;
    for currency in [
        "EUR", "GBP", "USD", "AUD", "CAD", "NZD", "CNY", "BRL", "PLN", "TRY", "JPY", "CZK", "RUB",
        "AED", "SAR", "HKD", "SGD", "CHF", "ZAR",
    ] {
        sqlx::query(
            "INSERT INTO fx_rate_quotes (fx_rate_id, currency, units_per_eur) VALUES ($1, $2, 1000000)",
        )
        .bind(fx_rate_id.as_uuid())
        .bind(currency)
        .execute(pool)
        .await?;
    }
    Ok(fx_rate_id)
}

async fn wait_for_product_response(
    product_listing_id: ProductListingId,
) -> Result<Value, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        refresh_index(PRODUCT_LISTINGS_INDEX).await;
        if let Some(response) = product_response(product_listing_id).await? {
            return Ok(response);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!(
        "timed out waiting for ProductListing OpenSearch projection for {product_listing_id}"
    ))
    .into())
}

async fn wait_for_product_event(
    product_listing_id: ProductListingId,
    event_id: EventId,
) -> Result<Value, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        refresh_index(PRODUCT_LISTINGS_INDEX).await;
        if let Some(response) = product_response(product_listing_id).await?
            && response.pointer("/_source/eventId").and_then(Value::as_str)
                == Some(event_id.to_string().as_str())
        {
            return Ok(response);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!(
        "timed out waiting for ProductListing OpenSearch projection revision {event_id}"
    ))
    .into())
}

async fn wait_for_product_tombstone(
    product_listing_id: ProductListingId,
    version: i64,
) -> Result<Value, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        refresh_index(PRODUCT_LISTINGS_INDEX).await;
        if let Some(response) = product_response(product_listing_id).await?
            && response["_version"] == version
            && response["_source"]["projectionDeleted"] == true
        {
            assert_eq!(
                json!({"productListingId": product_listing_id, "projectionDeleted": true}),
                response["_source"]
            );
            return Ok(response);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!(
        "timed out waiting for ProductListing OpenSearch deletion for {product_listing_id}"
    ))
    .into())
}

async fn assert_product_readers_empty() -> support::TestResult {
    use fxrate_core::{FX_RATE_SCALE, FxRateQuote, FxRateSource, NewFxRateSnapshot};
    use localization::Language;
    use money::Currency;
    use product_listing_core::product_listing_search::ProductListingSearch;
    use product_listing_opensearch::{
        OpenSearchProductListingSearchReader, OpenSearchProductListingSimilarProductListingsReader,
    };
    use product_listing_service::ports::{
        CompiledProductListingSearch, ProductListingPriceFilterPlan,
        ProductListingSearchReadRequest, ProductListingSearchReader,
        ProductListingSimilarProductListingsReader, ProductListingSimilarProductListingsRequest,
    };
    use strum::IntoEnumIterator;
    let snapshot = NewFxRateSnapshot::capture_eur(
        FxRateId::new(),
        time::OffsetDateTime::UNIX_EPOCH,
        FxRateSource::FxRatesApi,
        Currency::Eur,
        Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
    )?
    .into_persisted(1_i64.try_into()?);
    let plan = ProductListingPriceFilterPlan::compile(snapshot, Currency::Eur, None)?;
    let request = ProductListingSearchReadRequest {
        compiled_search: CompiledProductListingSearch {
            search: ProductListingSearch::new(Language::En, Currency::Eur),
            price_filter_plan: plan.clone(),
        },
        sort: None,
        cursor: None,
    };
    refresh_index(PRODUCT_LISTINGS_INDEX).await;
    let client = get_opensearch_client().await.clone();
    let reader = OpenSearchProductListingSearchReader::new(client.clone());
    let ordinary = reader.search(&request).await?;
    assert!(ordinary.items.is_empty());
    assert_eq!(Some(0), ordinary.total);
    let embedding = vec![0.1; 768];
    assert!(
        reader
            .search_hybrid(&request, &embedding)
            .await?
            .items
            .is_empty()
    );
    assert!(
        OpenSearchProductListingSimilarProductListingsReader::new(client)
            .find_similar_product_listings(&ProductListingSimilarProductListingsRequest {
                product_listing_id: ProductListingId::new(),
                embedding,
                language: Language::En,
                price_filter_plan: plan,
            })
            .await?
            .is_empty()
    );
    Ok(())
}

async fn assert_no_product_projection(
    product_listing_id: ProductListingId,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        refresh_index(PRODUCT_LISTINGS_INDEX).await;
        if product_response(product_listing_id).await?.is_some() {
            return Err(std::io::Error::other(format!(
                "unexpected ProductListing OpenSearch projection for {product_listing_id}"
            ))
            .into());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}

async fn assert_product_response_unchanged_for(
    product_listing_id: ProductListingId,
    expected: &Value,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        refresh_index(PRODUCT_LISTINGS_INDEX).await;
        let actual = product_response(product_listing_id).await?.ok_or_else(|| {
            std::io::Error::other("ProductListing projection disappeared after redelivery")
        })?;
        if &actual != expected {
            return Err(std::io::Error::other(
                "ProductListing projection changed after redelivery",
            )
            .into());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}

async fn product_response(
    product_listing_id: ProductListingId,
) -> Result<Option<Value>, Box<dyn std::error::Error>> {
    let response = get_opensearch_client()
        .await
        .get(GetParts::IndexId(
            PRODUCT_LISTINGS_INDEX,
            &product_listing_id.to_string(),
        ))
        .send()
        .await?;
    if response.status_code().as_u16() == 404 {
        return Ok(None);
    }
    let response = response.error_for_status_code()?;
    Ok(Some(response.json().await?))
}
