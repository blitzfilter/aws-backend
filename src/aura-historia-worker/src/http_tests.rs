use super::*;
use crate::{QueueConfig, cdc::WorkerQueue, serve_with_runtime};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
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
        "record":{"notification_delivery_id":"10000000-0000-0000-0000-000000000001"}, "future_metadata":"x".repeat(padding)}]}).to_string()
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
