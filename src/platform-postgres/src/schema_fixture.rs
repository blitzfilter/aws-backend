//! Local-only, opt-in schema fixture; reuse the existing ID-based cleanup authority unchanged.
use super::super::{BUSINESS_MIGRATIONS, PostgresSchemaError};
use crate::{
    PostgresPoolConfig, PostgresTlsConfig,
    test_support::{TestDirectory, TestResult, openssl, run},
};
use sqlx::{AssertSqlSafe, PgPool};
use std::{
    error::Error,
    io,
    time::{Duration, Instant},
};

#[path = "docker_fixture.rs"]
mod docker_fixture;
use docker_fixture::DockerResources;

pub(super) struct SchemaFixture {
    docker: DockerResources,
    _directory: TestDirectory,
    pub(super) admin: PgPool,
    pub(super) reader_config: PostgresPoolConfig,
}

impl SchemaFixture {
    pub(super) async fn start() -> Result<Self, Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        let name = directory
            .0
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("invalid schema fixture name")?
            .to_owned();
        let password = String::from_utf8(run(openssl().args(["rand", "-hex", "24"]))?.stdout)
            .map_err(|_| "fixture password generation invalid")?
            .trim()
            .to_owned();
        if password.len() != 48 || !password.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("fixture password generation invalid".into());
        }
        directory.file("password", password.as_bytes(), 0o600)?;
        let mut docker = DockerResources::local(&directory.0)?;
        docker.create_network(&name)?;
        docker.create_container(&name, |command| {
            command
                .args([
                    "--publish",
                    "127.0.0.1::5432",
                    "--tmpfs",
                    "/var/lib/postgresql/data:rw,nosuid,size=256m",
                    "--mount",
                ])
                .arg(format!(
                    "type=bind,source={},target=/policy,readonly",
                    directory.0.display()
                ))
                .args([
                    "--env",
                    "POSTGRES_PASSWORD_FILE=/policy/password",
                    "--env",
                    "POSTGRES_USER=schema_owner",
                    "--env",
                    "POSTGRES_DB=schema_test",
                    include_str!("../../test-api/postgres/image-ref.txt").trim(),
                    "postgres",
                    "-c",
                    "shared_preload_libraries=pg_ttl_index",
                    "-c",
                    "statement_timeout=10000",
                    "-c",
                    "lock_timeout=2000",
                ]);
        })?;
        docker.start_container()?;
        let output = run(docker.command().args([
            "inspect",
            "--format",
            "{{(index (index .NetworkSettings.Ports \"5432/tcp\") 0).HostPort}}",
            docker.container_id()?,
        ]))?;
        let port: u16 = String::from_utf8(output.stdout)
            .map_err(|_| "fixture port lookup invalid")?
            .trim()
            .parse()
            .map_err(|_| "fixture port lookup invalid")?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let ready = docker
                .command()
                .args([
                    "exec",
                    docker.container_id()?,
                    "pg_isready",
                    "-h",
                    "127.0.0.1",
                    "-U",
                    "schema_owner",
                    "-d",
                    "schema_test",
                ])
                .output()
                .map_err(|_| "fixture readiness command unavailable")?;
            if ready.status.success() {
                break;
            }
            if Instant::now() >= deadline {
                return Err("schema fixture startup timed out (logs suppressed)".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let config = |username: &str| {
            PostgresPoolConfig::new(
                "127.0.0.1".into(),
                port,
                "schema_test".into(),
                username.into(),
                password.clone(),
                1,
                PostgresTlsConfig::new("test", "disable", None, "business-schema-test")?,
            )
        };
        let admin = config("schema_owner")?.connect().await?;
        // Password is generated hex, not external SQL input; shared config disables SQL logging.
        sqlx::raw_sql(AssertSqlSafe(format!(
            "CREATE ROLE schema_reader LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION
             NOBYPASSRLS PASSWORD '{password}';
             ALTER ROLE schema_reader SET default_transaction_read_only = on;
             ALTER ROLE schema_reader SET search_path = pg_catalog;
             REVOKE CREATE ON SCHEMA public FROM PUBLIC;
             GRANT USAGE ON SCHEMA public TO schema_reader;"
        )))
        .execute(&admin)
        .await
        .map_err(PostgresSchemaError::from)?;
        Ok(Self {
            docker,
            _directory: directory,
            admin,
            reader_config: config("schema_reader")?,
        })
    }

    pub(super) async fn migrate(&self) -> TestResult {
        sqlx::raw_sql("CREATE EXTENSION pg_ttl_index WITH SCHEMA public")
            .execute(&self.admin)
            .await
            .map_err(PostgresSchemaError::from)?;
        // Provision only the newly owned fixture. Production gate never calls run/ensure/skip.
        BUSINESS_MIGRATIONS
            .run(&self.admin)
            .await
            .map_err(|_| "owned fixture migration failed (provider output suppressed)")?;
        sqlx::raw_sql("GRANT SELECT ON public._sqlx_migrations TO schema_reader")
            .execute(&self.admin)
            .await
            .map_err(PostgresSchemaError::from)?;
        Ok(())
    }

    pub(super) async fn close(mut self) -> io::Result<()> {
        self.admin.close().await;
        self.docker.cleanup()
    }
}
