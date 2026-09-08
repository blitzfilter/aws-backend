use crate::IntegrationTestService;
use crate::localstack::{get_aws_config, get_endpoint_url};
use async_trait::async_trait;
use aws_sdk_sqs::Client;
use aws_sdk_sqs::types::{DeleteMessageBatchRequestEntry, QueueAttributeName};
use derive_builder::Builder;
use std::collections::HashMap;
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
/// when used with the `#[aura_integration_test]` macro.
#[derive(Debug, Builder)]
pub struct Sqs {
    pub name: &'static str,
}

impl Sqs {
    pub fn queue_url(&self) -> String {
        queue_url(self.name)
    }

    pub fn dead_letter_queue_url(&self) -> String {
        queue_url(&format!("dead-letter-{}", self.name))
    }
}

fn queue_url(name: &str) -> String {
    format!("{}/000000000000/{name}", get_endpoint_url())
}

/// Configurable Standard source/DLQ pair. Cleanup touches only these two queues.
#[derive(Debug, Clone)]
pub struct SqsQueuePair {
    pub name: String,
    pub dead_letter_name: String,
    pub attributes: HashMap<QueueAttributeName, String>,
    pub dead_letter_attributes: HashMap<QueueAttributeName, String>,
    pub max_receive_count: u32,
}

impl SqsQueuePair {
    pub fn new(name: impl Into<String>, dead_letter_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            dead_letter_name: dead_letter_name.into(),
            attributes: HashMap::new(),
            dead_letter_attributes: HashMap::new(),
            max_receive_count: 3,
        }
    }

    /// For tests without production queue-name constraints, including concurrent processes.
    pub fn unique(test_name: &str) -> Self {
        let suffix = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4().simple());
        let label: String = test_name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .take(24)
            .collect();
        Self::new(format!("{label}-{suffix}"), format!("{label}-dlq-{suffix}"))
    }

    pub fn queue_url(&self) -> String {
        queue_url(&self.name)
    }

    pub fn dead_letter_queue_url(&self) -> String {
        queue_url(&self.dead_letter_name)
    }
}

/// Production-shaped worker queues, isolated by the process-local LocalStack container.
/// Scope stays a string: test-api must not depend on the worker runtime.
#[derive(Debug, Clone, Copy)]
pub struct WorkerSqs {
    scope: &'static str,
    visibility_seconds: u32,
}

impl WorkerSqs {
    pub const fn new(scope: &'static str, visibility_seconds: u32) -> Self {
        Self {
            scope,
            visibility_seconds,
        }
    }

    pub fn queues(&self) -> SqsQueuePair {
        use QueueAttributeName as A;
        let mut pair = SqsQueuePair::new(
            format!("aura-worker-{}-test", self.scope),
            format!("aura-worker-{}-dlq-test", self.scope),
        );
        pair.max_receive_count = 5;
        pair.attributes = HashMap::from([
            (A::MessageRetentionPeriod, "604800".to_owned()),
            (A::ReceiveMessageWaitTimeSeconds, "20".to_owned()),
            (A::VisibilityTimeout, self.visibility_seconds.to_string()),
            (A::SqsManagedSseEnabled, "true".to_owned()),
            (A::Policy, tls_policy(&pair.name)),
            (
                A::RedriveAllowPolicy,
                serde_json::json!({"redrivePermission": "denyAll"}).to_string(),
            ),
        ]);
        pair.dead_letter_attributes = HashMap::from([
            (A::MessageRetentionPeriod, "1209600".to_owned()),
            (A::SqsManagedSseEnabled, "true".to_owned()),
            (A::Policy, tls_policy(&pair.dead_letter_name)),
            (
                A::RedriveAllowPolicy,
                serde_json::json!({
                    "redrivePermission": "byQueue",
                    "sourceQueueArns": [queue_arn(&pair.name)],
                })
                .to_string(),
            ),
        ]);
        pair
    }

    pub fn queue_url(&self) -> String {
        self.queues().queue_url()
    }

    pub fn dead_letter_queue_url(&self) -> String {
        self.queues().dead_letter_queue_url()
    }
}

fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:eu-central-1:000000000000:{name}")
}

fn tls_policy(name: &str) -> String {
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "DenyInsecureTransport",
            "Effect": "Deny",
            "Principal": "*",
            "Action": "sqs:*",
            "Resource": queue_arn(name),
            "Condition": {"Bool": {"aws:SecureTransport": "false"}},
        }],
    })
    .to_string()
}

#[async_trait]
impl IntegrationTestService for Sqs {
    fn service_names(&self) -> &'static [&'static str] {
        &["sqs"]
    }

    async fn set_up(&self) {
        SqsQueuePair::new(self.name, format!("dead-letter-{}", self.name))
            .set_up()
            .await;
    }

    async fn tear_down(&self) {
        SqsQueuePair::new(self.name, format!("dead-letter-{}", self.name))
            .tear_down()
            .await;
    }
}

#[async_trait]
impl IntegrationTestService for WorkerSqs {
    fn service_names(&self) -> &'static [&'static str] {
        &["sqs"]
    }

    async fn set_up(&self) {
        self.queues().set_up().await;
    }

    async fn tear_down(&self) {
        // LocalStack retains cancelled server-side long polls after purge. A new queue
        // incarnation prevents the previous test's receive from hiding this test's jobs.
        // LocalStack does not enforce AWS's 60-second queue recreation cooldown.
        for url in [self.queue_url(), self.dead_letter_queue_url()] {
            get_sqs_client()
                .await
                .delete_queue()
                .queue_url(url)
                .send()
                .await
                .unwrap_or_else(|error| panic!("delete worker fixture queue: {error}"));
        }
    }
}

#[async_trait]
impl IntegrationTestService for SqsQueuePair {
    fn service_names(&self) -> &'static [&'static str] {
        &["sqs"]
    }

    async fn set_up(&self) {
        let sqs_client = get_sqs_client().await;

        let dead_letter_queue_url = sqs_client
            .create_queue()
            .queue_name(&self.dead_letter_name)
            .set_attributes(Some(self.dead_letter_attributes.clone()))
            .send()
            .await
            .unwrap_or_else(|e| panic!("Failed creating DLQ '{}': {e}", self.name))
            .queue_url()
            .expect("Dead-letter queue URL not returned")
            .to_string();

        let dead_letter_queue_arn = sqs_client
            .get_queue_attributes()
            .queue_url(&dead_letter_queue_url)
            .attribute_names(QueueAttributeName::QueueArn)
            .send()
            .await
            .unwrap()
            .attributes
            .unwrap()
            .get(&QueueAttributeName::QueueArn)
            .unwrap()
            .to_string();

        let redrive_policy = serde_json::json!({
            "deadLetterTargetArn": dead_letter_queue_arn,
            "maxReceiveCount": self.max_receive_count
        })
        .to_string();

        let queue_url = sqs_client
            .create_queue()
            .queue_name(&self.name)
            .set_attributes(Some(self.attributes.clone()))
            .attributes(QueueAttributeName::RedrivePolicy, redrive_policy)
            .send()
            .await
            .unwrap_or_else(|e| panic!("Failed creating SQS queue '{}': {e}", self.name))
            .queue_url()
            .expect("Queue URL not returned")
            .to_string();

        // LocalStack may return a container-local :4566 URL. Verify its identity through
        // the SDK, but expose the canonical host endpoint URL to callers and child processes.
        for (returned, canonical, name) in [
            (queue_url, self.queue_url(), &self.name),
            (
                dead_letter_queue_url,
                self.dead_letter_queue_url(),
                &self.dead_letter_name,
            ),
        ] {
            for url in [returned, canonical] {
                let attributes = sqs_client
                    .get_queue_attributes()
                    .queue_url(url)
                    .attribute_names(QueueAttributeName::QueueArn)
                    .send()
                    .await
                    .unwrap_or_else(|error| panic!("read queue identity for {name}: {error}"));
                assert_eq!(
                    Some(&queue_arn(name)),
                    attributes
                        .attributes()
                        .and_then(|attrs| attrs.get(&QueueAttributeName::QueueArn)),
                    "returned and canonical URLs must address the same queue",
                );
            }
        }
    }

    async fn tear_down(&self) {
        let client = get_sqs_client().await;
        // Use purge_queue to remove ALL messages, including those with active visibility
        // timeouts (invisible messages that drain_queue cannot reach). In LocalStack used
        // for integration tests, the AWS-imposed 60-second cooldown between purge_queue
        // calls is not enforced, making this safe for per-test teardown.
        for queue_url in [self.queue_url(), self.dead_letter_queue_url()] {
            client
                .purge_queue()
                .queue_url(&queue_url)
                .send()
                .await
                .unwrap_or_else(|e| panic!("Failed purging SQS queue '{}': {e}", queue_url));
        }
        debug!("Purged SQS queues '{}' for test isolation", self.name);
    }
}

/// Drains all **visible** messages from each of the given SQS queue URLs using a
/// receive-and-delete loop.
///
/// Unlike `purge_queue`, this approach avoids the AWS-imposed 60-second
/// cooldown between purge calls, but it only removes currently visible messages.
/// Messages with an active visibility timeout (invisible messages) are not removed.
///
/// Prefer [`Sqs::tear_down`] for test teardown when full isolation including
/// invisible messages is required.
#[allow(dead_code)]
pub(crate) async fn drain_queues(queue_urls: Vec<String>) {
    for queue_url in queue_urls {
        drain_queue(&queue_url).await;
    }
}

/// Receives and deletes all currently **visible** messages from a single SQS queue.
#[allow(dead_code)]
async fn drain_queue(queue_url: &str) {
    let client = get_sqs_client().await;
    loop {
        let resp = client
            .receive_message()
            .queue_url(queue_url)
            .max_number_of_messages(10)
            .wait_time_seconds(0)
            .send()
            .await
            .unwrap_or_else(|e| {
                panic!("shouldn't fail receiving messages from SQS queue '{queue_url}': {e}")
            });

        let messages = resp.messages.unwrap_or_default();
        if messages.is_empty() {
            break;
        }

        let entries: Vec<DeleteMessageBatchRequestEntry> = messages
            .into_iter()
            .enumerate()
            .filter_map(|(idx, m)| {
                m.receipt_handle.map(|handle| {
                    DeleteMessageBatchRequestEntry::builder()
                        .id(idx.to_string())
                        .receipt_handle(handle)
                        .build()
                        .expect("shouldn't fail building DeleteMessageBatchRequestEntry")
                })
            })
            .collect();

        client
            .delete_message_batch()
            .queue_url(queue_url)
            .set_entries(Some(entries))
            .send()
            .await
            .unwrap_or_else(|e| {
                panic!("shouldn't fail deleting messages from SQS queue '{queue_url}': {e}")
            });
    }
    debug!("Drained SQS queue '{queue_url}'.");
}
