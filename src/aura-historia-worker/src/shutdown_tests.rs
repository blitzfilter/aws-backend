use super::*;
use std::{
    io::Write,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Instant,
};
use tokio::sync::oneshot;

const STALL_MODE: &str = "AURA_WORKER_TEST_STALLED_SHUTDOWN";

#[rstest::rstest]
#[case::consumer_drop("consumer_drop")]
#[case::consumer_poll("consumer_poll")]
#[case::runtime_drop("runtime_drop")]
#[case::runtime_blocking("runtime_blocking")]
#[case::shared_cleanup("shared_cleanup")]
#[case::http_drop_after_consumer("http_drop_after_consumer")]
#[case::http_poll_after_consumer("http_poll_after_consumer")]
#[case::http_poll_immediate_after_consumer("http_poll_immediate_after_consumer")]
#[case::http_drop_after_signal("http_drop_after_signal")]
#[case::http_poll_after_signal("http_poll_after_signal")]
fn should_exit_nonzero_within_bound_when_cancellation_or_runtime_stalls(#[case] mode: &str) {
    let limit = if mode.starts_with("http_") {
        Duration::from_secs(28)
    } else if mode.starts_with("consumer_") {
        Duration::from_secs(8)
    } else if mode == "shared_cleanup" {
        Duration::from_secs(6)
    } else {
        Duration::from_secs(4)
    };
    assert_shutdown_child(mode, limit, 1, &format!("stall_entered:{mode}"));
}

fn assert_shutdown_child(mode: &str, limit: Duration, code: i32, marker: &str) -> Duration {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "shutdown_tests::stalled_shutdown_subprocess",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env(STALL_MODE, mode)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if started.elapsed() >= limit {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "{mode}: child exceeded {limit:?}; stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(Some(code), output.status.code(), "{mode}: {output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(marker),
        "must reach the controlled boundary, not fail startup: {output:?}"
    );
    assert!(started.elapsed() < limit);
    started.elapsed()
}

#[test]
fn should_preserve_full_twenty_second_http_drain_after_consumer_finishes() {
    let elapsed = assert_shutdown_child(
        "http_delay",
        Duration::from_secs(24),
        0,
        "http_drain_completed",
    );
    assert!(elapsed >= aura_historia_worker::WORKER_HTTP_DRAIN_TIMEOUT);
}

#[rstest::rstest]
#[case::duration("duration_overflow")]
#[case::instant("instant_overflow")]
fn should_exit_nonzero_without_panicking_when_watchdog_arithmetic_overflows(#[case] mode: &str) {
    assert_shutdown_child(mode, Duration::from_secs(3), 1, "arithmetic_checked");
}

fn stall(mode: &str) {
    println!("stall_entered:{mode}");
    std::io::stdout().flush().unwrap();
    // Keep the sender alive: this is a synchronous stall, not an async pending/yield loop.
    // Parent also kills on its own wall-clock bound if production deadline handling regresses.
    let (_held, blocked) = mpsc::channel::<()>();
    let _result = blocked.recv_timeout(Duration::from_secs(30));
    std::process::exit(94);
}

struct StalledDrop(String);
impl Drop for StalledDrop {
    fn drop(&mut self) {
        stall(&self.0);
    }
}

async fn http_shutdown_case(mode: &str) -> Result<(), MainError> {
    let signalled = mode.ends_with("signal") || mode == "http_delay";
    let (finish_consumer, consumer_finished) = oneshot::channel();
    let consumer = tokio::spawn(async move { consumer_finished.await.unwrap() });
    let finish_consumer = if signalled {
        Some(finish_consumer)
    } else {
        finish_consumer.send(()).unwrap();
        None
    };
    let (stop_http, stopped) = oneshot::channel();
    let server = async move {
        stopped.await.unwrap();
        if let Some(finish_consumer) = finish_consumer {
            finish_consumer.send(()).unwrap();
        }
        if mode == "http_delay" {
            tokio::time::sleep(aura_historia_worker::WORKER_HTTP_DRAIN_TIMEOUT).await;
            return Ok(());
        }
        // Let the consumer and its cleanup complete first; HTTP must retain the outer watchdog.
        if !mode.contains("immediate") {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if mode.contains("poll") {
            stall(mode);
        }
        let mut connections = tokio::task::JoinSet::new();
        let (started, wait_started) = oneshot::channel();
        let guard = StalledDrop(mode.to_owned());
        connections.spawn(async move {
            let _guard = guard;
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        wait_started.await.unwrap();
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        Ok::<(), WorkerRunError>(())
    };
    supervise_runtime(
        aura_historia_worker::WorkerRuntime::empty(),
        consumer,
        server,
        async {
            if !signalled {
                std::future::pending::<()>().await;
            }
        },
        stop_http,
        Duration::from_secs(1),
    )
    .await
}

#[test]
fn stalled_shutdown_subprocess() {
    let Ok(mode) = std::env::var(STALL_MODE) else {
        return;
    };
    // One blocked worker also prevents Tokio timer progress. The OS deadline must still fire.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        if mode.starts_with("http_") {
            let result = http_shutdown_case(&mode).await;
            assert_eq!("http_delay", mode, "stalled HTTP must not complete");
            assert!(result.is_ok(), "normal HTTP drain must finish: {result:?}");
            return;
        }
        let (started, wait_started) = oneshot::channel();
        match mode.as_str() {
            "duration_overflow" | "instant_overflow" => {
                println!("arithmetic_checked");
                std::io::stdout().flush().unwrap();
                let _deadline = if mode == "duration_overflow" {
                    shutdown::FatalDeadline::after_drain(Duration::MAX)
                } else {
                    shutdown::FatalDeadline::after(Duration::MAX)
                };
                panic!("unrepresentable watchdog budget must fail closed");
            }
            "consumer_drop" | "consumer_poll" => {
                let (block_poll, poll_blocked) = oneshot::channel();
                let drop_stalls = mode == "consumer_drop";
                let task = tokio::spawn(async move {
                    let _guard = drop_stalls.then(|| StalledDrop("consumer_drop".into()));
                    started.send(()).unwrap();
                    if !drop_stalls {
                        poll_blocked.await.unwrap();
                        stall("consumer_poll");
                    }
                    std::future::pending::<()>().await;
                });
                wait_started.await.unwrap();
                let (stop_http, stopped) = oneshot::channel();
                let server = async move {
                    stopped.await.unwrap();
                    let _closed = block_poll.send(());
                    Ok::<(), WorkerRunError>(())
                };
                let result = supervise_runtime(
                    aura_historia_worker::WorkerRuntime::empty(),
                    task,
                    server,
                    async {},
                    stop_http,
                    Duration::from_millis(50),
                )
                .await;
                panic!("stalled cancellation must exit, not return: {result:?}");
            }
            "shared_cleanup" => {
                shutdown::join_cancelled(
                    async { tokio::time::sleep(Duration::from_secs(3)).await },
                    async { stall("shared_cleanup") },
                )
                .await;
                panic!("child cleanup must not receive a fresh five seconds after parent join");
            }
            "runtime_drop" => {
                tokio::spawn(async move {
                    let _guard = StalledDrop("runtime_drop".into());
                    started.send(()).unwrap();
                    std::future::pending::<()>().await;
                });
                wait_started.await.unwrap();
            }
            "runtime_blocking" => {
                tokio::task::spawn_blocking(move || {
                    started.send(()).unwrap();
                    stall("runtime_blocking");
                });
                wait_started.await.unwrap();
            }
            _ => panic!("unknown test mode"),
        }
    });
    shutdown::teardown(runtime);
    if mode == "http_delay" {
        println!("http_drain_completed");
        return;
    }
    panic!("stalled runtime teardown must exit, not return");
}

#[test]
fn should_confirm_runtime_destruction_before_clean_teardown_returns() {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = Dropped(dropped.clone());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (started, wait_started) = oneshot::channel();
        tokio::spawn(async move {
            let _guard = guard;
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        wait_started.await.unwrap();
    });
    shutdown::teardown(runtime);
    assert!(dropped.load(Ordering::SeqCst));
}
