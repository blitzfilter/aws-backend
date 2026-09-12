//! Public real-executable CLI tests. Local TCP spies are NOT PostgreSQL/schema proof.
//! Genuine two-history, restricted-readonly-role snapshot tests need an owned PG fixture.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CANARY: &str = "private-secret-canary";
const CHILD_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy)]
enum SpyMode {
    Reject,
    Hold,
    CloudWatch { fail_operation: &'static str },
}

struct Spy {
    listener: TcpListener,
    connections: usize,
    mode: SpyMode,
    held_connections: Vec<TcpStream>,
    operations: Vec<String>,
}

impl Spy {
    fn new() -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            connections: 0,
            mode: SpyMode::Reject,
            held_connections: Vec::new(),
            operations: Vec::new(),
        })
    }

    fn drain(&mut self) -> io::Result<()> {
        // DB spies only reject/hold TCP; they never emulate a PostgreSQL schema.
        loop {
            match self.listener.accept() {
                Ok((socket, _)) => {
                    self.connections += 1;
                    match self.mode {
                        SpyMode::Reject => drop(socket),
                        SpyMode::Hold => self.held_connections.push(socket),
                        SpyMode::CloudWatch { fail_operation } => {
                            self.reply_cloudwatch(socket, fail_operation)?
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }
    fn reply_cloudwatch(&mut self, mut socket: TcpStream, fail_operation: &str) -> io::Result<()> {
        socket.set_read_timeout(Some(Duration::from_millis(500)))?;
        socket.set_write_timeout(Some(Duration::from_millis(500)))?;
        let mut request = Vec::new();
        let (header_end, length) = loop {
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                let length = headers
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                    .ok_or_else(|| io::Error::other("missing fixture request length"))?
                    .1
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| io::Error::other("invalid fixture request length"))?;
                if length > 64 * 1024 {
                    return Err(io::Error::other("fixture request too large"));
                }
                break (end + 4, length);
            }
            read_request_bytes(&mut socket, &mut request)?;
        };
        while request.len() < header_end + length {
            read_request_bytes(&mut socket, &mut request)?;
        }
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let operation = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case("x-amz-target"))
            .ok_or_else(|| io::Error::other("unexpected non-CloudWatch fixture request"))?
            .1
            .trim()
            .to_owned();
        let short_operation = operation
            .strip_prefix("Logs_20140328.")
            .ok_or_else(|| io::Error::other("unexpected CloudWatch fixture protocol"))?;
        if !matches!(
            short_operation,
            "CreateLogGroup" | "CreateLogStream" | "PutLogEvents"
        ) {
            return Err(io::Error::other("unexpected CloudWatch fixture operation"));
        }
        let (status, body) = if short_operation == fail_operation {
            (
                "400 Bad Request",
                format!("{{\"__type\":\"InvalidParameterException\",\"message\":\"{CANARY}\"}}"),
            )
        } else if fail_operation == "PutLogEvents" {
            // Export also proves that existing group/stream responses remain idempotent.
            (
                "400 Bad Request",
                format!(
                    "{{\"__type\":\"ResourceAlreadyExistsException\",\"message\":\"{CANARY}\"}}"
                ),
            )
        } else {
            ("200 OK", "{}".into())
        };
        self.operations.push(operation);
        write!(
            socket,
            "HTTP/1.1 {status}\r\nContent-Type: application/x-amz-json-1.1\r\nContent-Length: {}\r\nx-amzn-RequestId: {CANARY}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )?;
        socket.flush()
    }
}

fn read_request_bytes(socket: &mut TcpStream, request: &mut Vec<u8>) -> io::Result<()> {
    let mut bytes = [0; 4096];
    let count = socket.read(&mut bytes)?;
    if count == 0 {
        return Err(io::Error::other("fixture request closed early"));
    }
    request.extend_from_slice(&bytes[..count]);
    if request.len() > 128 * 1024 {
        return Err(io::Error::other("fixture request too large"));
    }
    Ok(())
}

struct Fixture {
    directory: PathBuf,
    crawler: Spy,
    business: Spy,
    cloud: Spy,
}

impl Fixture {
    fn new() -> io::Result<Self> {
        let directory = std::env::temp_dir().join(format!("crawler-cli-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&directory)?;
        let result = (|| {
            Ok(Self {
                directory: directory.clone(),
                crawler: Spy::new()?,
                business: Spy::new()?,
                cloud: Spy::new()?,
            })
        })();
        if result.is_err() {
            fs::remove_dir_all(directory)?;
        }
        result
    }

    fn command(&self) -> io::Result<Command> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_server"));
        command
            .env_clear()
            .current_dir(&self.directory)
            // No Docker executable/socket, developer HOME, cloud profile, or inherited secrets.
            .env("PATH", &self.directory)
            .env("HOME", &self.directory)
            .env("STAGE", "test")
            .env("POSTGRES_SSL_MODE", "disable")
            .env(
                "LOCAL_DB_URL",
                format!(
                    "postgres://fixture:{CANARY}@{}/crawler",
                    self.crawler.listener.local_addr()?
                ),
            )
            .env(
                "BUSINESS_DATABASE_URL",
                format!(
                    "postgres://fixture:{CANARY}@{}/business",
                    self.business.listener.local_addr()?
                ),
            )
            .env("SPIDER_MAX_SIZE_BYTES", "8388608")
            .env("VERTEX_AI_PROJECT_ID", "fixture-project")
            .env("VERTEX_AI_LOCATION", "global")
            .env(
                "GOOGLE_APPLICATION_CREDENTIALS",
                self.directory.join("must-not-open.json"),
            )
            .env(
                "AWS_CONFIG_FILE",
                self.directory.join("must-not-open-config"),
            )
            .env(
                "AWS_SHARED_CREDENTIALS_FILE",
                self.directory.join("must-not-open-credentials"),
            )
            .env("AWS_REGION", "eu-central-1");
        let cloud = format!("http://{}", self.cloud.listener.local_addr()?);
        for key in [
            "AWS_ENDPOINT_URL",
            "AWS_ENDPOINT_URL_CLOUDWATCH_LOGS",
            "AWS_ENDPOINT_URL_STS",
            "AWS_EC2_METADATA_SERVICE_ENDPOINT",
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
        ] {
            command.env(key, &cloud);
        }
        command.env(
            "GCE_METADATA_HOST",
            self.cloud.listener.local_addr()?.to_string(),
        );
        Ok(command)
    }

    fn run(&mut self, mut command: Command) -> io::Result<Output> {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("child stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("child stderr unavailable"))?;
        let read = |mut pipe: Box<dyn Read + Send>| -> io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes)?;
            Ok(bytes)
        };
        let stdout = thread::spawn(move || read(Box::new(stdout)));
        let stderr = thread::spawn(move || read(Box::new(stderr)));
        let started = Instant::now();
        let outcome = loop {
            let poll = (|| {
                self.crawler.drain()?;
                self.business.drain()?;
                self.cloud.drain()?;
                child.try_wait()
            })();
            match poll {
                Ok(Some(status)) => break Ok(status),
                Err(error) => break Err(error),
                Ok(None) if started.elapsed() >= CHILD_TIMEOUT => {
                    break Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "crawler CLI exceeded child deadline",
                    ));
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
            }
        };
        if outcome.is_err() {
            let kill = child.kill();
            child.wait()?;
            kill?;
        }
        let stdout = stdout
            .join()
            .map_err(|_| io::Error::other("stdout reader failed"))??;
        let stderr = stderr
            .join()
            .map_err(|_| io::Error::other("stderr reader failed"))??;
        self.crawler.drain()?;
        self.business.drain()?;
        self.cloud.drain()?;
        let output = Output {
            status: outcome?,
            stdout,
            stderr,
        };
        assert!(!String::from_utf8_lossy(&output.stdout).contains(CANARY));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(CANARY));
        if !matches!(self.cloud.mode, SpyMode::CloudWatch { .. }) {
            assert_eq!(
                self.cloud.connections, 0,
                "CLI contacted a cloud/credential/proxy endpoint"
            );
        }
        Ok(output)
    }

    fn cloudwatch_command(&self) -> io::Result<Command> {
        let mut command = self.command()?;
        // Deliberately fake signing values for the owned loopback HTTP fixture only.
        command
            .env("AWS_ACCESS_KEY_ID", "fixture-access-key")
            .env("AWS_SECRET_ACCESS_KEY", "fixture-secret-key")
            .env("AWS_MAX_ATTEMPTS", "1")
            .env("CRAWLER_CLOUDWATCH_LOG_GROUP", "fixture-group")
            .env("CRAWLER_CLOUDWATCH_LOG_STREAM", "fixture-stream");
        Ok(command)
    }

    fn assert_no_database_activity(&self) {
        assert_eq!(self.crawler.connections, 0);
        assert_eq!(self.business.connections, 0);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.directory) {
            eprintln!("CLI fixture cleanup failed: {}", error.kind());
        }
    }
}

#[cfg(unix)]
#[test]
fn should_bound_actual_preflight_exit_when_error_output_is_blocked() -> io::Result<()> {
    use std::os::{fd::OwnedFd, unix::net::UnixStream};

    let mut fixture = Fixture::new()?;
    let (_reader, mut writer) = UnixStream::pair()?;
    writer.set_nonblocking(true)?;
    loop {
        match writer.write(b"x") {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error),
        }
    }
    writer.set_nonblocking(false)?;
    let mut child = fixture
        .command()?
        .arg("--check-config")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(OwnedFd::from(writer)))
        .spawn()?;
    let started = Instant::now();
    let result = loop {
        let polled = (|| {
            fixture.crawler.drain()?;
            fixture.business.drain()?;
            fixture.cloud.drain()?;
            child.try_wait()
        })();
        match polled {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                break Err(io::Error::other(
                    "preflight watchdog did not stop blocked output",
                ));
            }
            Err(error) => break Err(error),
        }
    };
    if result.is_err() {
        let killed = child.kill();
        let reaped = child.wait();
        killed?;
        reaped?;
    }
    assert_eq!(result?.code(), Some(1));
    assert!(started.elapsed() >= Duration::from_millis(800));
    assert!(fixture.crawler.connections > 0 && fixture.business.connections > 0);
    assert_eq!(fixture.cloud.connections, 0);
    Ok(())
}

#[test]
fn should_accept_valid_cloudwatch_stream_names_in_both_modes() -> io::Result<()> {
    for name in ["worker 1", "worker-ä"] {
        for check in [false, true] {
            let mut fixture = Fixture::new()?;
            let mut command = fixture.cloudwatch_command()?;
            command.env("CRAWLER_CLOUDWATCH_LOG_STREAM", name);
            if check {
                command.arg("--check-config");
            } else {
                fixture.cloud.mode = SpyMode::CloudWatch {
                    fail_operation: "CreateLogStream",
                };
            }
            let output = fixture.run(command)?;
            assert!(!output.status.success());
            assert!(!String::from_utf8_lossy(&output.stderr).contains("malformed configuration"));
            if check {
                assert!(fixture.crawler.connections > 0 && fixture.business.connections > 0);
                assert_eq!(fixture.cloud.connections, 0);
            } else {
                assert!(
                    fixture
                        .cloud
                        .operations
                        .iter()
                        .any(|operation| operation.ends_with("CreateLogStream"))
                );
            }
        }
    }
    Ok(())
}

#[test]
fn should_show_help_with_cleared_environment_without_side_effects() -> io::Result<()> {
    let mut fixture = Fixture::new()?;
    fs::write(fixture.directory.join(".env"), format!("STAGE={CANARY}\n"))?;
    let mut command = fixture.command()?;
    command
        .env_clear()
        .env("PATH", &fixture.directory)
        .arg("--help");
    let output = fixture.run(command)?;
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Usage: server"));
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("runtime COMMIT_SHA overrides are ignored")
    );
    assert!(output.stderr.is_empty());
    fixture.assert_no_database_activity();
    Ok(())
}

#[test]
fn should_reject_unknown_extra_and_non_unicode_args_before_configuration() -> io::Result<()> {
    let mut fixture = Fixture::new()?;
    let mut cases = vec![
        vec![OsString::from(CANARY)],
        vec!["--".into()],
        vec!["-h".into()],
        vec!["--help".into(), CANARY.into()],
        vec!["--check-config".into(), "--help".into()],
        vec!["--check-config".into(), "--check-config".into()],
    ];
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let invalid = OsString::from_vec([CANARY.as_bytes(), &[0xff]].concat());
        cases.extend([
            vec![invalid.clone()],
            vec!["--help".into(), invalid.clone()],
            vec!["--check-config".into(), invalid],
        ]);
    }
    for args in cases {
        let mut command = fixture.command()?;
        command
            .env_clear()
            .env("PATH", &fixture.directory)
            .args(args);
        let output = fixture.run(command)?;
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("arguments") || stderr.contains("expected no arguments"));
        assert!(!stderr.contains("configuration"));
    }
    fixture.assert_no_database_activity();
    Ok(())
}

#[test]
fn should_reject_missing_empty_and_malformed_environment_without_dotenv_or_dependencies()
-> io::Result<()> {
    let mut fixture = Fixture::new()?;
    fs::write(
        fixture.directory.join(".env"),
        "LOCAL_DB_URL=postgres://fixture:private-secret-canary@127.0.0.1:1/crawler\n",
    )?;
    for args in [vec![], vec!["--check-config"]] {
        for (key, value, expected) in [
            (
                "LOCAL_DB_URL",
                None,
                "missing required configuration: LOCAL_DB_URL",
            ),
            (
                "BUSINESS_DATABASE_URL",
                Some(""),
                "empty configuration: BUSINESS_DATABASE_URL",
            ),
            (
                "SPIDER_MAX_SIZE_BYTES",
                Some(CANARY),
                "malformed configuration: SPIDER_MAX_SIZE_BYTES",
            ),
            (
                "VERTEX_AI_PROJECT_ID",
                Some(""),
                "empty configuration: VERTEX_AI_PROJECT_ID",
            ),
            (
                "CRAWLER_REVIEW_REQUIRED",
                Some(CANARY),
                "malformed configuration: CRAWLER_REVIEW_REQUIRED",
            ),
            (
                "CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS",
                Some("0"),
                "malformed configuration: CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS",
            ),
        ] {
            let mut command = fixture.command()?;
            command
                .args(&args)
                .env("CRAWLER_CLOUDWATCH_LOG_GROUP", "fixture-group");
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
            let output = fixture.run(command)?;
            assert!(!output.status.success());
            assert!(
                String::from_utf8_lossy(&output.stderr).contains(expected),
                "unexpected redacted error: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    fixture.assert_no_database_activity();
    Ok(())
}

#[cfg(unix)]
#[test]
fn should_reject_non_unicode_environment_without_leaks_or_dependencies() -> io::Result<()> {
    use std::os::unix::ffi::OsStringExt;
    let mut fixture = Fixture::new()?;
    for key in [
        "LOCAL_DB_URL",
        "BUSINESS_DATABASE_URL",
        "VERTEX_AI_MODEL",
        "CRAWLER_CLOUDWATCH_LOG_GROUP",
        "CRAWLER_REVIEW_AUTH_TOKEN",
        "SPIDER_MAX_SIZE_BYTES",
    ] {
        let mut command = fixture.command()?;
        command.arg("--check-config").env(
            key,
            OsString::from_vec([CANARY.as_bytes(), &[0xff]].concat()),
        );
        let output = fixture.run(command)?;
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains(&format!("non-Unicode configuration: {key}"))
        );
    }
    fixture.assert_no_database_activity();
    Ok(())
}

#[test]
fn should_only_contact_both_explicit_databases_when_checking_with_cloudwatch_enabled()
-> io::Result<()> {
    let mut fixture = Fixture::new()?;
    let mut command = fixture.command()?;
    command
        .arg("--check-config")
        .env("CRAWLER_CLOUDWATCH_LOG_GROUP", "fixture-group")
        .env("CRAWLER_CLOUDWATCH_LOG_STREAM", "fixture-stream")
        // Runtime metadata cannot replace the compiled, validated release identity.
        .env("COMMIT_SHA", CANARY);
    let output = fixture.run(command)?;
    assert!(
        !output.status.success(),
        "TCP rejection spies cannot prove schema readiness"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("database connection check failed"));
    assert!(fixture.crawler.connections > 0);
    assert!(fixture.business.connections > 0);
    assert!(output.stdout.is_empty());
    Ok(())
}

#[test]
fn should_restore_cloudwatch_exports_without_printing_provider_bodies() -> io::Result<()> {
    let mut fixture = Fixture::new()?;
    fixture.cloud.mode = SpyMode::CloudWatch {
        fail_operation: "PutLogEvents",
    };
    // Keep startup at the bounded DB handshake while the real exporter sends startup logs.
    fixture.crawler.mode = SpyMode::Hold;
    let command = fixture.cloudwatch_command()?;
    let output = fixture.run(command)?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("CloudWatch log export enabled"));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("CloudWatch log export failed (details redacted)")
    );
    for operation in ["CreateLogGroup", "CreateLogStream", "PutLogEvents"] {
        assert!(
            fixture
                .cloud
                .operations
                .iter()
                .any(|actual| actual == &format!("Logs_20140328.{operation}")),
            "{operation} was not called"
        );
    }
    assert!(fixture.crawler.connections > 0);
    assert_eq!(fixture.business.connections, 0);
    Ok(())
}

#[test]
fn should_redact_cloudwatch_bootstrap_failures_before_daemon_database_activity() -> io::Result<()> {
    for operation in ["CreateLogGroup", "CreateLogStream"] {
        let mut fixture = Fixture::new()?;
        fixture.cloud.mode = SpyMode::CloudWatch {
            fail_operation: operation,
        };
        let command = fixture.cloudwatch_command()?;
        let output = fixture.run(command)?;
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("failed to initialize CloudWatch log destination")
        );
        assert!(
            fixture
                .cloud
                .operations
                .iter()
                .any(|actual| actual == &format!("Logs_20140328.{operation}"))
        );
        fixture.assert_no_database_activity();
    }
    Ok(())
}

#[test]
fn should_execute_daemon_startup_without_discovering_cloud_credentials_when_export_disabled()
-> io::Result<()> {
    let mut fixture = Fixture::new()?;
    let mut command = fixture.command()?;
    // These formerly accepted positive values must not become new crawling policy limits.
    command
        .env("CRAWLER_LLM_MAX_CONCURRENT_REQUESTS", "9")
        .env("CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS", "1")
        .env("COMMIT_SHA", CANARY);
    let output = fixture.run(command)?;
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Starting Crawler Server"));
    assert!(stdout.contains("iteration04e2"));
    assert!(stdout.contains("c7a46b9b434eb0dc02c26a4e91edc863cae34896"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to connect to Postgres"));
    assert!(fixture.crawler.connections > 0);
    assert_eq!(
        fixture.business.connections, 0,
        "crawler connection failure must stop daemon startup"
    );
    Ok(())
}
