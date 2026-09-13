use super::error::{TestError, TestResult};
use platform_postgres::{PostgresPoolConfig, PostgresTlsConfig};
use serde_json::Value;
use sqlx::{AssertSqlSafe, PgPool};
use std::time::Duration;

pub(super) const PASSWORD: &str = "crawler-fixture-password-canary";

// Descendant reuses exact private acquisition flags; no production visibility hooks.
#[path = "fresh_bootstrap.rs"]
pub(crate) mod fresh_bootstrap;

pub(super) struct Database {
    pub pool: PgPool,
    pub name: String,
    pub role: String,
    port: u16,
    created: bool,
    role_created: bool,
}

impl Database {
    fn new(port: u16, kind: &str) -> TestResult<Self> {
        let name = format!("cp_{}_{}", uuid::Uuid::now_v7().simple(), kind);
        let config = PostgresPoolConfig::new(
            "127.0.0.1".into(),
            port,
            name.clone(),
            "postgres".into(),
            "postgres".into(),
            2,
            PostgresTlsConfig::new("test", "disable", None, "crawler-fixture-observer")?,
        )?;
        Ok(Self {
            pool: config
                .pool_options()
                .connect_lazy_with(config.connect_options()),
            role: format!("{name}_ro"),
            name,
            port,
            created: false,
            role_created: false,
        })
    }

    pub fn url(&self) -> String {
        format!(
            "postgres://{}:{PASSWORD}@127.0.0.1:{}/{}",
            self.role, self.port, self.name
        )
    }

    async fn prepare(&mut self, admin: &PgPool, business: bool) -> TestResult {
        sqlx::raw_sql(AssertSqlSafe(format!(
            "CREATE DATABASE {} TEMPLATE template0",
            self.name
        )))
        .execute(admin)
        .await?;
        self.created = true;
        sqlx::raw_sql(AssertSqlSafe(format!(
            "ALTER DATABASE {} SET statement_timeout = '5s'; ALTER DATABASE {} SET lock_timeout = '500ms'",
            self.name, self.name,
        ))).execute(admin).await?;
        // Provision ONLY this just-created isolated database. SQLx, not raw replay, owns
        // the genuine ledger. No adoption, repair, incremental migration, or live data.
        if business {
            sqlx::raw_sql("CREATE EXTENSION pg_ttl_index")
                .execute(&self.pool)
                .await?;
            sqlx::migrate!("../../migrations").run(&self.pool).await?;
        } else {
            sqlx::migrate!("./migrations").run(&self.pool).await?;
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM public._sqlx_migrations WHERE success")
                .fetch_one(&self.pool)
                .await?;
        assert_eq!(
            count,
            if business { 1 } else { 6 },
            "review fixture when shipped baselines change"
        );
        sqlx::raw_sql(
            "CREATE TABLE public.preflight_canary (value text NOT NULL);
             INSERT INTO public.preflight_canary VALUES ('unchanged-fixture-row')",
        )
        .execute(&self.pool)
        .await?;
        sqlx::raw_sql(AssertSqlSafe(format!(
            "CREATE ROLE {} LOGIN PASSWORD '{PASSWORD}' NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS",
            self.role,
        ))).execute(admin).await?;
        self.role_created = true;
        sqlx::raw_sql(AssertSqlSafe(format!(
            "ALTER ROLE {role} SET default_transaction_read_only = on;
             REVOKE ALL ON SCHEMA public FROM PUBLIC;
             REVOKE ALL ON ALL TABLES IN SCHEMA public FROM PUBLIC;
             REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM PUBLIC;
             REVOKE EXECUTE ON ALL FUNCTIONS IN SCHEMA public FROM PUBLIC;
             REVOKE ALL ON DATABASE {database} FROM PUBLIC;
             GRANT CONNECT ON DATABASE {database} TO {role};
             GRANT USAGE ON SCHEMA public TO {role};
             GRANT SELECT ON public._sqlx_migrations TO {role}",
            role = self.role,
            database = self.name,
        )))
        .execute(&self.pool)
        .await?;
        // Advisory locks remain legal in READ ONLY transactions; deny every overload.
        let functions: Vec<String> = sqlx::query_scalar(
            "SELECT oid::regprocedure::text FROM pg_catalog.pg_proc
             WHERE pronamespace='pg_catalog'::regnamespace AND proname LIKE 'pg%advisory%'",
        )
        .fetch_all(&self.pool)
        .await?;
        assert!(!functions.is_empty());
        for function in functions {
            sqlx::raw_sql(AssertSqlSafe(format!(
                "REVOKE EXECUTE ON FUNCTION {function} FROM PUBLIC"
            )))
            .execute(&self.pool)
            .await?;
        }
        self.assert_restricted().await
    }

    pub async fn permission_probe(&self) -> TestResult<PgPool> {
        Ok(PostgresPoolConfig::from_url(
            &self.url(),
            1,
            PostgresTlsConfig::new("test", "disable", None, "crawler-fixture-permission-probe")?,
        )?
        .connect()
        .await?)
    }

    async fn assert_restricted(&self) -> TestResult {
        let unsafe_role: bool = sqlx::query_scalar(
            "SELECT rolsuper OR rolcreatedb OR rolcreaterole OR rolinherit OR rolreplication OR rolbypassrls
             FROM pg_catalog.pg_roles WHERE rolname=$1",
        ).bind(&self.role).fetch_one(&self.pool).await?;
        assert!(!unsafe_role);
        let forbidden: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_catalog.pg_class c
             WHERE c.relnamespace='public'::regnamespace AND c.relkind IN ('r','p')
             AND (has_table_privilege($1,c.oid,'INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
               OR (c.relname <> '_sqlx_migrations' AND has_table_privilege($1,c.oid,'SELECT')))",
        )
        .bind(&self.role)
        .fetch_one(&self.pool)
        .await?;
        assert_eq!(forbidden, 0, "role has runtime table access");
        let can_ddl: bool = sqlx::query_scalar(
            "SELECT has_schema_privilege($1,'public','CREATE')
                 OR has_database_privilege($1,current_database(),'CREATE,TEMPORARY')",
        )
        .bind(&self.role)
        .fetch_one(&self.pool)
        .await?;
        assert!(!can_ddl);
        let can_lock: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT FROM pg_catalog.pg_proc
             WHERE pronamespace='pg_catalog'::regnamespace AND proname LIKE 'pg%advisory%'
             AND has_function_privilege($1,oid,'EXECUTE'))",
        )
        .bind(&self.role)
        .fetch_one(&self.pool)
        .await?;
        assert!(!can_lock);
        let pool = self.permission_probe().await?;
        let result: TestResult = async {
            let read_only: String = sqlx::query_scalar("SHOW transaction_read_only")
                .fetch_one(&pool)
                .await?;
            assert!(
                read_only == "on",
                "restricted role is not read-only; value suppressed"
            );
            for sql in [
                "SELECT * FROM public.listing_sources",
                "INSERT INTO public.preflight_canary VALUES ('forbidden')",
                "UPDATE public.preflight_canary SET value='forbidden'",
                "DELETE FROM public.preflight_canary",
                "CREATE TABLE public.forbidden (id integer)",
                "CREATE TEMP TABLE forbidden (id integer)",
                "SELECT pg_try_advisory_lock(1,1)",
            ] {
                let result = sqlx::raw_sql(AssertSqlSafe(sql)).execute(&pool).await;
                assert!(
                    matches!(result, Err(sqlx::Error::Database(error))
                    if matches!(error.code().as_deref(), Some("42501" | "25006"))),
                    "restricted role accepted runtime access"
                );
            }
            Ok(())
        }
        .await;
        tokio::time::timeout(Duration::from_secs(5), pool.close()).await?;
        result
    }

    pub async fn no_runtime_sessions(&self) -> TestResult {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let count: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM pg_catalog.pg_stat_activity
                     WHERE datname=$1 AND (application_name='crawler-server' OR usename=$2)",
                )
                .bind(&self.name)
                .bind(&self.role)
                .fetch_one(&self.pool)
                .await?;
                if count == 0 {
                    return Ok::<_, sqlx::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        let locks: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_catalog.pg_locks WHERE locktype='advisory'
             AND database=(SELECT oid FROM pg_catalog.pg_database WHERE datname=$1)",
        )
        .bind(&self.name)
        .fetch_one(&self.pool)
        .await?;
        assert_eq!(locks, 0, "preflight retained an advisory lock");
        Ok(())
    }

    /// Fixture immutability, not a full production schema-drift attestation.
    pub async fn snapshot(&self) -> TestResult<Value> {
        let mut snapshot = serde_json::Map::new();
        let tables: Vec<(String, bool)> = sqlx::query_as(
            "SELECT format('%I.%I',n.nspname,c.relname), c.relkind='S'
             FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
             WHERE n.nspname='public' AND c.relkind IN ('r','p','S') ORDER BY c.relname",
        )
        .fetch_all(&self.pool)
        .await?;
        for (table, sequence) in tables {
            let sql = if sequence {
                // Sequences have no composite row type for to_jsonb(table_alias).
                format!("SELECT jsonb_build_array(last_value,log_cnt,is_called) FROM {table}")
            } else {
                format!(
                    "SELECT coalesce(jsonb_agg(row ORDER BY row::text),'[]'::jsonb)
                     FROM (SELECT to_jsonb(t) AS row FROM {table} t) rows"
                )
            };
            let rows: Value = sqlx::query_scalar(AssertSqlSafe(sql))
                .fetch_one(&self.pool)
                .await?;
            snapshot.insert(table, rows);
        }
        for (name, sql) in [
            (
                "relations",
                "SELECT jsonb_build_array(oid,relname,relkind,relpersistence,relowner,relacl,relrowsecurity) AS row FROM pg_catalog.pg_class WHERE relnamespace='public'::regnamespace",
            ),
            (
                "columns",
                "SELECT to_jsonb(a) AS row FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid WHERE c.relnamespace='public'::regnamespace",
            ),
            (
                "constraints",
                "SELECT to_jsonb(c) AS row FROM pg_catalog.pg_constraint c WHERE connamespace='public'::regnamespace",
            ),
            (
                "functions",
                "SELECT to_jsonb(p) AS row FROM pg_catalog.pg_proc p WHERE pronamespace='public'::regnamespace",
            ),
            (
                "extensions",
                "SELECT to_jsonb(e) AS row FROM pg_catalog.pg_extension e",
            ),
            (
                "views",
                "SELECT to_jsonb(v) AS row FROM pg_catalog.pg_views v WHERE schemaname='public'",
            ),
            (
                "policies",
                "SELECT to_jsonb(p) AS row FROM pg_catalog.pg_policies p WHERE schemaname='public'",
            ),
        ] {
            let rows: Value = sqlx::query_scalar(AssertSqlSafe(format!(
                "SELECT coalesce(jsonb_agg(row ORDER BY row::text),'[]'::jsonb) FROM ({sql}) rows",
            )))
            .fetch_one(&self.pool)
            .await?;
            snapshot.insert(name.into(), rows);
        }
        Ok(Value::Object(snapshot))
    }
}

pub(super) struct Databases {
    pub crawler: Database,
    pub business: Database,
}
impl Databases {
    pub fn new(port: u16) -> TestResult<Self> {
        Ok(Self {
            crawler: Database::new(port, "crawler")?,
            business: Database::new(port, "business")?,
        })
    }

    pub async fn prepare(&mut self, admin: &PgPool) -> TestResult {
        self.crawler.prepare(admin, false).await?;
        self.business.prepare(admin, true).await
    }

    pub async fn close(&mut self, admin: &PgPool) -> TestResult {
        let mut failures = Vec::new();
        for database in [&mut self.crawler, &mut self.business] {
            if let Err(error) =
                tokio::time::timeout(Duration::from_secs(5), database.pool.close()).await
            {
                failures.push(TestError::caused("OBSERVER_CLOSE", error));
            }
            // No FORCE: leaked application backends must fail cleanup, not be concealed.
            if database.created
                && let Err(error) =
                    sqlx::raw_sql(AssertSqlSafe(format!("DROP DATABASE {}", database.name)))
                        .execute(admin)
                        .await
            {
                failures.push(TestError::caused("DATABASE_DROP", error));
            }
            if database.role_created
                && let Err(error) =
                    sqlx::raw_sql(AssertSqlSafe(format!("DROP ROLE {}", database.role)))
                        .execute(admin)
                        .await
            {
                failures.push(TestError::caused("ROLE_DROP", error));
            }
            let absent: Result<bool, sqlx::Error> = sqlx::query_scalar(
                "SELECT NOT EXISTS(SELECT FROM pg_catalog.pg_database WHERE datname=$1)
                    AND NOT EXISTS(SELECT FROM pg_catalog.pg_roles WHERE rolname=$2)",
            )
            .bind(&database.name)
            .bind(&database.role)
            .fetch_one(admin)
            .await;
            match absent {
                Ok(true) => {}
                Ok(false) => failures.push(TestError::failure("DATABASE_ROLE_PRESENT")),
                Err(error) => failures.push(error.into()),
            }
        }
        if !failures.is_empty() {
            return Err(TestError::failures("DATABASE_CLEANUP", failures));
        }
        Ok(())
    }
}
