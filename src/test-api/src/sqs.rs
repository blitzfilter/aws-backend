use crate::IntegrationTestService;
use crate::localstack::get_aws_config;
use async_trait::async_trait;
use aws_sdk_sqs::Client;
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
pub struct Sqs {
    pub name: &'static str,
}

impl Sqs {
    pub fn queue_url(&self) -> String {
        format!(
            "http://sqs.eu-central-1.localhost.localstack.cloud:4566/000000000000/{}",
            self.name
        )
    }
}

#[async_trait]
impl IntegrationTestService for Sqs {
    fn service_names(&self) -> &'static [&'static str] {
        &["sqs"]
    }

    async fn set_up(&self) {
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

        assert_eq!(
            self.queue_url(),
            queue_url,
            "Expected Queue-URL '{}' and actual differ '{queue_url}'.",
            self.queue_url()
        );
    }
}
