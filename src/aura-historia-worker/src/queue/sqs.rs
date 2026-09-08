#[cfg(test)]
#[path = "sqs_tests.rs"]
mod tests;

use super::{
    API_TIMEOUT, Message, QueueError, SqsQueueConfig, Transport, bounded,
    config::{Attributes, validate_attributes},
};
use aws_sdk_sqs::{
    Client,
    config::Region,
    types::{MessageSystemAttributeName, QueueAttributeName},
};
use aws_smithy_types::{retry::RetryConfig, timeout::TimeoutConfig};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

/// Validated queue handle shared by ingress and a consumer. Its client retains the caller's
/// credential chain; endpoint/region, retries and timeouts are pinned to the typed contract.
#[derive(Clone)]
pub struct SqsQueue {
    pub(super) config: SqsQueueConfig,
    pub(super) transport: Arc<dyn Transport>,
}
impl std::fmt::Debug for SqsQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqsQueue")
            .field("scope", &self.config.scope())
            .finish_non_exhaustive()
    }
}

impl SqsQueue {
    pub async fn new(client: Client, config: SqsQueueConfig) -> Result<Self, QueueError> {
        let transport = AwsTransport::new(client, config.clone());
        let attributes = transport.attributes(config.queue_url.as_str()).await?;
        validate_attributes(&config, &attributes, false)?;
        let dlq_attributes = transport.attributes(config.dlq_url().as_str()).await?;
        validate_attributes(&config, &dlq_attributes, true)?;
        Ok(Self {
            config,
            transport: Arc::new(transport),
        })
    }

    /// Uses the workspace AWS default credential chain. Local endpoints must be explicit in config.
    pub async fn from_config(config: SqsQueueConfig) -> Result<Self, QueueError> {
        let sdk = aws_config::defaults(aws_config::BehaviorVersion::v2026_01_12())
            .region(Region::new(config.region.clone()))
            .load();
        let sdk = tokio::time::timeout(Duration::from_secs(15), sdk)
            .await
            .map_err(|_| QueueError::Timeout)?;
        Self::new(Client::new(&sdk), config).await
    }

    pub fn config(&self) -> &SqsQueueConfig {
        &self.config
    }

    pub(crate) async fn publish(&self, body: &str) -> Result<(), QueueError> {
        let mut observation = PublicationObservation {
            scope: self.config.scope(),
            started: Instant::now(),
            encoded_bytes: body.len(),
            outcome: "cancelled_acceptance_unknown",
        };
        let result = match validate_message_size(body) {
            Ok(()) => bounded(self.transport.send(body), API_TIMEOUT).await,
            Err(error) => Err(error),
        };
        observation.outcome = match result {
            Ok(()) => "published",
            Err(QueueError::MessageTooLarge) => "rejected_size",
            Err(QueueError::Timeout) => "timeout_acceptance_unknown",
            Err(QueueError::InvalidResponse) => "invalid_response_acceptance_unknown",
            Err(_) => "failed_acceptance_unknown",
        };
        result
    }
}

struct PublicationObservation {
    scope: crate::WorkerScope,
    started: Instant,
    encoded_bytes: usize,
    outcome: &'static str,
}
impl Drop for PublicationObservation {
    fn drop(&mut self) {
        // Batch/request cancellation may interrupt an SDK call after SQS accepted it.
        // Keep the latency and unknown outcome visible without ever logging its body.
        tracing::info!(
            scope = self.scope.as_str(),
            publication_duration_ms = self.started.elapsed().as_secs_f64() * 1000.0,
            encoded_bytes = self.encoded_bytes,
            outcome = self.outcome,
            "worker SQS publication finished"
        );
    }
}

fn validate_message_size(body: &str) -> Result<(), QueueError> {
    if body.len() > crate::wire::MAX_JOB_BYTES {
        return Err(QueueError::MessageTooLarge);
    }
    Ok(())
}

struct AwsTransport {
    client: Client,
    config: SqsQueueConfig,
}
impl AwsTransport {
    fn new(client: Client, config: SqsQueueConfig) -> Self {
        let client = Client::from_conf(
            client
                .config()
                .to_builder()
                .region(Region::new(config.region.clone()))
                .endpoint_url(config.endpoint_url())
                .endpoint_resolver(aws_sdk_sqs::config::endpoint::DefaultResolver::new())
                .use_fips(false)
                .use_dual_stack(false)
                .retry_config(RetryConfig::standard().with_max_attempts(2))
                .timeout_config(
                    TimeoutConfig::builder()
                        .connect_timeout(Duration::from_secs(3))
                        .read_timeout(Duration::from_secs(25))
                        .operation_attempt_timeout(Duration::from_secs(30))
                        .operation_timeout(Duration::from_secs(55))
                        .build(),
                )
                .build(),
        );
        Self { client, config }
    }

    async fn attributes(&self, url: &str) -> Result<Attributes, QueueError> {
        bounded(
            async {
                self.client
                    .get_queue_attributes()
                    .queue_url(url)
                    .attribute_names(QueueAttributeName::All)
                    .send()
                    .await
                    .map_err(|_| QueueError::Unavailable)?
                    .attributes
                    .ok_or(QueueError::InvalidResponse)
            },
            API_TIMEOUT,
        )
        .await
    }
}

#[async_trait::async_trait]
impl Transport for AwsTransport {
    async fn send(&self, body: &str) -> Result<(), QueueError> {
        validate_message_size(body)?;
        // Standard SQS: logical ordering/idempotency keys are in the body, never FIFO fields.
        let result = self
            .client
            .send_message()
            .queue_url(self.config.queue_url.as_str())
            .message_body(body)
            .send()
            .await
            .map_err(|_| QueueError::Unavailable)?;
        if !result.message_id().is_some_and(|id| !id.is_empty()) {
            return Err(QueueError::InvalidResponse);
        }
        Ok(())
    }

    async fn receive(&self) -> Result<Option<Message>, QueueError> {
        let result = self
            .client
            .receive_message()
            .queue_url(self.config.queue_url.as_str())
            .max_number_of_messages(1)
            .wait_time_seconds(20)
            .visibility_timeout(self.config.visibility_timeout().as_secs() as i32)
            .message_system_attribute_names(MessageSystemAttributeName::ApproximateReceiveCount)
            .message_system_attribute_names(MessageSystemAttributeName::SentTimestamp)
            .message_system_attribute_names(
                MessageSystemAttributeName::ApproximateFirstReceiveTimestamp,
            )
            .send()
            .await
            .map_err(|_| QueueError::Unavailable)?;
        received_message(result.messages.unwrap_or_default())
    }

    async fn visibility(&self, receipt: &str, seconds: i32) -> Result<(), QueueError> {
        self.client
            .change_message_visibility()
            .queue_url(self.config.queue_url.as_str())
            .receipt_handle(receipt)
            .visibility_timeout(seconds)
            .send()
            .await
            .map_err(|_| QueueError::Unavailable)?;
        Ok(())
    }
    async fn delete(&self, receipt: &str) -> Result<(), QueueError> {
        self.client
            .delete_message()
            .queue_url(self.config.queue_url.as_str())
            .receipt_handle(receipt)
            .send()
            .await
            .map_err(|_| QueueError::Unavailable)?;
        Ok(())
    }
    async fn probe(&self) -> Result<(), QueueError> {
        let attributes = self.attributes(self.config.queue_url.as_str()).await?;
        validate_attributes(&self.config, &attributes, false)
    }
}

fn received_message(
    mut messages: Vec<aws_sdk_sqs::types::Message>,
) -> Result<Option<Message>, QueueError> {
    if messages.len() > 1 {
        return Err(QueueError::InvalidResponse);
    }
    let Some(message) = messages.pop() else {
        return Ok(None);
    };
    let receive_count = u32::try_from(number_attribute(
        &message,
        MessageSystemAttributeName::ApproximateReceiveCount,
    )?)
    .map_err(|_| QueueError::InvalidResponse)?;
    if receive_count == 0 {
        return Err(QueueError::InvalidResponse);
    }
    let sent_timestamp_ms =
        timestamp_attribute(&message, MessageSystemAttributeName::SentTimestamp)?;
    let first_received_timestamp_ms = timestamp_attribute(
        &message,
        MessageSystemAttributeName::ApproximateFirstReceiveTimestamp,
    )?;
    let receipt = message
        .receipt_handle
        .filter(|receipt| !receipt.is_empty())
        .ok_or(QueueError::InvalidResponse)?;
    Ok(Some(Message {
        body: message.body,
        receipt,
        receive_count,
        sent_timestamp_ms,
        first_received_timestamp_ms,
    }))
}

fn number_attribute(
    message: &aws_sdk_sqs::types::Message,
    name: MessageSystemAttributeName,
) -> Result<u64, QueueError> {
    let value = message
        .attributes()
        .and_then(|attributes| attributes.get(&name))
        .ok_or(QueueError::InvalidResponse)?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(QueueError::InvalidResponse);
    }
    value.parse().map_err(|_| QueueError::InvalidResponse)
}

fn timestamp_attribute(
    message: &aws_sdk_sqs::types::Message,
    name: MessageSystemAttributeName,
) -> Result<u64, QueueError> {
    let millis = number_attribute(message, name)?;
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000)
        .map_err(|_| QueueError::InvalidResponse)?;
    Ok(millis)
}
