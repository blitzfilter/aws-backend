use super::{HEADER_READ_TIMEOUT, MAX_HTTP_CONNECTIONS, MAX_HTTP_HEADER_BYTES, MAX_HTTP_HEADERS};
use crate::{
    QueueConfig, WorkerRuntime,
    cdc::{DomainJobPayload, MAX_CDC_BODY_BYTES, WorkerQueue},
    serve_with_runtime,
};
use std::time::Duration;

async fn probe(runtime: WorkerRuntime, method: &str, path: &str) -> axum::response::Response {
    use hyper::service::Service;
    hyper_util::service::TowerToHyperService::new(super::router(runtime))
        .call(
            axum::http::Request::builder()
                .method(method)
                .uri(path)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn should_return_safe_noncacheable_identity_state_and_admission_including_drain_and_head() {
    use crate::operations::{OperationalConfig, OperationalIdentity};
    let (mut runtime, _receivers) =
        WorkerRuntime::with_notification_delivery_queue(QueueConfig::new(1)).unwrap();
    let config = crate::WorkerConfig::from_getter(|_| None).unwrap();
    let sha = "d5bd9ca854e713b0c587528f02037211b2020fd4";
    runtime.operational = Some(OperationalConfig {
        identity: OperationalIdentity::parse(
            crate::WorkerScope::NotificationDelivery,
            Some("prod"),
            Some(sha.into()),
            &config,
        )
        .unwrap(),
        drain: config.drain_timeout(),
        stop: config.stop_timeout(),
        execution: Duration::from_secs(240),
    });
    for draining in [false, true] {
        if draining {
            runtime.shutdown();
        }
        for path in [
            "/health",
            "/ready",
            "/admission",
            "/state",
            "/version",
            "/missing",
        ] {
            let response = probe(runtime.clone(), "GET", path).await;
            assert_eq!("no-store, max-age=0", response.headers()["cache-control"]);
            if path == "/admission" {
                assert_eq!(if draining { 503 } else { 200 }, response.status().as_u16());
            }
            if path == "/state" || path == "/version" {
                assert_eq!(200, response.status());
                let body = axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap();
                let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
                if path == "/version" {
                    assert_eq!(
                        serde_json::json!({"schema_version":1,"component":"aura-historia-worker",
                        "scope":"notification-delivery","source_sha":sha,"local":false}),
                        value
                    );
                } else {
                    assert_eq!(!draining, value["ingress_admission"]);
                    assert_eq!(
                        if draining { "DRAINING" } else { "RUNNING" },
                        value["lifecycle"]
                    );
                    assert_eq!(7, value.as_object().unwrap().len());
                }
            }
            let head = probe(runtime.clone(), "HEAD", path).await;
            assert_eq!("no-store, max-age=0", head.headers()["cache-control"]);
            assert!(
                axum::body::to_bytes(head.into_body(), 4096)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }
    assert_eq!(
        503,
        probe(WorkerRuntime::default(), "GET", "/admission")
            .await
            .status()
    );
    assert_eq!(
        503,
        probe(WorkerRuntime::default(), "GET", "/version")
            .await
            .status()
    );
}

const NOTIFICATION_DELIVERY_UUID: &str = "01900000-0000-7000-8000-000000000001";
const NOTIFICATION_DELIVERY_TYPE_ID: &str = "nd_01j0000000e008000000000001";
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::Instant,
};

async fn server(
    runtime: WorkerRuntime,
) -> (
    std::net::SocketAddr,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<(), crate::WorkerRunError>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(serve_with_runtime(listener, runtime, async move {
        let _closed = stopped.await;
    }));
    (address, stop, task)
}
fn body(padding: usize) -> String {
    serde_json::json!({"changes":[{"schema":"public","table":"notification_deliveries","operation":"insert",
        "record":{"notification_delivery_id":NOTIFICATION_DELIVERY_UUID}, "future_metadata":"x".repeat(padding)}]}).to_string()
}

fn assert_notification_delivery_type_id(payload: &DomainJobPayload) {
    let DomainJobPayload::NotificationDeliveryCreated(job) = payload else {
        panic!("expected notification delivery job");
    };
    assert_eq!(
        NOTIFICATION_DELIVERY_UUID,
        job.notification_delivery_id.as_uuid().to_string()
    );
    assert_eq!(
        NOTIFICATION_DELIVERY_TYPE_ID,
        job.notification_delivery_id.to_string()
    );
}
async fn response(stream: &mut TcpStream) -> String {
    let mut response = String::new();
    tokio::time::timeout(
        Duration::from_secs(12),
        stream.read_to_string(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    response
}

#[tokio::test]
async fn should_read_fragmented_and_chunked_full_bodies_before_any_publication() {
    for chunked in [false, true] {
        let (runtime, mut receivers) =
            WorkerRuntime::with_notification_delivery_queue(QueueConfig::new(1)).unwrap();
        let (address, stop, server) = server(runtime).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        let body = body(70_000);
        let headers = if chunked {
            "transfer-encoding: chunked".into()
        } else {
            format!("content-length: {}", body.len())
        };
        stream
            .write_all(
                format!("POST /cdc/sequin HTTP/1.1\r\nhost: localhost\r\n{headers}\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let (first, last) = body.split_at(65_000);
        if chunked {
            stream
                .write_all(format!("{:x}\r\n", first.len()).as_bytes())
                .await
                .unwrap();
        }
        stream.write_all(first.as_bytes()).await.unwrap();
        if chunked {
            stream.write_all(b"\r\n").await.unwrap();
        }
        assert!(
            receivers
                .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_millis(20))
                .await
                .is_err()
        );
        if chunked {
            stream
                .write_all(format!("{:x}\r\n", last.len()).as_bytes())
                .await
                .unwrap();
        }
        stream.write_all(last.as_bytes()).await.unwrap();
        if chunked {
            stream.write_all(b"\r\n0\r\n\r\n").await.unwrap();
        }
        assert!(
            response(&mut stream)
                .await
                .starts_with("HTTP/1.1 202 Accepted")
        );
        assert!(
            receivers
                .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_secs(1))
                .await
                .unwrap()
                .is_some()
        );
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    }
}
#[tokio::test]
async fn should_accept_request_when_headers_and_body_are_fragmented_across_tcp_writes() {
    let (runtime, mut receivers) =
        WorkerRuntime::with_notification_delivery_queue(QueueConfig::new(1)).unwrap();
    let (address, stop, server) = server(runtime).await;
    let mut stream = TcpStream::connect(address).await.unwrap();
    let body = body(0);

    stream
        .write_all(b"POST /cdc/sequin HTTP/1.1\r\nhost: local")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    stream
        .write_all(
            format!(
                "host\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let (first, last) = body.split_at(body.len() / 2);
    stream.write_all(first.as_bytes()).await.unwrap();
    assert!(
        receivers
            .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_millis(20))
            .await
            .is_err()
    );
    stream.write_all(last.as_bytes()).await.unwrap();

    assert!(
        response(&mut stream)
            .await
            .starts_with("HTTP/1.1 202 Accepted")
    );
    let job = receivers
        .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_secs(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(WorkerQueue::NotificationDelivery, job.target_queue);
    assert_notification_delivery_type_id(&job.payload);
    assert!(
        receivers
            .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_millis(20))
            .await
            .is_err()
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn should_accept_valid_cdc_body_at_configured_size_limit() {
    let base_body_len = body(0).len();
    let padding = MAX_CDC_BODY_BYTES.checked_sub(base_body_len).unwrap();
    let body = body(padding);
    assert_eq!(MAX_CDC_BODY_BYTES, body.len());

    let (runtime, mut receivers) =
        WorkerRuntime::with_notification_delivery_queue(QueueConfig::new(1)).unwrap();
    let (address, stop, server) = server(runtime).await;
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(
            format!(
                "POST /cdc/sequin HTTP/1.1\r\nhost: localhost\r\ncontent-length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    stream.write_all(body.as_bytes()).await.unwrap();

    assert!(
        response(&mut stream)
            .await
            .starts_with("HTTP/1.1 202 Accepted")
    );
    let job = receivers
        .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_secs(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(WorkerQueue::NotificationDelivery, job.target_queue);
    assert_notification_delivery_type_id(&job.payload);
    assert!(
        receivers
            .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_millis(20))
            .await
            .is_err()
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn should_reject_malformed_http_headers_without_publication() {
    let (runtime, mut receivers) =
        WorkerRuntime::with_notification_delivery_queue(QueueConfig::new(1)).unwrap();
    let (address, stop, server) = server(runtime).await;
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(
            b"POST /cdc/sequin HTTP/1.1\r\nhost: localhost\r\nbad header: value\r\ncontent-length: 0\r\n\r\n",
        )
        .await
        .unwrap();

    assert!(!response(&mut stream).await.contains("202 Accepted"));
    assert!(
        receivers
            .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_millis(20))
            .await
            .is_err()
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn should_reject_oversize_and_excessive_change_batches_without_publication() {
    let (runtime, mut receivers) =
        WorkerRuntime::with_notification_delivery_queue(QueueConfig::new(1)).unwrap();
    let (address, stop, server) = server(runtime).await;
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(
            format!(
                "POST /cdc/sequin HTTP/1.1\r\nhost: localhost\r\ncontent-length: {}\r\n\r\n",
                MAX_CDC_BODY_BYTES + 1
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    assert!(response(&mut stream).await.starts_with("HTTP/1.1 413"));
    let change = serde_json::from_str::<serde_json::Value>(&body(0)).unwrap()["changes"][0].clone();
    let batch = serde_json::json!({"changes":vec![change; 101]}).to_string();
    let response = reqwest::Client::new()
        .post(format!("http://{address}/cdc/sequin"))
        .body(batch)
        .send()
        .await
        .unwrap();
    assert_eq!(413, response.status().as_u16());
    assert!(
        receivers
            .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_millis(20))
            .await
            .is_err()
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}
#[tokio::test]
async fn should_report_dead_consumer_without_rejecting_independent_ingress() {
    let (mut runtime, mut receivers) =
        WorkerRuntime::with_notification_delivery_queue(QueueConfig::new(1)).unwrap();
    runtime.control = crate::queue::RuntimeControl::new(false);
    let (address, stop, server) = server(runtime).await;
    let client = reqwest::Client::new();
    for path in ["/health", "/ready"] {
        assert_eq!(
            503,
            client
                .get(format!("http://{address}{path}"))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        );
    }
    assert_eq!(
        202,
        client
            .post(format!("http://{address}/cdc/sequin"))
            .body(body(0))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    );
    assert!(
        receivers
            .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_secs(1))
            .await
            .unwrap()
            .is_some()
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}
#[tokio::test]
async fn should_reject_cdc_ingress_after_runtime_starts_stopping() {
    let (runtime, mut receivers) =
        WorkerRuntime::with_notification_delivery_queue(QueueConfig::new(1)).unwrap();
    let stopping = runtime.clone();
    let (address, _stop, server) = server(runtime).await;

    stopping.shutdown();
    if let Ok(response) = reqwest::Client::new()
        .post(format!("http://{address}/cdc/sequin"))
        .body(body(0))
        .send()
        .await
    {
        assert_eq!(503, response.status().as_u16());
    }
    assert!(
        receivers
            .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_millis(20))
            .await
            .is_err()
    );
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn should_bound_accepted_sockets_before_headers_and_drain_idle_connections() {
    let (address, stop, server) = server(WorkerRuntime::empty()).await;
    let mut sockets = Vec::new();
    for _ in 0..MAX_HTTP_CONNECTIONS {
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET /health HTTP/1.1\r\nhost:")
            .await
            .unwrap();
        sockets.push(stream);
    }
    let mut overflow = TcpStream::connect(address).await.unwrap();
    overflow
        .write_all(b"GET /health HTTP/1.1\r\nhost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut first = [0];
    assert!(
        tokio::time::timeout(Duration::from_millis(100), overflow.read(&mut first))
            .await
            .is_err()
    );
    drop(sockets.remove(0));
    assert!(
        tokio::time::timeout(Duration::from_secs(1), response(&mut overflow))
            .await
            .unwrap()
            .starts_with("HTTP/1.1 200")
    );
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    for mut socket in sockets {
        assert!(response(&mut socket).await.is_empty());
    }
    assert!(TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn should_deadline_fragmented_headers_without_body_publication() {
    let (runtime, mut receivers) =
        WorkerRuntime::with_notification_delivery_queue(QueueConfig::new(1)).unwrap();
    let (address, stop, server) = server(runtime).await;
    let mut stream = TcpStream::connect(address).await.unwrap();
    let started = Instant::now();
    stream
        .write_all(b"POST /cdc/sequin HTTP/1.1\r\nhost:")
        .await
        .unwrap();
    tokio::time::sleep(HEADER_READ_TIMEOUT / 2).await;
    stream
        .write_all(b" localhost\r\ncontent-length:")
        .await
        .unwrap();
    let response = response(&mut stream).await;
    assert!(!response.contains("202"));
    assert!(started.elapsed() >= HEADER_READ_TIMEOUT);
    assert!(started.elapsed() < HEADER_READ_TIMEOUT + Duration::from_secs(2));
    assert!(
        receivers
            .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_millis(20))
            .await
            .is_err()
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn should_reject_header_count_and_byte_overflow_without_publication() {
    let (runtime, mut receivers) =
        WorkerRuntime::with_notification_delivery_queue(QueueConfig::new(1)).unwrap();
    let (address, stop, server) = server(runtime).await;
    let prefix = "POST /cdc/sequin HTTP/1.1\r\nhost: localhost\r\nx-padding: ";
    for request in [
        format!(
            "POST /cdc/sequin HTTP/1.1\r\n{}\r\n",
            "x-header: value\r\n".repeat(MAX_HTTP_HEADERS + 1)
        ),
        format!(
            "{prefix}{}",
            "x".repeat(MAX_HTTP_HEADER_BYTES - prefix.len())
        ),
    ] {
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        assert!(response(&mut stream).await.starts_with("HTTP/1.1 431"));
    }
    assert!(
        receivers
            .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_millis(20))
            .await
            .is_err()
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn should_abort_owned_connections_when_server_owner_is_cancelled() {
    let runtime = WorkerRuntime::empty();
    let control = runtime.control.clone();
    let (address, _stop, server) = server(runtime).await;
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(b"GET /health HTTP/1.1\r\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    assert!(control.stopping());
    assert!(
        tokio::time::timeout(Duration::from_secs(1), response(&mut stream))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn should_timeout_incomplete_body_without_publication() {
    let (runtime, mut receivers) =
        WorkerRuntime::with_notification_delivery_queue(QueueConfig::new(1)).unwrap();
    let (address, stop, server) = server(runtime).await;
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(b"POST /cdc/sequin HTTP/1.1\r\nhost: localhost\r\ncontent-length: 100\r\n\r\n{}")
        .await
        .unwrap();
    assert!(response(&mut stream).await.starts_with("HTTP/1.1 408"));
    assert!(
        receivers
            .recv_timeout(WorkerQueue::NotificationDelivery, Duration::from_millis(20))
            .await
            .is_err()
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}
