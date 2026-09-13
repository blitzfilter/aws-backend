use platform_postgres::PostgresConnectError;
use sqlx::{Connection, PgConnection, PgPool};
use std::{fmt, time::Duration};

static CRAWLER_MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
const VERIFY_TIMEOUT: Duration = Duration::from_secs(5);

const REQUIRED_TABLES: &[&str] = &[
    "listing_sources",
    "listing_source_domains",
    "listing_source_urls",
    "listing_source_product_schemas",
    "listing_source_removed_page_schemas",
    "crawler_reviews",
    "crawler_review_pages",
    "crawler_review_urls",
    "_sqlx_migrations",
];

#[derive(thiserror::Error)]
pub enum CrawlerSchemaError {
    #[error("failed to read crawler schema readiness")]
    Read(#[source] PostgresConnectError),
    #[error("crawler tables or migration history missing; provision schema outside server startup")]
    MissingTables,
    #[error(
        "crawler migration history incomplete or mismatched; apply migrations outside server startup"
    )]
    MigrationHistoryMismatch,
    #[error("crawler schema verification deadline exceeded")]
    Timeout,
    #[error("crawler migration ledger is not a trusted persistent table")]
    InvalidLedger,
}

impl fmt::Debug for CrawlerSchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[derive(sqlx::FromRow)]
struct AppliedMigrationRow {
    version: i64,
    success: bool,
    checksum: Vec<u8>,
}

impl From<sqlx::Error> for CrawlerSchemaError {
    fn from(error: sqlx::Error) -> Self {
        Self::Read(PostgresConnectError::from(error))
    }
}

/// Exact baseline/history availability in one bounded, read-only catalog snapshot.
/// Never creates, repairs, or stamps history. This is not full DDL drift verification.
pub async fn verify_crawler_schema(pool: &PgPool) -> Result<(), CrawlerSchemaError> {
    tokio::time::timeout(VERIFY_TIMEOUT, verify_bounded(pool))
        .await
        .map_err(|_| CrawlerSchemaError::Timeout)?
}

async fn verify_bounded(pool: &PgPool) -> Result<(), CrawlerSchemaError> {
    let mut connection = pool.acquire().await?;
    // Abandoned queries/rollback must not be returned to runtime callers.
    connection.close_on_drop();
    let mut snapshot = connection
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await?;
    sqlx::raw_sql(
        "SET LOCAL statement_timeout = '2s';
         SET LOCAL lock_timeout = '500ms';
         SET LOCAL idle_in_transaction_session_timeout = '5s';
         SET LOCAL search_path = pg_catalog;
         SET LOCAL row_security = off;",
    )
    .execute(&mut *snapshot)
    .await?;
    verify_snapshot(&mut snapshot).await?;
    snapshot.commit().await?;
    connection.close().await?;
    Ok(())
}

async fn verify_snapshot(connection: &mut PgConnection) -> Result<(), CrawlerSchemaError> {
    let ledger_valid: Option<bool> = sqlx::query_scalar(
        "SELECT c.relkind = 'r' AND c.relpersistence = 'p' AND NOT c.relrowsecurity
         FROM pg_catalog.pg_class c
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'public' AND c.relname = '_sqlx_migrations'",
    )
    .fetch_optional(&mut *connection)
    .await?;
    match ledger_valid {
        Some(true) => {}
        Some(false) => return Err(CrawlerSchemaError::InvalidLedger),
        None => return Err(CrawlerSchemaError::MissingTables),
    }
    let tables_exist: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS (
            SELECT FROM pg_catalog.unnest($1::text[]) AS required(name)
            WHERE NOT EXISTS (
                SELECT FROM pg_catalog.pg_class c
                JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
                WHERE c.relname = required.name AND n.nspname = 'public'
                  AND c.relkind IN ('r', 'p') AND c.relpersistence = 'p'
            )
         )",
    )
    .bind(REQUIRED_TABLES)
    .fetch_one(&mut *connection)
    .await?;
    if !tables_exist {
        return Err(CrawlerSchemaError::MissingTables);
    }
    // One extra row suffices to reject unknown history. Bound corrupt checksum payloads too.
    let limit = i64::try_from(CRAWLER_MIGRATIONS.iter().count() + 1)
        .map_err(|_| CrawlerSchemaError::MigrationHistoryMismatch)?;
    let history = sqlx::query_as::<_, AppliedMigrationRow>(
        "SELECT version, success, pg_catalog.substr(checksum, 1, 49) AS checksum
         FROM public._sqlx_migrations ORDER BY version LIMIT $1",
    )
    .bind(limit)
    .fetch_all(&mut *connection)
    .await?;
    verify_migration_history(&history)
}

fn verify_migration_history(history: &[AppliedMigrationRow]) -> Result<(), CrawlerSchemaError> {
    if history.len() != CRAWLER_MIGRATIONS.iter().count()
        || history.iter().any(|applied| !applied.success)
        || CRAWLER_MIGRATIONS.iter().any(|required| {
            !history.iter().any(|applied| {
                applied.version == required.version
                    && applied.checksum.as_slice() == required.checksum.as_ref()
            })
        })
    {
        return Err(CrawlerSchemaError::MigrationHistoryMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn applied_history() -> Vec<AppliedMigrationRow> {
        sqlx::migrate!("./migrations")
            .iter()
            .map(|migration| AppliedMigrationRow {
                version: migration.version,
                success: true,
                checksum: migration.checksum.to_vec(),
            })
            .collect()
    }

    #[test]
    fn should_accept_existing_matching_history() -> Result<(), CrawlerSchemaError> {
        verify_migration_history(&applied_history())
    }

    #[test]
    fn should_reject_missing_or_pending_history() {
        assert!(verify_migration_history(&[]).is_err());
        let mut history = applied_history();
        history.pop();
        assert!(verify_migration_history(&history).is_err());
    }

    #[test]
    fn should_reject_failed_or_altered_history() {
        let mut history = applied_history();
        for row in &mut history {
            row.success = false;
        }
        assert!(verify_migration_history(&history).is_err());
        for row in &mut history {
            row.success = true;
            row.checksum.clear();
        }
        assert!(verify_migration_history(&history).is_err());
    }

    #[test]
    fn should_reject_extra_or_duplicate_history() {
        let mut history = applied_history();
        history.push(AppliedMigrationRow {
            version: i64::MAX,
            success: true,
            checksum: vec![0; 48],
        });
        assert!(verify_migration_history(&history).is_err());

        let mut history = applied_history();
        history[0].version = history[1].version;
        assert!(verify_migration_history(&history).is_err());
    }

    #[test]
    fn should_accept_history_independent_of_row_order() {
        let mut history = applied_history();
        history.reverse();
        assert!(verify_migration_history(&history).is_ok());
    }

    #[test]
    fn should_redact_database_errors_without_discarding_the_source() {
        const CANARY: &str = "postgres://private-user:private-password@private-host/private-db";
        for original in [
            sqlx::Error::Protocol(CANARY.into()),
            sqlx::Error::Io(std::io::Error::other(CANARY)),
            sqlx::Error::Tls(application::error::box_error(std::io::Error::other(CANARY))),
        ] {
            let error = CrawlerSchemaError::Read(PostgresConnectError::from(original));
            assert_eq!(
                crate::local_db::assert_redacted_chain(&error, &[CANARY, "private-"]),
                3
            );
        }
    }
}
