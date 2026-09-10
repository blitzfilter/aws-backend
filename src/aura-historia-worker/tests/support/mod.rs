pub mod sequin_router;

use aura_historia_worker::{
    WorkerRuntimeComposition, WorkerScope,
    queue::{SqsQueueConfig, WorkerQueueReceiver},
};
use std::{future::Future, time::Duration};
use test_api::{WorkerSqs, get_sqs_client};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Starts process-lived worker acceptance infrastructure without sharing worker state.
#[derive(Debug, Clone, Copy)]
pub struct WorkerAcceptanceHarness;

impl WorkerAcceptanceHarness {
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl test_api::IntegrationTestService for WorkerAcceptanceHarness {
    fn service_names(&self) -> &'static [&'static str] {
        &["opensearch", "s3", "sesv2", "sqs"]
    }

    async fn set_up(&self) {
        sequin_router::ensure_started()
            .await
            .unwrap_or_else(|error| panic!("start worker Sequin test router: {error}"));
    }

    async fn tear_down(&self) {
        sequin_router::assert_idle()
            .unwrap_or_else(|error| panic!("worker Sequin test router cleanup: {error}"));
    }
}

pub const fn queues(scope: WorkerScope) -> WorkerSqs {
    let visibility = match scope {
        WorkerScope::NotificationDelivery => 360,
        WorkerScope::ProductListingRawNormalization
        | WorkerScope::SearchFilterPercolator
        | WorkerScope::ProductListingEmbedding
        | WorkerScope::ProductListingTranslation => 300,
        _ => 60,
    };
    WorkerSqs::new(scope.as_str(), visibility)
}

pub async fn composition(scope: WorkerScope) -> TestResult<WorkerRuntimeComposition> {
    let config = SqsQueueConfig::new(
        scope,
        queues(scope).queue_url().parse()?,
        "eu-central-1".to_owned(),
        "test".to_owned(),
        Some(test_api::localstack::get_endpoint_url().parse()?),
    )?;
    Ok(WorkerRuntimeComposition::with_sqs(get_sqs_client().await.clone(), config).await?)
}

/// Independent receivers compete on one real Standard queue. No shared runtime deduplication.
pub async fn competing_consumers<F, Fut>(
    scope: WorkerScope,
    receiver: WorkerQueueReceiver,
    run: F,
) -> TestResult<tokio::task::JoinHandle<()>>
where
    F: Fn(WorkerQueueReceiver) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let (other_runtime, other_receiver) = composition(scope).await?.into_parts();
    Ok(tokio::spawn(async move {
        // Keep both futures owned here, including panic/cancellation. No detached test consumer
        // may keep polling a queue while the fixture purges it for the next serial test.
        let primary = std::panic::AssertUnwindSafe(run(receiver));
        let secondary = std::panic::AssertUnwindSafe(run(other_receiver));
        use test_api::FutureExt;
        let primary = async {
            let result = primary.catch_unwind().await;
            other_runtime.shutdown();
            result
        };
        let (primary, secondary) = tokio::join!(primary, secondary.catch_unwind());
        if let Err(panic) = primary.and(secondary) {
            std::panic::resume_unwind(panic);
        }
    }))
}

#[allow(dead_code)]
pub async fn wait_until_empty(scope: WorkerScope) -> TestResult {
    use aws_sdk_sqs::types::QueueAttributeName as A;
    let queues = queues(scope);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut empty_since = None;
    loop {
        let mut empty = true;
        for url in [queues.queue_url(), queues.dead_letter_queue_url()] {
            let result = get_sqs_client()
                .await
                .get_queue_attributes()
                .queue_url(url)
                .attribute_names(A::ApproximateNumberOfMessages)
                .attribute_names(A::ApproximateNumberOfMessagesNotVisible)
                .attribute_names(A::ApproximateNumberOfMessagesDelayed)
                .send()
                .await?;
            let attributes = result.attributes().ok_or("missing SQS counts")?;
            for key in [
                A::ApproximateNumberOfMessages,
                A::ApproximateNumberOfMessagesNotVisible,
                A::ApproximateNumberOfMessagesDelayed,
            ] {
                empty &= attributes.get(&key).ok_or("missing SQS count")? == "0";
            }
        }
        if empty {
            let since = empty_since.get_or_insert_with(tokio::time::Instant::now);
            if since.elapsed() >= Duration::from_secs(1) {
                return Ok(());
            }
        } else {
            empty_since = None;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("{} source/DLQ did not drain", scope.as_str()).into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[allow(dead_code)]
pub async fn post_change(change: serde_json::Value) -> TestResult {
    sequin_router::post_direct_to_primary(change)
        .await
        .map_err(Into::into)
}

#[allow(dead_code)]
pub async fn post_change_to(port: u16, change: serde_json::Value) -> TestResult {
    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/cdc/sequin"))
        .timeout(Duration::from_secs(12))
        .json(&change)
        .send()
        .await?;
    assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());
    Ok(())
}

#[allow(dead_code)]
pub async fn redeliver_product_event(pool: &sqlx::PgPool, event_id: uuid::Uuid) -> TestResult {
    let record: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_build_object('event_id', event_id, 'product_listing_id', product_listing_id, 'event_type', event_type, 'event_group', event_group, 'event_type_schema_version', event_type_schema_version, 'payload', payload) FROM product_listing_events WHERE event_id = $1",
    ).bind(event_id).fetch_one(pool).await?;
    post_change(serde_json::json!({
        "record": record, "action": "insert",
        "metadata": {"table_schema": "public", "table_name": "product_listing_events"},
    }))
    .await
}
