use super::fixture::*;
use serde_json::Value;
use sqlx::AssertSqlSafe;
use std::{fs, io, net::TcpListener, process::Command, sync::atomic::Ordering};

struct Tripwires {
    search: TcpListener,
    google: TcpListener,
    health: TcpListener,
}
impl Tripwires {
    fn new() -> TestResult<Self> {
        let wires = Self {
            search: TcpListener::bind("127.0.0.1:0")?,
            google: TcpListener::bind("127.0.0.1:0")?,
            health: TcpListener::bind("127.0.0.1:0")?,
        };
        for listener in [&wires.search, &wires.google, &wires.health] {
            listener.set_nonblocking(true)?;
        }
        Ok(wires)
    }
    fn command(&self, db: &Database, directory: &Directory) -> TestResult<Command> {
        fs::write(
            directory.0.join("invalid-adc.json"),
            b"{ deliberately invalid local ADC",
        )?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_aura-historia-cron"));
        command.arg("--check-config");
        child_environment(&mut command, db, &directory.0, true);
        let proxy = format!("http://{}", self.google.local_addr()?);
        command
            .env(
                "OPENSEARCH_ENDPOINT_URL",
                format!("http://{}", self.search.local_addr()?),
            )
            .env("VERTEX_AI_PROJECT_ID", "cron-project-canary")
            .env("VERTEX_AI_LOCATION", "cron-location-canary")
            .env("VERTEX_AI_MODEL", "cron-model-canary")
            // An occupied valid bind proves check-config does not start health either.
            .env(
                "AURA_HISTORIA_CRON_HEALTH_BIND_ADDR",
                self.health.local_addr()?.to_string(),
            )
            .env("GCE_METADATA_HOST", self.google.local_addr()?.to_string())
            .env("GCE_METADATA_IP", "127.0.0.1")
            .env("NO_PROXY", "")
            .env("no_proxy", "");
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env(key, &proxy);
        }
        Ok(command)
    }
    fn untouched(&self) -> TestResult {
        for listener in [&self.search, &self.google, &self.health] {
            assert!(
                matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock),
                "preflight reached a non-PostgreSQL tripwire"
            );
        }
        Ok(())
    }
}

async fn readonly_role(db: &Database) -> TestResult {
    sqlx::raw_sql(AssertSqlSafe(format!("CREATE ROLE {} LOGIN PASSWORD '{}' NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION", db.role, ROLE_PASSWORD))).execute(&db.pool).await?;
    db.role_owned.store(true, Ordering::Release);
    sqlx::raw_sql(AssertSqlSafe(format!(
        "ALTER ROLE {role} SET default_transaction_read_only=on;
         REVOKE ALL ON SCHEMA public FROM PUBLIC;
         REVOKE CREATE, TEMPORARY ON DATABASE {database} FROM PUBLIC;
         GRANT USAGE ON SCHEMA public TO {role};
         GRANT SELECT ON public._sqlx_migrations TO {role};",
        role = db.role,
        database = db.name
    )))
    .execute(&db.pool)
    .await?;
    // Session advisory locks are legal in read-only transactions. Deny every overload,
    // not just DML, so even a transient successful acquisition is impossible for this role.
    let functions: Vec<String> = sqlx::query_scalar("SELECT oid::regprocedure::text FROM pg_proc WHERE pronamespace='pg_catalog'::regnamespace AND proname LIKE 'pg%advisory%'").fetch_all(&db.pool).await?;
    assert!(!functions.is_empty());
    for function in functions {
        sqlx::raw_sql(AssertSqlSafe(format!(
            "REVOKE EXECUTE ON FUNCTION {function} FROM PUBLIC"
        )))
        .execute(&db.pool)
        .await?;
    }
    let forbidden: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
         WHERE n.nspname='public' AND c.relkind IN ('r','p')
         AND (has_table_privilege($1,c.oid,'INSERT,UPDATE,DELETE,TRUNCATE')
              OR (c.relname <> '_sqlx_migrations' AND has_table_privilege($1,c.oid,'SELECT')))",
    )
    .bind(&db.role)
    .fetch_one(&db.pool)
    .await?;
    assert_eq!(forbidden, 0, "preflight role has business access");
    let can_lock: bool = sqlx::query_scalar("SELECT EXISTS (SELECT FROM pg_proc WHERE pronamespace='pg_catalog'::regnamespace AND proname LIKE 'pg%advisory%' AND has_function_privilege($1,oid,'EXECUTE'))").bind(&db.role).fetch_one(&db.pool).await?;
    assert!(!can_lock);
    let can_ddl: bool = sqlx::query_scalar("SELECT has_schema_privilege($1,'public','CREATE') OR has_database_privilege($1,current_database(),'CREATE,TEMPORARY')").bind(&db.role).fetch_one(&db.pool).await?;
    assert!(!can_ddl);
    let pool = db.config("cron-permission-probe", true)?.connect().await?;
    let result: TestResult = async {
        for sql in ["SELECT * FROM public.users", "UPDATE public.search_filter_periodic_match_state SET updated=now()", "CREATE TABLE public.cron_forbidden(id integer)", "SELECT pg_try_advisory_lock(1,1)"] {
            let result = sqlx::raw_sql(AssertSqlSafe(sql)).execute(&pool).await;
            assert!(matches!(result, Err(sqlx::Error::Database(error)) if matches!(error.code().as_deref(), Some("42501" | "25006"))), "read-only role did not reject a forbidden operation");
        }
        Ok(())
    }.await;
    pool.close().await;
    result
}

/// Compare all public table contents (including genuine ledger/checkpoints) plus schema
/// catalogs. This is fixture immutability evidence, not a full production drift verifier.
async fn snapshot(db: &Database) -> TestResult<Value> {
    let mut snapshot = serde_json::Map::new();
    let tables: Vec<String> = sqlx::query_scalar("SELECT format('%I.%I',n.nspname,c.relname) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p') ORDER BY c.relname").fetch_all(&db.pool).await?;
    for table in tables {
        let rows: String = sqlx::query_scalar(AssertSqlSafe(format!("SELECT coalesce(jsonb_agg(row ORDER BY row::text),'[]'::jsonb)::text FROM (SELECT to_jsonb(t) AS row FROM {table} t) rows"))).fetch_one(&db.pool).await?;
        snapshot.insert(table, serde_json::from_str(&rows)?);
    }
    for (name, sql) in [
        (
            "relations",
            "SELECT to_jsonb(c) AS row FROM pg_class c WHERE relnamespace='public'::regnamespace",
        ),
        (
            "columns",
            "SELECT to_jsonb(a) AS row FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid WHERE c.relnamespace='public'::regnamespace",
        ),
        (
            "constraints",
            "SELECT to_jsonb(c) AS row FROM pg_constraint c WHERE connamespace='public'::regnamespace",
        ),
        (
            "functions",
            "SELECT to_jsonb(p) AS row FROM pg_proc p WHERE pronamespace='public'::regnamespace",
        ),
        (
            "extensions",
            "SELECT to_jsonb(e) AS row FROM pg_extension e",
        ),
    ] {
        let rows: String = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT coalesce(jsonb_agg(row ORDER BY row::text),'[]'::jsonb)::text FROM ({sql}) rows"
        )))
        .fetch_one(&db.pool)
        .await?;
        snapshot.insert(name.into(), serde_json::from_str(&rows)?);
    }
    Ok(Value::Object(snapshot))
}

async fn preflight(db: Database, corrupt: Option<&'static str>) -> TestResult {
    readonly_role(&db).await?;
    if let Some(sql) = corrupt {
        sqlx::raw_sql(AssertSqlSafe(sql)).execute(&db.pool).await?;
    }
    let before = snapshot(&db).await?;
    assert!(db.advisory_pids().await?.is_empty());
    let directory = Directory::new()?;
    let wires = Tripwires::new()?;
    let mut child = Process::spawn(&mut wires.command(&db, &directory)?)?;
    let (status, output) = child.wait().await?;
    assert_eq!(
        status.code(),
        Some(if corrupt.is_some() { 1 } else { 0 }),
        "unexpected actual-binary exit: {output}"
    );
    assert_eq!(output.contains("cron.config.checked"), corrupt.is_none());
    if corrupt.is_some() {
        assert!(
            output.contains("cron.process.failed") && output.contains("PREFLIGHT"),
            "failure was not schema preflight: {output}"
        );
    }
    for forbidden in [
        "cron.job.started",
        "cron.scheduler.started",
        ROLE_PASSWORD,
        "cron-project-canary",
        "cron-location-canary",
        "cron-model-canary",
        "invalid-adc.json",
    ] {
        assert!(
            !output.contains(forbidden),
            "preflight executed work or leaked a canary"
        );
    }
    wires.untouched()?;
    db.no_sessions("aura-historia-cron").await?;
    assert!(db.advisory_pids().await?.is_empty());
    assert_eq!(
        before,
        snapshot(&db).await?,
        "preflight mutated the isolated fixture"
    );
    Ok(())
}

#[tokio::test]
async fn should_check_actual_binary_with_only_ledger_read_access_and_no_cloud_io() -> TestResult {
    with_database(|db| preflight(db, None)).await
}

#[tokio::test]
async fn should_reject_missing_ledger_without_creating_or_stamping_it() -> TestResult {
    with_database(|db| {
        preflight(
            db,
            Some("ALTER TABLE public._sqlx_migrations RENAME TO fixture_hidden_ledger"),
        )
    })
    .await
}

#[tokio::test]
async fn should_reject_failed_ledger_without_repairing_it() -> TestResult {
    with_database(|db| preflight(db, Some("UPDATE public._sqlx_migrations SET success=false")))
        .await
}

#[tokio::test]
async fn should_reject_checksum_mismatch_without_repairing_it() -> TestResult {
    with_database(|db| {
        preflight(
            db,
            Some("UPDATE public._sqlx_migrations SET checksum=decode('00','hex')"),
        )
    })
    .await
}

#[tokio::test]
async fn should_reject_missing_baseline_table_without_repairing_it() -> TestResult {
    with_database(|db| {
        preflight(
            db,
            Some("ALTER TABLE public.users RENAME TO fixture_hidden_users"),
        )
    })
    .await
}

#[tokio::test]
async fn should_reject_strict_actual_cli_config_before_any_dependency_io() -> TestResult {
    with_database(|db| async move {
        readonly_role(&db).await?;
        let before = snapshot(&db).await?;
        for case in 0..8 {
            let directory = Directory::new()?;
            let wires = Tripwires::new()?;
            let postgres = TcpListener::bind("127.0.0.1:0")?;
            postgres.set_nonblocking(true)?;
            let mut command = wires.command(&db, &directory)?;
            command.env("POSTGRES_PORT", postgres.local_addr()?.port().to_string());
            match case {
                0 => { command.env_remove("STAGE"); }
                1 => { command.env_remove("POSTGRES_SSL_MODE"); }
                2 => { command.env("POSTGRES_SSL_MODE", ""); }
                3 => { command.env("PGOPTIONS", ""); }
                4 => { command.env("PERIODIC_MATCH_MAX_RUN_SECONDS", ""); }
                5 => { command.env("AURA_HISTORIA_CRON_ENABLED_JOBS", ""); }
                6 => { command.env("AURA_HISTORIA_CRON_HEALTH_BIND_ADDR", "0.0.0.0:8082"); }
                _ => { command.arg("--unexpected"); }
            }
            let mut child = Process::spawn(&mut command)?;
            let (status, output) = child.wait().await?;
            assert_eq!(status.code(), Some(1), "invalid CLI case {case} accepted");
            assert!(output.contains("cron.process.failed"));
            assert!(!output.contains("cron.config.checked"));
            wires.untouched()?;
            assert!(matches!(postgres.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock), "invalid CLI case {case} reached PostgreSQL");
        }
        db.no_sessions("aura-historia-cron").await?;
        assert!(db.advisory_pids().await?.is_empty());
        assert_eq!(before, snapshot(&db).await?);
        Ok(())
    }).await
}
