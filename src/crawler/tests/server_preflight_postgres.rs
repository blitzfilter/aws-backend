//! Opt-in, actual `server --check-config` against test-api's fixed Unix-Docker PostgreSQL.
//! Run only the parent entry below on an isolated local coding host: the existing fixture
//! publishes 0.0.0.0 for gateway tests. No external-reachability or release-artifact claim.
//! COMMIT_SHA identifies this test build only. No cloud, websites, live data, or daemon tests.
//! Parent verifies exit-hook cleanup; never run the ignored fixture child directly.
//! Includes crawler ledger view/RLS rejection and a held-lock deadline with role timeouts off.
//! Run with AURA_CRAWLER_ISOLATED_LOCAL_POSTGRES=1 and fixture COMMIT_SHA at build time:
//! cargo test -p crawler --test server_preflight_postgres --all-features --offline --locked
//! should_check_actual_server_with_readonly_postgres_and_owned_cleanup
//! -- --exact --ignored --nocapture --test-threads=1
#![cfg(target_os = "linux")]

#[path = "server_preflight_postgres/database.rs"]
mod database;
#[path = "server_preflight_postgres/error.rs"]
mod error;
#[path = "server_preflight_postgres/process.rs"]
mod process;
#[path = "server_preflight_postgres/support.rs"]
mod support;

use database::{Database, Databases, PASSWORD};
use error::{TestError, TestResult};
use futures::FutureExt;
use process::{CleanupMode, run_process};
use serde_json::Value;
use sqlx::{AssertSqlSafe, PgPool};
use std::{
    fs, io,
    net::TcpListener,
    panic::AssertUnwindSafe,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use support::Directory;

struct Tripwires {
    aws: TcpListener,
    google: TcpListener,
    review: TcpListener,
}
impl Tripwires {
    fn new() -> TestResult<Self> {
        let wires = Self {
            aws: TcpListener::bind("127.0.0.1:0")?,
            google: TcpListener::bind("127.0.0.1:0")?,
            review: TcpListener::bind("127.0.0.1:0")?,
        };
        for listener in [&wires.aws, &wires.google, &wires.review] {
            listener.set_nonblocking(true)?;
        }
        Ok(wires)
    }

    fn command(&self, databases: &Databases, directory: &Path) -> TestResult<Command> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_server"));
        command
            .arg("--check-config")
            .env_clear()
            .current_dir(directory)
            // Empty private PATH/HOME; no Docker executable, socket mount, dotenv,
            // developer credential files, cloud profiles, or inherited PG options.
            // This is a host subprocess, NOT a mount/network sandbox attestation.
            .env("PATH", directory)
            .env("HOME", directory)
            .env("TMPDIR", directory)
            .env("STAGE", "test")
            .env("POSTGRES_SSL_MODE", "disable")
            .env("LOCAL_DB_URL", databases.crawler.url())
            .env("BUSINESS_DATABASE_URL", databases.business.url())
            .env("SPIDER_MAX_SIZE_BYTES", "8388608")
            .env("VERTEX_AI_PROJECT_ID", "fixture-project-canary")
            .env("VERTEX_AI_LOCATION", "global")
            .env("VERTEX_AI_MODEL", "fixture-model-canary")
            .env(
                "GOOGLE_APPLICATION_CREDENTIALS",
                directory.join("missing-adc-canary.json"),
            )
            .env("CLOUDSDK_CONFIG", directory)
            .env("CRAWLER_CLOUDWATCH_LOG_GROUP", "fixture-log-group-canary")
            .env("CRAWLER_CLOUDWATCH_LOG_STREAM", "fixture-log-stream-canary")
            .env("AWS_REGION", "eu-central-1")
            // Fake signing values; the configured CloudWatch destination is our loopback
            // spy, not a real account. This does not claim OS-level network confinement.
            .env("AWS_ACCESS_KEY_ID", "fixture-access-canary")
            .env("AWS_SECRET_ACCESS_KEY", "fixture-secret-canary")
            .env("AWS_MAX_ATTEMPTS", "1")
            .env(
                "AWS_CONFIG_FILE",
                directory.join("missing-aws-config-canary"),
            )
            .env(
                "AWS_SHARED_CREDENTIALS_FILE",
                directory.join("missing-aws-credentials-canary"),
            )
            // Occupied valid address: successful preflight cannot bind the review server.
            .env(
                "CRAWLER_REVIEW_BIND_ADDR",
                self.review.local_addr()?.to_string(),
            )
            .env("CRAWLER_REVIEW_AUTH_TOKEN", "fixture-review-token-canary")
            .env("LOG_LEVEL", "info");
        let aws = format!("http://{}", self.aws.local_addr()?);
        for key in [
            "AWS_ENDPOINT_URL",
            "AWS_ENDPOINT_URL_CLOUDWATCH_LOGS",
            "AWS_ENDPOINT_URL_STS",
            "AWS_EC2_METADATA_SERVICE_ENDPOINT",
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
        ] {
            command.env(key, &aws);
        }
        let google = format!("http://{}", self.google.local_addr()?);
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env(key, &google);
        }
        command
            .env("NO_PROXY", "")
            .env("no_proxy", "")
            .env("GCE_METADATA_HOST", self.google.local_addr()?.to_string())
            .env("GCE_METADATA_IP", "127.0.0.1");
        assert!(!directory.join("missing-adc-canary.json").try_exists()?);
        Ok(command)
    }

    fn untouched(&self) -> TestResult {
        for (name, listener) in [
            ("AWS/logging", &self.aws),
            ("Google/ADC/proxy", &self.google),
            ("review", &self.review),
        ] {
            if !matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
            {
                return Err(format!("preflight contacted the {name} tripwire").into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
enum Target {
    Crawler,
    Business,
}
impl Target {
    fn database(self, databases: &Databases) -> &Database {
        match self {
            Self::Crawler => &databases.crawler,
            Self::Business => &databases.business,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Crawler => "crawler",
            Self::Business => "business",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    MissingLedger,
    MissingVersion,
    DirtyHistory,
    AlteredChecksum,
    UnknownVersion,
    WrongHistory,
    MissingTable,
    DeniedLedger,
    DeniedSchema,
    NoLogin,
    LedgerView,
    LedgerRls,
    LedgerLocked,
}

#[derive(Clone, Copy, Debug)]
enum Case {
    Valid,
    CatalogSearchPath,
    MissingConfig(&'static str),
    Fault(Target, Fault),
    SwappedTargets,
}
impl Case {
    fn pass_report(self) -> String {
        format!(
            "PASS {self:?}: both snapshots unchanged; no app backends/advisory locks; owned databases/roles removed"
        )
    }

    async fn inject(self, databases: &Databases) -> TestResult {
        let Self::Fault(target, fault) = self else {
            if matches!(self, Self::CatalogSearchPath) {
                for db in [&databases.crawler, &databases.business] {
                    sqlx::raw_sql(AssertSqlSafe(format!(
                        "ALTER ROLE {} SET search_path=pg_catalog",
                        db.role
                    )))
                    .execute(&db.pool)
                    .await?;
                }
            }
            return Ok(());
        };
        let db = target.database(databases);
        let sql = match fault {
            Fault::MissingLedger => "ALTER TABLE public._sqlx_migrations RENAME TO fixture_hidden_ledger".into(),
            Fault::MissingVersion => "DELETE FROM public._sqlx_migrations WHERE version=(SELECT min(version) FROM public._sqlx_migrations)".into(),
            Fault::DirtyHistory => "UPDATE public._sqlx_migrations SET success=false WHERE version=(SELECT min(version) FROM public._sqlx_migrations)".into(),
            Fault::AlteredChecksum => "UPDATE public._sqlx_migrations SET checksum=decode('00','hex') WHERE version=(SELECT min(version) FROM public._sqlx_migrations)".into(),
            Fault::UnknownVersion => "INSERT INTO public._sqlx_migrations (version,description,success,checksum,execution_time) VALUES (20990101000000,'fixture unknown',true,decode('00','hex'),0)".into(),
            Fault::MissingTable => format!("ALTER TABLE public.{} RENAME TO fixture_hidden_table", match target {
                Target::Crawler => "crawler_reviews", Target::Business => "users",
            }),
            Fault::DeniedLedger => format!("REVOKE SELECT ON public._sqlx_migrations FROM {}", db.role),
            Fault::DeniedSchema => format!("REVOKE USAGE ON SCHEMA public FROM {}", db.role),
            Fault::NoLogin => format!("ALTER ROLE {} NOLOGIN", db.role),
            Fault::LedgerView => format!(
                "ALTER TABLE public._sqlx_migrations RENAME TO fixture_backing_ledger;
                 CREATE VIEW public._sqlx_migrations AS SELECT * FROM public.fixture_backing_ledger;
                 GRANT SELECT ON public._sqlx_migrations TO {}", db.role,
            ),
            Fault::LedgerRls => format!(
                "ALTER TABLE public._sqlx_migrations ENABLE ROW LEVEL SECURITY;
                 CREATE POLICY fixture_read_ledger ON public._sqlx_migrations FOR SELECT TO {} USING (true)", db.role,
            ),
            Fault::LedgerLocked => format!(
                "ALTER ROLE {role} SET lock_timeout = '0';
                 ALTER ROLE {role} SET statement_timeout = '0'", role = db.role,
            ),
            Fault::WrongHistory => {
                let other = match target { Target::Crawler => &databases.business, Target::Business => &databases.crawler };
                let foreign: Value = sqlx::query_scalar(
                    "SELECT jsonb_agg(to_jsonb(m) ORDER BY version) FROM public._sqlx_migrations m",
                ).fetch_one(&other.pool).await?;
                // Deliberate wrong-target fault ONLY. Copy the other genuine SQLx history,
                // leaving this database's correct schema intact. Never repair or adopt it.
                sqlx::raw_sql("DELETE FROM public._sqlx_migrations").execute(&db.pool).await?;
                sqlx::query("INSERT INTO public._sqlx_migrations SELECT * FROM jsonb_populate_recordset(NULL::public._sqlx_migrations,$1)")
                    .bind(foreign).execute(&db.pool).await?;
                return Ok(());
            }
        };
        sqlx::raw_sql(AssertSqlSafe(sql)).execute(&db.pool).await?;
        if matches!(
            fault,
            Fault::LedgerView | Fault::LedgerRls | Fault::LedgerLocked
        ) {
            // Correct rows remain readable: view/RLS rejection must be a catalog decision,
            // not an accidental permission failure or policy hiding the genuine history.
            let pool = db.permission_probe().await?;
            let probe: TestResult = async {
                let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM public._sqlx_migrations")
                    .fetch_one(&pool).await?;
                assert_eq!(rows, 6);
                if matches!(fault, Fault::LedgerLocked) {
                    let timeouts_off: bool = sqlx::query_scalar(
                        "SELECT current_setting('lock_timeout')='0' AND current_setting('statement_timeout')='0'",
                    ).fetch_one(&pool).await?;
                    assert!(timeouts_off, "fixture defaults would mask runtime timeout behavior");
                }
                Ok(())
            }.await;
            tokio::time::timeout(Duration::from_secs(5), pool.close()).await?;
            probe?;
        }
        Ok(())
    }

    fn expected_error(self) -> Option<String> {
        match self {
            Self::Valid | Self::CatalogSearchPath => None,
            Self::MissingConfig(key) => Some(format!("missing required configuration: {key}")),
            Self::Fault(target, Fault::NoLogin) => Some(format!(
                "{} database connection check failed",
                target.label()
            )),
            Self::Fault(Target::Crawler, Fault::LedgerView | Fault::LedgerRls) => Some(
                "crawler database schema check failed: crawler migration ledger is not a trusted persistent table".into(),
            ),
            Self::Fault(Target::Crawler, Fault::LedgerLocked) => Some(
                "crawler database schema check failed: failed to read crawler schema readiness".into(),
            ),
            Self::Fault(target, _) => {
                Some(format!("{} database schema check failed", target.label()))
            }
            Self::SwappedTargets => Some("crawler database schema check failed".into()),
        }
    }
}

async fn check(case: Case, databases: &Databases) -> TestResult {
    case.inject(databases).await?;
    let before = [
        databases.crawler.snapshot().await?,
        databases.business.snapshot().await?,
    ];
    let directory = Directory::new()?;
    let result: TestResult = async {
        let wires = Tripwires::new()?;
        let mut command = wires.command(databases, &directory.0)?;
        match case {
            Case::MissingConfig(key) => {
                command.env_remove(key);
            }
            Case::SwappedTargets => {
                command
                    .env("LOCAL_DB_URL", databases.business.url())
                    .env("BUSINESS_DATABASE_URL", databases.crawler.url());
            }
            _ => {}
        }
        let locked = matches!(case, Case::Fault(Target::Crawler, Fault::LedgerLocked));
        let blocker = if locked {
            let mut transaction = databases.crawler.pool.begin().await?;
            sqlx::raw_sql("LOCK TABLE public._sqlx_migrations IN ACCESS EXCLUSIVE MODE")
                .execute(&mut *transaction).await?;
            Some(transaction)
        } else { None };
        let started = Instant::now();
        let output = run_process(&mut command, Duration::from_secs(if locked { 8 } else { 20 }), CleanupMode::Kill);
        let elapsed = started.elapsed();
        if let Some(mut blocker) = blocker {
            let evidence: TestResult = async {
                let still_held: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT FROM pg_catalog.pg_locks
                     WHERE pid=pg_backend_pid() AND locktype='relation'
                       AND relation='public._sqlx_migrations'::regclass
                       AND mode='AccessExclusiveLock' AND granted)",
                ).fetch_one(&mut *blocker).await?;
                assert!(still_held, "fixture released the ledger lock before process exit");
                databases.crawler.no_runtime_sessions().await?;
                databases.business.no_runtime_sessions().await?;
                Ok(())
            }.await;
            // Release our transaction before snapshots, including when the process failed.
            tokio::time::timeout(Duration::from_secs(5), blocker.rollback()).await??;
            evidence?;
        }
        // Assert cleanup/immutability even when the child timed out or returned a bad exit.
        wires.untouched()?;
        assert_eq!(
            fs::read_dir(&directory.0)?.count(),
            0,
            "preflight created logs/config/files"
        );
        for (db, before) in [&databases.crawler, &databases.business]
            .into_iter()
            .zip(&before)
        {
            db.no_runtime_sessions().await?;
            assert!(
                *before == db.snapshot().await?,
                "preflight mutated tables/history/catalogs; snapshots suppressed"
            );
        }
        let output = output?;
        let stdout = String::from_utf8(output.stdout)?;
        let stderr = String::from_utf8(output.stderr)?;
        assert!(
            !stdout.contains("canary") && !stderr.contains("canary"),
            "process leaked a synthetic secret"
        );
        assert!(!stdout.contains(PASSWORD) && !stderr.contains(PASSWORD));
        if locked {
            // Allow local scheduling/teardown overhead around 500ms, but reject a 2s
            // statement timeout, 5s verifier timeout, or the test's 8s kill fallback.
            assert!(elapsed >= Duration::from_millis(400) && elapsed < Duration::from_secs(2),
                "held-ledger check did not fail at the bounded lock wait: {elapsed:?}");
            println!("held crawler ledger lock: exit after {}ms; blocker retained through exit; no application backends", elapsed.as_millis());
        }
        if let Some(expected) = case.expected_error() {
            assert_eq!(
                output.status.code(),
                Some(1),
                "{case:?}: wrong exit classification; child output suppressed"
            );
            assert!(
                stdout.is_empty(),
                "failed check emitted success/runtime output"
            );
            assert!(
                stderr.starts_with("crawler server: ") && stderr.contains(&expected),
                "{case:?}: wrong failure boundary; child output suppressed"
            );
        } else {
            assert_eq!(output.status.code(), Some(0), "{case:?}: wrong success exit; child output suppressed");
            assert!(
                stderr.is_empty(),
                "successful check emitted runtime/error logging"
            );
            assert_eq!(stdout.lines().count(), 1);
            assert!(
                stdout.contains("Configuration and both database histories verified read-only")
            );
            assert!(stdout.contains("Cloud authentication NOT verified. No daemon started."));
            let commit = option_env!("COMMIT_SHA").ok_or("build with fixture COMMIT_SHA")?;
            assert!(
                stdout.contains(commit),
                "binary did not report the fixture build identifier"
            );
        }
        Ok(())
    }
    .await;
    directory.close()?;
    result
}

async fn run_case(case: Case, admin: &PgPool, port: u16) -> TestResult {
    let mut databases = Databases::new(port)?;
    let work = AssertUnwindSafe(async {
        databases.prepare(admin).await?;
        check(case, &databases).await
    })
    .catch_unwind();
    let result = tokio::time::timeout(Duration::from_secs(30), work).await;
    let cleanup = databases.close(admin).await;
    cleanup?;
    match result {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(TestError::failure("CASE_ASSERTION")),
        Err(error) => Err(TestError::caused("CASE_DEADLINE", error)),
    }
}

fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case::Valid,
        Case::CatalogSearchPath,
        Case::MissingConfig("LOCAL_DB_URL"),
        Case::MissingConfig("BUSINESS_DATABASE_URL"),
        Case::SwappedTargets,
    ];
    for target in [Target::Crawler, Target::Business] {
        for fault in [
            Fault::MissingLedger,
            Fault::MissingVersion,
            Fault::DirtyHistory,
            Fault::AlteredChecksum,
            Fault::UnknownVersion,
            Fault::WrongHistory,
            Fault::MissingTable,
            Fault::DeniedLedger,
            Fault::DeniedSchema,
            Fault::NoLogin,
        ] {
            cases.push(Case::Fault(target, fault));
        }
    }
    for fault in [Fault::LedgerView, Fault::LedgerRls, Fault::LedgerLocked] {
        cases.push(Case::Fault(Target::Crawler, fault));
    }
    cases
}

#[test]
#[ignore = "requires isolated local Unix Docker and the shipped cached pg_ttl image; no pull; run this parent only"]
fn should_check_actual_server_with_readonly_postgres_and_owned_cleanup() -> TestResult {
    support::supervise_fixture()
}

#[tokio::test]
#[ignore = "internal fixture child; run only through should_check_actual_server_with_readonly_postgres_and_owned_cleanup"]
async fn should_run_owned_postgres_preflight_child() -> TestResult {
    let directory = std::env::var_os("AURA_CRAWLER_PREFLIGHT_PARENT")
        .ok_or("internal entry requires its cleanup-verifying parent")?;
    assert!(
        std::env::var("AURA_TEST_POSTGRES_IMAGE")? == support::IMAGE.trim(),
        "fixture image guard mismatch; value suppressed"
    );
    // This entry runs alone in the supervised child. Library panic payloads are not diagnostics.
    std::panic::set_hook(Box::new(|_| {
        eprintln!("fixture assertion failed: CASE_ASSERTION")
    }));
    let admin =
        tokio::time::timeout(Duration::from_secs(80), test_api::get_postgres_client()).await?;
    let port = admin.connect_options().get_port();
    let result: TestResult = async {
        support::observe_started_fixture(Path::new(&directory), port)?;
        // libtest's progress prefix has no newline before the first test-owned report.
        println!();
        let cases = cases();
        let count = cases.len();
        let mut failures = Vec::new();
        for case in cases {
            match run_case(case, &admin, port).await {
                Ok(()) => println!("{}", case.pass_report()),
                Err(error) => {
                    eprintln!("FAIL {case:?}: {}", error.kind());
                    failures.push(error);
                }
            }
        }
        if !failures.is_empty() {
            return Err(TestError::failures("CASES_FAILED", failures));
        }
        println!("PASS all {count} actual-server PostgreSQL cases");
        Ok(())
    }
    .await;
    tokio::time::timeout(Duration::from_secs(5), admin.close()).await?;
    result
}
