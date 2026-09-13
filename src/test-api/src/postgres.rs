use crate::IntegrationTestService;
use async_trait::async_trait;
use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, ConnectOptions, Executor, PgConnection, PgPool};
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use tokio::sync::OnceCell;
use tracing::debug;

#[path = "postgres_fixture.rs"]
mod fixture;
use fixture::DockerFixture;

const POSTGRES_USER: &str = "postgres";
const POSTGRES_PASSWORD: &str = "postgres";
const POSTGRES_DB: &str = "postgres";

const POSTGRES_CONTAINER_NAME_PREFIX: &str = "aura-historia-aws-backend-postgres-test";
const POSTGRES_PG_TTL_IMAGE: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "src/test-api/postgres/image-ref.txt"
));
const HOST_GATEWAY: &str = "host.docker.internal";

type MigrationInitializers = Mutex<HashMap<&'static str, Arc<OnceCell<()>>>>;

/// Guards the one-time startup of the Postgres container.
///
/// [`tokio::sync::OnceCell`] is used so concurrent async callers all await the same
/// initialisation future instead of racing to start duplicate containers.
static POSTGRES_CONTAINER_STARTED: OnceCell<u16> = OnceCell::const_new();
static OWNED_CONTAINER: Mutex<Option<DockerFixture>> = Mutex::new(None);
static MIGRATIONS_APPLIED: OnceLock<MigrationInitializers> = OnceLock::new();

fn postgres_container_name() -> String {
    format!("{POSTGRES_CONTAINER_NAME_PREFIX}-{}", std::process::id())
}

fn postgres_host_port() -> u16 {
    *POSTGRES_CONTAINER_STARTED
        .get()
        .expect("Postgres host port not initialized; call `ensure_container_started()` first")
}

fn connection_string() -> String {
    postgres_connection_string("localhost", POSTGRES_DB)
}

pub fn get_postgres_host_gateway_connection_string(database: &str) -> String {
    postgres_connection_string(HOST_GATEWAY, database)
}

#[cfg(feature = "sequin")]
pub(crate) fn get_postgres_host_port() -> u16 {
    postgres_host_port()
}

fn postgres_connection_string(host: &str, database: &str) -> String {
    format!(
        "postgres://{}:{}@{}:{}/{}",
        POSTGRES_USER,
        POSTGRES_PASSWORD,
        host,
        postgres_host_port(),
        database,
    )
}

/// Opens a fresh [`PgConnection`] to the test Postgres container.
///
/// Each call establishes a new TCP connection. It is not subject to any pool semaphore and
/// is fully owned by the current Tokio runtime. The caller is responsible for dropping it
/// before their runtime shuts down.
async fn open_connection() -> PgConnection {
    let opts = PgConnectOptions::from_str(&connection_string())
        .expect("shouldn't fail parsing Postgres connection string");
    opts.connect()
        .await
        .expect("shouldn't fail connecting to Postgres test container")
}

async fn provision_postgres(port: u16) -> io::Result<()> {
    let url =
        format!("postgres://{POSTGRES_USER}:{POSTGRES_PASSWORD}@127.0.0.1:{port}/{POSTGRES_DB}");
    let options = PgConnectOptions::from_str(&url)
        .map_err(|_| io::Error::other("invalid local Postgres fixture connection options"))?
        .disable_statement_logging();
    let mut connection = loop {
        match options.connect().await {
            Ok(connection) => break connection,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
        }
    };
    connection
        .execute(AssertSqlSafe("CREATE EXTENSION pg_ttl_index"))
        .await
        .map_err(|_| io::Error::other("Postgres fixture pg_ttl_index provisioning failed"))?;
    connection
        .execute(AssertSqlSafe("SELECT ttl_start_worker()"))
        .await
        .map_err(|_| io::Error::other("Postgres fixture pg_ttl_index worker startup failed"))?;
    Ok(())
}

/// Ensures one process-lived, locally owned container; startup cancellation also cleans up.
async fn ensure_container_started() {
    POSTGRES_CONTAINER_STARTED
        .get_or_try_init(start_container)
        .await
        .unwrap_or_else(|error| panic!("Postgres fixture startup failed: {error}"));
}

async fn start_container() -> io::Result<u16> {
    let started = Instant::now();
    let mut container = DockerFixture::local()?;
    let image = match std::env::var("AURA_TEST_POSTGRES_IMAGE") {
        Ok(image) => image,
        Err(std::env::VarError::NotPresent) => POSTGRES_PG_TTL_IMAGE.trim().to_owned(),
        Err(_) => {
            return Err(io::Error::other(
                "invalid local Postgres fixture image override",
            ));
        }
    };
    container.create(&postgres_container_name(), &image)?;
    {
        let mut owned = OWNED_CONTAINER
            .lock()
            .map_err(|_| io::Error::other("Postgres fixture ownership lock poisoned"))?;
        if owned.is_some() {
            return Err(io::Error::other(
                "Postgres fixture still has an owned container",
            ));
        }
        *owned = Some(container);
    }
    let mut startup = StartupCleanup { armed: true };
    install_cleanup()?;
    let port = {
        let owned = OWNED_CONTAINER
            .lock()
            .map_err(|_| io::Error::other("Postgres fixture ownership lock poisoned"))?;
        owned
            .as_ref()
            .ok_or_else(|| io::Error::other("Postgres fixture lost startup ownership"))?
            .start()?
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), provision_postgres(port))
        .await
        .map_err(|_| io::Error::other("Postgres fixture readiness/provisioning timed out"))??;
    debug!(
        elapsed_ms = started.elapsed().as_millis(),
        pid = std::process::id(),
        "Postgres container started with pg_ttl_index."
    );
    startup.armed = false;
    Ok(port)
}

struct StartupCleanup {
    armed: bool,
}

impl Drop for StartupCleanup {
    fn drop(&mut self) {
        if self.armed {
            cleanup();
        }
    }
}

fn cleanup_owned_container() -> io::Result<()> {
    let mut owned = match OWNED_CONTAINER.lock() {
        Ok(owned) => owned,
        // A panic must not erase already acquired cleanup authority.
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(container) = owned.as_mut() {
        container.cleanup()?;
    }
    owned.take();
    Ok(())
}

extern "C" fn cleanup() {
    if let Err(error) = cleanup_owned_container() {
        eprintln!("{error}");
    }
}

/// Register only after successful ID acquisition; hooks never discover ownership by name.
fn install_cleanup() -> io::Result<()> {
    static INIT: OnceLock<Result<(), &'static str>> = OnceLock::new();
    INIT.get_or_init(|| {
        // SAFETY: fixed C-ABI callback, process-lived state, no unwinding from cleanup.
        if unsafe { libc::atexit(cleanup) } != 0 {
            return Err("could not register owned Postgres exit cleanup");
        }
        crate::signal::register_signal_cleanup(|| cleanup());
        Ok(())
    })
    .map_err(io::Error::other)
}

/// Returns a fresh [`PgPool`] connected to the test Postgres container.
///
/// A **new pool is created on every call** and is owned by the caller. This is intentional:
/// `#[tokio::test]` creates a separate Tokio runtime per test and shuts it down when the
/// test ends. `PgPool` internally spawns tasks (e.g. `return_to_pool`) on the current
/// runtime via `tokio::runtime::Handle::spawn`. When the runtime shuts down those tasks are
/// dropped without running, permanently leaking the pool's internal semaphore permits. After
/// only a handful of tests the pool's `acquire` call times out even though no real connections
/// are in use.
///
/// Returning an owned, per-call pool means the caller drops it at the end of the test
/// function, while the test's runtime is still alive, so all pool-internal cleanup futures
/// are properly driven to completion.
///
/// # Returns
///
/// A newly created [`PgPool`] that the caller should drop before its Tokio runtime shuts down
/// (i.e. before the test function returns).
pub async fn get_postgres_client() -> PgPool {
    ensure_container_started().await;

    let pool = PgPool::connect(&connection_string())
        .await
        .expect("shouldn't fail creating Postgres pool for test container");

    debug!("Successfully created Postgres PgPool for current test.");
    pool
}

/// Test helper representing a plain Postgres database for integration tests.
///
/// Unlike AWS service helpers this helper does **not** use LocalStack. It spins up a real
/// Postgres Docker container via an explicit local Unix Docker CLI and manages it independently.
///
/// # Lifecycle
///
/// - **Before each test** (`set_up`): Starts the Postgres container once per process.
///   [`Postgres::new`] and [`Postgres::new_schema_once`] apply schema-only migrations once per
///   test process; `setup_script` still runs before each test. [`Postgres::new_per_test`]
///   replays migrations before each test when they provide seed data.
/// - **After each test** (`tear_down`): Opens a fresh connection and truncates application-owned
///   tables in the `public` schema so that each test starts with a clean slate. Extension-owned
///   metadata and table definitions (DDL) are preserved.
///
/// # Connection strategy
///
/// `set_up` and `tear_down` each open a **new** [`PgConnection`] and close it when done.
/// This avoids cross-runtime resource invalidation: `#[tokio::test]` creates a new Tokio
/// runtime per test and shuts it down afterwards. Any I/O handle (socket, timer, etc.)
/// created on one runtime becomes unusable on another. A fresh connection per call is the
/// only correct approach when the service struct is shared as a `const` across tests.
///
/// # Usage
///
/// ```rust
/// use test_api::*;
///
/// const POSTGRES: Postgres = Postgres::new("migrations");
///
/// #[aura_integration_test(services = [POSTGRES])]
/// async fn should_insert_and_read_row() {
///     let pool = get_postgres_client().await;
///     sqlx::query("INSERT INTO items (id) VALUES (1)").execute(pool).await.unwrap();
/// }
/// ```
///
/// # Notes
///
/// - `service_names` returns `&[]` because Postgres is not a LocalStack service. The
///   `#[aura_integration_test]` macro will still start LocalStack if other services in the same
///   test require it.
/// - The container is shared across all tests within the same test-suite binary. Only the
///   data is reset between tests.
/// - Adding a new migration file to `migrations_dir` is automatically picked up by all tests
///   — no changes to test code required.
#[derive(Debug, Clone, Copy)]
pub struct Postgres {
    /// Path to the directory containing versioned `*.sql` migration files, relative to the
    /// workspace root. Files are executed in lexicographic (filename) order, matching the
    /// ordering used by `sqlx::migrate!` at runtime.
    pub migrations_dir: &'static str,
    /// Optional SQL file, relative to workspace root, run after migrations and before the test.
    pub setup_script: Option<&'static str>,
    migrate_once: bool,
}

async fn apply_migrations(migrations_dir: &'static str) {
    let started = Instant::now();
    let workspace_root = env!("CARGO_WORKSPACE_DIR");
    let dir_path = Path::new(workspace_root).join(migrations_dir);
    let mut entries: Vec<_> = std::fs::read_dir(&dir_path)
        .unwrap_or_else(|error| {
            panic!(
                "failed to read migrations directory '{}': {error}",
                dir_path.display()
            )
        })
        .filter_map(|entry| {
            let entry =
                entry.unwrap_or_else(|error| panic!("failed to read migration entry: {error}"));
            let path = entry.path();
            (path.extension().and_then(|extension| extension.to_str()) == Some("sql"))
                .then_some(path)
        })
        .collect();
    entries.sort();

    let mut connection = open_connection().await;
    for path in &entries {
        let sql = std::fs::read_to_string(path).unwrap_or_else(|error| {
            panic!(
                "failed to read migration file '{}': {error}",
                path.display()
            )
        });
        connection
            .execute(AssertSqlSafe(sql))
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "failed to execute migration file '{}': {error}",
                    path.display()
                )
            });
    }

    debug!(
        migrations_dir,
        count = entries.len(),
        elapsed_ms = started.elapsed().as_millis(),
        "Applied Postgres migrations."
    );
}

async fn apply_setup_script(setup_script: &'static str) {
    let script_path = Path::new(env!("CARGO_WORKSPACE_DIR")).join(setup_script);
    let sql = std::fs::read_to_string(&script_path).unwrap_or_else(|error| {
        panic!(
            "failed to read setup script '{}': {error}",
            script_path.display()
        )
    });
    let mut connection = open_connection().await;
    connection
        .execute(AssertSqlSafe(sql))
        .await
        .unwrap_or_else(|error| {
            panic!(
                "failed to execute setup script '{}': {error}",
                script_path.display()
            )
        });
    debug!(path = %script_path.display(), "Applied Postgres setup script.");
}

impl Postgres {
    /// Uses a migration directory containing schema only, without migration-provided seed data.
    /// The directory is applied once per test process; data isolation still uses truncation.
    pub const fn new(migrations_dir: &'static str) -> Self {
        Self {
            migrations_dir,
            setup_script: None,
            migrate_once: true,
        }
    }

    /// Alias for [`Postgres::new`] for callers that want to document schema-only intent.
    pub const fn new_schema_once(migrations_dir: &'static str) -> Self {
        Self {
            migrations_dir,
            setup_script: None,
            migrate_once: true,
        }
    }

    /// Reapplies migrations before each test to restore migration-provided seed data.
    pub const fn new_per_test(migrations_dir: &'static str) -> Self {
        Self {
            migrations_dir,
            setup_script: None,
            migrate_once: false,
        }
    }

    pub const fn with_setup_script(
        migrations_dir: &'static str,
        setup_script: &'static str,
    ) -> Self {
        Self {
            migrations_dir,
            setup_script: Some(setup_script),
            migrate_once: false,
        }
    }
}

#[async_trait]
impl IntegrationTestService for Postgres {
    /// Returns an empty slice because Postgres is managed independently of LocalStack.
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    /// Starts the Postgres container and applies migrations according to the selected lifecycle.
    async fn set_up(&self) {
        ensure_container_started().await;

        if self.migrate_once {
            let migrations = MIGRATIONS_APPLIED
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .unwrap_or_else(|error| panic!("Postgres migration state lock poisoned: {error}"))
                .entry(self.migrations_dir)
                .or_insert_with(|| Arc::new(OnceCell::const_new()))
                .clone();
            let migrations_dir = self.migrations_dir;

            migrations
                .get_or_init(|| async move {
                    apply_migrations(migrations_dir).await;
                })
                .await;
        } else {
            apply_migrations(self.migrations_dir).await;
        }

        if let Some(setup_script) = self.setup_script {
            apply_setup_script(setup_script).await;
        }
    }

    /// Truncates application-owned tables in the `public` schema to ensure test isolation.
    ///
    /// Table definitions (DDL) are intentionally kept intact so that the next test's
    /// `set_up` can rely on `CREATE TABLE IF NOT EXISTS` being a no-op.
    async fn tear_down(&self) {
        let started = Instant::now();
        let mut conn = open_connection().await;

        // Exclude relations owned by installed extensions. Extension metadata must survive
        // per-test cleanup, and this catalog query avoids coupling to extension table names.
        let tables: Vec<String> = sqlx::query_scalar::<_, String>(AssertSqlSafe(
            "SELECT relation.relname \
             FROM pg_class AS relation \
             JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace \
             LEFT JOIN pg_depend AS extension_dependency \
               ON extension_dependency.classid = 'pg_class'::regclass \
              AND extension_dependency.objid = relation.oid \
              AND extension_dependency.deptype = 'e' \
             WHERE namespace.nspname = 'public' \
               AND relation.relkind = 'r' \
               AND extension_dependency.objid IS NULL",
        ))
        .fetch_all(&mut conn)
        .await
        .expect("shouldn't fail querying table names for tear-down");

        if tables.is_empty() {
            return;
        }

        // Build a single TRUNCATE statement for all tables. CASCADE handles FK constraints.
        let table_list = tables
            .iter()
            .map(|table| format!("\"{}\"", table.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(", ");

        let truncate_sql = AssertSqlSafe(format!(
            "TRUNCATE TABLE {table_list} RESTART IDENTITY CASCADE"
        ));

        conn.execute(truncate_sql)
            .await
            .expect("shouldn't fail truncating tables for tear-down");

        debug!(
            tables = ?tables,
            elapsed_ms = started.elapsed().as_millis(),
            "Truncated application-owned public tables for test isolation."
        );
    }
}
