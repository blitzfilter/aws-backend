use sqlx::{Connection, PgConnection, PgPool, Postgres, Transaction, migrate::Migrator};
use std::{fmt, io, time::Duration};

static BUSINESS_MIGRATIONS: Migrator = sqlx::migrate!("../../migrations");
const VERIFY_TIMEOUT: Duration = Duration::from_secs(5);
const REQUIRED_EXTENSIONS: &[&str] = &["pg_trgm", "unaccent", "pg_ttl_index"];
// Availability only: these names do not attest columns, constraints, indexes or function bodies.
const BASELINE_TABLES: &[&str] = &[
    "users",
    "user_cognito_identities",
    "parties",
    "listing_sources",
    "listing_source_ingestion_methods",
    "listing_source_web_crawl_ingestion_configurations",
    "listing_source_shopify_ingestion_configurations",
    "listing_source_woocommerce_ingestion_configurations",
    "product_listing_raw_streams",
    "product_listing_raw_provider_observation_receipts",
    "product_listing_raw_revisions",
    "product_listing_raw_normalization_heads",
    "product_listing_raw_normalizations",
    "partnerships",
    "partnership_members",
    "partnership_listing_source_grants",
    "partnership_applications",
    "fx_rates",
    "fx_rate_quotes",
    "product_listings",
    "product_listing_translations",
    "product_listing_events",
    "product_listing_content_assessments",
    "product_listing_watchlist",
    "search_filters",
    "search_filter_periodic_match_state",
    "search_filter_matches",
    "notifications",
    "notification_deliveries",
    "access_tokens",
    "oauth_clients",
    "oauth_authorization_codes",
    "oauth_third_party_exchange_codes",
];

/// Startup failure safe to format, including every exposed error-chain member.
/// Use [`Self::code`] for a stable, nonsecret failure category; no provider data is exposed.
#[derive(Debug, thiserror::Error)]
#[error("business schema verification failed: {code}")]
pub struct PostgresSchemaError {
    code: &'static str,
    #[source]
    source: Option<RedactedSchemaCause>,
}

impl PostgresSchemaError {
    pub const fn code(&self) -> &'static str {
        self.code
    }

    fn rejected(code: &'static str) -> Self {
        Self { code, source: None }
    }
}

// Same redaction boundary as connection errors: retain the cause, never expose raw source/accessors.
struct RedactedSchemaCause {
    original: sqlx::Error,
}

impl RedactedSchemaCause {
    fn code(&self) -> &'static str {
        match &self.original {
            sqlx::Error::Database(error) => match error.code().as_deref() {
                Some("42501") => "SCHEMA_PERMISSION_DENIED",
                Some("55P03" | "57014") => "SCHEMA_TIMEOUT",
                Some("42P01" | "42703" | "42804" | "42809") => "SCHEMA_HISTORY_INVALID",
                _ => "SCHEMA_DEPENDENCY_UNAVAILABLE",
            },
            sqlx::Error::PoolTimedOut => "SCHEMA_TIMEOUT",
            sqlx::Error::Io(error) if error.kind() == io::ErrorKind::TimedOut => "SCHEMA_TIMEOUT",
            sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_) => "SCHEMA_HISTORY_INVALID",
            _ => "SCHEMA_DEPENDENCY_UNAVAILABLE",
        }
    }
}

impl fmt::Display for RedactedSchemaCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PostgreSQL schema check cause: {} (details redacted)",
            self.code()
        )
    }
}

impl fmt::Debug for RedactedSchemaCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for RedactedSchemaCause {}

impl From<sqlx::Error> for PostgresSchemaError {
    fn from(original: sqlx::Error) -> Self {
        let source = RedactedSchemaCause { original };
        Self {
            code: source.code(),
            source: Some(source),
        }
    }
}

#[derive(sqlx::FromRow)]
struct AppliedMigrationRow {
    version: i64,
    success: bool,
    checksum: Vec<u8>,
}

fn expected_migrations() -> impl Iterator<Item = &'static sqlx::migrate::Migration> {
    BUSINESS_MIGRATIONS
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
}

/// Verify the exact compiled business SQLx history and baseline availability, without writes.
///
/// Reads only `public._sqlx_migrations` and PostgreSQL catalogs in one repeatable-read,
/// read-only snapshot. Requires public-schema USAGE, catalog access and ledger SELECT;
/// no business-row reads, DDL, migration execution, history creation or baseline stamping.
/// Unknown future migrations deliberately block startup until a reviewed compatible-superset
/// protocol exists. Matching history is NOT full schema drift or TTL-worker health proof.
///
/// Five seconds bounds acquire through close; statements are limited to two seconds and
/// lock waits to 500 ms. One checked-out connection is closed, never returned to the pool,
/// including on error/cancellation. Call once before accepting API/worker work, not per request.
pub async fn verify_business_schema(pool: &PgPool) -> Result<(), PostgresSchemaError> {
    tokio::time::timeout(VERIFY_TIMEOUT, verify_bounded(pool))
        .await
        .map_err(|elapsed| {
            PostgresSchemaError::from(sqlx::Error::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                elapsed,
            )))
        })?
}

async fn verify_bounded(pool: &PgPool) -> Result<(), PostgresSchemaError> {
    let mut connection = pool.acquire().await?;
    // Cancellation must not return a connection with a pending query/rollback to the pool.
    connection.close_on_drop();
    let mut transaction = begin_snapshot(&mut connection).await?;
    verify_snapshot(&mut transaction).await?;
    transaction.commit().await?;
    connection.close().await?;
    Ok(())
}

async fn begin_snapshot(
    connection: &mut PgConnection,
) -> Result<Transaction<'_, Postgres>, PostgresSchemaError> {
    let mut transaction = connection
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await?;
    sqlx::raw_sql(
        "SET LOCAL statement_timeout = '2s';
         SET LOCAL lock_timeout = '500ms';
         SET LOCAL idle_in_transaction_session_timeout = '5s';
         SET LOCAL search_path = pg_catalog;
         SET LOCAL row_security = off;",
    )
    .execute(&mut *transaction)
    .await?;
    Ok(transaction)
}

async fn verify_snapshot(connection: &mut PgConnection) -> Result<(), PostgresSchemaError> {
    let ledger_valid: Option<bool> = sqlx::query_scalar(
        "SELECT c.relkind = 'r' AND c.relpersistence = 'p' AND NOT c.relrowsecurity
         FROM pg_catalog.pg_class c
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'public' AND c.relname = '_sqlx_migrations'",
    )
    .fetch_optional(&mut *connection)
    .await?;
    match ledger_valid {
        None => return Err(PostgresSchemaError::rejected("SCHEMA_HISTORY_MISSING")),
        Some(false) => return Err(PostgresSchemaError::rejected("SCHEMA_HISTORY_INVALID")),
        Some(true) => {}
    }

    // One overflow row suffices to reject extra history. Cap corrupt checksum payloads too.
    let limit = i64::try_from(expected_migrations().count() + 1)
        .map_err(|_| PostgresSchemaError::rejected("SCHEMA_EXPECTATION_INVALID"))?;
    let history = sqlx::query_as::<_, AppliedMigrationRow>(
        "SELECT version, success, pg_catalog.substr(checksum, 1, 49) AS checksum
         FROM public._sqlx_migrations ORDER BY version LIMIT $1",
    )
    .bind(limit)
    .fetch_all(&mut *connection)
    .await?;
    verify_history(&history)?;

    let extensions_available: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS (
            SELECT FROM pg_catalog.unnest($1::text[]) AS required(name)
            WHERE NOT EXISTS (
                SELECT FROM pg_catalog.pg_extension e
                JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace
                WHERE e.extname = required.name AND n.nspname = 'public'
            )
         )",
    )
    .bind(REQUIRED_EXTENSIONS)
    .fetch_one(&mut *connection)
    .await?;
    if !extensions_available {
        return Err(PostgresSchemaError::rejected("SCHEMA_EXTENSION_MISSING"));
    }

    let baseline_available: bool = sqlx::query_scalar(
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
    .bind(BASELINE_TABLES)
    .fetch_one(&mut *connection)
    .await?;
    if !baseline_available {
        return Err(PostgresSchemaError::rejected("SCHEMA_BASELINE_MISSING"));
    }
    Ok(())
}

fn verify_history(history: &[AppliedMigrationRow]) -> Result<(), PostgresSchemaError> {
    if history.iter().any(|row| !row.success) {
        return Err(PostgresSchemaError::rejected("SCHEMA_HISTORY_DIRTY"));
    }
    if history.len() > expected_migrations().count()
        || history
            .iter()
            .any(|row| !expected_migrations().any(|migration| migration.version == row.version))
    {
        return Err(PostgresSchemaError::rejected("SCHEMA_HISTORY_UNKNOWN"));
    }
    if history.len() < expected_migrations().count() {
        return Err(PostgresSchemaError::rejected("SCHEMA_HISTORY_MISSING"));
    }
    if history
        .iter()
        .zip(expected_migrations())
        .any(|(row, migration)| {
            row.version != migration.version
                || row.checksum.as_slice() != migration.checksum.as_ref()
        })
    {
        return Err(PostgresSchemaError::rejected("SCHEMA_HISTORY_MISMATCH"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
