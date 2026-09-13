use super::*;
use crate::config::LifecycleConfig;
use rstest::rstest;
use std::time::Instant;
use tokio::net::TcpSocket;

const SHA: &str = "ea91dace22616e9c7fcc3c2234c6758b7f7d594c";
const CANARY: &str = "private-operations-request-canary";
const TEST_TIMEOUT: Duration = Duration::from_secs(2);
const HEALTH: &[u8] = b"GET /health HTTP/1.1\r\n\r\n";
const PARTIAL: &[u8] = b"GET /health HTTP/1.1\r\nX-Slow: ";
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct RunningServer {
    address: SocketAddr,
    lifecycle: Lifecycle,
    stop: watch::Sender<bool>,
    tasks: JoinSet<io::Result<()>>,
}

impl RunningServer {
    async fn start() -> TestResult<Self> {
        let lifecycle = Lifecycle::new(LifecycleConfig::default());
        assert!(lifecycle.ready());
        // Keep the OS-assigned port owned. Only bind-defense tests call bind directly;
        // constructing this private fixture avoids a reserve/drop/rebind port race.
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = OperationsServer {
            listener,
            lifecycle: lifecycle.clone(),
            commit_sha: SHA.into(),
        };
        let (stop, receiver) = watch::channel(false);
        let mut tasks = JoinSet::new();
        tasks.spawn(server.run_until(receiver));
        Ok(Self {
            address,
            lifecycle,
            stop,
            tasks,
        })
    }

    async fn connect(&self, request: &[u8]) -> TestResult<TcpStream> {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let mut client = TcpStream::connect(self.address).await?;
            client.write_all(request).await?;
            Ok::<_, io::Error>(client)
        })
        .await?
        .map_err(Into::into)
    }

    async fn healthy(&self) -> TestResult {
        let mut client = self.connect(HEALTH).await?;
        assert_safe_response(&response(&mut client).await?, "200 OK", "READY")?;
        assert!(!self.lifecycle.failed());
        assert_eq!(self.lifecycle.state(), CrawlerState::Ready);
        Ok(())
    }

    async fn saturate(&self) -> TestResult<Vec<TcpStream>> {
        let mut clients = Vec::new();
        for _ in 0..16 {
            clients.push(self.connect(PARTIAL).await?);
        }
        wait_for_tasks(17).await?;
        // Let every spawned wrapper register its one-second timer before pausing time.
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(task_count(), 17);
        Ok(clients)
    }

    async fn finish(&mut self) -> TestResult {
        self.stop.send_replace(true);
        tokio::time::timeout(TEST_TIMEOUT, self.tasks.join_next())
            .await?
            .ok_or("missing listener task")???;
        assert!(self.tasks.is_empty());
        assert_eq!(
            task_count(),
            0,
            "connection tasks must be joined, not detached"
        );
        assert!(!self.lifecycle.failed());
        Ok(())
    }
}

// Each test has its own current-thread runtime: only the listener and its connection
// tasks are spawned. Metrics prove actual admission, not merely TCP backlog occupancy.
// JoinSet drop aborts the fixture on assertion/error; runtime drop owns its descendants.
fn task_count() -> usize {
    tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks()
}

async fn wait_for_tasks(expected: usize) -> TestResult {
    let end = Instant::now() + TEST_TIMEOUT;
    while task_count() != expected {
        if Instant::now() >= end {
            return Err(format!("expected {expected} owned tasks, got {}", task_count()).into());
        }
        // A yielding runnable task also prevents paused-clock auto-advance across an
        // assertion boundary. The guard uses wall time, including while Tokio is paused.
        tokio::task::yield_now().await;
    }
    Ok(())
}

async fn response(client: &mut TcpStream) -> TestResult<String> {
    let mut text = String::new();
    tokio::time::timeout(TEST_TIMEOUT, client.read_to_string(&mut text)).await??;
    Ok(text)
}

fn assert_safe_response(response: &str, status: &str, state: &str) -> TestResult {
    let (headers, body) = response.split_once("\r\n\r\n").ok_or("missing body")?;
    assert_eq!(
        headers.lines().next(),
        Some(format!("HTTP/1.1 {status}").as_str())
    );
    for header in [
        "Content-Type: application/json".to_owned(),
        "Cache-Control: no-store".to_owned(),
        "Connection: close".to_owned(),
        format!("Content-Length: {}", body.len()),
    ] {
        assert_eq!(headers.lines().filter(|line| *line == header).count(), 1);
    }
    assert_eq!(headers.lines().count(), 5);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(body)?,
        serde_json::json!({ "commit_sha": SHA, "state": state })
    );
    assert!(!response.contains(CANARY));
    Ok(())
}

async fn assert_closed(client: &mut TcpStream) -> TestResult {
    let mut byte = [0];
    match tokio::time::timeout(TEST_TIMEOUT, client.read(&mut byte)).await? {
        Ok(0) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
            ) =>
        {
            Ok(())
        }
        result => Err(format!("expected close without response, got {result:?}").into()),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn should_reject_bind_when_loopback_port_is_occupied() -> TestResult {
    let owner = TcpListener::bind("127.0.0.1:0").await?;
    let lifecycle = Lifecycle::new(LifecycleConfig::default());
    let result = OperationsServer::bind(owner.local_addr()?, lifecycle.clone(), SHA.into()).await;
    assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::AddrInUse));
    assert_eq!(lifecycle.state(), CrawlerState::Starting);
    assert!(!lifecycle.failed());
    // Rejection must neither replace nor close the fixture's listener.
    let _client = TcpStream::connect(owner.local_addr()?).await?;
    tokio::time::timeout(TEST_TIMEOUT, owner.accept()).await??;
    Ok(())
}

#[rstest]
#[case("127.0.0.1:0")]
#[case("[::1]:0")]
#[case("0.0.0.0:9083")]
#[case("[::]:9083")]
#[case("192.0.2.1:9083")]
#[case("[2001:db8::1]:9083")]
#[case("[::ffff:127.0.0.1]:9083")]
#[tokio::test(flavor = "current_thread")]
async fn should_reject_invalid_bind_before_touching_socket(#[case] address: &str) -> TestResult {
    let lifecycle = Lifecycle::new(LifecycleConfig::default());
    let result = OperationsServer::bind(address.parse()?, lifecycle.clone(), SHA.into()).await;
    assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::InvalidInput));
    assert_eq!(lifecycle.state(), CrawlerState::Starting);
    assert!(!lifecycle.failed());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn should_queue_seventeenth_request_and_recover_when_one_slot_finishes() -> TestResult {
    let mut server = RunningServer::start().await?;
    let mut clients = server.saturate().await?;
    let mut queued = server.connect(HEALTH).await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), queued.read(&mut [0]))
            .await
            .is_err()
    );
    assert_eq!(task_count(), 17, "only 16 connection tasks may be admitted");

    clients[0].write_all(b"\r\n\r\n").await?;
    tokio::time::timeout(Duration::from_millis(500), async {
        assert_safe_response(&response(&mut clients[0]).await?, "200 OK", "READY")?;
        assert_safe_response(&response(&mut queued).await?, "200 OK", "READY")
    })
    .await??;
    wait_for_tasks(16).await?;
    drop(clients);
    wait_for_tasks(1).await?;
    server.healthy().await?;
    server.finish().await
}

#[tokio::test(flavor = "current_thread")]
async fn should_expire_slow_headers_at_original_deadline_and_recover_all_slots() -> TestResult {
    let mut server = RunningServer::start().await?;
    let mut clients = server.saturate().await?;
    tokio::time::pause();
    for _ in 0..3 {
        tokio::time::advance(Duration::from_millis(300)).await;
        for client in &clients {
            assert_eq!(client.try_write(b"x")?, 1);
        }
        tokio::task::yield_now().await;
        assert_eq!(task_count(), 17);
    }
    tokio::time::advance(Duration::from_millis(200)).await;
    wait_for_tasks(1).await?;
    tokio::time::resume();
    for client in &mut clients {
        assert_closed(client).await?;
    }
    // Refill every slot, not just one, after the expired JoinSet entries are reaped.
    let recovered = server.saturate().await?;
    drop(recovered);
    wait_for_tasks(1).await?;
    server.healthy().await?;
    server.finish().await
}

#[tokio::test(flavor = "current_thread")]
async fn should_stop_saturated_listener_without_new_admissions_or_deadline_extension() -> TestResult
{
    let mut server = RunningServer::start().await?;
    let mut clients = server.saturate().await?;
    let mut queued = server.connect(HEALTH).await?;
    tokio::time::pause();
    tokio::time::advance(Duration::from_millis(600)).await;
    server.stop.send_replace(true);
    tokio::task::yield_now().await;
    assert_eq!(
        task_count(),
        17,
        "stop must drain rather than abort active probes"
    );
    for _ in 0..3 {
        // Synchronous loopback attempts cannot auto-advance the paused Tokio clock.
        let result =
            std::net::TcpStream::connect_timeout(&server.address, Duration::from_millis(100));
        assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::ConnectionRefused));
        tokio::time::advance(Duration::from_millis(100)).await;
        assert_eq!(task_count(), 17);
    }
    tokio::time::advance(Duration::from_millis(200)).await;
    // 1.1s from admission, only 0.5s from stop: restarting the request deadline fails.
    wait_for_tasks(0).await?;
    server.finish().await?;
    tokio::time::resume();
    assert_closed(&mut queued).await?;
    for client in &mut clients {
        assert_closed(client).await?;
    }
    Ok(())
}

#[rstest]
#[case::full_unterminated(4096, false)]
#[case::terminator_crosses_limit(4097, true)]
#[case::double_buffer(8192, true)]
#[tokio::test(flavor = "current_thread")]
async fn should_close_oversized_headers_without_response_and_keep_listener_healthy(
    #[case] length: usize,
    #[case] terminated: bool,
) -> TestResult {
    let mut server = RunningServer::start().await?;
    let mut request = format!("GET /health HTTP/1.1\r\nX-{CANARY}: ").into_bytes();
    request.resize(length - if terminated { 4 } else { 0 }, b'x');
    if terminated {
        request.extend_from_slice(b"\r\n\r\n");
    }
    let mut client = server.connect(&request).await?;
    // Buffer rejection must be immediate, not the one-second slow-header timeout.
    tokio::time::timeout(Duration::from_millis(500), assert_closed(&mut client)).await??;
    server.healthy().await?;
    server.finish().await
}

#[tokio::test(flavor = "current_thread")]
async fn should_accept_headers_ending_exactly_at_buffer_limit() -> TestResult {
    let mut server = RunningServer::start().await?;
    let mut request = PARTIAL.to_vec();
    request.resize(4092, b'x');
    request.extend_from_slice(b"\r\n\r\n");
    let mut client = server.connect(&request).await?;
    assert_safe_response(&response(&mut client).await?, "200 OK", "READY")?;
    server.finish().await
}

#[rstest]
#[case::empty_eof(false, false)]
#[case::partial_eof(true, false)]
#[case::partial_reset(true, true)]
#[tokio::test(flavor = "current_thread")]
async fn should_keep_eof_and_reset_request_local_while_runtime_is_draining(
    #[case] partial: bool,
    #[case] reset: bool,
) -> TestResult {
    let mut server = RunningServer::start().await?;
    let socket = TcpSocket::new_v4()?;
    if reset {
        #[allow(
            deprecated,
            reason = "zero linger forces an owned peer RST without a blocking wait"
        )]
        socket.set_linger(Some(Duration::ZERO))?;
    }
    let mut client = tokio::time::timeout(TEST_TIMEOUT, socket.connect(server.address)).await??;
    if partial {
        client.write_all(PARTIAL).await?;
    }
    wait_for_tasks(2).await?;
    server.lifecycle.stop(false);
    if reset {
        drop(client);
    } else {
        client.shutdown().await?;
        assert_closed(&mut client).await?;
    }
    wait_for_tasks(1).await?;
    // Runtime stop and operations stop are intentionally separate: probes remain
    // available while cron/review drain, and peer failures must not latch failure.
    let mut probe = server.connect(b"GET /ready HTTP/1.1\r\n\r\n").await?;
    assert_safe_response(
        &response(&mut probe).await?,
        "503 Service Unavailable",
        "DRAINING",
    )?;
    assert!(!server.lifecycle.failed());
    server.finish().await
}

#[rstest]
#[case("GET", "/health?token=")]
#[case("GET", "/ready?token=")]
#[case("GET", "/ops/version?token=")]
#[case("POST", "/health")]
#[case("HEAD", "/ready")]
#[case("OPTIONS", "/ops/version")]
#[case("DELETE", "/ops/version")]
#[tokio::test(flavor = "current_thread")]
async fn should_reject_queries_and_methods_with_only_safe_no_store_json(
    #[case] method: &str,
    #[case] path: &str,
) -> TestResult {
    let mut server = RunningServer::start().await?;
    let target = if path.contains('?') {
        format!("{path}{CANARY}")
    } else {
        path.into()
    };
    let request = format!("{method} {target} HTTP/1.1\r\nAuthorization: Bearer {CANARY}\r\n\r\n");
    let mut client = server.connect(request.as_bytes()).await?;
    assert_safe_response(&response(&mut client).await?, "404 Not Found", "READY")?;
    server.healthy().await?;
    server.finish().await
}
