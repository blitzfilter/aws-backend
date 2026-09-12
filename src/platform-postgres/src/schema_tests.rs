use super::*;
use crate::test_support::{TestResult, assert_redacted_chain};
use rstest::rstest;
use std::{error::Error, time::Instant};

#[path = "schema_fixture.rs"]
mod fixture;
use fixture::SchemaFixture;

const CANARY: &str = "postgres://schema-private-user:secret-canary@private-host/private-db";

fn applied_history() -> Vec<AppliedMigrationRow> {
    expected_migrations()
        .map(|migration| AppliedMigrationRow {
            version: migration.version,
            success: true,
            checksum: migration.checksum.to_vec(),
        })
        .collect()
}

#[test]
fn should_accept_exact_compiled_history() -> TestResult {
    verify_history(&applied_history())?;
    Ok(())
}

#[rstest]
#[case("missing", "SCHEMA_HISTORY_MISSING")]
#[case("dirty", "SCHEMA_HISTORY_DIRTY")]
#[case("changed", "SCHEMA_HISTORY_MISMATCH")]
#[case("long-checksum", "SCHEMA_HISTORY_MISMATCH")]
#[case("unknown", "SCHEMA_HISTORY_UNKNOWN")]
#[case("extra", "SCHEMA_HISTORY_UNKNOWN")]
#[case("duplicate", "SCHEMA_HISTORY_UNKNOWN")]
fn should_reject_nonexact_history(#[case] change: &str, #[case] code: &str) -> TestResult {
    let mut history = applied_history();
    let first = history.first_mut().ok_or("compiled baseline missing")?;
    match change {
        "missing" => history.clear(),
        "dirty" => first.success = false,
        "changed" => first.checksum.clear(),
        "long-checksum" => first.checksum.push(0),
        "unknown" => first.version = i64::MAX,
        "extra" => history.push(AppliedMigrationRow {
            version: i64::MAX,
            success: true,
            checksum: vec![0; 48],
        }),
        "duplicate" => history.extend(applied_history()),
        _ => return Err("unknown test case".into()),
    }
    let error = verify_history(&history)
        .err()
        .ok_or("nonexact history accepted")?;
    assert_eq!(error.code(), code);
    assert!(error.source().is_none());
    Ok(())
}

#[test]
fn should_bind_expectations_to_immutable_baseline_and_sqlx_checksums() -> TestResult {
    let baseline = expected_migrations()
        .next()
        .ok_or("compiled baseline missing")?;
    assert_eq!(baseline.version, 20260725090000);
    assert!(!baseline.no_tx);
    assert_eq!(baseline.checksum.len(), 48);
    let recomputed = sqlx::migrate::Migration::new(
        baseline.version,
        baseline.description.clone(),
        baseline.migration_type,
        baseline.sql.clone(),
        baseline.no_tx,
    );
    assert_eq!(baseline.checksum, recomputed.checksum);
    let tables: Vec<_> = baseline
        .sql
        .as_str()
        .lines()
        .filter_map(|line| line.strip_prefix("CREATE TABLE "))
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(BASELINE_TABLES, tables);
    Ok(())
}

#[rstest]
#[case(sqlx::Error::Protocol(CANARY.into()), "SCHEMA_DEPENDENCY_UNAVAILABLE")]
#[case(
    sqlx::Error::Io(io::Error::other(CANARY)),
    "SCHEMA_DEPENDENCY_UNAVAILABLE"
)]
#[case(
    sqlx::Error::Tls(Box::new(io::Error::other(CANARY))),
    "SCHEMA_DEPENDENCY_UNAVAILABLE"
)]
#[case(
    sqlx::Error::Configuration(Box::new(io::Error::other(CANARY))),
    "SCHEMA_DEPENDENCY_UNAVAILABLE"
)]
#[case(
    sqlx::Error::Decode(Box::new(io::Error::other(CANARY))),
    "SCHEMA_HISTORY_INVALID"
)]
#[case(
    sqlx::Error::Io(io::Error::new(io::ErrorKind::TimedOut, CANARY)),
    "SCHEMA_TIMEOUT"
)]
#[case(sqlx::Error::PoolTimedOut, "SCHEMA_TIMEOUT")]
#[case(sqlx::Error::PoolClosed, "SCHEMA_DEPENDENCY_UNAVAILABLE")]
fn should_preserve_but_redact_dependency_causes(#[case] original: sqlx::Error, #[case] code: &str) {
    let error = PostgresSchemaError::from(original);
    assert_eq!(error.code(), code);
    assert_redacted_chain(&error, &[CANARY, "private-", "secret-canary"]);
}

async fn execute(pool: &PgPool, sql: &'static str) -> TestResult {
    sqlx::raw_sql(sql)
        .execute(pool)
        .await
        .map_err(PostgresSchemaError::from)?;
    Ok(())
}

async fn history_fingerprint(pool: &PgPool) -> Result<Vec<String>, PostgresSchemaError> {
    Ok(sqlx::query_scalar(
        "SELECT pg_catalog.md5(pg_catalog.row_to_json(m)::text || m.xmin::text)
         FROM public._sqlx_migrations m ORDER BY version",
    )
    .fetch_all(pool)
    .await?)
}

async fn assert_rejection(pool: &PgPool, code: &str) -> TestResult {
    let error = verify_business_schema(pool)
        .await
        .err()
        .ok_or("invalid schema accepted")?;
    assert_eq!(error.code(), code);
    if error.source().is_some() {
        assert_redacted_chain(
            &error,
            &[
                CANARY,
                "schema_reader",
                "schema_owner",
                "permission denied",
                "canceling statement",
            ],
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires cached test-api pg_ttl image and local Docker; never pulls"]
async fn should_verify_with_read_only_role_without_history_or_business_writes() -> TestResult {
    let fixture = SchemaFixture::start().await?;
    fixture.migrate().await?;
    execute(&fixture.admin,
        "INSERT INTO public.users (user_id, email, tier, role)
         VALUES ('00000000-0000-0000-0000-000000000001', 'fixture@example.invalid', 'FREE', 'USER')",
    ).await?;
    let reader = fixture.reader_config.connect().await?;
    let read_only: bool = sqlx::query_scalar(
        "SELECT current_setting('default_transaction_read_only') = 'on'
         AND NOT (SELECT rolsuper OR rolcreatedb OR rolcreaterole OR rolbypassrls
                  FROM pg_catalog.pg_roles WHERE rolname = current_user)
         AND NOT has_schema_privilege('public', 'CREATE')
         AND NOT has_table_privilege('public._sqlx_migrations', 'INSERT,UPDATE,DELETE,TRUNCATE')
         AND NOT has_table_privilege('public.users', 'SELECT,INSERT,UPDATE,DELETE,TRUNCATE')",
    )
    .fetch_one(&reader)
    .await
    .map_err(PostgresSchemaError::from)?;
    assert!(read_only);
    let history_before = history_fingerprint(&fixture.admin).await?;
    let business_sql = "SELECT md5(row_to_json(u)::text || u.xmin::text) FROM public.users u";
    let business_before: Vec<String> = sqlx::query_scalar(business_sql)
        .fetch_all(&fixture.admin)
        .await
        .map_err(PostgresSchemaError::from)?;
    for _ in 0..2 {
        verify_business_schema(&reader).await?;
    }
    assert_eq!(history_before, history_fingerprint(&fixture.admin).await?);
    let business_after: Vec<String> = sqlx::query_scalar(business_sql)
        .fetch_all(&fixture.admin)
        .await
        .map_err(PostgresSchemaError::from)?;
    assert_eq!(business_before, business_after);
    reader.close().await;
    fixture.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires cached test-api pg_ttl image and local Docker; never pulls"]
async fn should_reject_absent_history_without_creating_or_stamping_it() -> TestResult {
    let fixture = SchemaFixture::start().await?;
    let reader = fixture.reader_config.connect().await?;
    assert_rejection(&reader, "SCHEMA_HISTORY_MISSING").await?;
    // Even an existing business table must not be mistaken for recorded migration history.
    execute(
        &fixture.admin,
        "CREATE TABLE public.users (canary integer); INSERT INTO public.users VALUES (7)",
    )
    .await?;
    assert_rejection(&reader, "SCHEMA_HISTORY_MISSING").await?;
    // Prove no writes even when the caller has DDL privileges.
    assert_rejection(&fixture.admin, "SCHEMA_HISTORY_MISSING").await?;
    let absent: bool = sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NULL")
        .fetch_one(&fixture.admin)
        .await
        .map_err(PostgresSchemaError::from)?;
    assert!(absent);
    let canaries: Vec<i32> = sqlx::query_scalar("SELECT canary FROM public.users")
        .fetch_all(&fixture.admin)
        .await
        .map_err(PostgresSchemaError::from)?;
    assert_eq!(canaries, [7]);
    reader.close().await;
    fixture.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires cached test-api pg_ttl image and local Docker; never pulls"]
async fn should_reject_real_missing_dirty_mismatched_unknown_and_hidden_history() -> TestResult {
    let fixture = SchemaFixture::start().await?;
    fixture.migrate().await?;
    execute(
        &fixture.admin,
        "CREATE TABLE public.saved_history AS TABLE public._sqlx_migrations",
    )
    .await?;
    let reader = fixture.reader_config.connect().await?;
    for (sql, code) in [
        (
            "DELETE FROM public._sqlx_migrations",
            "SCHEMA_HISTORY_MISSING",
        ),
        (
            "UPDATE public._sqlx_migrations SET success = false",
            "SCHEMA_HISTORY_DIRTY",
        ),
        (
            "UPDATE public._sqlx_migrations SET checksum = decode('00', 'hex')",
            "SCHEMA_HISTORY_MISMATCH",
        ),
        (
            "UPDATE public._sqlx_migrations SET checksum = checksum || decode('00', 'hex')",
            "SCHEMA_HISTORY_MISMATCH",
        ),
        (
            "INSERT INTO public._sqlx_migrations (version, description, success, checksum, execution_time) VALUES (9223372036854775807, 'future', true, decode('00', 'hex'), 0)",
            "SCHEMA_HISTORY_UNKNOWN",
        ),
        (
            "UPDATE public._sqlx_migrations SET version = 1",
            "SCHEMA_HISTORY_UNKNOWN",
        ),
        (
            "ALTER TABLE public._sqlx_migrations ENABLE ROW LEVEL SECURITY",
            "SCHEMA_HISTORY_INVALID",
        ),
    ] {
        execute(&fixture.admin, sql).await?;
        let before = history_fingerprint(&fixture.admin).await?;
        assert_rejection(&reader, code).await?;
        assert_eq!(before, history_fingerprint(&fixture.admin).await?);
        execute(
            &fixture.admin,
            "ALTER TABLE public._sqlx_migrations DISABLE ROW LEVEL SECURITY;
             DELETE FROM public._sqlx_migrations;
             INSERT INTO public._sqlx_migrations SELECT * FROM public.saved_history",
        )
        .await?;
    }
    // Never execute a substituted ledger view, even if it could fabricate valid rows.
    execute(
        &fixture.admin,
        "ALTER TABLE public._sqlx_migrations RENAME TO real_history;
         CREATE VIEW public._sqlx_migrations AS SELECT * FROM public.real_history",
    )
    .await?;
    assert_rejection(&reader, "SCHEMA_HISTORY_INVALID").await?;
    reader.close().await;
    fixture.close().await?;
    Ok(())
}

#[rstest]
#[case("DROP EXTENSION pg_trgm CASCADE")]
#[case("DROP EXTENSION unaccent CASCADE")]
#[case("DROP EXTENSION pg_ttl_index CASCADE")]
#[case("CREATE SCHEMA misplaced; ALTER EXTENSION pg_trgm SET SCHEMA misplaced")]
#[tokio::test]
#[ignore = "requires cached test-api pg_ttl image and local Docker; never pulls"]
async fn should_reject_required_extension_absent_from_public(
    #[case] sql: &'static str,
) -> TestResult {
    let fixture = SchemaFixture::start().await?;
    fixture.migrate().await?;
    execute(&fixture.admin, sql).await?;
    let reader = fixture.reader_config.connect().await?;
    assert_rejection(&reader, "SCHEMA_EXTENSION_MISSING").await?;
    reader.close().await;
    fixture.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires cached test-api pg_ttl image and local Docker; never pulls"]
async fn should_reject_missing_baseline_and_redact_permission_or_dependency_errors() -> TestResult {
    let fixture = SchemaFixture::start().await?;
    fixture.migrate().await?;
    let reader = fixture.reader_config.connect().await?;
    execute(
        &fixture.admin,
        "REVOKE SELECT ON public._sqlx_migrations FROM schema_reader",
    )
    .await?;
    assert_rejection(&reader, "SCHEMA_PERMISSION_DENIED").await?;
    execute(&fixture.admin, "GRANT SELECT ON public._sqlx_migrations TO schema_reader; DROP TABLE public.user_cognito_identities").await?;
    assert_rejection(&reader, "SCHEMA_BASELINE_MISSING").await?;
    reader.close().await;
    assert_rejection(&reader, "SCHEMA_DEPENDENCY_UNAVAILABLE").await?;
    fixture.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires cached test-api pg_ttl image and local Docker; never pulls"]
async fn should_bound_lock_statement_and_acquire_waits_with_redacted_causes() -> TestResult {
    let fixture = SchemaFixture::start().await?;
    fixture.migrate().await?;
    let reader = fixture
        .reader_config
        .pool_options()
        .acquire_timeout(Duration::from_secs(30))
        .connect_with(fixture.reader_config.connect_options())
        .await
        .map_err(PostgresSchemaError::from)?;
    let mut blocker = fixture
        .admin
        .begin()
        .await
        .map_err(PostgresSchemaError::from)?;
    sqlx::query("LOCK TABLE public._sqlx_migrations IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .map_err(PostgresSchemaError::from)?;
    let started = Instant::now();
    assert_rejection(&reader, "SCHEMA_TIMEOUT").await?;
    assert!(started.elapsed() < Duration::from_secs(3));
    blocker
        .rollback()
        .await
        .map_err(PostgresSchemaError::from)?;
    verify_business_schema(&reader).await?;

    let mut connection = reader.acquire().await.map_err(PostgresSchemaError::from)?;
    let mut snapshot = begin_snapshot(&mut connection).await?;
    let error = sqlx::query("SELECT pg_catalog.pg_sleep(10)")
        .execute(&mut *snapshot)
        .await
        .err()
        .ok_or("statement deadline not enforced")?;
    let error = PostgresSchemaError::from(error);
    assert_eq!(error.code(), "SCHEMA_TIMEOUT");
    assert_redacted_chain(&error, &["canceling statement", "schema_reader"]);
    snapshot
        .rollback()
        .await
        .map_err(PostgresSchemaError::from)?;
    // Pool acquire is configured for 30s: the gate's own 5s deadline must win.
    let started = Instant::now();
    assert_rejection(&reader, "SCHEMA_TIMEOUT").await?;
    assert!(started.elapsed() < Duration::from_secs(7));
    drop(connection);
    reader.close().await;
    fixture.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires cached test-api pg_ttl image and local Docker; never pulls"]
async fn should_keep_checks_in_one_read_only_snapshot() -> TestResult {
    let fixture = SchemaFixture::start().await?;
    fixture.migrate().await?;
    let reader = fixture.reader_config.connect().await?;
    let mut connection = reader.acquire().await.map_err(PostgresSchemaError::from)?;
    let mut snapshot = begin_snapshot(&mut connection).await?;
    let settings: (String, String, String, String) = sqlx::query_as(
        "SELECT current_setting('transaction_read_only'), current_setting('transaction_isolation'),
                current_setting('statement_timeout'), current_setting('lock_timeout')",
    )
    .fetch_one(&mut *snapshot)
    .await
    .map_err(PostgresSchemaError::from)?;
    assert_eq!(
        settings,
        (
            "on".into(),
            "repeatable read".into(),
            "2s".into(),
            "500ms".into()
        )
    );
    verify_snapshot(&mut snapshot).await?;
    execute(
        &fixture.admin,
        "UPDATE public._sqlx_migrations SET success = false",
    )
    .await?;
    verify_snapshot(&mut snapshot).await?;
    snapshot.commit().await.map_err(PostgresSchemaError::from)?;
    drop(connection);
    assert_rejection(&reader, "SCHEMA_HISTORY_DIRTY").await?;
    reader.close().await;
    fixture.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires cached test-api pg_ttl image and local Docker; never pulls"]
async fn should_discard_connection_when_gate_is_cancelled() -> TestResult {
    let fixture = SchemaFixture::start().await?;
    fixture.migrate().await?;
    let reader = fixture.reader_config.connect().await?;
    let mut blocker = fixture
        .admin
        .begin()
        .await
        .map_err(PostgresSchemaError::from)?;
    sqlx::query("LOCK TABLE public._sqlx_migrations IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .map_err(PostgresSchemaError::from)?;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), verify_business_schema(&reader))
            .await
            .is_err()
    );
    reader.close().await;
    blocker
        .rollback()
        .await
        .map_err(PostgresSchemaError::from)?;
    let sessions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_catalog.pg_stat_activity WHERE usename = 'schema_reader'",
    )
    .fetch_one(&fixture.admin)
    .await
    .map_err(PostgresSchemaError::from)?;
    assert_eq!(sessions, 0);
    fixture.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires cached test-api pg_ttl image and local Docker; never pulls"]
async fn should_bound_direct_sqlx_session_locks_without_creating_history() -> TestResult {
    use sqlx::migrate::{Migrate, MigrateError};

    let fixture = SchemaFixture::start().await?;
    let mut first = fixture.reader_config.connect_session().await?;
    let mut second = fixture.reader_config.connect_session().await?;
    for session in [&mut first, &mut second] {
        sqlx::raw_sql("SET lock_timeout = '150ms'; SET statement_timeout = '2s'")
            .execute(session)
            .await
            .map_err(PostgresSchemaError::from)?;
    }
    first
        .lock()
        .await
        .map_err(|_| "first SQLx lock failed (details suppressed)")?;
    let started = Instant::now();
    let error = second
        .lock()
        .await
        .err()
        .ok_or("SQLx did not serialize sessions")?;
    let MigrateError::Execute(original) = error else {
        return Err("unexpected SQLx lock failure category".into());
    };
    assert!(matches!(
        original
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("55P03")
    ));
    let error = PostgresSchemaError::from(original);
    assert_eq!(error.code(), "SCHEMA_TIMEOUT");
    assert_redacted_chain(&error, &["canceling statement", "schema_reader"]);
    assert!(started.elapsed() < Duration::from_secs(2));
    first
        .unlock()
        .await
        .map_err(|_| "SQLx unlock failed (details suppressed)")?;
    second
        .lock()
        .await
        .map_err(|_| "SQLx lock after unlock failed (details suppressed)")?;
    // The lock belongs to the session, not a transaction or a history-table name.
    second.close().await.map_err(PostgresSchemaError::from)?;
    first
        .lock()
        .await
        .map_err(|_| "SQLx lock after disconnect failed (details suppressed)")?;
    first
        .unlock()
        .await
        .map_err(|_| "SQLx unlock failed (details suppressed)")?;
    first.close().await.map_err(PostgresSchemaError::from)?;
    let absent: bool = sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NULL")
        .fetch_one(&fixture.admin)
        .await
        .map_err(PostgresSchemaError::from)?;
    assert!(absent);
    fixture.close().await?;
    Ok(())
}
