use application::error::{BoxError, box_error};
use platform_postgres::{PostgresConnectError, PostgresPoolConfig, PostgresPoolConfigError};
use sqlx::{AssertSqlSafe, PgPool, migrate::MigrateError};
use std::fmt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

mod config;
pub mod crawler_domain_configuration_repository;
mod schema;

pub use config::{LocalDevelopmentConfig, ServerDatabaseConfig, parse_postgres_environment};
pub use schema::{CrawlerSchemaError, verify_crawler_schema};

#[derive(thiserror::Error)]
pub enum LocalDatabaseError {
    #[error("local bootstrap and demos require STAGE=local, ephemeral, or test")]
    LocalStageRequired,
    #[error(transparent)]
    Config(#[from] PostgresPoolConfigError),
    #[error(transparent)]
    Connect(#[from] PostgresConnectError),
    #[error("failed to start local PostgreSQL Docker Compose")]
    DockerStart(#[source] BoxError),
    #[error("local PostgreSQL Docker Compose exited unsuccessfully")]
    DockerExit,
    #[error("failed to check or create local PostgreSQL database")]
    DatabaseCreation(#[source] PostgresConnectError),
    #[error("failed to apply local crawler migrations")]
    Migration(#[source] BoxError),
}

impl fmt::Debug for LocalDatabaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// The raw migration cause stays private, including when callers walk Error::source.
struct RedactedMigrationCause {
    original: MigrateError,
}

impl RedactedMigrationCause {
    fn classification(&self) -> &'static str {
        match &self.original {
            MigrateError::Execute(_) | MigrateError::ExecuteMigration(_, _) => {
                "crawler migration execution failed"
            }
            MigrateError::Source(_) => "crawler migration source rejected",
            _ => "crawler migration state rejected",
        }
    }
}

impl fmt::Display for RedactedMigrationCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.classification())
    }
}

impl fmt::Debug for RedactedMigrationCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedactedMigrationCause")
            .field("classification", &self.classification())
            .finish_non_exhaustive()
    }
}

impl std::error::Error for RedactedMigrationCause {}

impl From<MigrateError> for LocalDatabaseError {
    fn from(original: MigrateError) -> Self {
        Self::Migration(box_error(RedactedMigrationCause { original }))
    }
}

pub const LOCAL_POSTGRES_HOST: &str = "localhost";
pub const LOCAL_POSTGRES_PORT: u16 = 5432;
pub const LOCAL_POSTGRES_USER: &str = "postgres";
pub const LOCAL_POSTGRES_PASSWORD: &str = "postgres";
pub const LOCAL_POSTGRES_ADMIN_DB: &str = "postgres";

pub const SERVER_DB_NAME: &str = "crawler_server";
pub const DEMO_DB_NAME: &str = "crawler_demo";
pub const DEMO_SCRAPER_DB_NAME: &str = "crawler_demo_scraper";
pub const DEMO_SPIDER_DB_NAME: &str = "crawler_demo_spider";

/// Local development only. The returned URL contains credentials; never log it.
pub fn database_url(_local: &LocalDevelopmentConfig, db_name: &str) -> String {
    format!(
        "postgres://{user}:{password}@{host}:{port}/{db}",
        user = LOCAL_POSTGRES_USER,
        password = LOCAL_POSTGRES_PASSWORD,
        host = LOCAL_POSTGRES_HOST,
        port = LOCAL_POSTGRES_PORT,
        db = db_name
    )
}

pub fn server_db_url(local: &LocalDevelopmentConfig) -> String {
    database_url(local, SERVER_DB_NAME)
}

pub fn demo_db_url(local: &LocalDevelopmentConfig) -> String {
    database_url(local, DEMO_DB_NAME)
}

pub fn demo_scraper_db_url(local: &LocalDevelopmentConfig) -> String {
    database_url(local, DEMO_SCRAPER_DB_NAME)
}

pub fn demo_spider_db_url(local: &LocalDevelopmentConfig) -> String {
    database_url(local, DEMO_SPIDER_DB_NAME)
}

fn docker_compose_file() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docker-compose.yml")
        .to_string_lossy()
        .to_string()
}

fn start_local_postgres(_local: &LocalDevelopmentConfig) -> Result<(), LocalDatabaseError> {
    let compose_file = docker_compose_file();
    let status = Command::new("docker")
        .args(["compose", "-f", &compose_file, "up", "-d"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| LocalDatabaseError::DockerStart(box_error(error)))?;

    if status.success() {
        Ok(())
    } else {
        Err(LocalDatabaseError::DockerExit)
    }
}

async fn connect_admin_pool_with_retry(
    config: &PostgresPoolConfig,
) -> Result<PgPool, PostgresConnectError> {
    let mut attempt = 0u32;
    let mut delay = Duration::from_millis(200);

    loop {
        attempt += 1;
        match config.connect().await {
            Ok(pool) => return Ok(pool),
            Err(e) if attempt < 30 => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(3));
                if attempt >= 29 {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

async fn create_database_if_missing(
    pool: &PgPool,
    db_name: &str,
) -> Result<(), LocalDatabaseError> {
    let exists: bool = sqlx::query_scalar(AssertSqlSafe(
        "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)",
    ))
    .bind(db_name)
    .fetch_one(pool)
    .await
    .map_err(|error| LocalDatabaseError::DatabaseCreation(PostgresConnectError::from(error)))?;

    if exists {
        return Ok(());
    }

    // PostgreSQL delimited identifiers escape a quote by doubling it.
    let escaped = db_name.replace('"', "\"\"");
    let create_sql = AssertSqlSafe(format!(r#"CREATE DATABASE "{escaped}""#));
    sqlx::query(create_sql)
        .execute(pool)
        .await
        .map_err(|error| LocalDatabaseError::DatabaseCreation(PostgresConnectError::from(error)))?;

    Ok(())
}

pub async fn bootstrap_local_database(
    local: &LocalDevelopmentConfig,
    db_name: &str,
) -> Result<(), LocalDatabaseError> {
    local.pool_config(db_name, 2)?;
    let admin_config = local.pool_config(LOCAL_POSTGRES_ADMIN_DB, 2)?;
    start_local_postgres(local)?;
    let admin_pool = connect_admin_pool_with_retry(&admin_config).await?;
    create_database_if_missing(&admin_pool, db_name).await
}

/// Explicit development command only; never called by server startup.
pub async fn bootstrap_all_local_databases(
    local: &LocalDevelopmentConfig,
) -> Result<(), LocalDatabaseError> {
    let databases = [
        SERVER_DB_NAME,
        DEMO_DB_NAME,
        DEMO_SCRAPER_DB_NAME,
        DEMO_SPIDER_DB_NAME,
    ]
    .into_iter()
    .map(|name| local.pool_config(name, 2).map(|config| (name, config)))
    .collect::<Result<Vec<_>, _>>()?;
    let admin_config = local.pool_config(LOCAL_POSTGRES_ADMIN_DB, 2)?;
    start_local_postgres(local)?;
    let admin_pool = connect_admin_pool_with_retry(&admin_config).await?;
    for (db_name, config) in databases {
        create_database_if_missing(&admin_pool, db_name).await?;
        let pool = config.connect().await?;
        migrate_local_database(local, &pool).await?;
        pool.close().await;
    }
    Ok(())
}

pub async fn migrate_local_database(
    _local: &LocalDevelopmentConfig,
    pool: &PgPool,
) -> Result<(), LocalDatabaseError> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(LocalDatabaseError::from)
}

#[cfg(test)]
fn assert_redacted_chain(error: &(dyn std::error::Error + 'static), canaries: &[&str]) -> usize {
    let mut current = Some(error);
    let mut count = 0;
    while let Some(member) = current {
        let formatted = format!("{member} {member:?} {member:#?}");
        assert!(
            canaries.iter().all(|canary| !formatted.contains(canary)),
            "error source chain leaked a canary"
        );
        assert!(!member.is::<sqlx::Error>(), "raw SQLx error escaped");
        assert!(!member.is::<MigrateError>(), "raw migration error escaped");
        assert!(
            !member.is::<std::env::VarError>(),
            "raw environment error escaped"
        );
        count += 1;
        assert!(count <= 4, "unexpected error source chain");
        current = member.source();
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_retain_private_migration_causes_without_exposing_their_data()
    -> Result<(), Box<dyn std::error::Error>> {
        const CANARY: &str = "private-user private-password private-path private-provider-body";
        let originals = [
            MigrateError::Execute(sqlx::Error::Protocol(CANARY.into())),
            MigrateError::ExecuteMigration(
                sqlx::Error::Tls(box_error(std::io::Error::other(CANARY))),
                1,
            ),
            MigrateError::Source(box_error(std::io::Error::other(CANARY))),
            MigrateError::CreateSchemasNotSupported(CANARY.into()),
            MigrateError::VersionMismatch(1),
            MigrateError::Dirty(1),
        ];
        for original in originals {
            let expected = std::mem::discriminant(&original);
            let error = LocalDatabaseError::from(original);
            assert_eq!(assert_redacted_chain(&error, &[CANARY, "private-"]), 2);
            let source = std::error::Error::source(&error)
                .and_then(|source| source.downcast_ref::<RedactedMigrationCause>())
                .ok_or("classified migration cause missing")?;
            assert_eq!(std::mem::discriminant(&source.original), expected);
            assert!(std::error::Error::source(source).is_none());
        }
        Ok(())
    }
}
