use crate::localstack::get_aws_config;
use crate::{IntegrationTestService, Lambda, get_lambda_client};
use async_trait::async_trait;
use aws_sdk_sqs::Client;
use aws_sdk_sqs::types::QueueAttributeName;
use derive_builder::Builder;
use tokio::sync::OnceCell;
use tracing::debug;

/// A lazily-initialized, globally shared SQS client for integration testing.
///
/// This `OnceCell` ensures that the client is only created once during the test lifecycle,
/// using the shared [`SdkConfig`] provided by [`get_aws_config()`].
static SQS_CLIENT: OnceCell<Client> = OnceCell::const_new();

/// Returns a shared `aws_sdk_sqs::Client` for interacting with LocalStack.
///
/// The client is initialized only once using a global `OnceCell`, and internally depends on
/// [`get_aws_config()`] for configuration (test credentials, region, LocalStack endpoint).
///
/// # Returns
///
/// A reference to a lazily-initialized `Client` instance.
pub async fn get_sqs_client() -> &'static Client {
    let client = SQS_CLIENT
        .get_or_init(|| async { Client::new(get_aws_config().await) })
        .await;
    debug!("Successfully initialized SQS-Client.");
    client
}

/// Marker type representing the SQS service in LocalStack-based tests.
///
/// Implements the [`IntegrationTestService`] trait to support lifecycle management
/// when used with the `#[localstack_test]` macro.
#[derive(Debug, Builder)]
pub struct SqsWithLambda {
    pub name: &'static str,
    pub lambda: &'static Lambda,
    pub max_batch_size: i32,
    pub max_batch_window_seconds: i32,
}

impl SqsWithLambda {
    pub fn queue_url(&self) -> String {
        format!(
            "http://sqs.eu-central-1.localhost.localstack.cloud:4566/000000000000/{}",
            self.name
        )
    }
}

#[async_trait]
impl IntegrationTestService for SqsWithLambda {
    fn service_names(&self) -> &'static [&'static str] {
        &["sqs", "lambda"]
    }

    async fn set_up(&self) {
        self.lambda.set_up().await;
        let sqs_client = get_sqs_client().await;
        let queue_url = sqs_client
            .create_queue()
            .queue_name(self.name)
            .send()
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "shouldn't fail creating SQS with name '{}'.: {e}",
                    self.name
                )
            })
            .queue_url()
            .expect("queue URL not returned")
            .to_string();

        let queue_arn = sqs_client
            .get_queue_attributes()
            .queue_url(&queue_url)
            .attribute_names(QueueAttributeName::QueueArn)
            .send()
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "shouldn't fail retrieving ARN for queue '{}': {e}",
                    self.name
                )
            })
            .attributes()
            .unwrap()
            .get(&QueueAttributeName::QueueArn)
            .expect("Missing QueueArn")
            .to_string();

        let lambda_client = get_lambda_client().await;
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
                    "shouldn't fail creating event-source-mapping for Lambda '{}' with SQS '{}': {e}",
                    self.lambda.name, self.name
                )
            });
    }
}
