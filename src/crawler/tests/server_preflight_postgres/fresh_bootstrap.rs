//! Iteration05 black-box CLI tests. Same sequential, cached Unix-Docker fixture;
//! only newly acquired template0 databases/roles. Never reset accepted fixtures.
//! Initializer roles are fixture-only superusers for conservative catalog inspection;
//! verification demotes only that owned role and denies all advisory functions.
use super::{Database, Databases, PASSWORD};
use crate::{
    Target,
    error::{TestError, TestResult},
    process::{CleanupMode, Process, run_process},
    support::Directory,
};
use futures::FutureExt;
use serde_json::Value;
use sqlx::{AssertSqlSafe, PgPool, migrate::Migrate};
use std::{
    io,
    net::TcpListener,
    panic::AssertUnwindSafe,
    path::Path,
    process::{Command, Output},
    time::{Duration, Instant},
};
use tokio::time::timeout;

#[path = "fresh_bootstrap_ttl.rs"]
mod ttl_faults;
use ttl_faults::TtlFault;

const EXIT: Duration = Duration::from_secs(8);
const SQL: Duration = Duration::from_secs(5);
const CLI_FAILURES: &[&str] = &[
    "NOT_FRESH",
    "DEPENDENCY_FAILED",
    "UNKNOWN_OUTCOME",
    "PREREQUISITE_MISSING",
];
const EVIDENCE: &[&str] = &[
    "unknown_schemas",
    "ttl_preloaded",
    "ttl_public",
    "pg_class",
    "pg_type",
    "pg_proc",
    "pg_constraint",
    "pg_attrdef",
    "pg_trigger",
    "pg_rewrite",
    "snapshot_unchanged",
];

#[derive(Clone, Copy, Debug)]
pub(crate) enum Case {
    Preconditions,
    Success(Target),
    VerifyBaseline(Target),
    EmptyLedger(Target),
    Table(Target),
    Schema(Target),
    Extension(Target),
    SqlxLocked,
    Contention,
    PostwriteFailure,
    TtlFault(TtlFault),
}
pub(crate) fn cases() -> Vec<Case> {
    let mut cases = vec![Case::Preconditions];
    for target in [Target::Business, Target::Crawler] {
        cases.extend([
            Case::Success(target),
            Case::VerifyBaseline(target),
            Case::EmptyLedger(target),
            Case::Table(target),
            Case::Schema(target),
            Case::Extension(target),
        ]);
    }
    cases.extend([Case::SqlxLocked, Case::Contention, Case::PostwriteFailure]);
    cases.extend(TtlFault::ALL.map(Case::TtlFault));
    cases
}

pub(crate) fn reports(text: &str) -> Vec<String> {
    cases()
        .into_iter()
        .map(|case| {
            let pass = format!("PASS fresh bootstrap {case:?}");
            let failure = format!("FAIL fresh bootstrap {case:?}: ");
            if let Some(kind) = text.lines().find_map(|line| line.strip_prefix(&failure)) {
                let kind = CLI_FAILURES
                    .iter()
                    .copied()
                    .find(|known| *known == kind)
                    .unwrap_or_else(|| TestError::report_kind(kind));
                format!("{failure}{kind}")
            } else if text.lines().any(|line| line == pass) {
                pass
            } else {
                format!("UNREPORTED fresh bootstrap {case:?}")
            }
        })
        .collect()
}

/// Parent rebuilds numeric fixture evidence only, never relays captured child text.
pub(crate) fn evidence_reports(text: &str) -> Vec<String> {
    let mut reports = Vec::new();
    for case in cases() {
        let prefix = format!("fresh bootstrap {case:?}: ");
        if let Some(ms) = text.lines().find_map(|line| {
            line.strip_prefix(&prefix)
                .and_then(|value| value.strip_suffix("ms"))
                .and_then(|value| value.parse::<u64>().ok())
        }) {
            reports.push(format!("{prefix}{ms}ms"));
        }
    }
    for key in EVIDENCE {
        let prefix = format!("EVIDENCE fresh business {key}=");
        if let Some(count) = text.lines().find_map(|line| {
            line.strip_prefix(&prefix)
                .and_then(|value| value.parse::<u64>().ok())
        }) {
            reports.push(format!("{prefix}{count}"));
        }
    }
    reports
}

// Read-only reproduction of the runtime's extension-closure policy on the failing
// owned target. Counts only: no catalog payloads/identifiers or server errors escape.
async fn business_evidence(database: &Database) -> TestResult {
    for (key, sql) in [
        (
            "unknown_schemas",
            "SELECT count(*) FROM pg_catalog.pg_namespace WHERE nspname NOT IN ('pg_catalog','information_schema','pg_toast','public')",
        ),
        (
            "ttl_preloaded",
            "SELECT count(*) FROM unnest(string_to_array(current_setting('shared_preload_libraries'),',')) s WHERE trim(s)='pg_ttl_index'",
        ),
        (
            "ttl_public",
            "SELECT count(*) FROM pg_catalog.pg_extension e JOIN pg_catalog.pg_namespace n ON n.oid=e.extnamespace WHERE e.extname='pg_ttl_index' AND n.nspname='public'",
        ),
    ] {
        let count: i64 = timeout(
            SQL,
            sqlx::query_scalar(AssertSqlSafe(sql)).fetch_one(&database.pool),
        )
        .await??;
        println!("EVIDENCE fresh business {key}={count}");
    }
    for catalog in [
        "pg_class",
        "pg_type",
        "pg_proc",
        "pg_constraint",
        "pg_attrdef",
        "pg_trigger",
        "pg_rewrite",
    ] {
        let count: i64 = timeout(SQL, sqlx::query_scalar(AssertSqlSafe(format!(
            "WITH RECURSIVE allowed(classid,objid) AS (
                SELECT 'pg_catalog.pg_extension'::regclass::oid, oid FROM pg_catalog.pg_extension
                UNION
                SELECT d.classid,d.objid FROM pg_catalog.pg_depend d
                JOIN allowed a ON a.classid=d.refclassid AND a.objid=d.refobjid
                WHERE d.objsubid=0 AND d.refobjsubid=0
                  AND (d.deptype='i' OR (d.deptype='e' AND a.classid='pg_catalog.pg_extension'::regclass))
             ) SELECT count(*) FROM pg_catalog.{catalog} o WHERE o.oid>=16384
               AND NOT EXISTS(SELECT FROM allowed a WHERE a.classid='pg_catalog.{catalog}'::regclass AND a.objid=o.oid)",
        ))).fetch_one(&database.pool)).await??;
        println!("EVIDENCE fresh business {catalog}={count}");
    }
    Ok(())
}

fn key(target: Target) -> &'static str {
    match target {
        Target::Business => "BUSINESS_DATABASE_URL",
        Target::Crawler => "LOCAL_DB_URL",
    }
}

fn command(directory: &Path, mode: &str, target: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bootstrap-local"));
    command
        .env_clear()
        .current_dir(directory)
        .env("PATH", directory)
        .env("HOME", directory)
        .env("TMPDIR", directory)
        .env("STAGE", "test")
        .env("POSTGRES_SSL_MODE", "disable")
        .args([mode, target]);
    command
}

fn selected(directory: &Path, database: &Database, target: Target, mode: &str) -> Command {
    let mut command = command(directory, mode, target.label());
    // Other stream URL deliberately absent; no ambient provider/config inputs.
    command.env(key(target), database.url());
    command
}

fn outcome(output: &Output, code: i32, text: &str) -> TestResult {
    // Never format captured output, URLs, SQL, or provider bodies in diagnostics.
    if output.status.code() != Some(code)
        || output.stdout != format!("{text}\n").as_bytes()
        || !output.stderr.is_empty()
    {
        // Only the frozen CLI taxonomy can enter evidence reports.
        for label in CLI_FAILURES {
            if output.stdout == format!("{label}\n").as_bytes() {
                return Err(TestError::failure(label));
            }
        }
        return Err(TestError::failure("CASE_ASSERTION"));
    }
    Ok(())
}

async fn snapshot(database: &Database) -> TestResult<Value> {
    timeout(SQL, async {
        let state = database.snapshot().await?;
        let schemas: Value = sqlx::query_scalar(
            "SELECT coalesce(jsonb_agg(to_jsonb(n) ORDER BY n.oid),'[]'::jsonb) FROM pg_catalog.pg_namespace n",
        ).fetch_one(&database.pool).await?;
        // Stable catalog definitions only: no relpages/reltuples/visibility counts,
        // freeze horizons or pg_stat_* runtime statistics. Keep global Database.snapshot unchanged.
        let catalogs: Value = sqlx::query_scalar(
            "SELECT jsonb_build_object(
                'pg_class', (SELECT coalesce(jsonb_agg(to_jsonb(c) ORDER BY c.oid),'[]'::jsonb) FROM (
                    SELECT oid, relname, relnamespace, reltype, reloftype, relowner, relam,
                        reltablespace, reltoastrelid, relhasindex, relisshared, relpersistence,
                        relkind, relnatts, relchecks, relhasrules, relhastriggers, relhassubclass,
                        relrowsecurity, relforcerowsecurity, relispopulated, relreplident,
                        relispartition, relrewrite, relacl, reloptions, relpartbound
                    FROM pg_catalog.pg_class WHERE oid >= 16384) c),
                'pg_attribute', (SELECT coalesce(jsonb_agg(to_jsonb(a) ORDER BY a.attrelid,a.attnum),'[]'::jsonb)
                    FROM pg_catalog.pg_attribute a WHERE a.attrelid >= 16384),
                'pg_attrdef', (SELECT coalesce(jsonb_agg(to_jsonb(d) ORDER BY d.oid),'[]'::jsonb)
                    FROM pg_catalog.pg_attrdef d WHERE d.adrelid >= 16384),
                'pg_depend', (SELECT coalesce(jsonb_agg(to_jsonb(d) ORDER BY d.classid,d.objid,d.objsubid,
                        d.refclassid,d.refobjid,d.refobjsubid,d.deptype),'[]'::jsonb)
                    FROM pg_catalog.pg_depend d WHERE d.objid >= 16384 OR d.refobjid >= 16384),
                'pg_extension', (SELECT coalesce(jsonb_agg(to_jsonb(e) ORDER BY e.oid),'[]'::jsonb)
                    FROM pg_catalog.pg_extension e),
                'pg_constraint', (SELECT coalesce(jsonb_agg(to_jsonb(c) ORDER BY c.oid),'[]'::jsonb)
                    FROM pg_catalog.pg_constraint c WHERE c.oid >= 16384),
                'pg_index', (SELECT coalesce(jsonb_agg(to_jsonb(i) ORDER BY i.indexrelid),'[]'::jsonb)
                    FROM pg_catalog.pg_index i WHERE i.indexrelid >= 16384))"
        ).fetch_one(&database.pool).await?;
        Ok(serde_json::json!({"state": state, "schemas": schemas, "catalogs": catalogs}))
    }).await?
}

async fn unchanged(database: &Database, before: &Value) -> TestResult {
    timeout(SQL, database.no_runtime_sessions()).await??;
    if *before != snapshot(database).await? {
        return Err(TestError::failure("CASE_ASSERTION"));
    }
    Ok(())
}

async fn prepare(database: &mut Database, admin: &PgPool, target: Target) -> TestResult {
    // Set capability flags immediately after each acknowledged acquisition. Cleanup is
    // the existing exact-name, no-FORCE owner; no lookup ever confers deletion authority.
    timeout(
        SQL,
        sqlx::raw_sql(AssertSqlSafe(format!(
            "CREATE DATABASE {} TEMPLATE template0",
            database.name,
        )))
        .execute(admin),
    )
    .await??;
    database.created = true;
    timeout(
        SQL,
        sqlx::raw_sql(AssertSqlSafe(format!(
            "CREATE ROLE {} LOGIN SUPERUSER PASSWORD '{PASSWORD}'",
            database.role,
        )))
        .execute(admin),
    )
    .await??;
    database.role_created = true;
    timeout(SQL, sqlx::raw_sql(AssertSqlSafe(format!(
        "ALTER DATABASE {} SET statement_timeout='5s'; ALTER DATABASE {} SET lock_timeout='500ms'",
        database.name, database.name,
    ))).execute(admin)).await??;
    if matches!(target, Target::Business) {
        timeout(
            SQL,
            sqlx::raw_sql("CREATE EXTENSION pg_ttl_index").execute(&database.pool),
        )
        .await??;
    }
    Ok(())
}

async fn invoke(
    database: &Database,
    directory: &Path,
    target: Target,
    mode: &str,
    code: i32,
    text: &str,
) -> TestResult {
    let result = run_process(
        &mut selected(directory, database, target, mode),
        EXIT,
        CleanupMode::Kill,
    );
    timeout(SQL, database.no_runtime_sessions()).await??;
    outcome(&result?, code, text)
}

async fn ledger(database: &Database, target: Target) -> TestResult {
    let source = match target {
        Target::Business => sqlx::migrate!("../../migrations"),
        Target::Crawler => sqlx::migrate!("./migrations"),
    };
    let rows: Vec<(i64, String, bool, Vec<u8>)> = timeout(SQL, sqlx::query_as(
        "SELECT version, description, success, checksum FROM public._sqlx_migrations ORDER BY version",
    ).fetch_all(&database.pool)).await??;
    assert_eq!(rows.len(), source.iter().count());
    for ((version, description, success, checksum), migration) in rows.iter().zip(source.iter()) {
        assert!(
            *version == migration.version
                && description == &*migration.description
                && *success
                && checksum == &*migration.checksum,
            "actual SQLx history differs from embedded source; values suppressed"
        );
    }
    Ok(())
}

async fn restrict_verify(database: &Database, admin: &PgPool) -> TestResult {
    timeout(
        SQL,
        sqlx::raw_sql(AssertSqlSafe(format!(
        "ALTER ROLE {role} NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS;
         ALTER ROLE {role} SET default_transaction_read_only=on;
         GRANT CONNECT ON DATABASE {database} TO {role};
         GRANT USAGE ON SCHEMA public TO {role};
         GRANT SELECT ON public._sqlx_migrations TO {role}",
        role=database.role, database=database.name,
    )))
        .execute(&database.pool),
    )
    .await??;
    let functions: Vec<String> = timeout(
        SQL,
        sqlx::query_scalar(
            "SELECT oid::regprocedure::text FROM pg_catalog.pg_proc
         WHERE pronamespace='pg_catalog'::regnamespace AND proname LIKE 'pg%advisory%'",
        )
        .fetch_all(&database.pool),
    )
    .await??;
    assert!(!functions.is_empty());
    for function in functions {
        timeout(SQL, sqlx::raw_sql(AssertSqlSafe(format!(
            "REVOKE EXECUTE ON FUNCTION {function} FROM PUBLIC; REVOKE EXECUTE ON FUNCTION {function} FROM {}", database.role,
        ))).execute(&database.pool)).await??;
    }
    let forbidden: bool = timeout(SQL, sqlx::query_scalar(
        "SELECT EXISTS(SELECT FROM pg_catalog.pg_proc WHERE pronamespace='pg_catalog'::regnamespace
         AND proname LIKE 'pg%advisory%' AND has_function_privilege($1,oid,'EXECUTE'))",
    ).bind(&database.role).fetch_one(&database.pool)).await??;
    assert!(!forbidden, "verify role can acquire advisory locks");
    let readonly: bool = timeout(SQL, sqlx::query_scalar(
        "SELECT EXISTS(SELECT FROM pg_catalog.pg_db_role_setting s JOIN pg_catalog.pg_roles r ON r.oid=s.setrole
         WHERE r.rolname=$1 AND 'default_transaction_read_only=on'=ANY(s.setconfig))",
    ).bind(&database.role).fetch_one(admin)).await??;
    assert!(readonly);
    Ok(())
}

async fn success(
    database: &Database,
    admin: &PgPool,
    directory: &Path,
    target: Target,
) -> TestResult {
    let fresh = snapshot(database).await?;
    invoke(
        database,
        directory,
        target,
        "--verify",
        5,
        "VERIFICATION_FAILED",
    )
    .await?;
    unchanged(database, &fresh).await?;
    let initialized = match target {
        Target::Business => "INITIALIZED_BUSINESS",
        Target::Crawler => "INITIALIZED_CRAWLER",
    };
    let initialization = invoke(
        database,
        directory,
        target,
        "--initialize-fresh",
        0,
        initialized,
    )
    .await;
    if initialization.is_err() {
        unchanged(database, &fresh).await?;
        if matches!(target, Target::Business) {
            println!("EVIDENCE fresh business snapshot_unchanged=1");
            business_evidence(database).await?;
        }
        return initialization;
    }
    ledger(database, target).await?;
    let initialized_state = snapshot(database).await?;
    invoke(
        database,
        directory,
        target,
        "--initialize-fresh",
        4,
        "NOT_FRESH",
    )
    .await?;
    unchanged(database, &initialized_state).await?;
    verify(database, admin, directory, target).await
}

async fn verify(
    database: &Database,
    admin: &PgPool,
    directory: &Path,
    target: Target,
) -> TestResult {
    restrict_verify(database, admin).await?;
    let before = snapshot(database).await?;
    let verified = match target {
        Target::Business => "VERIFIED_BUSINESS",
        Target::Crawler => "VERIFIED_CRAWLER",
    };
    invoke(database, directory, target, "--verify", 0, verified).await?;
    unchanged(database, &before).await?;
    // Actual availability gate, not just checksum comparison. Only this newly owned DB.
    timeout(
        SQL,
        sqlx::raw_sql("ALTER TABLE public.listing_sources RENAME TO missing_listing_sources")
            .execute(&database.pool),
    )
    .await??;
    let missing = snapshot(database).await?;
    invoke(
        database,
        directory,
        target,
        "--verify",
        5,
        "VERIFICATION_FAILED",
    )
    .await?;
    unchanged(database, &missing).await
}

fn preconditions(directory: &Path) -> TestResult {
    // Synchronous process polling cannot be preempted by the outer Tokio timeout.
    // Share one absolute budget across this multi-command precondition matrix.
    let deadline = Instant::now() + EXIT;
    let run = |command: &mut Command| {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| TestError::failure("CASE_DEADLINE"))?;
        run_process(command, remaining, CleanupMode::Kill)
    };
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!(
        "postgres://fixture:{PASSWORD}@{}/fixture",
        listener.local_addr()?
    );
    for mode in ["--initialize-fresh", "--verify"] {
        for target in [Target::Business, Target::Crawler] {
            for stage in [None, Some("dev"), Some("prod"), Some("unknown")] {
                let mut child = command(directory, mode, target.label());
                child.env(key(target), &url);
                if let Some(stage) = stage {
                    child.env("STAGE", stage);
                } else {
                    child.env_remove("STAGE");
                }
                let output = run(&mut child)?;
                outcome(
                    &output,
                    3,
                    if stage.is_none() {
                        "CONFIG_ERROR"
                    } else {
                        "UNSUPPORTED_STAGE"
                    },
                )?;
            }
            let mut missing = command(directory, mode, target.label());
            let other = match target {
                Target::Business => Target::Crawler,
                Target::Crawler => Target::Business,
            };
            missing.env(key(other), &url);
            outcome(&run(&mut missing)?, 3, "CONFIG_ERROR")?;
        }
        let mut invalid = command(directory, mode, "both");
        invalid
            .env("LOCAL_DB_URL", &url)
            .env("BUSINESS_DATABASE_URL", &url);
        outcome(&run(&mut invalid)?, 2, "USAGE_ERROR")?;
    }
    for target in [Target::Business, Target::Crawler] {
        let mut remote = command(directory, "--initialize-fresh", target.label());
        // Reserved .invalid: never a live destination; refusal must precede DNS/connect.
        remote.env(
            key(target),
            format!("postgres://fixture:{PASSWORD}@fresh-bootstrap.invalid/fixture"),
        );
        outcome(&run(&mut remote)?, 3, "NONLOCAL_ENDPOINT")?;
    }
    assert!(
        matches!(listener.accept(), Err(error) if error.kind()==io::ErrorKind::WouldBlock),
        "precondition failure contacted database tripwire"
    );
    Ok(())
}

async fn locks(database: &Database, directory: &Path, contention: bool) -> TestResult {
    let before = snapshot(database).await?;
    let mut blocker = timeout(SQL, database.pool.acquire()).await??;
    timeout(SQL, Migrate::lock(&mut *blocker)).await??;
    // Observe SQLx's actual key, never duplicate its implementation/hash in the test.
    let blocker_pid: i32 = timeout(
        SQL,
        sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *blocker),
    )
    .await??;
    let started = Instant::now();
    let mut first = Process::spawn(
        &mut selected(directory, database, Target::Crawler, "--initialize-fresh"),
        EXIT,
        CleanupMode::Kill,
    )?;
    let work: TestResult = async {
        timeout(Duration::from_secs(3), async {
            loop {
                if first.try_reap()?.is_some() { return Err(TestError::failure("CASE_ASSERTION")); }
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT FROM pg_catalog.pg_locks w JOIN pg_catalog.pg_locks b
                     ON w.database=b.database AND w.classid=b.classid AND w.objid=b.objid AND w.objsubid=b.objsubid
                     JOIN pg_catalog.pg_stat_activity a ON a.pid=w.pid
                     WHERE b.pid=$1 AND b.locktype='advisory' AND b.granted AND NOT w.granted
                     AND a.datname=$2 AND a.usename=$3)",
                ).bind(blocker_pid).bind(&database.name).bind(&database.role).fetch_one(&database.pool).await?;
                if waiting { return Ok(()); }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await??;
        if contention {
            let second = run_process(&mut selected(directory, database, Target::Crawler, "--initialize-fresh"), EXIT, CleanupMode::Kill)?;
            outcome(&second, 5, "DEPENDENCY_FAILED")?;
            assert!(before == snapshot(database).await?, "contender mutated fresh target");
            timeout(SQL, Migrate::unlock(&mut *blocker)).await??;
            outcome(&first.finish(EXIT)?, 0, "INITIALIZED_CRAWLER")?;
            ledger(database, Target::Crawler).await?;
        } else {
            outcome(&first.finish(EXIT)?, 5, "DEPENDENCY_FAILED")?;
            let elapsed = started.elapsed();
            assert!(elapsed >= Duration::from_millis(1800) && elapsed < Duration::from_secs(4),
                "SQLx lock timeout outside expected bound");
            let held: bool = timeout(SQL, sqlx::query_scalar(
                "SELECT EXISTS(SELECT FROM pg_catalog.pg_locks WHERE pid=pg_backend_pid() AND locktype='advisory' AND granted)",
            ).fetch_one(&mut *blocker)).await??;
            assert!(held, "SQLx blocker released before child exit");
        }
        Ok(())
    }.await;
    // Close exact blocker even on assertion/error; Process Drop owns partial child cleanup.
    drop(first);
    timeout(SQL, blocker.close()).await??;
    timeout(SQL, database.no_runtime_sessions()).await??;
    if !contention {
        unchanged(database, &before).await?;
    }
    work
}

async fn postwrite(database: &Database, directory: &Path) -> TestResult {
    // Allow conservative catalog inspection and ledger creation, but deny trusted
    // extension installation. SQLx creates its ledger before crawler migration #1.
    timeout(
        SQL,
        sqlx::raw_sql(AssertSqlSafe(format!(
            "ALTER ROLE {role} NOSUPERUSER;
         GRANT SELECT ON ALL TABLES IN SCHEMA pg_catalog TO {role};
         GRANT USAGE, CREATE ON SCHEMA public TO {role};
         REVOKE CREATE ON DATABASE {database} FROM PUBLIC, {role}",
            role = database.role,
            database = database.name,
        )))
        .execute(&database.pool),
    )
    .await??;
    invoke(
        database,
        directory,
        Target::Crawler,
        "--initialize-fresh",
        7,
        "UNKNOWN_OUTCOME",
    )
    .await?;
    let count: i64 = timeout(
        SQL,
        sqlx::query_scalar("SELECT count(*) FROM public._sqlx_migrations")
            .fetch_one(&database.pool),
    )
    .await??;
    assert_eq!(count, 0, "failed migration unexpectedly recorded success");
    let after = snapshot(database).await?;
    invoke(
        database,
        directory,
        Target::Crawler,
        "--initialize-fresh",
        4,
        "NOT_FRESH",
    )
    .await?;
    unchanged(database, &after).await
}

pub(crate) async fn run_case(case: Case, admin: &PgPool, port: u16) -> TestResult {
    let directory = Directory::new()?;
    let mut databases = Databases::new(port)?;
    let target = match case {
        Case::Success(t)
        | Case::VerifyBaseline(t)
        | Case::EmptyLedger(t)
        | Case::Table(t)
        | Case::Schema(t)
        | Case::Extension(t) => t,
        Case::TtlFault(_) => Target::Business,
        _ => Target::Crawler,
    };
    let work = AssertUnwindSafe(async {
        if matches!(case, Case::Preconditions) { return preconditions(&directory.0); }
        let database = match target { Target::Business => &mut databases.business, Target::Crawler => &mut databases.crawler };
        prepare(database, admin, target).await?;
        match case {
            Case::Success(_) => success(database, admin, &directory.0, target).await,
            Case::VerifyBaseline(_) => {
                // Independent verification proof even if fresh initialization is blocked.
                // Genuine SQLx baselines on this new DB; never a fallback for Success.
                let source = match target {
                    Target::Business => sqlx::migrate!("../../migrations"),
                    Target::Crawler => sqlx::migrate!("./migrations"),
                };
                timeout(SQL, source.run(&database.pool)).await??;
                ledger(database, target).await?;
                verify(database, admin, &directory.0, target).await
            }
            Case::SqlxLocked => locks(database, &directory.0, false).await,
            Case::Contention => locks(database, &directory.0, true).await,
            Case::PostwriteFailure => postwrite(database, &directory.0).await,
            Case::TtlFault(fault) => fault.check(database, &directory.0).await,
            _ => {
                let sql = match case {
                    Case::EmptyLedger(_) => "CREATE TABLE public._sqlx_migrations (version bigint PRIMARY KEY, description text NOT NULL, installed_on timestamptz NOT NULL DEFAULT now(), success boolean NOT NULL, checksum bytea NOT NULL, execution_time bigint NOT NULL)",
                    Case::Table(_) => "CREATE TABLE public.fresh_canary (value text)",
                    Case::Schema(_) => "CREATE SCHEMA unknown_fresh_canary",
                    Case::Extension(Target::Business) => "CREATE EXTENSION pgcrypto",
                    Case::Extension(Target::Crawler) => "CREATE EXTENSION unaccent",
                    _ => return Err(TestError::failure("CASE_ASSERTION")),
                };
                timeout(SQL, sqlx::raw_sql(AssertSqlSafe(sql)).execute(&database.pool)).await??;
                let before = snapshot(database).await?;
                let result = invoke(database, &directory.0, target, "--initialize-fresh", 4, "NOT_FRESH").await;
                unchanged(database, &before).await?;
                result
            }
        }
    }).catch_unwind();
    let result = timeout(Duration::from_secs(30), work).await;
    // Existing no-FORCE cleanup, additionally bounded for this new suite only.
    let cleanup = timeout(Duration::from_secs(15), databases.close(admin)).await;
    let empty = std::fs::read_dir(&directory.0)?.count() == 0;
    directory.close()?;
    cleanup??;
    assert!(empty, "bootstrap created local files");
    match result {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(TestError::failure("CASE_ASSERTION")),
        Err(error) => Err(TestError::caused("CASE_DEADLINE", error)),
    }
}

#[test]
fn should_reconstruct_only_owned_fresh_reports_without_provider_output() {
    let reports = reports(
        "FAIL fresh bootstrap Preconditions: private-provider-body\nPASS fresh bootstrap Success(Business)\n",
    );
    assert_eq!(reports.len(), 30);
    assert_eq!(
        reports[0],
        "FAIL fresh bootstrap Preconditions: UNRECOGNIZED_CLASSIFICATION"
    );
    assert_eq!(reports[1], "PASS fresh bootstrap Success(Business)");
    assert_eq!(
        evidence_reports(
            "fresh bootstrap Preconditions: 123ms\nEVIDENCE fresh business pg_class=2\nEVIDENCE fresh business pg_proc=private-provider-body\n"
        ),
        vec![
            "fresh bootstrap Preconditions: 123ms",
            "EVIDENCE fresh business pg_class=2"
        ]
    );
    assert!(
        reports
            .iter()
            .all(|report| !report.contains("private-provider-body"))
    );
}
