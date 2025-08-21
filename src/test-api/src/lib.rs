#[cfg(feature = "api-gateway")]
mod api_gateway;
#[cfg(feature = "dynamodb")]
mod dynamodb;
#[cfg(feature = "lambda")]
mod lambda;
pub mod localstack;
#[cfg(feature = "opensearch")]
mod opensearch;
mod s3;
#[cfg(feature = "sqs")]
mod sqs;
#[cfg(all(feature = "sqs", feature = "lambda"))]
mod sqs_lambda;

#[cfg(feature = "api-gateway")]
pub use api_gateway::*;
use async_trait::async_trait;
#[cfg(feature = "dynamodb")]
pub use dynamodb::{DynamoDB, get_dynamodb_client, mk_partial_put_batch_failure};
#[cfg(feature = "lambda")]
pub use lambda::{Lambda, get_lambda_client};
#[cfg(feature = "opensearch")]
pub use opensearch::{OpenSearch, get_opensearch_client, read_by_id, refresh_index};
pub use s3::S3;
pub use serial_test::serial;
#[cfg(feature = "sqs")]
pub use sqs::{Sqs, SqsBuilder, SqsBuilderError, get_sqs_client};
#[cfg(all(feature = "sqs", feature = "lambda"))]
pub use sqs_lambda::{
    SqsLambdaEventSourceMapping, SqsLambdaEventSourceMappingBuilder,
    SqsLambdaEventSourceMappingBuilderError,
};
pub use test_api_macros::localstack_test;
pub use tokio;

/// A trait for defining integration test lifecycle behavior for a LocalStack-backed AWS service.
///
/// Implement this trait for each service you want to use with the `#[localstack_test]` macro.
/// It provides a consistent interface for setting up and tearing down test-specific resources.
///
/// # Required Items
///
/// - `SERVICE_NAME`: The name of the AWS service as expected by LocalStack (e.g., `"s3"`, `"dynamodb"`).
/// - `async fn set_up()`: Prepares the service for the test (e.g., create buckets, tables, etc.).
///
/// # Optional
///
/// - `async fn tear_down()`: Cleans up after the test (defaults to a no-op).
///
/// # Notes
///
/// - `async_trait` is required to enable async methods in traits.
/// - The trait is intended for use with the `#[localstack_test]` macro.
///
#[async_trait]
pub trait IntegrationTestService: Sized {
    /// The name of the AWS service as expected by LocalStack (e.g., `"s3"`, `"dynamodb"`)
    fn service_names(&self) -> &'static [&'static str];
    /// Prepares the service for the test (e.g., create buckets, tables, etc.)
    async fn set_up(&self);
    /// Cleans up after the test (defaults to a no-op)
    async fn tear_down(&self) {}
}

#[macro_export]
macro_rules! extract_apigw_response_json_body {
    ($response:expr) => {{
        match &$response.clone().body {
            Some(aws_lambda_events::encodings::Body::Text(body)) => {
                serde_json::from_str::<serde_json::Value>(body)
                    .expect("Failed to parse JSON from response body")
            }
            _ => panic!("Expected response body to be Text."),
        }
    }};
}
