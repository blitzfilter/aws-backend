mod dynamodb;
pub mod localstack;
mod opensearch;
mod s3;

use async_trait::async_trait;
pub use dynamodb::{DynamoDB, get_dynamodb_client};
pub use opensearch::{OpenSearch, get_opensearch_client};
pub use s3::S3;
pub use serial_test::serial;
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
    const SERVICE_NAME: &'static str;
    /// Prepares the service for the test (e.g., create buckets, tables, etc.)
    async fn set_up();
    /// Cleans up after the test (defaults to a no-op)
    async fn tear_down() {}
}
