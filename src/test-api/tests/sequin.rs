use std::time::Duration;

use aura_historia_worker::cdc::WorkerQueue;
use aura_historia_worker::{QueueConfig, WorkerRuntime};
use test_api::*;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const WORKER_SEQUIN: Sequin = Sequin::worker_webhook();

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_deliver_product_event_change_to_worker_queues() {
    let (runtime, mut receivers) = WorkerRuntime::with_all_queues(QueueConfig::new(8)).unwrap();
    let listener = TcpListener::bind(get_sequin_worker_webhook_bind_addr())
        .await
        .unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(aura_historia_worker::serve_with_runtime(
        listener,
        runtime,
        async move {
            let _ = shutdown_rx.await;
        },
    ));

    let pool = get_postgres_client().await;
    insert_product_event_under_test(&pool).await;

    let product_job = recv_or_fail(&mut receivers, WorkerQueue::ProductListingOpenSearch).await;
    let percolator_job = recv_or_fail(&mut receivers, WorkerQueue::SearchFilterPercolator).await;

    let _send_result = shutdown_tx.send(());
    server.await.unwrap().unwrap();

    assert!(product_job.is_some());
    assert!(percolator_job.is_some());
}

async fn insert_product_event_under_test(pool: &sqlx::PgPool) {
    let product_listing_id = uuid::Uuid::now_v7();
    let event_id = uuid::Uuid::now_v7();
    let listing_source_id = uuid::Uuid::now_v7();
    let operator_party_id = uuid::Uuid::now_v7();
    let mut transaction = pool.begin().await.unwrap_or_else(|error| {
        panic!("failed to begin product-event fixture transaction: {error}")
    });

    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'Sequin test source', party_id FROM operator")
        .bind(operator_party_id)
        .bind(format!("sequin-test-source-{listing_source_id}"))
        .bind(listing_source_id)
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to insert Sequin test source: {error}"));
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Sequin test product', 'en', NULL, 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_listing_id)
        .bind(product_listing_title_slug_id("Sequin test product"))
        .bind(event_id)
        .bind(listing_source_id)
        .bind(product_listing_id.to_string())
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to insert Sequin test product: {error}"));
    let discovery_payload = serde_json::json!({
        "listingSourceId": listing_source_id,
        "sourceListingId": product_listing_id.to_string(),
        "title": { "language": "en", "text": "Sequin test product" },
        "description": null,
        "pricing": {
            "price": null,
            "priceEstimateMin": null,
            "priceEstimateMax": null
        },
        "availability": null,
        "url": "https://example.test/product",
        "imageCount": 0,
        "auction": { "start": null, "end": null }
    });
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(event_id)
        .bind(product_listing_id)
        .bind(discovery_payload)
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to insert Sequin test product event: {error}"));
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("failed to commit Sequin test product event: {error}"));
}

async fn recv_or_fail(
    receivers: &mut aura_historia_worker::cdc::WorkerQueueReceivers,
    queue: WorkerQueue,
) -> Option<aura_historia_worker::cdc::DomainJob> {
    match receivers.recv_timeout(queue, Duration::from_secs(60)).await {
        Ok(job) => job,
        Err(error) => panic!("timed out waiting for Sequin job on {queue:?}: {error}"),
    }
}

fn product_listing_title_slug_id(title: &str) -> String {
    product_listing_core::product_listing_slug_id::ProductListingSlugId::from_title_and_suffix(
        title, "a1b2c3",
    )
    .unwrap_or_else(|error| panic!("valid product listing title slug: {error}"))
    .to_string()
}
