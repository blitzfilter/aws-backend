use platform_postgres::PostgresConnectError;
use sqlx::{AssertSqlSafe, PgPool};
use std::fmt;

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

/// Read-only guard over the existing SQLx ledger. Never creates, repairs, or stamps history.
pub async fn verify_crawler_schema(pool: &PgPool) -> Result<(), CrawlerSchemaError> {
    let tables_exist: bool = sqlx::query_scalar(AssertSqlSafe(
        "SELECT bool_and(pg_catalog.to_regclass(name) IS NOT NULL)
         FROM unnest($1::text[]) AS required(name)",
    ))
    .bind(REQUIRED_TABLES)
    .fetch_one(pool)
    .await
    .map_err(|error| CrawlerSchemaError::Read(PostgresConnectError::from(error)))?;
    if !tables_exist {
        return Err(CrawlerSchemaError::MissingTables);
    }
    let history = sqlx::query_as::<_, AppliedMigrationRow>(AssertSqlSafe(
        "SELECT version, success, checksum FROM _sqlx_migrations",
    ))
    .fetch_all(pool)
    .await
    .map_err(|error| CrawlerSchemaError::Read(PostgresConnectError::from(error)))?;
    verify_migration_history(&history)
}

fn verify_migration_history(history: &[AppliedMigrationRow]) -> Result<(), CrawlerSchemaError> {
    let migrations = sqlx::migrate!("./migrations");
    if history.iter().any(|applied| !applied.success)
        || migrations.iter().any(|required| {
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
