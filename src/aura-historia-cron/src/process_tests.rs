use super::*;
use crate::{
    CronRuntimeError, jobs::SearchFilterPeriodicMatchJob, scheduled_job::CronJobExecutionOutcome,
};
use search_filter_service::use_cases::{
    RunPeriodicSearchFilterMatchingCommand, RunPeriodicSearchFilterMatchingError,
    RunPeriodicSearchFilterMatchingOutcome, RunPeriodicSearchFilterMatchingUseCase,
};
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const CHILD: &str = "process::tests::signal_child";

#[test]
fn should_parse_only_explicit_supported_cli_modes() {
    for (args, expected) in [
        (vec![], RunMode::Daemon),
        (vec!["--check-config"], RunMode::CheckConfig),
        (vec!["--run-once", JOB], RunMode::Once),
    ] {
        assert_eq!(
            RunMode::parse(args.into_iter().map(OsString::from)).unwrap(),
            expected
        );
    }
    for args in [
        vec!["--run-once"],
        vec!["--run-once", "unknown"],
        vec!["--check-config", "--run-once", JOB],
        vec!["--help"],
        vec!["unexpected"],
    ] {
        assert!(RunMode::parse(args.into_iter().map(OsString::from)).is_err());
    }
}

#[test]
fn should_preserve_run_once_terminal_outcomes() {
    assert!(crate::outcome_result(CronJobExecutionOutcome::Succeeded).is_ok());
    assert!(crate::outcome_result(CronJobExecutionOutcome::SkippedLocalOverlap).is_ok());
    assert!(matches!(
        crate::outcome_result(CronJobExecutionOutcome::Panicked(CronErrorCause::new(
            std::io::Error::other("private panic")
        ))),
        Err(CronRuntimeError::JobPanicked(_))
    ));
    assert!(matches!(
        crate::outcome_result(CronJobExecutionOutcome::TimedOut(None)),
        Err(CronRuntimeError::JobTimedOut(_))
    ));
    assert!(matches!(
        crate::outcome_result(CronJobExecutionOutcome::Cancelled),
        Err(CronRuntimeError::JobCancelled)
    ));
    assert!(matches!(
        crate::outcome_result(CronJobExecutionOutcome::SkippedShutdown),
        Err(CronRuntimeError::JobCancelled)
    ));
    assert!(matches!(
        crate::outcome_result(CronJobExecutionOutcome::Failed(
            crate::scheduled_job::CronJobExecutionError::from_source(
                RunPeriodicSearchFilterMatchingError::FxSnapshotNotFound
            )
        )),
        Err(CronRuntimeError::Job(_))
    ));
}

struct Process {
    child: Child,
    lines: mpsc::Receiver<String>,
    reader: Option<thread::JoinHandle<()>>,
    observed: Vec<String>,
}
impl Process {
    fn start(mode: &str, behavior: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_with_env(mode, behavior, &[])
    }

    fn start_with_env(
        mode: &str,
        behavior: &str,
        overrides: &[(&str, &str)],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut child = Command::new(std::env::current_exe()?)
            .args(["--exact", CHILD, "--ignored", "--nocapture"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("CRON_TEST_MODE", mode)
            .env("CRON_TEST_BEHAVIOR", behavior)
            .envs(overrides.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().ok_or("missing child stdout")?;
        let (send, lines) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if send.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            child,
            lines,
            reader: Some(reader),
            observed: vec![],
        })
    }
    fn line(&mut self, prefix: &str) -> Result<String, Box<dyn std::error::Error>> {
        if let Some(value) = self
            .observed
            .iter()
            .find_map(|line| line.strip_prefix(prefix))
        {
            return Ok(value.to_owned());
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let line = self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))?;
            self.observed.push(line.clone());
            if let Some(value) = line.strip_prefix(prefix) {
                return Ok(value.to_owned());
            }
        }
    }
    fn signal(&self, signal: &str) -> TestResult {
        let result = Command::new("kill")
            .args([signal, &self.child.id().to_string()])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .status()?;
        if !result.success() {
            return Err("signal failed".into());
        }
        Ok(())
    }
    async fn finish(&mut self) -> Result<std::process::ExitStatus, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(9);
        loop {
            if let Some(status) = self.child.try_wait()? {
                if let Some(reader) = self.reader.take() {
                    reader.join().map_err(|_| "output reader panicked")?;
                }
                self.observed.extend(self.lines.try_iter());
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err("cron subprocess exceeded shutdown ceiling".into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
impl Drop for Process {
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

async fn scenario(mode: &str, behavior: &str, signal: &str) -> TestResult {
    let mut child = Process::start(mode, behavior)?;
    let address = child.line("ADDRESS ")?;
    assert!(address.parse::<std::net::SocketAddr>()?.ip().is_loopback());
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()?;
    if behavior != "idle" && behavior != "startup" {
        child.line("JOB_STARTED")?;
    }
    let stuck = matches!(behavior, "poll" | "drop");
    let expected_state = if behavior == "startup" {
        "starting"
    } else {
        "ready"
    };
    if !stuck {
        let response = client
            .get(format!("http://{address}/ops/status"))
            .send()
            .await?;
        assert_eq!(response.headers()["cache-control"], "no-store");
        let status = response.text().await?;
        assert!(
            status.contains(&format!("\"state\":\"{expected_state}\"")),
            "{status}"
        );
        assert!(status.contains("\"schema_version\":1"));
        assert!(status.contains("\"source_sha\":null"));
        assert!(status.contains("\"drain_seconds\":1"));
        assert!(status.contains("\"stop_seconds\":31"));
        assert!(status.contains("\"cleanup_seconds\":5"));
        assert!(status.contains("\"runtime_teardown_seconds\":1"));
        if behavior != "startup" {
            assert!(status.contains("\"execution_seconds\":7200"));
        }
        assert!(!status.contains("password"));
        let version = client
            .get(format!("http://{address}/ops/version"))
            .send()
            .await?;
        assert_eq!(version.status(), reqwest::StatusCode::OK);
        assert_eq!(version.headers()["cache-control"], "no-store");
        let version = version.text().await?;
        assert!(version.contains("\"schema_version\":1"));
        assert!(version.contains("\"component\":\"aura-historia-cron\""));
        assert!(version.contains("\"stage\":\"test\""));
        assert!(version.contains("\"source_sha\":null"));
    }
    let start = Instant::now();
    child.signal(signal)?;
    if !matches!(behavior, "idle" | "startup") && !stuck {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let response = client
                .get(format!("http://{address}/ops/status"))
                .send()
                .await?;
            let status = response.text().await?;
            if status.contains("\"state\":\"draining\"") {
                assert!(status.contains("\"accepting\":false"));
                assert!(status.contains("\"active_executions\":1"));
                break;
            }
            if Instant::now() >= deadline {
                return Err("draining was not observable".into());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let ready = client.get(format!("http://{address}/ready")).send().await?;
        assert_eq!(ready.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(ready.headers()["cache-control"], "no-store");
        child.signal(if signal == "-TERM" { "-INT" } else { "-TERM" })?;
    }
    let exit = child.finish().await?;
    assert_eq!(exit.success(), matches!(behavior, "complete" | "idle"));
    assert!(start.elapsed() < Duration::from_secs(8));
    let count = |marker: &str| {
        child
            .observed
            .iter()
            .filter(|line| line.as_str() == marker)
            .count()
    };
    assert_eq!(
        count("JOB_STARTED"),
        usize::from(!matches!(behavior, "idle" | "startup")),
        "new execution admitted during drain"
    );
    assert_eq!(
        count("JOB_FINISHED"),
        usize::from(matches!(behavior, "complete" | "fail")),
        "false finish"
    );
    if matches!(behavior, "complete" | "cancel" | "fail") {
        assert_eq!(count("JOB_DROPPED"), 1, "job not destroyed before exit");
    }
    if matches!(behavior, "poll" | "drop") {
        assert_eq!(exit.code(), Some(1));
        assert_eq!(
            count("RUNTIME_RETURNED"),
            0,
            "claimed stopped with unknown work"
        );
    }
    Ok(())
}

#[tokio::test]
async fn should_reject_nonloopback_listeners_before_startup_in_both_process_modes() -> TestResult {
    for mode in ["daemon", "once"] {
        for address in [
            "0.0.0.0:8082",
            "[::]:8082",
            "192.0.2.1:8082",
            "[2001:db8::1]:8082",
        ] {
            let mut child = Process::start_with_env(mode, "idle", &[("CRON_TEST_BIND", address)])?;
            assert_eq!(child.finish().await?.code(), Some(1));
            assert!(
                !child
                    .observed
                    .iter()
                    .any(|line| line.starts_with("ADDRESS ") || line == "STARTUP_STARTED")
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn should_reject_unsafe_shutdown_budgets_before_process_startup() -> TestResult {
    for (drain, stop) in [
        ("300", "329"),
        ("330", "330"),
        ("3571", "3600"),
        ("300", "3601"),
        ("3601", "3600"),
    ] {
        let mut child = Process::start_with_env(
            "once",
            "idle",
            &[("CRON_TEST_DRAIN", drain), ("CRON_TEST_STOP", stop)],
        )?;
        assert_eq!(child.finish().await?.code(), Some(1));
        assert!(!child.observed.iter().any(|line| line == "STARTUP_STARTED"));
    }
    Ok(())
}

#[tokio::test]
async fn should_fail_run_once_on_occupied_private_listener_before_startup() -> TestResult {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?.to_string();
    let mut child = Process::start_with_env("once", "idle", &[("CRON_TEST_BIND", &address)])?;
    assert_eq!(child.finish().await?.code(), Some(1));
    assert!(
        !child
            .observed
            .iter()
            .any(|line| line == "STARTUP_STARTED" || line == "JOB_STARTED")
    );
    Ok(())
}

#[tokio::test]
async fn should_drain_active_completion_on_real_signals_for_daemon_and_run_once() -> TestResult {
    for mode in ["daemon", "once"] {
        for signal in ["-TERM", "-INT"] {
            scenario(mode, "complete", signal).await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn should_cancel_and_join_without_false_finish_or_new_execution_on_real_signals() -> TestResult
{
    for mode in ["daemon", "once"] {
        for signal in ["-TERM", "-INT"] {
            scenario(mode, "cancel", signal).await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn should_preserve_failed_outcome_after_signal_in_both_modes() -> TestResult {
    for mode in ["once", "daemon"] {
        for signal in ["-TERM", "-INT"] {
            scenario(mode, "fail", signal).await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn should_stop_idle_daemon_without_selecting_a_job() -> TestResult {
    scenario("daemon", "idle", "-INT").await
}
#[tokio::test]
async fn should_cancel_startup_before_ready_or_execution() -> TestResult {
    scenario("daemon", "startup", "-TERM").await
}
#[tokio::test]
async fn should_bound_stuck_poll_and_drop_without_claiming_stopped() -> TestResult {
    for mode in ["daemon", "once"] {
        for behavior in ["poll", "drop"] {
            scenario(mode, behavior, "-TERM").await?;
        }
    }
    Ok(())
}

struct Inbound {
    behavior: String,
}
struct ExecutionDrop {
    stuck: bool,
}
impl Drop for ExecutionDrop {
    fn drop(&mut self) {
        if self.stuck {
            thread::sleep(Duration::from_secs(60));
        }
        println!("JOB_DROPPED");
    }
}
#[async_trait::async_trait]
impl RunPeriodicSearchFilterMatchingUseCase for Inbound {
    async fn execute(
        &self,
        _: RunPeriodicSearchFilterMatchingCommand,
    ) -> Result<RunPeriodicSearchFilterMatchingOutcome, RunPeriodicSearchFilterMatchingError> {
        let _drop = ExecutionDrop {
            stuck: self.behavior == "drop",
        };
        println!("JOB_STARTED");
        match self.behavior.as_str() {
            "complete" | "fail" => {
                tokio::time::sleep(Duration::from_millis(750)).await;
                println!("JOB_FINISHED");
                if self.behavior == "fail" {
                    return Err(RunPeriodicSearchFilterMatchingError::FxSnapshotNotFound);
                }
                Ok(RunPeriodicSearchFilterMatchingOutcome::SkippedAlreadyRunning)
            }
            "poll" => {
                thread::sleep(Duration::from_secs(60));
                std::future::pending().await
            }
            _ => std::future::pending().await,
        }
    }
}

#[test]
#[ignore = "subprocess entry; parent owns signals and teardown"]
fn signal_child() -> TestResult {
    let mode = std::env::var("CRON_TEST_MODE")?;
    let behavior = std::env::var("CRON_TEST_BEHAVIOR")?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        let signals = ShutdownSignals::register()?;
        let socket = std::net::TcpListener::bind("127.0.0.1:0")?;
        let address = socket.local_addr()?;
        drop(socket);
        let config = CronRuntimeConfig::from_getter(
            |name| match name {
                "STAGE" => Some("test".into()),
                crate::CRON_HEALTH_BIND_ADDR_ENV => {
                    Some(std::env::var("CRON_TEST_BIND").unwrap_or_else(|_| address.to_string()))
                }
                crate::CRON_SHUTDOWN_GRACE_SECONDS_ENV => {
                    Some(std::env::var("CRON_TEST_DRAIN").unwrap_or_else(|_| "1".into()))
                }
                crate::CRON_STOP_TIMEOUT_SECONDS_ENV => {
                    Some(std::env::var("CRON_TEST_STOP").unwrap_or_else(|_| "31".into()))
                }
                _ => None,
            },
            &[JOB],
        )?;
        let (stop, mut stopped) = watch::channel(false);
        let idle = behavior == "idle";
        let startup_blocked = behavior == "startup";
        let job = Arc::new(SearchFilterPeriodicMatchJob::new(Arc::new(Inbound {
            behavior,
        })));
        let work = crate::run_with_startup(
            config,
            async move {
                println!("STARTUP_STARTED");
                if startup_blocked {
                    std::future::pending::<()>().await;
                }
                Ok(vec![JobRegistration {
                    name: JOB,
                    schedule: if idle {
                        "0 0 15 * * * 2099"
                    } else {
                        "* * * * * * *"
                    }
                    .into(),
                    max_run_duration: Some(Duration::from_secs(7200)),
                    job,
                }])
            },
            mode == "once",
            async move {
                let _closed = stopped.wait_for(|stop| *stop).await;
            },
        );
        // Wait until the actual listener is responding, without mutating runtime state.
        let announce = async {
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            loop {
                if client
                    .get(format!("http://{address}/health"))
                    .send()
                    .await
                    .is_ok()
                {
                    println!("ADDRESS {address}");
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        let result = supervise_signals(signals, stop, Duration::from_secs(1), async {
            tokio::pin!(work);
            tokio::select! {
                result = &mut work => result,
                () = announce => work.await,
            }
        })
        .await;
        println!("RUNTIME_RETURNED");
        Ok::<_, Box<dyn std::error::Error>>(result)
    });
    shutdown::teardown(runtime);
    if !matches!(result, Ok(Ok(()))) {
        std::process::exit(1);
    }
    Ok(())
}
