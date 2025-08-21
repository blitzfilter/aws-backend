use crate::{IntegrationTestService, Lambda, Sqs, get_lambda_client, get_sqs_client};
use async_trait::async_trait;
use aws_sdk_sqs::types::QueueAttributeName;
use derive_builder::Builder;
use tracing::debug;

/// Marker type representing an EventSource-Mapping between SQS and Lambda services in LocalStack-based tests.
///
/// Implements the [`IntegrationTestService`] trait to support lifecycle management
/// when used with the `#[localstack_test]` macro.
#[derive(Debug, Builder)]
pub struct SqsLambdaEventSourceMapping {
    pub sqs: &'static Sqs,
    pub lambda: &'static Lambda,
    pub max_batch_size: i32,
    pub max_batch_window_seconds: i32,
}

#[async_trait]
impl IntegrationTestService for SqsLambdaEventSourceMapping {
    fn service_names(&self) -> &'static [&'static str] {
        &["sqs", "lambda"]
    }

    async fn set_up(&self) {
        self.sqs.set_up().await;
        self.lambda.set_up().await;

        let sqs_client = get_sqs_client().await;

        let queue_arn = sqs_client
            .get_queue_attributes()
            .queue_url(self.sqs.queue_url())
            .attribute_names(QueueAttributeName::QueueArn)
            .send()
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "shouldn't fail retrieving ARN for queue '{}': {e}",
                    self.sqs.name
                )
            })
            .attributes()
            .unwrap()
            .get(&QueueAttributeName::QueueArn)
            .expect("shouldn't miss QueueArn")
            .to_owned();

        let lambda_client = get_lambda_client().await;

        // check if mapping already exists
        let mappings = lambda_client
            .list_event_source_mappings()
            .function_name(self.lambda.name)
            .event_source_arn(queue_arn.clone())
            .send()
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "failed to list EventSourceMappings for Lambda '{}' and SQS '{}': {e}",
                    self.lambda.name, self.sqs.name
                )
            });

        let esm_already_exists = mappings
            .event_source_mappings()
            .iter()
            .any(|m| m.function_arn().is_some());

        if esm_already_exists {
            debug!(
                "EventSourceMapping already exists for Lambda '{}' and queue '{}'. Skipping creation.",
                self.lambda.name, self.sqs.name
            );
            return;
        }

        debug!(
            "EventSourceMapping does not yet exist for Lambda '{}' and queue '{}'. Creating it.",
            self.lambda.name, self.sqs.name
        );

        lambda_client
                .create_event_source_mapping()
                .event_source_arn(queue_arn)
                .function_name(self.lambda.name)
                .batch_size(self.max_batch_size)
                .maximum_batching_window_in_seconds(self.max_batch_window_seconds)
                .enabled(true)
                .send()
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "shouldn't fail creating EventSource-Mapping for Lambda '{}' with SQS '{}': {e}",
                        self.lambda.name, self.sqs.name
                    )
                });

        debug!(
            "Created EventSourceMapping for Lambda '{}' and queue '{}'",
            self.lambda.name, self.sqs.name
        );
    }
}
