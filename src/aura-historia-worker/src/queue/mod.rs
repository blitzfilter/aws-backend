//! Standard SQS transport and single-receipt consumer lifecycle.
mod config;
mod consumer;
mod sqs;
mod tasks;

pub use config::{
    AWS_REGION_ENV, QueueError, SQS_ENDPOINT_ENV, SqsQueueConfig, WORKER_QUEUE_URL_ENV,
};
pub use consumer::WorkerQueueReceiver;
pub(crate) use consumer::{JobOutcome, RuntimeControl};
pub use sqs::SqsQueue;

use std::time::Duration;

const API_TIMEOUT: Duration = Duration::from_secs(5);
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(27);

// Receipt handles deliberately have no Debug or Display implementation.
struct Message {
    body: Option<String>,
    receipt: String,
    receive_count: u32,
    sent_timestamp_ms: u64,
    first_received_timestamp_ms: u64,
}

#[async_trait::async_trait]
trait Transport: Send + Sync {
    async fn send(&self, body: &str) -> Result<(), QueueError>;
    async fn receive(&self) -> Result<Option<Message>, QueueError>;
    async fn visibility(&self, receipt: &str, seconds: i32) -> Result<(), QueueError>;
    async fn delete(&self, receipt: &str) -> Result<(), QueueError>;
    async fn probe(&self) -> Result<(), QueueError>;
}

async fn bounded<T>(
    future: impl std::future::Future<Output = Result<T, QueueError>>,
    timeout: Duration,
) -> Result<T, QueueError> {
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| QueueError::Timeout)?
}
