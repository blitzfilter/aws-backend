#[cfg(feature = "api-gateway")]
mod api_gateway;
#[cfg(feature = "aura-historia-api")]
mod aura_historia_api;
#[cfg(feature = "cloudformation")]
mod cloudformation;
#[cfg(feature = "cognito")]
mod cloudformation_output;
#[cfg(feature = "cognito")]
mod cognito;
#[cfg(feature = "eventbridge")]
mod eventbridge;
pub mod localstack;
#[cfg(feature = "opensearch")]
mod opensearch;
#[cfg(feature = "postgres")]
mod postgres;
mod s3;
#[cfg(feature = "sequin")]
mod sequin;
#[cfg(feature = "ses")]
mod ses;
mod signal;
#[cfg(feature = "sqs")]
mod sqs;

#[cfg(feature = "api-gateway")]
pub use api_gateway::*;
use async_trait::async_trait;
#[cfg(feature = "aura-historia-api")]
pub use aura_historia_api::{AuraHistoriaApi, AuraHistoriaApiAppFactory};
#[cfg(feature = "cloudformation")]
pub use cloudformation::Cloudformation;
#[cfg(feature = "cognito")]
pub use cognito::*;

#[cfg(feature = "eventbridge")]
pub use eventbridge::get_eventbridge_client;
pub use futures_util::FutureExt;
#[cfg(feature = "opensearch")]
pub use opensearch::{OpenSearch, get_opensearch_client, read_by_id, refresh_index};
#[cfg(feature = "postgres")]
pub use postgres::{Postgres, get_postgres_client, get_postgres_host_gateway_connection_string};
pub use s3::S3;
#[cfg(feature = "sequin")]
pub use sequin::*;
pub use serial_test::serial;
#[cfg(feature = "ses")]
pub use ses::*;
#[cfg(feature = "sqs")]
pub use sqs::{Sqs, SqsBuilder, SqsBuilderError, SqsQueuePair, WorkerSqs, get_sqs_client};
pub use test_api_macros::aura_integration_test;
pub use tokio;
pub use tracing;

/// A trait for defining integration test lifecycle behavior for an Aura integration test service.
///
/// Implement this trait for each service you want to use with the `#[aura_integration_test]` macro.
/// It provides a consistent interface for setting up and tearing down test-specific resources.
///
/// # Required Items
///
/// - `SERVICE_NAME`: The name of the AWS service as expected by LocalStack (e.g., `"s3"`).
/// - `async fn set_up()`: Prepares the service for the test (e.g., create buckets).
///
/// # Optional
///
/// - `async fn tear_down()`: Cleans up after the test (defaults to a no-op).
///
/// # Notes
///
/// - `async_trait` is required to enable async methods in traits.
/// - The trait is intended for use with the `#[aura_integration_test]` macro.
///
#[async_trait]
pub trait IntegrationTestService: Sized {
    /// The AWS LocalStack service names. Return `&[]` for non-LocalStack services like Postgres.
    fn service_names(&self) -> &'static [&'static str];
    /// Extra environment variables to set on the LocalStack container.
    fn env_vars(&self) -> Vec<(&'static str, &'static str)> {
        vec![]
    }
    /// Prepares the service for the test (e.g., create buckets).
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
