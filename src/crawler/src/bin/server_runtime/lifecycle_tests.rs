use super::*;
use crate::operations::OperationsServer;
use crate::shutdown::ProcessShutdown;
use crawler::local_db::crawler_domain_configuration_repository::CrawlerDomainConfigurationRepositoryImpl;
use crawler::review::repository::CrawlerReviewRepository;
use crawler::review::server::{ReviewServer, ReviewServerConfig};
use crawler::service::crawler_domain_configuration::CrawlerDomainAdministrationHandler;
use rstest::rstest;
use std::io::{self, BufRead, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::{fd::OwnedFd, unix::net::UnixStream};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;

const SHA: &str = "ea91dace22616e9c7fcc3c2234c6758b7f7d594c";
const CANARY: &str = "private-lifecycle-error";
const GRACE: Duration = Duration::from_millis(700);

fn budgets() -> LifecycleConfig {
    // Private subprocess acceleration only. Production uses strict validated seconds.
    LifecycleConfig {
        shutdown_grace: GRACE,
        stop_timeout: Duration::from_secs(2),
        startup_timeout: Duration::from_secs(2),
    }
}

struct ChildProcess {
    child: Child,
    lines: mpsc::Receiver<String>,
    reader: Option<thread::JoinHandle<io::Result<()>>>,
}

impl ChildProcess {
    fn start(scenario: &str, stderr: Stdio) -> io::Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .env_clear()
            .env("CRAWLER_LIFECYCLE_TEST", scenario)
            .args([
                "--exact",
                "lifecycle::tests::lifecycle_child",
                "--ignored",
                "--nocapture",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing child stdout"))?;
        let (send, lines) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in io::BufReader::new(stdout).lines() {
                if send.send(line?).is_err() {
                    break;
                }
            }
            Ok(())
        });
        Ok(Self {
            child,
            lines,
            reader: Some(reader),
        })
    }

    fn line(&self, prefix: &str) -> io::Result<String> {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let line = self
                .lines
                .recv_timeout(end.saturating_duration_since(Instant::now()))
                .map_err(|_| io::Error::other(format!("missing child marker {prefix}")))?;
            if line.starts_with(prefix) {
                return Ok(line);
            }
        }
    }

    fn signal(&self, name: &str) -> io::Result<()> {
        let result = Command::new("/bin/kill")
            .args([name, &self.child.id().to_string()])
            .status()?;
        if !result.success() {
            return Err(io::Error::other("fixture signal failed"));
        }
        Ok(())
    }

    fn release(&mut self) -> io::Result<()> {
        self.child
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("missing child stdin"))?
            .write_all(b"release\n")
    }

    fn finish(&mut self) -> io::Result<ExitStatus> {
        let end = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = self.child.try_wait()? {
                if let Some(reader) = self.reader.take() {
                    reader
                        .join()
                        .map_err(|_| io::Error::other("fixture output task failed"))??;
                }
                return Ok(status);
            }
            if Instant::now() >= end {
                return Err(io::Error::other("lifecycle child exceeded process bound"));
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _killed = self.child.kill();
            let _reaped = self.child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _joined = reader.join();
        }
    }
}

fn probe(addr: SocketAddr, path: &str) -> io::Result<String> {
    let mut socket = TcpStream::connect_timeout(&addr, Duration::from_secs(1))?;
    socket.set_read_timeout(Some(Duration::from_secs(1)))?;
    socket.set_write_timeout(Some(Duration::from_secs(1)))?;
    write!(socket, "GET {path} HTTP/1.1\r\nHost: fixture\r\n\r\n")?;
    let mut response = String::new();
    socket.read_to_string(&mut response)?;
    Ok(response)
}

fn addresses(child: &ChildProcess) -> io::Result<(SocketAddr, SocketAddr)> {
    let line = child.line("BOUND ")?;
    let mut parts = line.split_whitespace().skip(1);
    let mut next = || {
        parts
            .next()
            .ok_or_else(|| io::Error::other("missing fixture address"))?
            .parse()
            .map_err(|_| io::Error::other("invalid fixture address"))
    };
    Ok((next()?, next()?))
}

#[rstest]
#[case("-INT")]
#[case("-TERM")]
fn should_join_active_capture_checkpoint_and_review_when_signalled(
    #[case] signal: &str,
) -> io::Result<()> {
    let mut child = ChildProcess::start("drain", Stdio::null())?;
    let (ops, review) = addresses(&child)?;
    child.line("ACTIVE")?;
    let ready = probe(ops, "/ready")?;
    assert!(ready.starts_with("HTTP/1.1 200"));
    assert!(ready.contains("Cache-Control: no-store"));
    let json: serde_json::Value = serde_json::from_str(
        ready
            .split("\r\n\r\n")
            .nth(1)
            .ok_or_else(|| io::Error::other("missing probe body"))?,
    )?;
    assert_eq!(
        json,
        serde_json::json!({ "commit_sha": SHA, "state": "READY" })
    );
    assert!(probe(ops, "/ops/version")?.contains(SHA));
    assert!(probe(ops, "/api/reviews")?.starts_with("HTTP/1.1 404"));
    assert!(probe(ops, "/health")?.starts_with("HTTP/1.1 200"));

    // A real accepted review connection remains owned while its partial headers finish.
    let mut request = TcpStream::connect(review)?;
    request.set_read_timeout(Some(Duration::from_secs(1)))?;
    request.write_all(b"GET /health HTTP/1.1\r\nHost: fixture\r\n")?;
    thread::sleep(Duration::from_millis(30));
    child.signal(signal)?;
    child.line("DRAINING")?;
    child.signal(signal)?;
    child.signal(if signal == "-INT" { "-TERM" } else { "-INT" })?;
    let response = probe(ops, "/ready")?;
    assert!(response.starts_with("HTTP/1.1 503"));
    assert!(response.contains("DRAINING"));
    assert!(child.child.try_wait()?.is_none());
    request.write_all(b"\r\n")?;
    let mut response = String::new();
    request.read_to_string(&mut response)?;
    assert!(response.starts_with("HTTP/1.1 200"));
    child.release()?;
    child.line("CHECKPOINT_JOINED")?;
    assert_eq!(child.finish()?.code(), Some(0));
    Ok(())
}

#[rstest]
#[case("failure")]
fn should_latch_failure_before_active_capture_finishes_then_exit_nonzero(
    #[case] scenario: &str,
) -> io::Result<()> {
    let mut child = ChildProcess::start(scenario, Stdio::null())?;
    let (ops, _) = addresses(&child)?;
    child.line("DRAINING")?;
    assert!(probe(ops, "/ready")?.starts_with("HTTP/1.1 503"));
    thread::sleep(Duration::from_millis(100));
    assert!(child.child.try_wait()?.is_none());
    child.release()?;
    child.line("CHECKPOINT_JOINED")?;
    assert_eq!(child.finish()?.code(), Some(1));
    Ok(())
}

#[rstest]
#[case("review_fd_failure")]
#[case("review_fd_destructor")]
fn should_notify_real_review_accept_failure_before_drain_or_destructor(
    #[case] scenario: &str,
) -> io::Result<()> {
    let mut child = ChildProcess::start(scenario, Stdio::null())?;
    let (ops, review) = addresses(&child)?;
    child.line("ACTIVE")?;
    assert!(probe(ops, "/ready")?.starts_with("HTTP/1.1 200"));
    let mut partials = Vec::new();
    for _ in 0..2 {
        let mut request = TcpStream::connect(review)?;
        request.set_read_timeout(Some(Duration::from_secs(1)))?;
        request.write_all(b"GET /health HTTP/1.1\r\nHost: fixture\r\n")?;
        partials.push(request);
    }
    // FIFO accept barrier: the two earlier connections are owned before FD exhaustion.
    assert!(probe(review, "/health")?.starts_with("HTTP/1.1 200"));
    thread::sleep(Duration::from_millis(30));
    child.release()?; // First fixture command requests child-only FD exhaustion.
    child.line("FD_EXHAUSTED")?;
    let at = Instant::now();
    let _trigger = TcpStream::connect(review)?;
    child.line("REVIEW_FAILURE_NOTIFIED")?;
    assert!(
        at.elapsed() < GRACE,
        "failure callback arrived only after review drain"
    );
    if scenario == "review_fd_failure" {
        let response = probe(ops, "/ready")?;
        assert!(response.starts_with("HTTP/1.1 503"));
        assert!(response.contains("DRAINING"));
        assert!(at.elapsed() < GRACE);
        assert!(child.child.try_wait()?.is_none());
        for mut request in partials {
            request.write_all(b"\r\n")?;
            let mut response = String::new();
            request.read_to_string(&mut response)?;
            assert!(response.starts_with("HTTP/1.1 200"));
        }
        child.release()?;
        child.line("CHECKPOINT_JOINED")?;
    }
    assert_eq!(child.finish()?.code(), Some(1));
    assert!(at.elapsed() < Duration::from_secs(3));
    Ok(())
}

#[rstest]
#[case("blocked_poll")]
#[case("blocked_stop_borrow")]
#[case("blocked_destructor")]
#[case("late_poll")]
#[case("deadline")]
fn should_force_failure_within_original_deadline(#[case] scenario: &str) -> io::Result<()> {
    let mut child = ChildProcess::start(scenario, Stdio::null())?;
    addresses(&child)?;
    child.line("ACTIVE")?;
    if scenario == "blocked_stop_borrow" {
        child.line("BORROW_HELD")?;
    }
    let at = Instant::now();
    child.signal("-TERM")?;
    if scenario == "late_poll" {
        child.release()?;
    }
    thread::sleep(Duration::from_millis(150));
    child.signal("-INT")?;
    assert_eq!(child.finish()?.code(), Some(1));
    assert!(at.elapsed() < Duration::from_secs(3));
    Ok(())
}

#[test]
fn should_start_failure_deadline_without_signals_or_a_yielding_workload() -> io::Result<()> {
    let mut child = ChildProcess::start("failure_blocked_poll", Stdio::null())?;
    child.line("ACTIVE")?;
    let at = Instant::now();
    assert_eq!(child.finish()?.code(), Some(1));
    assert!(at.elapsed() < Duration::from_secs(3));
    Ok(())
}

#[rstest]
#[case("early_cron_failure")]
#[case("early_review_failure")]
#[case("failed_future_destructor")]
fn should_keep_failure_watchdog_during_destructor_after_early_task_exit(
    #[case] scenario: &str,
) -> io::Result<()> {
    let mut child = ChildProcess::start(scenario, Stdio::null())?;
    child.line("FAILURE_RETURNED")?;
    assert_eq!(child.finish()?.code(), Some(1));
    Ok(())
}

#[rstest]
#[case("cleanup_destructor")]
#[case("runtime_destructor")]
fn should_keep_watchdog_through_actual_cleanup_and_runtime_destruction(
    #[case] scenario: &str,
) -> io::Result<()> {
    let mut child = ChildProcess::start(scenario, Stdio::null())?;
    addresses(&child)?;
    child.line("ACTIVE")?;
    child.signal("-INT")?;
    child.line("DRAINING")?;
    child.release()?;
    child.line("CHECKPOINT_JOINED")?;
    let at = Instant::now();
    assert_eq!(child.finish()?.code(), Some(1));
    assert!(at.elapsed() < Duration::from_secs(2));
    Ok(())
}

#[rstest]
#[case("panic_collector", false)]
#[case("panic_collector", true)]
#[case("panic_contended", false)]
#[case("panic_contended", true)]
fn should_redact_caught_panic_and_drain_collector_even_with_blocked_stderr(
    #[case] scenario: &str,
    #[case] blocked: bool,
) -> io::Result<()> {
    let (_reader, mut writer) = UnixStream::pair()?;
    if blocked {
        writer.set_nonblocking(true)?;
        loop {
            match writer.write(&[b'x'; 4096]) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        writer.set_nonblocking(false)?;
    }
    let stderr = if blocked {
        Stdio::from(OwnedFd::from(writer))
    } else {
        Stdio::piped()
    };
    let mut child = ChildProcess::start(scenario, stderr)?;
    child.line("DRAINING")?;
    assert!(
        child.child.try_wait()?.is_none(),
        "caught panic must allow accepted collector drain"
    );
    child.release()?;
    child.line("CHECKPOINT_JOINED")?;
    assert_eq!(child.finish()?.code(), Some(1));
    if let Some(mut stderr) = child.child.stderr.take() {
        let mut text = String::new();
        stderr.read_to_string(&mut text)?;
        assert!(text.is_empty(), "panic hook must produce no output");
    }
    Ok(())
}

#[rstest]
#[case("panic_destructor")]
#[case("panic_held_borrow")]
#[case("panic_reentrant")]
fn should_fence_panic_before_unwind_can_block(#[case] scenario: &str) -> io::Result<()> {
    let mut child = ChildProcess::start(scenario, Stdio::piped())?;
    child.line("ACTIVE")?;
    let at = Instant::now();
    assert_eq!(child.finish()?.code(), Some(1));
    assert!(at.elapsed() < Duration::from_secs(3));
    let mut text = String::new();
    child
        .child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("missing panic stderr"))?
        .read_to_string(&mut text)?;
    assert!(text.is_empty());
    Ok(())
}

#[test]
fn should_keep_watchdog_through_blocked_final_output() -> io::Result<()> {
    let (_reader, mut writer) = UnixStream::pair()?;
    writer.set_nonblocking(true)?;
    loop {
        match writer.write(&[b'x'; 4096]) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error),
        }
    }
    writer.set_nonblocking(false)?;
    let mut child = ChildProcess::start("output", Stdio::from(OwnedFd::from(writer)))?;
    addresses(&child)?;
    child.line("ACTIVE")?;
    child.signal("-TERM")?;
    child.line("DRAINING")?;
    child.release()?;
    child.line("CHECKPOINT_JOINED")?;
    assert_eq!(child.finish()?.code(), Some(1));
    Ok(())
}

#[rstest]
#[case("-INT")]
#[case("-TERM")]
fn should_skip_work_when_signal_arrives_before_runtime_start(
    #[case] signal: &str,
) -> io::Result<()> {
    let mut child = ChildProcess::start("before_start", Stdio::null())?;
    child.line("REGISTERED")?;
    child.signal(signal)?;
    child.line("SKIPPED")?;
    assert_eq!(child.finish()?.code(), Some(0));
    Ok(())
}

#[test]
fn should_bound_startup_even_when_workload_cannot_poll() -> io::Result<()> {
    let mut child = ChildProcess::start("startup_blocked", Stdio::null())?;
    child.line("REGISTERED")?;
    let at = Instant::now();
    assert_eq!(child.finish()?.code(), Some(1));
    assert!(at.elapsed() < Duration::from_secs(3));
    Ok(())
}

#[tokio::test]
async fn should_retain_all_child_errors_and_reject_late_success() -> io::Result<()> {
    let lifecycle = Lifecycle::new(budgets());
    let (ops_stop, _) = watch::channel(false);
    let failure = || async { Err(RedactedDaemonCause::new(io::Error::other(CANARY))) };
    let result = run_owned(&lifecycle, failure(), failure(), failure(), ops_stop).await;
    let Err(error) = result else {
        return Err(io::Error::other("child failures were discarded"));
    };
    assert!(error.cron.is_err() && error.review.is_err() && error.operations.is_err());
    assert!(!format!("{error} {error:?} {error:#?}").contains(CANARY));
    lifecycle.begin_cleanup();
    lifecycle.begin_teardown();
    assert_eq!(lifecycle.exit_code(true), 1);
    assert_eq!(lifecycle.state(), CrawlerState::Draining);
    Ok(())
}

#[test]
fn should_reject_synchronous_late_startup_and_drain_completion() {
    let mut config = budgets();
    config.startup_timeout = Duration::from_millis(10);
    let startup = Lifecycle::new(config);
    thread::sleep(Duration::from_millis(20));
    assert!(!startup.ready());
    assert!(startup.failed());
    config.startup_timeout = Duration::from_secs(2);
    config.shutdown_grace = Duration::from_millis(10);
    let drain = Lifecycle::new(config);
    assert!(drain.ready());
    drain.stop(false);
    thread::sleep(Duration::from_millis(20));
    drain.begin_cleanup();
    drain.begin_teardown();
    assert_eq!(drain.exit_code(true), 1);
    assert_eq!(drain.state(), CrawlerState::Draining);
}

#[test]
fn should_never_reset_deadlines_or_publish_ready_after_stop() {
    let lifecycle = Lifecycle::new(budgets());
    assert_eq!(lifecycle.state().as_str(), "STARTING");
    lifecycle.stop(false);
    let deadline = lifecycle.drain_deadline();
    lifecycle.stop(false);
    lifecycle.configure(LifecycleConfig::default());
    assert_eq!(lifecycle.drain_deadline(), deadline);
    assert!(!lifecycle.ready());
    lifecycle.begin_cleanup();
    lifecycle.begin_teardown();
    assert_eq!(lifecycle.exit_code(true), 0);
    assert_eq!(lifecycle.lock().state.as_str(), "STOPPED");
}

#[test]
fn should_publish_sticky_failure_without_waiting_for_contended_timeline() {
    let lifecycle = Lifecycle::new(budgets());
    assert!(lifecycle.ready());
    let held = lifecycle.lock();
    lifecycle.fail_early();
    let deadline = lifecycle.0.failure_deadline.load(Ordering::Acquire);
    assert_ne!(deadline, NO_FAILURE);
    lifecycle.fail_early();
    assert_eq!(
        lifecycle.0.failure_deadline.load(Ordering::Acquire),
        deadline
    );
    drop(held);
    assert_eq!(lifecycle.state(), CrawlerState::Draining);
    assert!(lifecycle.failed());
    lifecycle.configure(LifecycleConfig::default());
    lifecycle.fail_early();
    assert_eq!(
        lifecycle.0.failure_deadline.load(Ordering::Acquire),
        deadline
    );
    assert!(lifecycle.0.process_deadline.load(Ordering::Acquire) <= deadline);
    assert!(!lifecycle.ready());
}

fn block_with_stop_borrow(lifecycle: &Lifecycle) {
    let stop = lifecycle.stop_receiver();
    let _held_borrow = stop.borrow();
    println!("BORROW_HELD");
    thread::sleep(Duration::from_secs(60));
}

fn panic_with_blocked_unwind(lifecycle: &Lifecycle, held_borrow: bool) {
    let stop = lifecycle.stop_receiver();
    let _borrow = held_borrow.then(|| stop.borrow());
    let _blocked = BlockOnDrop;
    std::panic::panic_any(CANARY);
}

struct FailedFutureWithBlockedDrop;
impl Future for FailedFutureWithBlockedDrop {
    type Output = Result<(), RedactedDaemonCause>;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        println!("FAILURE_RETURNED");
        std::task::Poll::Ready(Err(RedactedDaemonCause::new(io::Error::other(CANARY))))
    }
}
impl Drop for FailedFutureWithBlockedDrop {
    fn drop(&mut self) {
        thread::sleep(Duration::from_secs(60));
    }
}

struct BlockingPendingShutdown;
impl Future for BlockingPendingShutdown {
    type Output = ();
    fn poll(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        std::task::Poll::Pending
    }
}
impl Drop for BlockingPendingShutdown {
    fn drop(&mut self) {
        thread::sleep(Duration::from_secs(60));
    }
}

fn exhaust_child_descriptors() -> io::Result<Vec<std::fs::File>> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: initialized writable rlimit, valid resource constant; this helper runs ONLY
    // in the owned subprocess. No parent/host resource limit is changed.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    limit.rlim_cur = limit.rlim_cur.min(64);
    // SAFETY: valid initialized rlimit, soft limit only lowered, hard limit preserved.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut files = Vec::new();
    loop {
        match std::fs::File::open("/dev/null") {
            Ok(file) => files.push(file),
            Err(error) if error.raw_os_error() == Some(libc::EMFILE) => return Ok(files),
            Err(error) => return Err(error),
        }
    }
}

struct BlockOnDrop;
impl Drop for BlockOnDrop {
    fn drop(&mut self) {
        thread::sleep(Duration::from_secs(60));
    }
}

#[test]
#[ignore = "subprocess helper; parent owns signals, loopback probes and fixture capture release"]
fn lifecycle_child() -> io::Result<()> {
    let scenario = std::env::var("CRAWLER_LIFECYCLE_TEST").map_err(io::Error::other)?;
    let mut config = budgets();
    if scenario == "startup_blocked" {
        config.startup_timeout = Duration::from_millis(100);
    }
    let shutdown = ProcessShutdown::install(config)?;
    let life = &shutdown.lifecycle;
    println!("REGISTERED");
    if scenario == "startup_blocked" {
        thread::sleep(Duration::from_secs(60));
    }
    if scenario == "before_start" {
        while !life.stopping() {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!life.ready());
        life.begin_cleanup();
        life.begin_teardown();
        println!("SKIPPED");
        shutdown.exit(true);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let successful = runtime.block_on(async {
        if scenario == "failed_future_destructor" {
            let (ops_stop, _) = watch::channel(false);
            let result = run_owned(
                life,
                FailedFutureWithBlockedDrop,
                std::future::pending(),
                std::future::pending(),
                ops_stop,
            )
            .await;
            return Err(io::Error::other(format!(
                "blocked future unexpectedly returned: {result:?}"
            )));
        }
        if matches!(
            scenario.as_str(),
            "early_cron_failure" | "early_review_failure"
        ) {
            let work = |fail: bool| {
                let life = life.clone();
                async move {
                    if fail {
                        Err(RedactedDaemonCause::new(io::Error::other(CANARY)))
                    } else {
                        life.wait().await;
                        Ok(())
                    }
                }
            };
            let (ops_stop, _) = watch::channel(false);
            let result = run_owned(
                life,
                work(scenario == "early_cron_failure"),
                work(scenario == "early_review_failure"),
                work(false),
                ops_stop,
            )
            .await;
            assert!(result.is_err());
            life.begin_cleanup();
            println!("FAILURE_RETURNED");
            drop(BlockOnDrop);
        }
        let database_spy = TcpListener::bind("127.0.0.1:0")?;
        database_spy.set_nonblocking(true)?;
        let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy_with(
            sqlx::postgres::PgConnectOptions::new()
                .host("127.0.0.1")
                .port(database_spy.local_addr()?.port())
                .username("fixture")
                .password("fixture")
                .database("fixture")
                .ssl_mode(sqlx::postgres::PgSslMode::Disable),
        );
        let review = ReviewServer::new(
            CrawlerReviewRepository::new(pool.clone()),
            Arc::new(CrawlerDomainAdministrationHandler::new(Arc::new(
                CrawlerDomainConfigurationRepositoryImpl::new(pool.clone()),
            ))),
            ReviewServerConfig {
                bind_addr: "127.0.0.1:0".parse().map_err(io::Error::other)?,
                auth_token: Some("fixture".into()),
            },
        )
        .bind()
        .await?;
        let reservation = TcpListener::bind("127.0.0.1:0")?;
        let ops_addr = reservation.local_addr()?;
        drop(reservation);
        let ops = OperationsServer::bind(ops_addr, life.clone(), SHA.into()).await?;
        println!("BOUND {ops_addr} {}", review.local_addr()?);
        let fd_failure = matches!(
            scenario.as_str(),
            "review_fd_failure" | "review_fd_destructor"
        );
        let (exhaust, exhaust_requested) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        thread::spawn(move || {
            let mut line = String::new();
            if fd_failure {
                let result = io::stdin().read_line(&mut line);
                let _closed = exhaust.send(result);
                line.clear();
            }
            let result = io::stdin().read_line(&mut line);
            let _closed = release.send(result);
        });
        let spares = Arc::new(Mutex::new(Vec::new()));
        let exhaustor = if fd_failure {
            let spares = spares.clone();
            Some(tokio::spawn(async move {
                exhaust_requested.await.map_err(io::Error::other)??;
                let files = exhaust_child_descriptors()?;
                *spares
                    .lock()
                    .map_err(|_| io::Error::other("fixture descriptor lock poisoned"))? = files;
                println!("FD_EXHAUSTED");
                Ok::<_, io::Error>(())
            }))
        } else {
            None
        };
        let (capture, mut captures) = tokio::sync::mpsc::channel(1);
        let collector = tokio::spawn(async move {
            assert!(captures.recv().await.is_some());
            released.await.map_err(io::Error::other)??;
            assert!(captures.recv().await.is_none());
            // Synthetic issued capture + local metadata/checkpoint; no domain/database calls.
            println!("CHECKPOINT_JOINED");
            Ok::<_, io::Error>(())
        });
        let scheduler_life = life.clone();
        let scheduler_scenario = scenario.clone();
        let cron = async move {
            capture.send(()).await.map_err(RedactedDaemonCause::new)?;
            println!("ACTIVE");
            if scheduler_scenario == "panic_collector" {
                let caught = std::panic::catch_unwind(|| std::panic::panic_any(CANARY));
                assert!(caught.is_err());
            }
            if matches!(
                scheduler_scenario.as_str(),
                "panic_destructor" | "panic_held_borrow"
            ) {
                panic_with_blocked_unwind(
                    &scheduler_life,
                    scheduler_scenario == "panic_held_borrow",
                );
            }
            if scheduler_scenario == "panic_contended" {
                let (held, acquired) = mpsc::sync_channel(1);
                let (release, released) = mpsc::sync_channel(1);
                thread::scope(|scope| {
                    let contended_life = &scheduler_life;
                    scope.spawn(move || {
                        let _held = contended_life.lock();
                        held.send(()).unwrap();
                        released.recv().unwrap();
                    });
                    acquired.recv().unwrap();
                    let caught = std::panic::catch_unwind(|| std::panic::panic_any(CANARY));
                    // The hook must return while another thread still owns the deadline mutex.
                    assert!(caught.is_err());
                    release.send(()).unwrap();
                });
            }
            if scheduler_scenario == "panic_reentrant" {
                let _held = scheduler_life.lock();
                let _blocked = BlockOnDrop;
                std::panic::panic_any(CANARY);
            }
            if scheduler_scenario == "blocked_stop_borrow" {
                block_with_stop_borrow(&scheduler_life);
            }
            if matches!(
                scheduler_scenario.as_str(),
                "failure" | "failure_blocked_poll"
            ) {
                scheduler_life.failure_sender().send_replace(true);
            }
            if scheduler_scenario == "failure_blocked_poll" {
                thread::sleep(Duration::from_secs(60));
            }
            scheduler_life.wait().await;
            println!("DRAINING");
            if scheduler_scenario == "blocked_poll" {
                thread::sleep(Duration::from_secs(60));
            }
            if scheduler_scenario == "blocked_destructor" {
                drop(BlockOnDrop);
            }
            if scheduler_scenario == "late_poll" {
                thread::sleep(GRACE + Duration::from_millis(100));
            }
            drop(capture);
            collector
                .await
                .map_err(RedactedDaemonCause::new)?
                .map_err(RedactedDaemonCause::new)?;
            if matches!(
                scheduler_scenario.as_str(),
                "failure" | "panic_collector" | "panic_contended"
            ) {
                Err(RedactedDaemonCause::new(io::Error::other(CANARY)))
            } else {
                Ok(())
            }
        };
        let review_life = life.clone();
        let blocking_review_drop = scenario == "review_fd_destructor";
        let (ops_stop, ops_shutdown) = watch::channel(false);
        let result = run_owned(
            life,
            cron,
            async move {
                let on_failure = || {
                    review_life.fail_early();
                    if fd_failure {
                        // Release fixture spares only AFTER the real review failure, so operations
                        // can accept a fresh probe while accepted review requests still drain.
                        spares
                            .lock()
                            .unwrap_or_else(|_| crate::shutdown::fatal())
                            .clear();
                        println!("REVIEW_FAILURE_NOTIFIED");
                    }
                };
                let result = if blocking_review_drop {
                    review
                        .run_until_with_failure(BlockingPendingShutdown, GRACE, on_failure)
                        .await
                } else {
                    review
                        .run_until_with_failure(review_life.wait(), GRACE, on_failure)
                        .await
                };
                result.map_err(RedactedDaemonCause::new)
            },
            async move {
                ops.run_until(ops_shutdown)
                    .await
                    .map_err(RedactedDaemonCause::new)
            },
            ops_stop,
        )
        .await;
        life.begin_cleanup();
        if let Some(exhaustor) = exhaustor {
            exhaustor.await.map_err(io::Error::other)??;
        }
        pool.close().await;
        assert!(
            matches!(database_spy.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
        if scenario == "cleanup_destructor" {
            drop(BlockOnDrop);
        }
        if scenario == "runtime_destructor" {
            let (entered, started) = tokio::sync::oneshot::channel();
            tokio::task::spawn_blocking(move || {
                let _closed = entered.send(());
                thread::sleep(Duration::from_secs(60));
            });
            started.await.map_err(io::Error::other)?;
        }
        Ok::<_, io::Error>(result.is_ok())
    })?;
    life.begin_teardown();
    drop(runtime);
    if scenario == "output" {
        io::stderr().write_all(&[b'x'; 64 * 1024])?;
    }
    shutdown.exit(successful)
}
