//! Opt-in local fixture. Only its acquired container ID authorizes cleanup.
use super::support::{OwnedDirectory, TestResult, inputs};
use platform_postgres::PostgresPoolConfig;
use sqlx::PgPool;
use std::{
    process::{Command, Output},
    time::Duration,
};

pub(super) struct LocalPostgres {
    directory: OwnedDirectory,
    id: String,
    pub(super) admin: PgPool,
    pub(super) port: u16,
}

fn docker(directory: &OwnedDirectory) -> Command {
    let mut command = Command::new("/usr/bin/timeout");
    command
        .args([
            "20s",
            "/usr/bin/docker",
            "--host",
            "unix:///var/run/docker.sock",
            "--config",
        ])
        .arg(&directory.0)
        .env_clear()
        .env("PATH", "/usr/bin:/bin");
    command
}

fn checked(command: &mut Command) -> TestResult<Output> {
    let output = command.output()?;
    if !output.status.success() {
        return Err("owned Docker fixture command failed; output suppressed".into());
    }
    Ok(output)
}

struct AcquiredContainer {
    directory: Option<OwnedDirectory>,
    id: String,
}

impl Drop for AcquiredContainer {
    fn drop(&mut self) {
        if let Some(directory) = &self.directory
            && checked(docker(directory).args(["rm", "--force", &self.id])).is_err()
        {
            eprintln!("owned PostgreSQL container cleanup failed");
        }
    }
}

impl LocalPostgres {
    pub(super) async fn start() -> TestResult<Self> {
        let directory = OwnedDirectory::new()?;
        let name = format!("aura-api-preflight-{}", uuid::Uuid::new_v4());
        let output = checked(docker(&directory).args([
            "create",
            "--pull=never",
            "--name",
            &name,
            "--publish",
            "127.0.0.1::5432",
            "--tmpfs",
            "/var/lib/postgresql/data:rw,nosuid,size=256m",
            "--env",
            "POSTGRES_HOST_AUTH_METHOD=trust",
            include_str!("../../test-api/postgres/image-ref.txt").trim(),
            "postgres",
            "-c",
            "shared_preload_libraries=pg_ttl_index",
            "-c",
            "statement_timeout=10000",
        ]))?;
        let id = std::str::from_utf8(&output.stdout)?.trim().to_owned();
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid created container ID; no cleanup authority by name".into());
        }
        let mut acquired = AcquiredContainer {
            directory: Some(directory),
            id,
        };
        let directory = acquired
            .directory
            .as_ref()
            .ok_or("missing owned directory")?;
        checked(docker(directory).args(["start", &acquired.id]))?;
        let output = checked(docker(directory).args([
            "inspect",
            "--format",
            "{{(index (index .NetworkSettings.Ports \"5432/tcp\") 0).HostPort}}",
            &acquired.id,
        ]))?;
        let port = std::str::from_utf8(&output.stdout)?.trim().parse::<u16>()?;
        let mut values = inputs();
        values.insert("POSTGRES_PORT", port.to_string());
        let config = PostgresPoolConfig::from_lookup("api-preflight-fixture", |key| {
            values.get(key).cloned()
        })?;
        let admin = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(pool) = config.connect().await {
                    break pool;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await?;
        let fixture = Self {
            directory: acquired.directory.take().ok_or("missing ownership")?,
            id: acquired.id.clone(),
            admin,
            port,
        };
        // Fixture provisioning only. Runtime/preflight have no migration capability.
        sqlx::raw_sql("CREATE EXTENSION pg_ttl_index")
            .execute(&fixture.admin)
            .await?;
        sqlx::migrate!("../../migrations")
            .run(&fixture.admin)
            .await?;
        sqlx::raw_sql(
            "CREATE ROLE api_preflight LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
             ALTER ROLE api_preflight SET default_transaction_read_only = on;
             REVOKE CREATE ON SCHEMA public FROM PUBLIC;
             GRANT USAGE ON SCHEMA public TO api_preflight;
             GRANT SELECT ON public._sqlx_migrations TO api_preflight;
             CREATE TABLE public.preflight_canary (value text NOT NULL);
             INSERT INTO public.preflight_canary VALUES ('unchanged');"
        ).execute(&fixture.admin).await?;
        Ok(fixture)
    }

    pub(super) async fn close(mut self) -> TestResult {
        tokio::time::timeout(Duration::from_secs(5), self.admin.close()).await?;
        checked(docker(&self.directory).args(["rm", "--force", &self.id]))?;
        self.id.clear();
        let directory_path = self.directory.0.clone();
        drop(self);
        assert!(
            !directory_path.exists(),
            "owned PostgreSQL test directory not removed"
        );
        Ok(())
    }
}

impl Drop for LocalPostgres {
    fn drop(&mut self) {
        if !self.id.is_empty()
            && checked(docker(&self.directory).args(["rm", "--force", &self.id])).is_err()
        {
            eprintln!("owned PostgreSQL container cleanup failed");
        }
    }
}
