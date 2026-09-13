//! Real production executable, provider-free startup only. No DB/schema or deployment simulation.
#![cfg(unix)]

use rstest::rstest;
use std::io::{self, Read};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CANARY: &str = "private-lifecycle-config";

struct Process {
    child: Child,
    output: Vec<thread::JoinHandle<io::Result<String>>>,
}

impl Process {
    fn start(command: &mut Command) -> io::Result<Self> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut output = Vec::new();
        let readers: Vec<Box<dyn Read + Send>> = vec![
            Box::new(
                child
                    .stdout
                    .take()
                    .ok_or_else(|| io::Error::other("missing child stdout"))?,
            ),
            Box::new(
                child
                    .stderr
                    .take()
                    .ok_or_else(|| io::Error::other("missing child stderr"))?,
            ),
        ];
        for mut reader in readers {
            output.push(thread::spawn(move || {
                let mut text = String::new();
                reader.read_to_string(&mut text)?;
                Ok(text)
            }));
        }
        Ok(Self { child, output })
    }

    fn finish(&mut self, end: Instant) -> io::Result<(ExitStatus, String)> {
        loop {
            if let Some(status) = self.child.try_wait()? {
                let mut text = String::new();
                for reader in self.output.drain(..) {
                    text.push_str(
                        &reader
                            .join()
                            .map_err(|_| io::Error::other("output reader failed"))??,
                    );
                }
                assert!(!text.contains(CANARY));
                assert!(!text.contains("STOPPED"));
                assert!(!text.contains("\"state\":\"READY\""));
                return Ok((status, text));
            }
            if Instant::now() >= end {
                return Err(io::Error::other(
                    "actual server exceeded fixture process deadline",
                ));
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _killed = self.child.kill();
            let _reaped = self.child.wait();
        }
        for reader in self.output.drain(..) {
            let _joined = reader.join();
        }
    }
}

fn listener() -> io::Result<TcpListener> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn command(
    database: &TcpListener,
    cloud: &TcpListener,
    operations: &TcpListener,
    review: &TcpListener,
) -> io::Result<Command> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_server"));
    command
        .env_clear()
        .env("PATH", "")
        .env("HOME", "/nonexistent-crawler-lifecycle-fixture")
        .env("STAGE", "test")
        .env("POSTGRES_SSL_MODE", "disable")
        .env(
            "LOCAL_DB_URL",
            format!(
                "postgres://fixture:{CANARY}@{}/crawler",
                database.local_addr()?
            ),
        )
        .env(
            "BUSINESS_DATABASE_URL",
            format!(
                "postgres://fixture:{CANARY}@{}/business",
                database.local_addr()?
            ),
        )
        .env("SPIDER_MAX_SIZE_BYTES", "8388608")
        .env("VERTEX_AI_PROJECT_ID", "fixture-project")
        .env("VERTEX_AI_LOCATION", "global")
        .env(
            "GOOGLE_APPLICATION_CREDENTIALS",
            "/nonexistent-crawler-lifecycle-fixture/credentials.json",
        )
        .env("AWS_ACCESS_KEY_ID", "fixture")
        .env("AWS_SECRET_ACCESS_KEY", "fixture")
        .env("AWS_REGION", "eu-central-1")
        .env("AWS_EC2_METADATA_DISABLED", "true")
        .env(
            "AWS_CONFIG_FILE",
            "/nonexistent-crawler-lifecycle-fixture/config",
        )
        .env(
            "AWS_SHARED_CREDENTIALS_FILE",
            "/nonexistent-crawler-lifecycle-fixture/aws",
        )
        .env("CRAWLER_SHUTDOWN_GRACE_SECONDS", "1")
        .env("CRAWLER_STOP_TIMEOUT_SECONDS", "31")
        .env("CRAWLER_STARTUP_TIMEOUT_SECONDS", "2")
        .env(
            "CRAWLER_OPERATIONS_BIND_ADDR",
            operations.local_addr()?.to_string(),
        )
        .env("CRAWLER_REVIEW_BIND_ADDR", review.local_addr()?.to_string());
    let endpoint = format!("http://{}", cloud.local_addr()?);
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
        command.env(key, &endpoint);
    }
    command.env("GCE_METADATA_HOST", cloud.local_addr()?.to_string());
    Ok(command)
}

fn no_activity(listener: &TcpListener) {
    assert!(matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}

fn accept_startup(database: &TcpListener, process: &mut Process) -> io::Result<TcpStream> {
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        match database.accept() {
            Ok((socket, _)) => return Ok(socket),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
        if process.child.try_wait()?.is_some() || Instant::now() >= end {
            return Err(io::Error::other(
                "server never reached explicit database handshake",
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[rstest]
#[case("-INT")]
#[case("-TERM")]
fn should_stop_actual_server_during_provider_free_startup(#[case] signal: &str) -> io::Result<()> {
    let database = listener()?;
    let cloud = listener()?;
    let operations = listener()?;
    let review = listener()?;
    let mut process = Process::start(&mut command(&database, &cloud, &operations, &review)?)?;
    let mut connection = accept_startup(&database, &mut process)?;
    let at = Instant::now();
    assert!(
        Command::new("/bin/kill")
            .args([signal, &process.child.id().to_string()])
            .status()?
            .success()
    );
    let (status, _) = process.finish(at + Duration::from_secs(3))?;
    assert_eq!(status.code(), Some(0));
    // Drain any PostgreSQL startup bytes; process termination must close the socket.
    connection.set_read_timeout(Some(Duration::from_secs(1)))?;
    let mut bytes = Vec::new();
    match connection.read_to_end(&mut bytes) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
        Err(error) => return Err(error),
    }
    no_activity(&cloud);
    no_activity(&operations);
    no_activity(&review);
    Ok(())
}

#[test]
fn should_fail_actual_startup_deadline_without_providers_or_listeners() -> io::Result<()> {
    let database = listener()?;
    let cloud = listener()?;
    let operations = listener()?;
    let review = listener()?;
    let mut command = command(&database, &cloud, &operations, &review)?;
    command.env("CRAWLER_STARTUP_TIMEOUT_SECONDS", "1");
    let at = Instant::now();
    let mut process = Process::start(&mut command)?;
    let _connection = accept_startup(&database, &mut process)?;
    let (status, text) = process.finish(at + Duration::from_secs(4))?;
    assert_eq!(status.code(), Some(1));
    assert!(text.contains("startup deadline exceeded"));
    no_activity(&cloud);
    no_activity(&operations);
    no_activity(&review);
    Ok(())
}

#[test]
fn should_validate_lifecycle_config_in_both_modes_without_dependency_activity() -> io::Result<()> {
    let database = listener()?;
    let cloud = listener()?;
    let operations = listener()?;
    let review = listener()?;
    for args in [vec![], vec!["--check-config"]] {
        for (key, value) in [
            ("CRAWLER_SHUTDOWN_GRACE_SECONDS", "+1"),
            ("CRAWLER_STOP_TIMEOUT_SECONDS", "30"),
            ("CRAWLER_STARTUP_TIMEOUT_SECONDS", "0"),
            ("CRAWLER_OPERATIONS_BIND_ADDR", "0.0.0.0:9083"),
            ("CRAWLER_OPERATIONS_BIND_ADDR", CANARY),
        ] {
            let mut command = command(&database, &cloud, &operations, &review)?;
            command.args(&args).env(key, value);
            let mut process = Process::start(&mut command)?;
            let (status, text) = process.finish(Instant::now() + Duration::from_secs(3))?;
            assert_eq!(status.code(), Some(1));
            assert!(text.contains(key));
        }
    }
    let mut command = command(&database, &cloud, &operations, &review)?;
    command.env_remove("LOCAL_DB_URL");
    let mut process = Process::start(&mut command)?;
    let (status, text) = process.finish(Instant::now() + Duration::from_secs(3))?;
    assert_eq!(status.code(), Some(1));
    assert!(text.contains("missing required configuration: LOCAL_DB_URL"));
    no_activity(&database);
    no_activity(&cloud);
    no_activity(&operations);
    no_activity(&review);
    Ok(())
}
