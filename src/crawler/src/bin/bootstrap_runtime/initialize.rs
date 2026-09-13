use super::{CLOSE_TIMEOUT, Code, Failure, Target, WORK_TIMEOUT, verify_schema};
use platform_postgres::PostgresPoolConfig;
use sqlx::{
    Connection, PgConnection, Postgres, QueryBuilder,
    migrate::{Migrate, Migrator},
};
use std::collections::BTreeSet;
use tokio::time::timeout;

#[path = "ttl.rs"]
mod ttl;

static BUSINESS: Migrator = sqlx::migrate!("../../migrations");
static CRAWLER: Migrator = sqlx::migrate!("./migrations");
const BUSINESS_VERSIONS: &[i64] = &[20260725090000];
const CRAWLER_VERSIONS: &[i64] = &[
    20260101000000,
    20260514000000,
    20260711000000,
    20260817000000,
    20260831000000,
    20260831000001,
];
// Same database-scoped key for both targets: no simultaneous business/crawler initializer.
const INIT_LOCK: i64 = 0x4155524146524553;
const OBJECT_LIMIT: i64 = 4096;

fn source(target: Target) -> Result<&'static Migrator, Failure> {
    let (source, versions) = match target {
        Target::Business => (&BUSINESS, BUSINESS_VERSIONS),
        Target::Crawler => (&CRAWLER, CRAWLER_VERSIONS),
    };
    guard_source(source, versions)?;
    Ok(source)
}

fn guard_source(source: &Migrator, versions: &[i64]) -> Result<(), Failure> {
    // SQLx 0.9.0 semver-exempt fields deliberately audited against the exact workspace pin.
    // Refuse source evolution instead of silently turning this into incremental migration.
    if source.no_tx
        || !source.locking
        || source.ignore_missing
        || source.table_name != "_sqlx_migrations"
        || !source.create_schemas.is_empty()
        || source.iter().count() != versions.len()
        || source.iter().zip(versions).any(|(migration, version)| {
            migration.no_tx
                || migration.migration_type != sqlx::migrate::MigrationType::Simple
                || migration.version != *version
        })
    {
        return Err(Failure::new(Code::UnsupportedSource));
    }
    Ok(())
}

pub(super) async fn run(
    target: Target,
    config: &PostgresPoolConfig,
    possible_writes: &mut bool,
) -> Result<(), Failure> {
    let migrations = source(target)?;
    // Retain session and post-apply verification pool outside cancellable work.
    let pool = config
        .pool_options()
        .connect_lazy_with(config.connect_options());
    let mut owned = None;
    let result = timeout(WORK_TIMEOUT, async {
        owned = Some(
            config
                .connect_session()
                .await
                .map_err(|error| Failure::caused(Code::Dependency, error))?,
        );
        let connection = owned.as_mut().ok_or_else(|| Failure::new(Code::Runtime))?;
        sqlx::raw_sql(
            "SET statement_timeout = '30s';
             SET lock_timeout = '2s';
             SET idle_in_transaction_session_timeout = '10s';
             SET idle_session_timeout = '10s';
             SET search_path = public;
             SET row_security = off;",
        )
        .execute(&mut *connection)
        .await?;
        fresh(connection, target).await?;
        let locked: bool = sqlx::query_scalar("SELECT pg_catalog.pg_try_advisory_lock($1)")
            .bind(INIT_LOCK)
            .fetch_one(&mut *connection)
            .await?;
        if !locked {
            return Err(Failure::new(Code::Dependency));
        }
        // Take SQLx's own lock before the final freshness read as well. run retains its
        // default lock (reentrant on this session); closing releases both acquisitions.
        Migrate::lock(connection)
            .await
            .map_err(|error| Failure::caused(Code::Dependency, error))?;
        fresh(connection, target).await?;
        // Even ledger creation is a potential durable effect. Never infer rollback from
        // a SQLx error, timeout, close, or a later read-only verification failure.
        *possible_writes = true;
        migrations
            .run(&mut *connection)
            .await
            .map_err(|error| Failure::caused(Code::UnknownOutcome, error))?;
        verify_schema(target, &pool).await
    })
    .await
    .unwrap_or_else(|error| Err(Failure::caused(Code::Deadline, error)));
    let closed = timeout(CLOSE_TIMEOUT, async {
        let session_close = async {
            match owned {
                Some(connection) => connection
                    .close()
                    .await
                    .map_err(|error| Failure::caused(Code::Cleanup, error)),
                None => Ok(()),
            }
        };
        let (session, ()) = tokio::join!(session_close, pool.close());
        session
    })
    .await
    .map_err(|error| Failure::caused(Code::Cleanup, error))
    .and_then(|result| result);
    // Cleanup failure wins over a pre-write result; every post-write failure is unknown.
    closed
        .and(result)
        .map_err(|error| error.after_writes(*possible_writes))
}

#[derive(sqlx::FromRow)]
struct Extension {
    name: String,
    schema: String,
}

fn extensions_allowed(target: Target, extensions: &[Extension]) -> bool {
    extensions.iter().all(
        |extension| match (extension.name.as_str(), extension.schema.as_str()) {
            ("plpgsql", "pg_catalog") => true,
            ("pg_trgm" | "unaccent" | "pg_ttl_index", "public") => target == Target::Business,
            ("pgcrypto", "public") => target == Target::Crawler,
            _ => false,
        },
    )
}

#[derive(Default, sqlx::FromRow)]
struct FreshnessPrelude {
    public_schema_exists: bool,
    unknown_schema_exists: bool,
    ledger_exists: bool,
    large_object_exists: bool,
    subscription_exists: bool,
}

impl FreshnessPrelude {
    fn allows_initialization(&self) -> bool {
        self.public_schema_exists
            && !(self.unknown_schema_exists
                || self.ledger_exists
                || self.large_object_exists
                || self.subscription_exists)
    }
}

/// Conservative stock-initdb freshness policy, not hostile-admin attestation. PostgreSQL
/// reserves OIDs below FirstNormalObjectId (16384) for built-ins; all higher-OID objects
/// in every non-shared OID-bearing system catalog must be extension members or internal
/// descendants. Shared roles/databases are outside target scope; subscriptions are
/// shared but database-owned, so checked separately. Large objects can have explicit
/// low OIDs: refuse ALL, not just normal OIDs. Unknown namespaces and any ledger override
/// extension allowance. No normal/auto dependency traversal: an unrelated
/// object referencing an extension (including a new index on its table) must not pass.
/// Only ttl's separately validated, empty 3.0.0 configuration-table structure adds
/// specific automatic-dependent OIDs; it does not broaden the recursive closure.
/// This may refuse unusual templates/prerequisite layouts rather than adopt them.
async fn fresh(connection: &mut PgConnection, target: Target) -> Result<(), Failure> {
    let prelude: FreshnessPrelude = sqlx::query_as(
        "SELECT EXISTS (SELECT FROM pg_catalog.pg_namespace WHERE nspname = 'public') AS public_schema_exists,
         EXISTS (SELECT FROM pg_catalog.pg_namespace
             WHERE nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast', 'public')) AS unknown_schema_exists,
         EXISTS (SELECT FROM pg_catalog.pg_class WHERE relname = '_sqlx_migrations') AS ledger_exists,
         EXISTS (SELECT FROM pg_catalog.pg_largeobject_metadata) AS large_object_exists,
         EXISTS (SELECT FROM pg_catalog.pg_subscription
             WHERE subdbid = (SELECT oid FROM pg_catalog.pg_database
                 WHERE datname = pg_catalog.current_database())) AS subscription_exists",
    )
    .fetch_one(&mut *connection)
    .await?;
    if !prelude.allows_initialization() {
        return Err(Failure::new(Code::NotFresh));
    }
    let extensions: Vec<Extension> = sqlx::query_as(
        "SELECT e.extname AS name, n.nspname AS schema FROM pg_catalog.pg_extension e
         JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace LIMIT 9",
    )
    .fetch_all(&mut *connection)
    .await?;
    if extensions.len() > 8 || !extensions_allowed(target, &extensions) {
        return Err(Failure::new(Code::NotFresh));
    }
    let allowed: Vec<(i64, i64)> = sqlx::query_as(
        "WITH RECURSIVE allowed(classid, objid) AS (
             SELECT 'pg_catalog.pg_extension'::pg_catalog.regclass::oid, oid
             FROM pg_catalog.pg_extension
             UNION
             SELECT d.classid, d.objid FROM pg_catalog.pg_depend d
             JOIN allowed a ON a.classid = d.refclassid AND a.objid = d.refobjid
             WHERE d.objsubid = 0 AND d.refobjsubid = 0
               AND (d.deptype = 'i' OR
                    (d.deptype = 'e' AND a.classid = 'pg_catalog.pg_extension'::pg_catalog.regclass))
         ) SELECT classid::bigint, objid::bigint FROM allowed LIMIT $1"
    ).bind(OBJECT_LIMIT + 1).fetch_all(&mut *connection).await?;
    if allowed.len() > OBJECT_LIMIT as usize {
        return Err(Failure::new(Code::NotFresh));
    }
    let mut allowed: BTreeSet<_> = allowed.into_iter().collect();
    if target == Target::Business
        && extensions
            .iter()
            .any(|extension| extension.name == "pg_ttl_index")
    {
        allowed.extend(ttl::allowance(connection).await?);
    }
    let catalogs: Vec<(i64, String)> = sqlx::query_as(
        "SELECT c.oid::bigint, c.relname FROM pg_catalog.pg_class c
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'pg_catalog' AND c.relkind = 'r' AND NOT c.relisshared
           AND c.oid < 16384 AND EXISTS (
             SELECT FROM pg_catalog.pg_attribute a
             WHERE a.attrelid = c.oid AND a.attname = 'oid' AND a.attnum > 0
               AND NOT a.attisdropped AND a.atttypid = 'pg_catalog.oid'::pg_catalog.regtype)
         ORDER BY c.oid LIMIT 129",
    )
    .fetch_all(&mut *connection)
    .await?;
    if catalogs.is_empty() || catalogs.len() > 128 {
        return Err(Failure::new(Code::NotFresh));
    }
    for (class, name) in catalogs {
        // Identifier comes from a built-in catalog, still validate before QueryBuilder.
        if !catalog_identifier(&name) {
            return Err(Failure::new(Code::NotFresh));
        }
        let mut query = QueryBuilder::<Postgres>::new("SELECT oid::bigint FROM pg_catalog.\"");
        query
            .push(&name)
            .push("\" WHERE oid >= 16384 LIMIT ")
            .push_bind(OBJECT_LIMIT + 1);
        let objects: Vec<i64> = query
            .build_query_scalar()
            .fetch_all(&mut *connection)
            .await?;
        if !objects_allowed(class, &objects, &allowed) {
            return Err(Failure::new(Code::NotFresh));
        }
    }
    if target == Target::Business {
        let preloaded: String =
            sqlx::query_scalar("SELECT pg_catalog.current_setting('shared_preload_libraries')")
                .fetch_one(&mut *connection)
                .await?;
        if !extensions
            .iter()
            .any(|extension| extension.name == "pg_ttl_index")
            || !ttl_preloaded(&preloaded)
        {
            return Err(Failure::new(Code::Prerequisite));
        }
    }
    Ok(())
}

fn objects_allowed(class: i64, objects: &[i64], allowed: &BTreeSet<(i64, i64)>) -> bool {
    objects.len() <= OBJECT_LIMIT as usize
        && objects.iter().all(|oid| allowed.contains(&(class, *oid)))
}

fn catalog_identifier(name: &str) -> bool {
    name.starts_with("pg_")
        && name.len() <= 63
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
}

fn ttl_preloaded(setting: &str) -> bool {
    setting.split(',').any(|library| {
        matches!(
            library.trim().trim_matches('"'),
            "pg_ttl_index" | "$libdir/pg_ttl_index"
        )
    })
}

#[cfg(test)]
#[path = "initialize_tests.rs"]
mod tests;
