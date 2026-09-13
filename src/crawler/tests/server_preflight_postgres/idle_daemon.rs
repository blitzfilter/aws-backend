//! Black-box production daemon only: no runtime imports, stubs, or alternate mode.
//! Reuses the supervised fixture sequentially; each case owns fresh baseline DBs/roles.
//! ADC construction is not authentication proof. The pinned provider accepts this explicit
//! synthetic authorized_user file; its eager background refresh uses only our local spy.
use super::{
    Fault, Target, Tripwires,
    database::{Databases, PASSWORD},
    error::{TestError, TestResult},
    idle_auth::AuthSpy,
    process::{CleanupMode, Process},
    support::Directory,
};
use futures::FutureExt;
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, PgPool};
use std::{
    fs,
    net::{SocketAddr, TcpListener},
    os::unix::fs::OpenOptionsExt,
    panic::AssertUnwindSafe,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Copy, Debug)]
pub(super) enum Case {
    MissingConfig,
    CrawlerHistory,
    BusinessHistory,
    Credentials,
    ReviewBind,
    OperationsBind,
    Sigterm,
    Sigint,
}
pub(super) const CASES: [Case; 8] = [
    Case::MissingConfig,
    Case::CrawlerHistory,
    Case::BusinessHistory,
    Case::Credentials,
    Case::ReviewBind,
    Case::OperationsBind,
    Case::Sigterm,
    Case::Sigint,
];
const START: Duration = Duration::from_secs(8);
const EXIT: Duration = Duration::from_secs(4);
const POLL: Duration = Duration::from_millis(10);
const STARTUP_MESSAGES: [&str; 5] = [
    "Crawler cron configuration loaded",
    "Crawler-local schema and migration history verified (read-only)",
    "Business schema and migration history verified (read-only)",
    "Crawler LLM governor configured",
    "Crawler concrete dependencies initialized; binding listeners",
];

pub(super) fn reports(text: &str) -> Vec<String> {
    CASES
        .into_iter()
        .map(|case| {
            let pass = format!("PASS idle daemon {case:?}");
            let failure = format!("FAIL idle daemon {case:?}: ");
            if let Some(kind) = text.lines().find_map(|line| line.strip_prefix(&failure)) {
                format!("{failure}{}", TestError::report_kind(kind))
            } else if text.lines().any(|line| line == pass) {
                pass
            } else {
                format!("UNREPORTED idle daemon {case:?}")
            }
        })
        .collect()
}

fn command(
    wires: &Tripwires,
    databases: &Databases,
    directory: &Path,
    review: SocketAddr,
    operations: SocketAddr,
    auth: &AuthSpy,
) -> TestResult<Command> {
    let mut command = wires.base_command(databases, directory)?;
    // No inherited credential fallback; no valid account or refresh token exists here.
    let adc = json!({
        "type": "authorized_user",
        "client_id": "idle-client-canary",
        "client_secret": "idle-secret-canary",
        "refresh_token": "idle-refresh-canary",
        "token_uri": format!("http://{}/token", auth.address()),
    });
    let path = directory.join("idle-adc.json");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    std::io::Write::write_all(&mut file, adc.to_string().as_bytes())?;
    command
        .env_remove("CRAWLER_CLOUDWATCH_LOG_GROUP")
        .env_remove("CRAWLER_CLOUDWATCH_LOG_STREAM")
        .env("AWS_EC2_METADATA_DISABLED", "true")
        .env("GOOGLE_APPLICATION_CREDENTIALS", path)
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1")
        .env("VERTEX_AI_PROJECT_ID", "idle-fixture-project")
        .env("VERTEX_AI_MODEL", "idle-fixture-model")
        .env("CRAWLER_REVIEW_BIND_ADDR", review.to_string())
        .env("CRAWLER_OPERATIONS_BIND_ADDR", operations.to_string())
        .env("CRAWLER_STARTUP_TIMEOUT_SECONDS", "6")
        .env("CRAWLER_SHUTDOWN_GRACE_SECONDS", "2")
        .env("CRAWLER_STOP_TIMEOUT_SECONDS", "32")
        // Must not override embedded build identity.
        .env("COMMIT_SHA", "ffffffffffffffffffffffffffffffffffffffff");
    Ok(command)
}

async fn grant_idle_permissions(databases: &Databases) -> TestResult {
    // Preserve ledger-only restrictions for all 28 preflight cases. Only these fresh
    // idle DBs allow the empty authoritative snapshot UPDATE and candidate SELECTs.
    sqlx::raw_sql(AssertSqlSafe(format!(
        "ALTER ROLE {role} SET default_transaction_read_only = off;
         GRANT SELECT ON public.listing_sources, public.listing_source_domains,
             public.listing_source_urls, public.crawler_reviews TO {role};
         GRANT UPDATE (crawl_enabled, updated) ON public.listing_sources TO {role}",
        role = databases.crawler.role,
    )))
    .execute(&databases.crawler.pool)
    .await?;
    sqlx::raw_sql(AssertSqlSafe(format!(
        "GRANT SELECT ON public.listing_sources, public.listing_source_ingestion_methods,
             public.listing_source_web_crawl_ingestion_configurations TO {}",
        databases.business.role,
    )))
    .execute(&databases.business.pool)
    .await?;
    for db in [&databases.crawler, &databases.business] {
        let forbidden: bool = sqlx::query_scalar(
            "SELECT r.rolsuper OR r.rolcreatedb OR r.rolcreaterole OR r.rolinherit
                 OR r.rolreplication OR r.rolbypassrls
                 OR has_schema_privilege(r.oid,'public','CREATE')
                 OR has_database_privilege(r.oid,current_database(),'CREATE,TEMPORARY')
                 OR EXISTS (SELECT FROM pg_catalog.pg_class c
                    WHERE c.relnamespace='public'::regnamespace AND c.relkind IN ('r','p')
                    AND has_table_privilege(r.oid,c.oid,'INSERT,DELETE,TRUNCATE'))
                 OR EXISTS (SELECT FROM pg_catalog.pg_proc p
                    WHERE p.pronamespace='pg_catalog'::regnamespace AND p.proname LIKE 'pg%advisory%'
                    AND has_function_privilege(r.oid,p.oid,'EXECUTE'))
             FROM pg_catalog.pg_roles r WHERE r.rolname=$1",
        ).bind(&db.role).fetch_one(&db.pool).await?;
        assert!(!forbidden, "idle runtime role gained unrelated privileges");
        let sources: i64 = sqlx::query_scalar("SELECT count(*) FROM public.listing_sources")
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(
            sources, 0,
            "idle fixture must have no authoritative/local sources"
        );
    }
    for table in ["listing_source_domains", "listing_source_urls"] {
        let count: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT count(*) FROM public.{table}"
        )))
        .fetch_one(&databases.crawler.pool)
        .await?;
        assert_eq!(count, 0, "idle fixture must have no crawl candidates");
    }
    Ok(())
}

fn safe_output(text: &str) {
    assert!(
        !text.contains("canary") && !text.contains(PASSWORD),
        "daemon leaked synthetic secret; output suppressed"
    );
    assert!(
        !text.contains("postgres://") && !text.contains("postgresql://"),
        "daemon leaked URL; output suppressed"
    );
    assert!(
        !text.contains("STOPPED"),
        "STOPPED must not be announced before process exit"
    );
}

fn startup_order(text: &str) -> TestResult {
    let mut rest = text;
    for (message, kind) in STARTUP_MESSAGES.into_iter().zip([
        "IDLE_CONFIG_ORDER",
        "IDLE_CRAWLER_HISTORY_ORDER",
        "IDLE_BUSINESS_HISTORY_ORDER",
        "IDLE_LLM_ORDER",
        "IDLE_DEPENDENCIES_ORDER",
    ]) {
        let (_, after) = rest
            .split_once(message)
            .ok_or_else(|| TestError::failure(kind))?;
        rest = after;
    }
    Ok(())
}

async fn probe(address: SocketAddr, method: &str, path: &str) -> TestResult<(u16, String, Value)> {
    assert!(address.ip().is_loopback());
    tokio::time::timeout(Duration::from_secs(1), async {
        let mut stream = tokio::net::TcpStream::connect(address).await?;
        stream
            .write_all(
                format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await?;
        let mut bytes = Vec::new();
        stream.take(4097).read_to_end(&mut bytes).await?;
        assert!(bytes.len() <= 4096, "oversized probe response");
        let text = String::from_utf8(bytes)?;
        safe_output(&text);
        let (headers, body) = text
            .split_once("\r\n\r\n")
            .ok_or("malformed probe response")?;
        let status = headers
            .split_whitespace()
            .nth(1)
            .ok_or("missing probe status")?
            .parse()?;
        Ok((
            status,
            headers.to_ascii_lowercase(),
            serde_json::from_str(body)?,
        ))
    })
    .await?
}

async fn failed_startup(
    process: &mut Process,
    operations: SocketAddr,
    occupied: bool,
) -> TestResult<std::process::Output> {
    tokio::time::timeout(START, async {
        loop {
            if process.try_reap()?.is_some() {
                return process.finish(Duration::ZERO);
            }
            // An occupied operations address belongs to this test, not the daemon.
            // All other rejected startup paths must never open the readiness listener.
            if !occupied {
                refused(operations).await?;
            }
            tokio::time::sleep(POLL).await;
        }
    })
    .await?
}

async fn refused(address: SocketAddr) -> TestResult {
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        tokio::net::TcpStream::connect(address),
    )
    .await?;
    assert!(
        matches!(result, Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused),
        "unexpected listener after failed startup/exit"
    );
    Ok(())
}

fn assert_server_environment(process: &Process, directory: &Path) -> TestResult {
    // Host subprocess, not a mount/network sandbox. No Docker variables, executable on
    // PATH, or inherited socket descriptor is supplied; the host socket is not hidden.
    let bytes = fs::read(format!("/proc/{}/environ", process.id()))?;
    let entries = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
    for expected in [
        format!("PATH={}", directory.display()),
        format!("HOME={}", directory.display()),
        format!(
            "GOOGLE_APPLICATION_CREDENTIALS={}",
            directory.join("idle-adc.json").display()
        ),
    ] {
        assert!(
            entries.contains(&expected.as_bytes()),
            "daemon isolation environment mismatch"
        );
    }
    assert!(entries.iter().all(|entry| {
        !entry.starts_with(b"DOCKER_")
            && !entry
                .windows(b"docker.sock".len())
                .any(|part| part == b"docker.sock")
    }));
    assert!(!directory.join("docker").try_exists()?);
    Ok(())
}

struct ProbeAddresses {
    review: SocketAddr,
    operations: SocketAddr,
}

async fn idle(
    case: Case,
    process: &mut Process,
    databases: &Databases,
    wires: &Tripwires,
    directory: &Path,
    addresses: ProbeAddresses,
    auth: &AuthSpy,
) -> TestResult {
    let ProbeAddresses { review, operations } = addresses;
    assert_server_environment(process, directory)?;
    tokio::time::timeout(START, async {
        loop {
            if process.try_reap()?.is_some() {
                return Err(TestError::failure("IDLE_EARLY_EXIT"));
            }
            let text = process.stdout()?;
            safe_output(&text);
            // These are emitted by real cron after authoritative sync and empty lookup.
            if text.contains("Spider scheduler pass finished")
                && text.contains("Raw capture channel drained")
                && auth.requests() == 3
            {
                startup_order(&text)?;
                let complete = text.rsplit_once('\n').map_or("", |(complete, _)| complete);
                let records = complete
                    .lines()
                    .map(serde_json::from_str::<Value>)
                    .collect::<Result<Vec<_>, _>>()?;
                assert!(
                    records.iter().any(|record| record["fields"]["message"]
                        == "Raw capture channel drained"
                        && record["fields"]["summary"]
                            .as_str()
                            .is_some_and(|summary| summary.contains("accepted: 0,"))),
                    "idle collector accepted work"
                );
                assert!(records.iter().any(|record| record["fields"]["message"]
                    == "Spider scheduler pass finished"
                    && record["fields"]["total"] == 0));
                break;
            }
            wires
                .untouched()
                .map_err(|error| error.context("IDLE_NETWORK_ACTIVITY"))?;
            tokio::time::sleep(POLL).await;
        }
        Ok::<_, TestError>(())
    })
    .await??;
    for db in [&databases.crawler, &databases.business] {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_catalog.pg_stat_activity
             WHERE datname=$1 AND usename=$2 AND application_name='crawler-server'",
        )
        .bind(&db.name)
        .bind(&db.role)
        .fetch_one(&db.pool)
        .await?;
        assert!(
            count > 0,
            "READY proof did not observe both live runtime pools"
        );
    }
    let commit = option_env!("COMMIT_SHA").ok_or("build with fixture COMMIT_SHA")?;
    for (method, path, expected) in [
        ("GET", "/health", 200),
        ("GET", "/ready", 200),
        ("GET", "/ops/version", 200),
        ("GET", "/api/listing_sources", 404),
        ("POST", "/ready", 404),
    ] {
        let (status, headers, body) = probe(operations, method, path).await?;
        assert_eq!(status, expected);
        assert!(headers.contains("\r\ncache-control: no-store\r\n"));
        assert!(headers.contains("\r\ncontent-type: application/json\r\n"));
        assert!(
            body == json!({"commit_sha": commit, "state": "READY"}),
            "probe exposed unexpected fields/identity/state"
        );
    }
    let (status, _, body) = probe(review, "GET", "/health").await?;
    assert_eq!(status, 200);
    assert!(body == json!({"ok": true}));
    let (status, _, _) = probe(review, "GET", "/api/health").await?;
    assert_eq!(status, 401, "review authentication was bypassed");
    wires
        .untouched()
        .map_err(|error| error.context("IDLE_NETWORK_ACTIVITY"))?;
    let started = Instant::now();
    process.signal(match case {
        Case::Sigterm => libc::SIGTERM,
        Case::Sigint => libc::SIGINT,
        _ => unreachable!(),
    })?;
    let output = process.finish(EXIT)?;
    assert_eq!(
        output.status.code(),
        Some(0),
        "idle daemon did not exit gracefully; output suppressed"
    );
    assert!(started.elapsed() < EXIT, "idle signal exit exceeded bound");
    safe_output(&String::from_utf8(output.stdout)?);
    assert!(
        output.stderr.is_empty(),
        "idle daemon emitted stderr; output suppressed"
    );
    Ok(())
}

async fn check(case: Case, databases: &Databases) -> TestResult {
    grant_idle_permissions(databases).await?;
    match case {
        Case::CrawlerHistory => {
            super::Case::Fault(Target::Crawler, Fault::MissingVersion)
                .inject(databases)
                .await?
        }
        Case::BusinessHistory => {
            super::Case::Fault(Target::Business, Fault::MissingVersion)
                .inject(databases)
                .await?
        }
        _ => {}
    }
    let before = [
        databases.crawler.snapshot().await?,
        databases.business.snapshot().await?,
    ];
    let directory = Directory::new()?;
    let result: TestResult = async {
        let wires = Tripwires::new()?;
        let review_guard = TcpListener::bind("127.0.0.1:0")?;
        let operations_guard = TcpListener::bind("127.0.0.1:0")?;
        let review = review_guard.local_addr()?;
        let operations = operations_guard.local_addr()?;
        let mut auth = AuthSpy::start()?;
        let mut command = command(&wires, databases, &directory.0, review, operations, &auth)?;
        match case {
            Case::MissingConfig => { command.env_remove("LOCAL_DB_URL"); }
            Case::Credentials => { fs::write(directory.0.join("idle-adc.json"), b"{}")?; }
            _ => {}
        }
        let review_guard = matches!(case, Case::ReviewBind).then_some(review_guard);
        let operations_guard = matches!(case, Case::OperationsBind).then_some(operations_guard);
        let mut process = Process::spawn(&mut command, START + EXIT, CleanupMode::Kill)?;
        let work: TestResult = async {
            if matches!(case, Case::Sigterm | Case::Sigint) {
                idle(case, &mut process, databases, &wires, &directory.0, ProbeAddresses { review, operations }, &auth).await
            } else {
                let output = failed_startup(&mut process, operations, matches!(case, Case::OperationsBind)).await?;
                assert_eq!(output.status.code(), Some(1), "startup gate did not fail closed; output suppressed");
                let stdout = String::from_utf8(output.stdout)?;
                let stderr = String::from_utf8(output.stderr)?;
                safe_output(&stdout);
                safe_output(&stderr);
                assert!(!stdout.contains("Starting crawler cron job loops"));
                let expected = match case {
                    Case::MissingConfig => "missing required configuration: LOCAL_DB_URL",
                    Case::CrawlerHistory => "crawler migration history incomplete or mismatched",
                    Case::BusinessHistory => "business schema verification failed:",
                    Case::Credentials => "failed to initialize Google application default credentials",
                    Case::ReviewBind => "crawler review server failed",
                    Case::OperationsBind => "crawler operations listener failed",
                    _ => unreachable!(),
                };
                assert!(stderr.starts_with("crawler server: ") && stderr.to_ascii_lowercase().contains(&expected.to_ascii_lowercase()),
                    "wrong daemon failure boundary; output suppressed");
                match case {
                    Case::MissingConfig => assert!(stdout.is_empty()),
                    Case::CrawlerHistory => assert!(stdout.contains(STARTUP_MESSAGES[0]) && !stdout.contains(STARTUP_MESSAGES[1])),
                    Case::BusinessHistory => assert!(stdout.contains(STARTUP_MESSAGES[1]) && !stdout.contains(STARTUP_MESSAGES[2])),
                    Case::Credentials => assert!(stdout.contains(STARTUP_MESSAGES[2]) && !stdout.contains(STARTUP_MESSAGES[4])),
                    _ => startup_order(&stdout)?,
                }
                Ok(())
            }
        }.await;
        // Reap before checking sessions, including failure/timeout. Kill fallback never
        // turns a failed proof into graceful success. Drop also covers unwind/cancellation.
        drop(process);
        auth.close()?;
        if matches!(case, Case::MissingConfig | Case::CrawlerHistory | Case::BusinessHistory | Case::Credentials) {
            assert_eq!(auth.requests(), 0, "auth refresh preceded configuration/history/credential validation");
        } else if matches!(case, Case::Sigterm | Case::Sigint) {
            assert_eq!(auth.requests(), 3, "unexpected auth refresh count");
        }
        for (db, before) in [&databases.crawler, &databases.business].into_iter().zip(&before) {
            db.no_runtime_sessions().await?;
            assert!(*before == db.snapshot().await?, "idle daemon changed persisted data/history/catalogs; snapshots suppressed");
        }
        wires.untouched().map_err(|error| error.context("IDLE_NETWORK_ACTIVITY"))?;
        if let Some(listener) = &review_guard {
            listener.set_nonblocking(true)?;
            assert!(matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock));
        }
        if let Some(listener) = &operations_guard {
            listener.set_nonblocking(true)?;
            assert!(matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock));
        }
        drop((review_guard, operations_guard));
        refused(review).await?;
        refused(operations).await?;
        assert_eq!(fs::read_dir(&directory.0)?.count(), 1, "daemon created unexpected files");
        work
    }.await;
    directory.close()?;
    result
}

pub(super) async fn run_case(case: Case, admin: &PgPool, port: u16) -> TestResult {
    let mut databases = Databases::new(port)?;
    let work = AssertUnwindSafe(async {
        databases.prepare(admin).await?;
        check(case, &databases).await
    })
    .catch_unwind();
    let result = tokio::time::timeout(Duration::from_secs(25), work).await;
    databases.close(admin).await?;
    match result {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(TestError::failure("CASE_ASSERTION")),
        Err(error) => Err(TestError::caused("CASE_DEADLINE", error)),
    }
}

#[test]
fn should_redact_idle_reports_and_require_every_case() {
    let reports =
        reports("FAIL idle daemon Credentials: private-provider-body\nPASS idle daemon Sigterm\n");
    assert_eq!(reports.len(), CASES.len());
    assert!(reports[3] == "FAIL idle daemon Credentials: UNRECOGNIZED_CLASSIFICATION");
    assert!(reports[6] == "PASS idle daemon Sigterm");
    assert!(reports[7] == "UNREPORTED idle daemon Sigint");
    assert!(
        reports
            .iter()
            .all(|report| !report.contains("private-provider-body"))
    );
}
