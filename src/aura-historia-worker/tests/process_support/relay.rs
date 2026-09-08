//! External HTTP barriers only. Every SQS response/receive count comes from LocalStack.
use super::{TestResult, eventually};
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Mutex, watch},
    task::{JoinHandle, JoinSet},
};

#[derive(Clone)]
pub struct Receipt {
    pub consumer: &'static str,
    pub message_id: String,
    pub body: String,
    pub handle: String,
    pub count: u32,
}

#[derive(Clone)]
pub enum Observation {
    Sent {
        body: String,
        message_id: String,
    },
    Accepted {
        body: String,
    },
    Received(Receipt),
    DeleteHeld {
        handle: String,
    },
    Deleted {
        handle: String,
    },
    VisibilityChanged {
        handle: String,
        seconds: i64,
    },
    Rejected {
        body: String,
        status: StatusCode,
    },
    SqsAttempt {
        operation: String,
        invocation: String,
        handle: Option<String>,
    },
    ResponseWithheld {
        operation: String,
        invocation: String,
    },
    ResponseLost {
        invocation: String,
    },
    SdkRetryDropped {
        invocation: String,
    },
    DeleteReplied {
        handle: String,
        invocation: String,
    },
    Failed(String),
}

#[derive(Clone)]
pub struct Observations(watch::Sender<Vec<Observation>>);

impl Observations {
    pub fn new() -> Self {
        Self(watch::channel(Vec::new()).0)
    }

    fn record(&self, event: Observation) {
        self.0.send_modify(|events| events.push(event));
    }

    pub async fn wait<T>(
        &self,
        boundary: &str,
        select: impl Fn(&[Observation]) -> Option<T>,
    ) -> TestResult<T> {
        let mut events = self.0.subscribe();
        eventually(boundary, async {
            loop {
                {
                    let current = events.borrow_and_update();
                    if let Some(error) = current.iter().find_map(|event| match event {
                        Observation::Failed(error) => Some(error.clone()),
                        _ => None,
                    }) {
                        return Err(error.into());
                    }
                    if let Some(value) = select(&current) {
                        return Ok(value);
                    }
                }
                events.changed().await?;
            }
        })
        .await
    }

    pub async fn publication(&self, event_id: uuid::Uuid) -> TestResult<(String, String)> {
        let event_id = event_id.to_string();
        let publication = self
            .wait("successful real SQS SendMessage", |events| {
                events.iter().find_map(|event| match event {
                    Observation::Sent { body, message_id } if body.contains(&event_id) => {
                        Some((body.clone(), message_id.clone()))
                    }
                    _ => None,
                })
            })
            .await?;
        self.wait(
            "Sequin delivery accepted by child HTTP with 202",
            |events| {
                events
                    .iter()
                    .any(|event| {
                        matches!(event,
                            Observation::Accepted { body } if body.contains(&event_id)
                        )
                    })
                    .then_some(())
            },
        )
        .await?;
        Ok(publication)
    }

    pub async fn publications(
        &self,
        event_id: uuid::Uuid,
        count: usize,
    ) -> TestResult<Vec<(String, String)>> {
        let event_id = event_id.to_string();
        self.wait("distinct real SQS publications", |events| {
            let sent: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    Observation::Sent { body, message_id } if body.contains(&event_id) => {
                        Some((body.clone(), message_id.clone()))
                    }
                    _ => None,
                })
                .collect();
            (sent.len() >= count).then_some(sent)
        })
        .await
    }

    pub async fn accepted_batch(&self, event_id: uuid::Uuid) -> TestResult<String> {
        let event_id = event_id.to_string();
        self.wait("actual Sequin batch accepted by child", |events| {
            events.iter().find_map(|event| match event {
                Observation::Accepted { body } if body.contains(&event_id) => Some(body.clone()),
                _ => None,
            })
        })
        .await
    }

    pub async fn rejected_batch(&self, event_id: uuid::Uuid) -> TestResult<(String, StatusCode)> {
        let event_id = event_id.to_string();
        self.wait("actual child non-202 response to Sequin", |events| {
            events.iter().find_map(|event| match event {
                Observation::Rejected { body, status } if body.contains(&event_id) => {
                    Some((body.clone(), *status))
                }
                _ => None,
            })
        })
        .await
    }

    pub async fn response_withheld(&self, operation: &str) -> TestResult<String> {
        self.wait(
            "LocalStack accepted operation, response held outside child",
            |events| {
                events.iter().find_map(|event| match event {
                    Observation::ResponseWithheld {
                        operation: observed,
                        invocation,
                    } if observed == operation => Some(invocation.clone()),
                    _ => None,
                })
            },
        )
        .await
    }

    pub async fn delete_replied(&self, receipt: &Receipt) -> TestResult<String> {
        self.wait(
            "real DeleteMessage response forwarded after acknowledgment retry",
            |events| {
                events.iter().find_map(|event| match event {
                    Observation::DeleteReplied { handle, invocation }
                        if handle == &receipt.handle =>
                    {
                        Some(invocation.clone())
                    }
                    _ => None,
                })
            },
        )
        .await
    }

    pub fn assert_response_lost_once(&self, invocation: &str) {
        let events = self.0.borrow();
        let lost = events
            .iter()
            .filter(|event| {
                matches!(event,
                    Observation::ResponseLost { invocation: observed } if observed == invocation
                )
            })
            .count();
        assert_eq!(
            1, lost,
            "one successful provider response must actually be dropped"
        );
        let attempts = events
            .iter()
            .filter(|event| {
                matches!(event,
                    Observation::SqsAttempt { invocation: observed, .. } if observed == invocation
                )
            })
            .count();
        assert!(
            (1..=2).contains(&attempts),
            "SDK attempts bounded at two: {attempts}"
        );
        let disconnected_retries = events
            .iter()
            .filter(|event| {
                matches!(event,
                    Observation::SdkRetryDropped { invocation: observed } if observed == invocation
                )
            })
            .count();
        assert_eq!(
            attempts - 1,
            disconnected_retries,
            "same SDK invocation must not hide ambiguity by republishing"
        );
    }

    pub fn assert_delete_retry_bounded(&self, receipt: &Receipt, lost: &str, replied: &str) {
        assert_ne!(
            lost, replied,
            "must reach runtime acknowledgment retry, not only SDK retry"
        );
        let events = self.0.borrow();
        let attempts: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                Observation::SqsAttempt {
                    operation,
                    invocation,
                    handle,
                } if operation == "DeleteMessage" && handle.as_deref() == Some(&receipt.handle) => {
                    Some(invocation)
                }
                _ => None,
            })
            .collect();
        let operations: std::collections::HashSet<_> = attempts.iter().copied().collect();
        assert_eq!(
            2,
            operations.len(),
            "one failed delete operation, one successful ack retry"
        );
        assert!(
            attempts.len() <= 6,
            "at most three runtime deletes, two SDK attempts each"
        );
        let successful = events
            .iter()
            .filter(|event| {
                matches!(event,
                    Observation::Deleted { handle } if handle == &receipt.handle
                )
            })
            .count();
        assert_eq!(
            2, successful,
            "both the lost response and retried delete must come from real SQS"
        );
        let receives = events
            .iter()
            .filter(|event| {
                matches!(event,
                    Observation::Received(observed) if observed.message_id == receipt.message_id
                )
            })
            .count();
        assert_eq!(
            1, receives,
            "acknowledgment retry must not receive the job again"
        );
    }

    pub async fn received(&self, message_id: &str, count: u32) -> TestResult<Receipt> {
        self.wait(&format!("real SQS receive {count}"), |events| {
            events.iter().find_map(|event| match event {
                Observation::Received(receipt)
                    if receipt.message_id == message_id && receipt.count == count =>
                {
                    Some(receipt.clone())
                }
                _ => None,
            })
        })
        .await
    }

    pub async fn completed(&self, message_id: &str) -> TestResult {
        self.wait("successful delete of the unrelated real job", |events| {
            events
                .iter()
                .any(|event| match event {
                    Observation::Received(receipt) if receipt.message_id == message_id => {
                        events.iter().any(|event| {
                            matches!(event,
                                Observation::Deleted { handle } if handle == &receipt.handle
                            )
                        })
                    }
                    _ => false,
                })
                .then_some(())
        })
        .await
    }

    pub async fn delete_held(&self, receipt: &Receipt) -> TestResult {
        self.wait("handler finished, DeleteMessage not forwarded", |events| {
            events
                .iter()
                .any(|event| {
                    matches!(event,
                        Observation::DeleteHeld { handle } if handle == &receipt.handle
                    )
                })
                .then_some(())
        })
        .await
    }

    pub async fn deleted(&self, receipt: &Receipt) -> TestResult {
        self.wait("successful real SQS DeleteMessage", |events| {
            events
                .iter()
                .any(|event| {
                    matches!(event,
                        Observation::Deleted { handle } if handle == &receipt.handle
                    )
                })
                .then_some(())
        })
        .await
    }

    pub async fn retry_settled(&self, receipt: &Receipt) -> TestResult<i64> {
        self.wait("worker retry visibility applied by real SQS", |events| {
            events.iter().find_map(|event| match event {
                Observation::VisibilityChanged { handle, seconds } if handle == &receipt.handle => {
                    Some(*seconds)
                }
                _ => None,
            })
        })
        .await
    }

    pub fn assert_never_deleted(&self, receipts: &[Receipt]) {
        assert!(
            !self.0.borrow().iter().any(|event| match event {
                Observation::Deleted { handle } | Observation::DeleteHeld { handle } => {
                    receipts.iter().any(|receipt| &receipt.handle == handle)
                }
                _ => false,
            }),
            "poison must never reach DeleteMessage"
        );
    }
}

/// One accepted operation loses its reply. Retries of that same SDK invocation are
/// disconnected before forwarding, so SDK retry cannot conceal the ambiguous outcome.
#[derive(Clone)]
pub struct ResponseLoss {
    operation: &'static str,
    invocation: Arc<Mutex<Option<String>>>,
    release: watch::Sender<bool>,
}

impl ResponseLoss {
    pub fn release(&self) {
        self.release.send_replace(true);
    }

    async fn released(&self) -> TestResult {
        let mut release = self.release.subscribe();
        release.wait_for(|released| *released).await?;
        Ok(())
    }
}

#[derive(Clone)]
enum Destination {
    Sqs {
        consumer: &'static str,
        origin: String,
        hold_deletes: Arc<AtomicBool>,
    },
    Webhook,
}

#[derive(Clone)]
struct RelayState {
    upstream: String,
    destination: Destination,
    observations: Observations,
    client: reqwest::Client,
    loss: watch::Sender<Option<ResponseLoss>>,
}

pub struct Relay {
    pub endpoint: String,
    hold_deletes: Arc<AtomicBool>,
    loss: watch::Sender<Option<ResponseLoss>>,
    task: Option<JoinHandle<TestResult>>,
}

impl Relay {
    pub async fn sqs(
        consumer: &'static str,
        observations: Observations,
        hold: bool,
    ) -> TestResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let hold_deletes = Arc::new(AtomicBool::new(hold));
        Self::start(
            listener,
            RelayState {
                upstream: test_api::localstack::get_endpoint_url().to_owned(),
                destination: Destination::Sqs {
                    consumer,
                    origin: endpoint,
                    hold_deletes: hold_deletes.clone(),
                },
                observations,
                client: client()?,
                loss: watch::channel(None).0,
            },
            hold_deletes,
        )
    }

    pub async fn webhook(child: SocketAddr, observations: Observations) -> TestResult<Self> {
        let listener = TcpListener::bind(test_api::get_sequin_worker_webhook_bind_addr()).await?;
        Self::start(
            listener,
            RelayState {
                upstream: format!("http://{child}"),
                destination: Destination::Webhook,
                observations,
                client: client()?,
                loss: watch::channel(None).0,
            },
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn start(
        listener: TcpListener,
        state: RelayState,
        hold_deletes: Arc<AtomicBool>,
    ) -> TestResult<Self> {
        let endpoint = format!("http://{}", listener.local_addr()?);
        let observations = state.observations.clone();
        let loss = state.loss.clone();
        let task = tokio::spawn(async move {
            // Own all connections. Aborting this server also aborts held deletes and long polls.
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted?;
                        let state = state.clone();
                        let service = hyper::service::service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                            forward(state.clone(), request.map(Body::new))
                        });
                        connections.spawn(async move {
                            hyper::server::conn::http1::Builder::new()
                                .keep_alive(false)
                                .serve_connection(TokioIo::new(socket), service)
                                .await
                        });
                    }
                    completed = connections.join_next(), if !connections.is_empty() => {
                        // Connection loss is expected when a child is SIGKILLed. A task panic isn't.
                        if let Some(Err(error)) = completed {
                            observations.record(Observation::Failed(format!("relay task failed: {error}")));
                        }
                    }
                }
            }
        });
        Ok(Self {
            endpoint,
            hold_deletes,
            loss,
            task: Some(task),
        })
    }

    pub fn queue_url(&self) -> TestResult<String> {
        let canonical = url::Url::parse(&super::WORKER_SQS.queue_url())?;
        Ok(format!("{}{}", self.endpoint, canonical.path()))
    }

    pub fn lose_response_once(&self, operation: &'static str) -> ResponseLoss {
        assert!(matches!(operation, "SendMessage" | "DeleteMessage"));
        let loss = ResponseLoss {
            operation,
            invocation: Arc::new(Mutex::new(None)),
            release: watch::channel(false).0,
        };
        assert!(
            self.loss.send_replace(Some(loss.clone())).is_none(),
            "only one fault per relay"
        );
        loss
    }

    pub fn allow_new_deletes(&self) {
        // Previously held requests stay held forever, even after their child has died.
        // Releasing them could delete a message before the restarted worker receives it.
        self.hold_deletes.store(false, Ordering::SeqCst);
    }

    pub async fn stop(mut self) -> TestResult {
        if let Some(task) = self.task.take() {
            task.abort();
            match task.await {
                Err(error) if error.is_cancelled() => {}
                Err(error) => return Err(error.into()),
                Ok(result) => result?,
            }
        }
        Ok(())
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

fn client() -> TestResult<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()?)
}

async fn forward(state: RelayState, request: Request) -> Result<Response, std::io::Error> {
    match try_forward(&state, request).await {
        Ok(Some(response)) => Ok(response),
        // A Hyper service error closes the actual socket without any HTTP response.
        Ok(None) => Err(std::io::Error::other("test relay injected response loss")),
        Err(error) => {
            state.observations.record(Observation::Failed(format!(
                "external relay failed: {error}"
            )));
            Err(std::io::Error::other("external relay failed"))
        }
    }
}

fn transport_headers(mut headers: HeaderMap) -> HeaderMap {
    for header in ["host", "connection", "content-length", "transfer-encoding"] {
        headers.remove(header);
    }
    headers
}

async fn try_forward(state: &RelayState, request: Request) -> TestResult<Option<Response>> {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 2 * 1024 * 1024).await?;
    let mut forwarded_body = bytes.to_vec();
    let mut operation = "";
    let mut sqs_request = Value::Null;
    let mut invocation = String::new();
    let mut lose_response = None;
    if let Destination::Sqs {
        origin,
        hold_deletes,
        ..
    } = &state.destination
    {
        operation = parts
            .headers
            .get("x-amz-target")
            .ok_or("SQS JSON target missing")?
            .to_str()?
            .rsplit('.')
            .next()
            .ok_or("SQS operation missing")?;
        sqs_request = serde_json::from_slice(&bytes)?;
        let queue_url = url::Url::parse(field(&sqs_request, "QueueUrl")?)?;
        if queue_url.origin() != url::Url::parse(origin)?.origin() {
            return Err("worker bypassed relay queue origin".into());
        }
        let source = url::Url::parse(&super::WORKER_SQS.queue_url())?;
        let dlq = url::Url::parse(&super::WORKER_SQS.dead_letter_queue_url())?;
        if queue_url.path() != source.path() && queue_url.path() != dlq.path() {
            return Err("relay request targets a queue outside this fixture".into());
        }
        if matches!(operation, "SendMessage" | "DeleteMessage") {
            invocation = parts
                .headers
                .get("amz-sdk-invocation-id")
                .ok_or("SDK invocation ID missing")?
                .to_str()?
                .to_owned();
            state.observations.record(Observation::SqsAttempt {
                operation: operation.to_owned(),
                invocation: invocation.clone(),
                handle: sqs_request
                    .get("ReceiptHandle")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
            let loss = state.loss.borrow().clone();
            if let Some(loss) = loss.filter(|loss| loss.operation == operation) {
                let mut selected = loss.invocation.lock().await;
                if selected.as_ref() == Some(&invocation) {
                    state
                        .observations
                        .record(Observation::SdkRetryDropped { invocation });
                    return Ok(None);
                } else if selected.is_none() {
                    *selected = Some(invocation.clone());
                    lose_response = Some(loss.clone());
                } else {
                    // Do not let a new runtime attempt pass before the test's DB barrier.
                    drop(selected);
                    loss.released().await?;
                }
            }
        }
        if operation == "DeleteMessage" && hold_deletes.load(Ordering::SeqCst) {
            state.observations.record(Observation::DeleteHeld {
                handle: field(&sqs_request, "ReceiptHandle")?.to_owned(),
            });
            // Never forward this request, not even when the child socket is killed.
            return std::future::pending().await;
        }
        // The worker's QueueUrl and endpoint share the relay origin. Only this external
        // fixture maps them back to the canonical LocalStack origin; names/ARNs stay intact.
        sqs_request["QueueUrl"] = format!("{}{}", state.upstream, queue_url.path()).into();
        forwarded_body = serde_json::to_vec(&sqs_request)?;
    }
    let path = parts
        .uri
        .path_and_query()
        .map_or("/", |value| value.as_str());
    let result = state
        .client
        .request(parts.method, format!("{}{path}", state.upstream))
        .headers(transport_headers(parts.headers.clone()))
        .body(forwarded_body)
        .send()
        .await?;
    let status = result.status();
    let headers = transport_headers(result.headers().clone());
    let response_body = result.bytes().await?;
    if matches!(state.destination, Destination::Webhook) {
        let body = String::from_utf8(bytes.to_vec())?;
        state
            .observations
            .record(if status == StatusCode::ACCEPTED {
                Observation::Accepted { body }
            } else {
                Observation::Rejected { body, status }
            });
    } else if status.is_success() {
        if let Destination::Sqs { consumer, .. } = &state.destination {
            let result: Value = serde_json::from_slice(&response_body)?;
            match operation {
                "SendMessage" => state.observations.record(Observation::Sent {
                    body: field(&sqs_request, "MessageBody")?.to_owned(),
                    message_id: field(&result, "MessageId")?.to_owned(),
                }),
                "ReceiveMessage" => {
                    if let Some(messages) = result.get("Messages").and_then(Value::as_array) {
                        for message in messages {
                            state.observations.record(Observation::Received(Receipt {
                                consumer,
                                message_id: field(message, "MessageId")?.to_owned(),
                                body: field(message, "Body")?.to_owned(),
                                handle: field(message, "ReceiptHandle")?.to_owned(),
                                count: field(&message["Attributes"], "ApproximateReceiveCount")?
                                    .parse()?,
                            }));
                        }
                    }
                }
                "DeleteMessage" => state.observations.record(Observation::Deleted {
                    handle: field(&sqs_request, "ReceiptHandle")?.to_owned(),
                }),
                "ChangeMessageVisibility" => {
                    state.observations.record(Observation::VisibilityChanged {
                        handle: field(&sqs_request, "ReceiptHandle")?.to_owned(),
                        seconds: sqs_request["VisibilityTimeout"]
                            .as_i64()
                            .ok_or("visibility missing")?,
                    })
                }
                _ => {}
            }
        }
    } else {
        return Err(format!("upstream HTTP {status} for {operation}").into());
    }
    if let Some(loss) = lose_response {
        state.observations.record(Observation::ResponseWithheld {
            operation: operation.to_owned(),
            invocation: invocation.clone(),
        });
        loss.released().await?;
        state
            .observations
            .record(Observation::ResponseLost { invocation });
        return Ok(None);
    }
    if operation == "DeleteMessage" {
        state.observations.record(Observation::DeleteReplied {
            handle: field(&sqs_request, "ReceiptHandle")?.to_owned(),
            invocation,
        });
    }
    let mut response = Response::new(Body::from(response_body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(Some(response))
}

fn field<'a>(value: &'a Value, name: &str) -> TestResult<&'a str> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("SQS field {name} missing").into())
}
