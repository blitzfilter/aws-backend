use platform_postgres::{PostgresPoolConfig, PostgresTlsConfig};
use sqlx::{AssertSqlSafe, PgPool};
use std::{
    fs,
    future::Future,
    io::{self, Read},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub(super) const WAIT: Duration = Duration::from_secs(12);
pub(super) const ROLE_PASSWORD: &str = "cron-local-fixture-only";
static NEXT: AtomicU64 = AtomicU64::new(0);
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn unique_name() -> TestResult<String> {
    Ok(format!(
        "cron_i04_{}_{:x}_{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

#[derive(Clone)]
pub(super) struct Database {
    pub pool: PgPool,
    pub name: String,
    pub port: u16,
    pub role: String,
    pub role_owned: Arc<AtomicBool>,
}

impl Database {
    pub fn config(&self, application: &str, readonly: bool) -> TestResult<PostgresPoolConfig> {
        Ok(PostgresPoolConfig::new(
            "127.0.0.1".into(),
            self.port,
            self.name.clone(),
            if readonly {
                self.role.clone()
            } else {
                "postgres".into()
            },
            if readonly {
                ROLE_PASSWORD.into()
            } else {
                "postgres".into()
            },
            2,
            PostgresTlsConfig::new("test", "disable", None, application)?,
        )?)
    }

    pub async fn sessions(&self, application: &str) -> TestResult<Vec<i32>> {
        Ok(sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=$1 AND application_name=$2 ORDER BY pid")
            .bind(&self.name).bind(application).fetch_all(&self.pool).await?)
    }

    pub async fn advisory_pids(&self) -> TestResult<Vec<i32>> {
        Ok(sqlx::query_scalar("SELECT DISTINCT pid FROM pg_locks WHERE locktype='advisory' AND database=(SELECT oid FROM pg_database WHERE datname=$1) ORDER BY pid")
            .bind(&self.name).fetch_all(&self.pool).await?)
    }

    pub async fn no_sessions(&self, application: &str) -> TestResult {
        bounded("PostgreSQL session close", async {
            loop {
                if self.sessions(application).await?.is_empty() {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
    }
}

/// Starts exactly test-api's guarded local Postgres fixture, without its raw-SQL replay.
/// Each case owns a newly created template0 database; only SQLx creates its business ledger.
pub(super) async fn with_database<F, Fut>(test: F) -> TestResult
where
    F: FnOnce(Database) -> Fut + Send + 'static,
    Fut: Future<Output = TestResult> + Send + 'static,
{
    let _serial = SERIAL.lock().await;
    if std::env::var("AURA_CRON_ISOLATED_LOCAL_POSTGRES").as_deref() != Ok("1") {
        return Err("requires AURA_CRON_ISOLATED_LOCAL_POSTGRES=1 on an isolated local host; fixture publishes 0.0.0.0".into());
    }
    // This suite uses the shipped cached pin, never an ambient image override or PG options.
    for key in [
        "AURA_TEST_POSTGRES_IMAGE",
        "PGHOST",
        "PGPORT",
        "PGDATABASE",
        "PGUSER",
        "PGPASSWORD",
        "PGSSLCERT",
        "PGSSLKEY",
        "PGSSLROOTCERT",
        "PGOPTIONS",
    ] {
        if std::env::var_os(key).is_some() {
            return Err(format!("clear {key} before local fixture validation").into());
        }
    }
    let admin = test_api::get_postgres_client().await;
    let name = unique_name()?;
    let port = admin.connect_options().get_port();
    let created = sqlx::raw_sql(AssertSqlSafe(format!(
        "CREATE DATABASE {name} TEMPLATE template0"
    )))
    .execute(&admin)
    .await;
    if let Err(error) = created {
        admin.close().await;
        return Err(error.into());
    }
    let config = PostgresPoolConfig::new(
        "127.0.0.1".into(),
        port,
        name.clone(),
        "postgres".into(),
        "postgres".into(),
        3,
        PostgresTlsConfig::new("test", "disable", None, "cron-fixture-observer")?,
    )?;
    let role_owned = Arc::new(AtomicBool::new(false));
    let role = format!("{name}_ro");
    let connected = config.connect().await;
    let result = match connected {
        Ok(pool) => {
            let db = Database {
                pool: pool.clone(),
                name: name.clone(),
                port,
                role: role.clone(),
                role_owned: role_owned.clone(),
            };
            let mut case = tokio::spawn(async move {
                sqlx::raw_sql("CREATE EXTENSION pg_ttl_index")
                    .execute(&db.pool)
                    .await?;
                sqlx::migrate!("../../migrations").run(&db.pool).await?;
                let applied: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM public._sqlx_migrations WHERE success",
                )
                .fetch_one(&db.pool)
                .await?;
                assert_eq!(
                    applied, 1,
                    "current development baseline only; review fixture if baseline changes"
                );
                test(db).await
            });
            let result = match tokio::time::timeout(Duration::from_secs(45), &mut case).await {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => Err(error.into()),
                Err(_) => {
                    case.abort();
                    let joined = case.await;
                    assert!(matches!(joined, Err(error) if error.is_cancelled()));
                    Err("local PostgreSQL case exceeded 45s (aborted and joined; no retry)".into())
                }
            };
            pool.close().await;
            result
        }
        Err(error) => Err(error.into()),
    };
    // No FORCE: a leaked child/session must fail cleanup, not be hidden by fixture teardown.
    let dropped = sqlx::raw_sql(AssertSqlSafe(format!("DROP DATABASE {name}")))
        .execute(&admin)
        .await;
    let dropped_role = if role_owned.load(Ordering::Acquire) {
        sqlx::raw_sql(AssertSqlSafe(format!("DROP ROLE {role}")))
            .execute(&admin)
            .await
            .map(|_| ())
    } else {
        Ok(())
    };
    admin.close().await;
    dropped?;
    dropped_role?;
    result
}

pub(super) async fn bounded<T>(
    label: &str,
    work: impl Future<Output = TestResult<T>>,
) -> TestResult<T> {
    tokio::time::timeout(WAIT, work)
        .await
        .map_err(|_| format!("timed out waiting for {label}; no retry"))?
}

pub(super) struct Directory(pub PathBuf);
impl Directory {
    pub fn new() -> TestResult<Self> {
        let path = std::env::temp_dir().join(unique_name()?);
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("owned cron fixture directory cleanup failed: {error}");
        }
    }
}

pub(super) async fn wait_file(directory: &Path, name: &str) -> TestResult {
    bounded(name, async {
        loop {
            if directory.join(name).try_exists()? {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
}

pub(super) fn mark(directory: &Path, name: &str) -> io::Result<()> {
    fs::write(directory.join(name), b"ready")
}

pub(super) fn child_environment(
    command: &mut Command,
    db: &Database,
    directory: &Path,
    readonly: bool,
) {
    command
        .env_clear()
        .env("STAGE", "test")
        .env("POSTGRES_SSL_MODE", "disable")
        .env("POSTGRES_HOST", "127.0.0.1")
        .env("POSTGRES_PORT", db.port.to_string())
        .env("POSTGRES_DATABASE", &db.name)
        .env(
            "POSTGRES_USERNAME",
            if readonly { &db.role } else { "postgres" },
        )
        .env(
            "POSTGRES_PASSWORD",
            if readonly { ROLE_PASSWORD } else { "postgres" },
        )
        .env("POSTGRES_MAX_CONNECTIONS", "2")
        .env("HOME", directory)
        .env("CLOUDSDK_CONFIG", directory)
        .env(
            "GOOGLE_APPLICATION_CREDENTIALS",
            directory.join("invalid-adc.json"),
        )
        .env("AWS_EC2_METADATA_DISABLED", "true")
        .env(
            "AURA_HISTORIA_CRON_ENABLED_JOBS",
            "search-filter-periodic-match",
        )
        .env("AURA_HISTORIA_CRON_HEALTH_BIND_ADDR", "127.0.0.1:0")
        .env("AURA_HISTORIA_CRON_SHUTDOWN_GRACE_SECONDS", "1")
        .env("AURA_HISTORIA_CRON_STOP_TIMEOUT_SECONDS", "31")
        .env("LOG_LEVEL", "info")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

pub(super) struct Process {
    child: Child,
    readers: Vec<JoinHandle<io::Result<String>>>,
}
impl Process {
    pub fn spawn(command: &mut Command) -> TestResult<Self> {
        let child = command.spawn()?;
        let mut owned = Self {
            child,
            readers: Vec::new(),
        };
        let stdout = owned.child.stdout.take().ok_or("missing child stdout")?;
        let stderr = owned.child.stderr.take().ok_or("missing child stderr")?;
        owned
            .readers
            .push(thread::spawn(move || read_output(stdout)));
        owned
            .readers
            .push(thread::spawn(move || read_output(stderr)));
        Ok(owned)
    }
    pub fn running(&mut self) -> TestResult<bool> {
        Ok(self.child.try_wait()?.is_none())
    }
    pub async fn wait_marker(&mut self, directory: &Path, name: &str) -> TestResult {
        bounded(name, async {
            loop {
                if directory.join(name).try_exists()? {
                    return Ok(());
                }
                if let Some(status) = self.child.try_wait()? {
                    let output = self.output()?;
                    return Err(
                        format!("private child exited {status} before {name}: {output}").into(),
                    );
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
    }
    pub fn kill(&mut self) -> TestResult<ExitStatus> {
        self.child.kill()?;
        Ok(self.child.wait()?)
    }
    pub async fn wait(&mut self) -> TestResult<(ExitStatus, String)> {
        let status = bounded("owned child exit", async {
            loop {
                if let Some(status) = self.child.try_wait()? {
                    return Ok(status);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        Ok((status, self.output()?))
    }
    fn output(&mut self) -> TestResult<String> {
        join_output_readers(&mut self.readers)
    }
}
fn join_output_readers(readers: &mut Vec<JoinHandle<io::Result<String>>>) -> TestResult<String> {
    let mut output = String::new();
    let mut read_errors = 0;
    let mut panics = 0;
    // Returning inside drain would detach every remaining reader on the first failure.
    for reader in readers.drain(..) {
        match reader.join() {
            Ok(Ok(text)) => output.push_str(&text),
            Ok(Err(_)) => read_errors += 1,
            Err(_) => panics += 1,
        }
    }
    if read_errors != 0 || panics != 0 {
        return Err(format!(
            "child output readers failed: read_errors={read_errors}, panics={panics}"
        )
        .into());
    }
    Ok(output)
}
fn read_output(mut reader: impl Read) -> io::Result<String> {
    // Drain continuously even past the retained diagnostic cap; a noisy child cannot block.
    let mut output = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(String::from_utf8_lossy(&output).into_owned());
        }
        let retain = count.min(65536usize.saturating_sub(output.len()));
        output.extend_from_slice(&buffer[..retain]);
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            if let Err(error) = self.child.kill() {
                eprintln!("owned cron child kill failed: {error}");
            }
            if let Err(error) = self.child.wait() {
                eprintln!("owned cron child reap failed: {error}");
            }
        }
        if let Err(error) = self.output() {
            eprintln!("owned cron child output join failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn should_join_all_output_readers_before_reporting_read_errors_or_panics() -> TestResult {
        for panic_first in [false, true] {
            let failed = thread::spawn(|| Err(io::Error::other("read-error-canary")));
            // Exercise JoinHandle's panic result without printing a panic payload.
            let panicked = thread::spawn(|| std::panic::resume_unwind(Box::new("panic-canary")));
            let (release, released) = mpsc::channel();
            let (reader_started, started) = mpsc::channel();
            let (reader_finished, finished) = mpsc::channel();
            let blocked = thread::spawn(move || {
                reader_started
                    .send(())
                    .map_err(|_| io::Error::other("reader start observer lost"))?;
                let result = released.recv_timeout(Duration::from_secs(5));
                reader_finished
                    .send(())
                    .map_err(|_| io::Error::other("reader finish observer lost"))?;
                result.map_err(|_| io::Error::other("reader gate was not released"))?;
                Ok("output-canary".to_owned())
            });
            let mut readers = if panic_first {
                vec![panicked, failed, blocked]
            } else {
                vec![failed, panicked, blocked]
            };
            thread::scope(|scope| -> TestResult {
                let (collector_started, collecting) = mpsc::channel();
                let (collector_finished, collected) = mpsc::channel();
                let collector = scope.spawn(move || -> TestResult<_> {
                    collector_started.send(())?;
                    let result = join_output_readers(&mut readers);
                    collector_finished.send(())?;
                    Ok((result, readers.is_empty()))
                });
                let observation = (|| -> TestResult<_> {
                    started.recv_timeout(Duration::from_secs(5))?;
                    collecting.recv_timeout(Duration::from_secs(5))?;
                    Ok(collected.recv_timeout(Duration::from_millis(100)))
                })();
                // Release and join before asserting, including on an observation failure.
                let released = release.send(());
                let joined = collector.join();
                let finished = finished.recv_timeout(Duration::from_secs(5));
                let observation = observation?;
                released?;
                let (result, empty) = joined.map_err(|_| "output collector panicked")??;
                finished?;
                assert!(
                    matches!(observation, Err(mpsc::RecvTimeoutError::Timeout)),
                    "collector returned while a reader was still blocked"
                );
                assert!(empty, "reader handles remain after collection");
                let error = result.err().ok_or("reader failures were not reported")?;
                assert_eq!(
                    error.to_string(),
                    "child output readers failed: read_errors=1, panics=1"
                );
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn should_preserve_output_order_when_all_readers_succeed() -> TestResult {
        let mut readers = vec![
            thread::spawn(|| Ok("stdout".to_owned())),
            thread::spawn(|| Ok("stderr".to_owned())),
        ];
        assert_eq!(join_output_readers(&mut readers)?, "stdoutstderr");
        assert!(readers.is_empty());
        assert_eq!(join_output_readers(&mut readers)?, "");
        Ok(())
    }
}
