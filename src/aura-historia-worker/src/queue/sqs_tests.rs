use super::super::{QueueError, SqsQueueConfig, Transport};
use super::{AwsTransport, received_message};
use aws_sdk_sqs::{
    Client,
    config::{
        Credentials, Region,
        endpoint::{Endpoint, EndpointFuture, Params, ResolveEndpoint},
    },
    types::{Message as SqsMessage, MessageSystemAttributeName as A},
};
use aws_smithy_types::{retry::RetryConfig, timeout::TimeoutConfig};
use axum::{Json, Router, body::Bytes, extract::State, http::HeaderMap, routing::post};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::oneshot, task::JoinSet};

fn wire_message() -> SqsMessage {
    SqsMessage::builder()
        .body("{}")
        .receipt_handle("private-receipt")
        .attributes(A::ApproximateReceiveCount, "2")
        .attributes(A::SentTimestamp, "1700000000001")
        .attributes(A::ApproximateFirstReceiveTimestamp, "1700000001002")
        .build()
}

#[test]
fn should_decode_only_one_receipt_with_valid_count_and_safe_timestamps() {
    assert!(received_message(vec![]).unwrap().is_none());
    let message = received_message(vec![wire_message()]).unwrap().unwrap();
    assert_eq!(2, message.receive_count);
    assert_eq!(1_700_000_000_001, message.sent_timestamp_ms);
    assert_eq!(1_700_000_001_002, message.first_received_timestamp_ms);
    assert_eq!("private-receipt", message.receipt);
    assert_eq!(Some("{}"), message.body.as_deref());
    assert_eq!(
        Some(QueueError::InvalidResponse),
        received_message(vec![wire_message(), wire_message()]).err()
    );
}

#[test]
fn should_reject_missing_invalid_or_overflowing_response_metadata() {
    for attribute in [
        A::ApproximateReceiveCount,
        A::SentTimestamp,
        A::ApproximateFirstReceiveTimestamp,
    ] {
        let mut message = wire_message();
        message.attributes.as_mut().unwrap().remove(&attribute);
        assert_eq!(
            Some(QueueError::InvalidResponse),
            received_message(vec![message]).err()
        );
        for invalid in ["", "bad", "-1", "+1", " 1", "1.5", "18446744073709551616"] {
            let mut message = wire_message();
            message
                .attributes
                .as_mut()
                .unwrap()
                .insert(attribute.clone(), invalid.into());
            assert_eq!(
                Some(QueueError::InvalidResponse),
                received_message(vec![message]).err()
            );
        }
    }
    for (attribute, value) in [
        (A::ApproximateReceiveCount, "0"),
        (A::ApproximateReceiveCount, "4294967296"),
        (A::SentTimestamp, "18446744073709551615"),
        (A::ApproximateFirstReceiveTimestamp, "18446744073709551615"),
    ] {
        let mut message = wire_message();
        message
            .attributes
            .as_mut()
            .unwrap()
            .insert(attribute, value.into());
        assert_eq!(
            Some(QueueError::InvalidResponse),
            received_message(vec![message]).err()
        );
    }
    for receipt in [None, Some(String::new())] {
        let mut message = wire_message();
        message.receipt_handle = receipt;
        assert_eq!(
            Some(QueueError::InvalidResponse),
            received_message(vec![message]).err()
        );
    }
    let mut missing_body = wire_message();
    missing_body.body = None;
    assert!(
        received_message(vec![missing_body])
            .unwrap()
            .unwrap()
            .body
            .is_none()
    );
}

#[derive(Debug)]
struct WrongEndpoint;
impl ResolveEndpoint for WrongEndpoint {
    fn resolve_endpoint(&self, _: &Params) -> EndpointFuture<'_> {
        EndpointFuture::ready(Ok(Endpoint::builder().url("http://127.0.0.1:1").build()))
    }
}

fn inherited_client() -> Client {
    Client::from_conf(
        aws_sdk_sqs::config::Builder::new()
            .behavior_version(aws_config::BehaviorVersion::v2026_01_12())
            .region(Region::new("us-west-2"))
            .credentials_provider(Credentials::new(
                "runtime-test-key",
                "runtime-test-secret",
                None,
                None,
                "worker-test",
            ))
            .endpoint_url("http://127.0.0.1:1")
            .endpoint_resolver(WrongEndpoint)
            .use_fips(true)
            .use_dual_stack(true)
            .retry_config(RetryConfig::standard().with_max_attempts(100))
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(Duration::from_secs(600))
                    .build(),
            )
            .build(),
    )
}

fn config(endpoint: &str) -> SqsQueueConfig {
    SqsQueueConfig::new(
        crate::WorkerScope::NotificationDelivery,
        format!("{endpoint}/000000000000/aura-worker-notification-delivery-ephemeral")
            .parse()
            .unwrap(),
        "eu-central-1".into(),
        "ephemeral".into(),
        Some(endpoint.parse().unwrap()),
    )
    .unwrap()
}

#[derive(Clone)]
struct StubState {
    requests: Arc<Mutex<Vec<(HeaderMap, Value)>>>,
    response: Value,
}
struct StubServer {
    config: SqsQueueConfig,
    state: StubState,
    stop: oneshot::Sender<()>,
    tasks: JoinSet<std::io::Result<()>>,
}
impl StubServer {
    async fn start(response: Value) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = config(&format!("http://{}", listener.local_addr().unwrap()));
        let state = StubState {
            requests: Arc::default(),
            response,
        };
        let router = Router::new()
            .route(
                "/",
                post(
                    async |State(state): State<StubState>, headers: HeaderMap, body: Bytes| {
                        let request = serde_json::from_slice(&body).unwrap();
                        state.requests.lock().unwrap().push((headers, request));
                        Json(state.response)
                    },
                ),
            )
            .with_state(state.clone());
        let (stop, stopped) = oneshot::channel();
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _closed = stopped.await;
                })
                .await
        });
        Self {
            config,
            state,
            stop,
            tasks,
        }
    }

    async fn finish(mut self) {
        self.stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), self.tasks.join_next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn should_pin_sqs_settings_preserve_credentials_and_request_only_safe_attributes() {
    let server = StubServer::start(json!({"Messages":[{"Body":"{}", "ReceiptHandle":"private-receipt",
        "Attributes":{"ApproximateReceiveCount":"2", "SentTimestamp":"1700000000001", "ApproximateFirstReceiveTimestamp":"1700000001002"}}]})).await;
    let transport = AwsTransport::new(inherited_client(), server.config.clone());
    let sdk = transport.client.config();
    assert_eq!(
        Some("eu-central-1"),
        sdk.region().map(|region| region.as_ref())
    );
    assert_eq!(
        Some(2),
        sdk.retry_config().map(|retry| retry.max_attempts())
    );
    let timeouts = sdk.timeout_config().unwrap();
    assert_eq!(Some(Duration::from_secs(3)), timeouts.connect_timeout());
    assert_eq!(Some(Duration::from_secs(25)), timeouts.read_timeout());
    assert_eq!(
        Some(Duration::from_secs(30)),
        timeouts.operation_attempt_timeout()
    );
    assert_eq!(Some(Duration::from_secs(55)), timeouts.operation_timeout());
    let message = tokio::time::timeout(Duration::from_secs(3), transport.receive())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(2, message.receive_count);
    assert_eq!(1_700_000_000_001, message.sent_timestamp_ms);
    assert_eq!(1_700_000_001_002, message.first_received_timestamp_ms);
    {
        let requests = server.state.requests.lock().unwrap();
        assert_eq!(1, requests.len());
        let (headers, request) = &requests[0];
        assert!(
            headers["x-amz-target"]
                .to_str()
                .unwrap()
                .ends_with(".ReceiveMessage")
        );
        let authorization = headers["authorization"].to_str().unwrap();
        assert!(authorization.contains("Credential=runtime-test-key/"));
        assert!(authorization.contains("/eu-central-1/sqs/aws4_request"));
        assert_eq!(server.config.queue_url().as_str(), request["QueueUrl"]);
        assert_eq!(1, request["MaxNumberOfMessages"]);
        assert_eq!(20, request["WaitTimeSeconds"]);
        assert_eq!(360, request["VisibilityTimeout"]);
        assert_eq!(
            json!([
                "ApproximateReceiveCount",
                "SentTimestamp",
                "ApproximateFirstReceiveTimestamp"
            ]),
            request["MessageSystemAttributeNames"]
        );
        assert!(request.get("AttributeNames").is_none());
        assert!(request.get("MessageAttributeNames").is_none());
    }
    server.finish().await;
}

#[tokio::test]
async fn should_reject_oversized_encoded_bytes_before_any_sdk_send() {
    let transport = AwsTransport::new(inherited_client(), config("http://127.0.0.1:1"));
    for body in [
        "x".repeat(crate::wire::MAX_JOB_BYTES + 1),
        "é".repeat(crate::wire::MAX_JOB_BYTES / 2 + 1),
    ] {
        assert_eq!(
            Err(QueueError::MessageTooLarge),
            transport.send(&body).await
        );
    }
}

#[tokio::test]
async fn should_send_exact_size_without_fifo_fields_and_reject_unconfirmed_send_metadata() {
    for response in [
        json!({"MessageId":"accepted-message"}),
        json!({}),
        json!({"MessageId":""}),
    ] {
        let accepted =
            response.get("MessageId").and_then(Value::as_str) == Some("accepted-message");
        let server = StubServer::start(response).await;
        let transport = AwsTransport::new(inherited_client(), server.config.clone());
        let body = "é".repeat(crate::wire::MAX_JOB_BYTES / 2);
        let result = tokio::time::timeout(Duration::from_secs(3), transport.send(&body))
            .await
            .unwrap();
        assert_eq!(
            if accepted {
                Ok(())
            } else {
                Err(QueueError::InvalidResponse)
            },
            result
        );
        {
            let requests = server.state.requests.lock().unwrap();
            assert_eq!(1, requests.len());
            let request = &requests[0].1;
            assert_eq!(body, request["MessageBody"]);
            assert!(request.get("MessageGroupId").is_none());
            assert!(request.get("MessageDeduplicationId").is_none());
        }
        server.finish().await;
    }
}
