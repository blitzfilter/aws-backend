use std::{
    error::Error,
    ffi::OsString,
    io::{self, Read},
    net::TcpListener,
    os::unix::ffi::OsStringExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

type TestResult = Result<(), Box<dyn Error>>;

struct Probe(Child);
impl Drop for Probe {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(Some(_))) {
            return;
        }
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

#[test]
#[ignore = "requires AURA_HISTORIA_CRON_TEST_BINARY pointing to a freshly built production binary"]
fn should_reject_invalid_job_env_in_every_binary_mode_before_network() -> TestResult {
    let binary = std::env::var_os("AURA_HISTORIA_CRON_TEST_BINARY")
        .ok_or("fresh production binary path required")?;
    let names = [
        "SEARCH_FILTER_PERIODIC_MATCH_CRON",
        "PERIODIC_MATCH_FILTER_PAGE_SIZE",
        "PERIODIC_MATCH_HYBRID_SCAN_LIMIT",
        "PERIODIC_MATCH_EVALUATION_LIMIT",
        "PERIODIC_MATCH_LLM_CONCURRENCY",
        "PERIODIC_MATCH_MAX_ATTEMPTS",
        "PERIODIC_MATCH_MAX_RUN_SECONDS",
        "PERIODIC_MATCH_PROJECTION_LAG_SECONDS",
        "PERIODIC_MATCH_REPLAY_OVERLAP_SECONDS",
    ];
    let modes: [&[&str]; 3] = [
        &["--check-config"],
        &["--run-once", "search-filter-periodic-match"],
        &[],
    ];
    let mut probes = 0;
    for args in modes {
        for name in names {
            for value in [
                OsString::from(""),
                OsString::from(" \t\n "),
                OsString::from("value_canary"),
                OsString::from_vec(b"value_canary\xff".to_vec()),
            ] {
                let postgres = TcpListener::bind("127.0.0.1:0")?;
                let opensearch = TcpListener::bind("127.0.0.1:0")?;
                postgres.set_nonblocking(true)?;
                opensearch.set_nonblocking(true)?;
                let mut probe = Probe(
                    Command::new(&binary)
                        .args(args)
                        .env_clear()
                        .env("STAGE", "test")
                        .env(
                            "AURA_HISTORIA_CRON_ENABLED_JOBS",
                            "search-filter-periodic-match",
                        )
                        .env("AURA_HISTORIA_CRON_HEALTH_BIND_ADDR", "127.0.0.1:0")
                        .env("POSTGRES_SSL_MODE", "disable")
                        .env("POSTGRES_HOST", "127.0.0.1")
                        .env("POSTGRES_PORT", postgres.local_addr()?.port().to_string())
                        .env("POSTGRES_DATABASE", "database_canary")
                        .env("POSTGRES_USERNAME", "username_canary")
                        .env("POSTGRES_PASSWORD", "password_canary")
                        .env(
                            "OPENSEARCH_ENDPOINT_URL",
                            format!("http://{}", opensearch.local_addr()?),
                        )
                        .env("VERTEX_AI_PROJECT_ID", "project_canary")
                        .env("VERTEX_AI_LOCATION", "location_canary")
                        .env("VERTEX_AI_MODEL", "model_canary")
                        .env(
                            "GOOGLE_APPLICATION_CREDENTIALS",
                            "/nonexistent/credentials_canary",
                        )
                        .env(name, value)
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()?,
                );
                let started = Instant::now();
                let status = loop {
                    if let Some(status) = probe.0.try_wait()? {
                        break status;
                    }
                    if started.elapsed() > Duration::from_secs(3) {
                        return Err(format!(
                            "binary did not fail pure validation for {name} in {args:?}"
                        )
                        .into());
                    }
                    thread::sleep(Duration::from_millis(5));
                };
                assert!(
                    !status.success(),
                    "binary accepted invalid {name} in {args:?}"
                );
                for listener in [&postgres, &opensearch] {
                    assert!(
                        matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock),
                        "binary reached a dependency for invalid {name} in {args:?}"
                    );
                }
                let mut output = String::new();
                probe
                    .0
                    .stdout
                    .take()
                    .ok_or("stdout missing")?
                    .read_to_string(&mut output)?;
                probe
                    .0
                    .stderr
                    .take()
                    .ok_or("stderr missing")?
                    .read_to_string(&mut output)?;
                for canary in [
                    "value_canary",
                    "database_canary",
                    "username_canary",
                    "password_canary",
                    "project_canary",
                    "location_canary",
                    "model_canary",
                    "credentials_canary",
                ] {
                    assert!(!output.contains(canary), "binary output leaked a canary");
                }
                assert!(
                    output.contains("cron.process.failed"),
                    "binary did not report a controlled failure"
                );
                assert!(!output.contains("cron.config.checked"));
                assert!(!output.contains("cron.scheduler.started"));
                probes += 1;
            }
        }
    }
    assert_eq!(probes, 108);
    Ok(())
}
