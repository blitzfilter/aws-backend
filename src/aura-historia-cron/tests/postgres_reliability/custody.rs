use super::fixture::*;
use async_trait::async_trait;
use aura_historia_cron::scheduled_job::{
    ActiveExecutionTracker, CronJob, CronJobExecutionError, CronJobExecutionOutcome, CronJobStatus,
    ScheduledJobRunner,
};
use platform_postgres::PostgresPoolConfig;
use search_filter_postgres::SqlxPeriodicSearchFilterMatchingRunLock;
use search_filter_service::ports::{
    PeriodicSearchFilterMatchingRunLease, PeriodicSearchFilterMatchingRunLock,
};
use sqlx::{Acquire, PgPool, Postgres, Transaction};
use std::{
    io,
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::Notify, task::JoinSet};

const LEASE: &str = "cron-lease-owner";
const WRITER: &str = "cron-transaction-owner";
const CONTENDER: &str = "cron-independent-owner";

fn lock(db: &Database, application: &str) -> TestResult<SqlxPeriodicSearchFilterMatchingRunLock> {
    Ok(SqlxPeriodicSearchFilterMatchingRunLock::new(
        db.config(application, false)?,
    ))
}

async fn acquire(
    db: &Database,
    application: &str,
) -> TestResult<Box<dyn PeriodicSearchFilterMatchingRunLease>> {
    lock(db, application)?
        .try_acquire()
        .await?
        .ok_or_else(|| "expected an independent lease acquisition".into())
}

async fn excluded(db: &Database) -> TestResult {
    assert!(
        lock(db, CONTENDER)?.try_acquire().await?.is_none(),
        "second owner acquired while work retained its lease"
    );
    db.no_sessions(CONTENDER).await
}

async fn transaction_open(db: &Database) -> TestResult<i32> {
    let sessions = db.sessions(WRITER).await?;
    assert_eq!(sessions.len(), 1);
    let pid = sessions[0];
    let open: bool = sqlx::query_scalar("SELECT state='idle in transaction' AND backend_xid IS NOT NULL FROM pg_stat_activity WHERE pid=$1").bind(pid).fetch_one(&db.pool).await?;
    assert!(
        open,
        "synthetic write must be in a real open PostgreSQL transaction"
    );
    let writing: bool = sqlx::query_scalar("SELECT EXISTS (SELECT FROM pg_locks WHERE pid=$1 AND locktype='relation' AND relation='public.users'::regclass AND mode='RowExclusiveLock' AND granted)").bind(pid).fetch_one(&db.pool).await?;
    assert!(writing);
    assert_eq!(writes(db).await?, 0, "uncommitted write became visible");
    Ok(pid)
}

async fn writes(db: &Database) -> TestResult<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM public.users")
        .fetch_one(&db.pool)
        .await?)
}

fn spawn(db: &Database, directory: &Directory, mode: &str) -> TestResult<Process> {
    let mut command = Command::new(std::env::current_exe()?);
    command.args([
        "--exact",
        "custody::custody_child",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]);
    child_environment(&mut command, db, &directory.0, false);
    command
        .env("CRON_PRIVATE_PG_FIXTURE", mode)
        .env("CRON_PRIVATE_PG_DIRECTORY", &directory.0);
    Process::spawn(&mut command)
}

async fn started(db: &Database, directory: &Directory, child: &mut Process) -> TestResult<i32> {
    child.wait_marker(&directory.0, "started").await?;
    assert!(child.running()?);
    let owners = db.sessions(LEASE).await?;
    assert_eq!(owners.len(), 1);
    assert_eq!(db.advisory_pids().await?, owners);
    let writer = transaction_open(db).await?;
    assert_ne!(
        owners[0], writer,
        "lease must have its own session, not the transaction connection"
    );
    excluded(db).await?;
    Ok(owners[0])
}

async fn joined_before_exit(
    db: &Database,
    directory: &Directory,
    child: &mut Process,
    committed: bool,
) -> TestResult {
    child
        .wait_marker(&directory.0, "joined-and-pool-closed")
        .await?;
    assert!(
        child.running()?,
        "process exit must not substitute for normal runtime cleanup"
    );
    db.no_sessions(LEASE).await?;
    db.no_sessions(WRITER).await?;
    assert!(db.advisory_pids().await?.is_empty());
    assert_eq!(writes(db).await?, i64::from(committed));
    let next = acquire(db, CONTENDER).await?;
    next.release().await?;
    db.no_sessions(CONTENDER).await?;
    assert!(db.advisory_pids().await?.is_empty());
    mark(&directory.0, "exit")?;
    let (status, output) = child.wait().await?;
    assert_eq!(
        status.code(),
        Some(0),
        "private runtime fixture failed: {output}"
    );
    Ok(())
}

#[tokio::test]
async fn should_exclude_independent_sessions_then_reacquire_after_release_and_disconnect()
-> TestResult {
    with_database(|db| async move {
        let first = acquire(&db, LEASE).await?;
        let first_pids = db.sessions(LEASE).await?;
        assert_eq!(first_pids.len(), 1);
        assert_eq!(db.advisory_pids().await?, first_pids);
        excluded(&db).await?;
        first.release().await?;
        db.no_sessions(LEASE).await?;
        assert!(db.advisory_pids().await?.is_empty());
        let second = acquire(&db, CONTENDER).await?;
        let second_pids = db.sessions(CONTENDER).await?;
        assert_eq!(second_pids.len(), 1);
        assert_ne!(first_pids, second_pids);
        assert!(lock(&db, LEASE)?.try_acquire().await?.is_none());
        // Dropping the public lease disconnects the dedicated SQLx session.
        drop(second);
        db.no_sessions(CONTENDER).await?;
        let third = acquire(&db, LEASE).await?;
        third.release().await?;
        db.no_sessions(LEASE).await?;
        assert!(db.advisory_pids().await?.is_empty());
        Ok(())
    })
    .await
}

async fn normal_outcome(mode: &'static str, committed: bool) -> TestResult {
    with_database(move |db| async move {
        let directory = Directory::new()?;
        let mut child = spawn(&db, &directory, mode)?;
        started(&db, &directory, &mut child).await?;
        mark(&directory.0, "finish")?;
        joined_before_exit(&db, &directory, &mut child, committed).await
    })
    .await
}

#[tokio::test]
async fn should_commit_before_successful_runtime_join_and_release() -> TestResult {
    normal_outcome("commit", true).await
}

#[tokio::test]
async fn should_roll_back_failed_work_before_runtime_join_and_release() -> TestResult {
    normal_outcome("fail", false).await
}

async fn destruction(mode: &'static str) -> TestResult {
    with_database(move |db| async move {
        let directory = Directory::new()?;
        let mut child = spawn(&db, &directory, mode)?;
        let owner = started(&db, &directory, &mut child).await?;
        if mode == "cancel" {
            mark(&directory.0, "cancel")?;
        }
        child.wait_marker(&directory.0, "destroying").await?;
        child
            .wait_marker(&directory.0, "runtime-retained-ownership")
            .await?;
        assert!(child.running()?);
        assert!(!directory.0.join("joined-and-pool-closed").try_exists()?);
        assert_eq!(db.advisory_pids().await?, vec![owner]);
        transaction_open(&db).await?;
        excluded(&db).await?;
        mark(&directory.0, "allow-destruction")?;
        joined_before_exit(&db, &directory, &mut child, false).await
    })
    .await
}

#[tokio::test]
async fn should_retain_pg_custody_and_local_overlap_until_timeout_destruction_is_joined()
-> TestResult {
    destruction("timeout").await
}

#[tokio::test]
async fn should_retain_pg_custody_and_closed_admission_until_cancel_destruction_is_joined()
-> TestResult {
    destruction("cancel").await
}

#[tokio::test]
async fn should_release_pg_sessions_and_roll_back_when_owner_process_is_killed() -> TestResult {
    with_database(|db| async move {
        let directory = Directory::new()?;
        let mut child = spawn(&db, &directory, "kill")?;
        started(&db, &directory, &mut child).await?;
        let status = child.kill()?;
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
        db.no_sessions(LEASE).await?;
        db.no_sessions(WRITER).await?;
        assert!(db.advisory_pids().await?.is_empty());
        assert_eq!(writes(&db).await?, 0);
        let next = acquire(&db, CONTENDER).await?;
        next.release().await?;
        db.no_sessions(CONTENDER).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn should_exit_failed_without_claiming_join_when_destruction_never_unblocks() -> TestResult {
    with_database(|db| async move {
        let directory = Directory::new()?;
        let mut child = spawn(&db, &directory, "stuck-drop")?;
        let owner = started(&db, &directory, &mut child).await?;
        child.wait_marker(&directory.0, "destroying").await?;
        child
            .wait_marker(&directory.0, "runtime-retained-ownership")
            .await?;
        assert_eq!(db.advisory_pids().await?, vec![owner]);
        transaction_open(&db).await?;
        excluded(&db).await?;
        let (status, _) = child.wait().await?;
        assert_eq!(
            status.code(),
            Some(1),
            "runtime OS watchdog must fail the private process"
        );
        assert!(!directory.0.join("joined-and-pool-closed").try_exists()?);
        db.no_sessions(LEASE).await?;
        db.no_sessions(WRITER).await?;
        assert!(db.advisory_pids().await?.is_empty());
        assert_eq!(writes(&db).await?, 0);
        let next = acquire(&db, CONTENDER).await?;
        next.release().await?;
        db.no_sessions(CONTENDER).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn should_expose_unsupported_fencing_when_only_the_lease_connection_is_lost() -> TestResult {
    with_database(|db| async move {
        let directory = Directory::new()?;
        let mut child = spawn(&db, &directory, "lease-lost")?;
        let owner = started(&db, &directory, &mut child).await?;
        // Exact observed PID, database and application guard; never terminate by broad name.
        let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE pid=$1 AND datname=$2 AND application_name=$3")
            .bind(owner).bind(&db.name).bind(LEASE).fetch_one(&db.pool).await?;
        assert!(terminated);
        db.no_sessions(LEASE).await?;
        let successor = acquire(&db, CONTENDER).await?;
        transaction_open(&db).await?;
        assert!(child.running()?);
        // This intentionally demonstrates the gap, not a safety guarantee: the old
        // transaction can still commit while another process owns the advisory lease.
        mark(&directory.0, "finish")?;
        child.wait_marker(&directory.0, "joined-and-pool-closed").await?;
        assert!(child.running()?);
        assert_eq!(writes(&db).await?, 1);
        assert_eq!(db.advisory_pids().await?, db.sessions(CONTENDER).await?);
        successor.release().await?;
        db.no_sessions(CONTENDER).await?;
        joined_before_exit(&db, &directory, &mut child, true).await
    }).await
}

// Everything below belongs only to this integration-test executable. The synthetic
// job holds a real lease and real SQLx transaction but does not run the business handler.
struct DestructionGate {
    directory: PathBuf,
    enabled: bool,
    destroying: Arc<Notify>,
}
impl Drop for DestructionGate {
    fn drop(&mut self) {
        if !self.enabled {
            return;
        }
        if let Err(error) = mark(&self.directory, "destroying") {
            eprintln!("fixture destruction marker failed: {error}");
            std::process::exit(2);
        }
        self.destroying.notify_one();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !self.directory.join("allow-destruction").exists() {
            if Instant::now() >= deadline {
                std::process::exit(2);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

struct OwnedWork<'a> {
    // Rust drops fields in declaration order. Keep both DB resources alive through
    // the deliberately blocked destructor; dropping the transaction is not commit.
    _destruction: DestructionGate,
    transaction: Transaction<'a, Postgres>,
    lease: Box<dyn PeriodicSearchFilterMatchingRunLease>,
}
struct TransactionJob {
    pool: PgPool,
    lease: SqlxPeriodicSearchFilterMatchingRunLock,
    directory: PathBuf,
    mode: String,
    started: Arc<Notify>,
    destroying: Arc<Notify>,
}
impl TransactionJob {
    async fn work(&self) -> TestResult {
        let lease = self
            .lease
            .try_acquire()
            .await?
            .ok_or("private job lease unavailable")?;
        let mut connection = self.pool.acquire().await?;
        connection.close_on_drop();
        let mut transaction = connection.begin().await?;
        sqlx::query("INSERT INTO public.users (user_id,email,tier,role) VALUES ('00000000-0000-4000-8000-000000000001','cron-fixture@example.invalid','FREE','USER')").execute(&mut *transaction).await?;
        let work = OwnedWork {
            _destruction: DestructionGate {
                directory: self.directory.clone(),
                enabled: matches!(self.mode.as_str(), "timeout" | "cancel" | "stuck-drop"),
                destroying: self.destroying.clone(),
            },
            transaction,
            lease,
        };
        mark(&self.directory, "started")?;
        self.started.notify_one();
        wait_file(&self.directory, "finish").await?;
        if self.mode == "fail" {
            return Err("synthetic job failure before commit".into());
        }
        work.transaction.commit().await?;
        work.lease.release().await?;
        Ok(())
    }
}
#[async_trait]
impl CronJob for TransactionJob {
    fn name(&self) -> &'static str {
        "pg-custody-fixture"
    }
    async fn execute(&self) -> Result<(), CronJobExecutionError> {
        self.work().await.map_err(|_| {
            CronJobExecutionError::from_source(io::Error::other(
                "synthetic PostgreSQL fixture job failed",
            ))
        })
    }
}

#[test]
#[ignore = "private parent-owned subprocess; never run directly or with blanket --ignored"]
fn custody_child() -> TestResult {
    let mode = std::env::var("CRON_PRIVATE_PG_FIXTURE")?;
    if !matches!(
        mode.as_str(),
        "commit" | "fail" | "timeout" | "cancel" | "kill" | "stuck-drop" | "lease-lost"
    ) {
        return Err("invalid private fixture mode".into());
    }
    let directory = PathBuf::from(
        std::env::var_os("CRON_PRIVATE_PG_DIRECTORY")
            .ok_or("private fixture directory required")?,
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let config = |app| {
            PostgresPoolConfig::from_lookup(app, |key| match std::env::var(key) {
                Ok(value) => Some(value),
                Err(std::env::VarError::NotPresent) => None,
                Err(std::env::VarError::NotUnicode(_)) => Some(String::new()),
            })
        };
        let pool = config(WRITER)?.connect().await?;
        let tracker = Arc::new(ActiveExecutionTracker::new());
        let started = Arc::new(Notify::new());
        let destroying = Arc::new(Notify::new());
        let runner = Arc::new(ScheduledJobRunner::new(
            Arc::new(TransactionJob {
                pool: pool.clone(),
                lease: SqlxPeriodicSearchFilterMatchingRunLock::new(config(LEASE)?),
                directory: directory.clone(),
                mode: mode.clone(),
                started: started.clone(),
                destroying: destroying.clone(),
            }),
            tracker.clone(),
            "* * * * * * *".into(),
            Some(Duration::from_secs(
                if matches!(mode.as_str(), "timeout" | "stuck-drop") {
                    2
                } else {
                    20
                },
            )),
        ));
        let attempt = tokio::spawn({
            let runner = runner.clone();
            async move { runner.execute_once().await }
        });
        started.notified().await;
        let mut draining = JoinSet::new();
        if mode == "cancel" {
            wait_file(&directory, "cancel").await?;
            let tracker = tracker.clone();
            draining.spawn(async move { tracker.drain(Duration::ZERO).await });
        }
        if matches!(mode.as_str(), "timeout" | "cancel" | "stuck-drop") {
            // No timer polling inside the process while a Tokio worker's drop is blocked.
            destroying.notified().await;
            assert!(
                !attempt.is_finished(),
                "runtime published terminal result before job destruction joined"
            );
            let overlap = runner.execute_once().await;
            assert_eq!(
                overlap.status(),
                if mode == "cancel" {
                    CronJobStatus::SkippedShutdown
                } else {
                    CronJobStatus::SkippedLocalOverlap
                }
            );
            if mode == "cancel" {
                assert!(
                    draining.try_join_next().is_none(),
                    "drain finished while destruction was blocked"
                );
            }
            mark(&directory, "runtime-retained-ownership")?;
        }
        let outcome = attempt.await?;
        assert!(
            matches!(
                (&*mode, &outcome),
                ("commit", CronJobExecutionOutcome::Succeeded)
                    | ("fail" | "lease-lost", CronJobExecutionOutcome::Failed(_))
                    | ("timeout", CronJobExecutionOutcome::TimedOut(None))
                    | ("cancel", CronJobExecutionOutcome::Cancelled)
            ),
            "unexpected private runtime outcome: {outcome:?}"
        );
        if mode == "cancel" {
            let drain = draining.join_next().await.ok_or("missing drain join")??;
            let error = drain.err().ok_or("cancelled drain reported success")?;
            assert_eq!(error.timeout_active, Some(1));
            assert!(matches!(
                error.failures.as_slice(),
                [CronJobExecutionOutcome::Cancelled]
            ));
        } else {
            tracker.drain(Duration::from_secs(1)).await?;
        }
        assert_eq!(runner.status().await, outcome.status());
        pool.close().await;
        mark(&directory, "joined-and-pool-closed")?;
        wait_file(&directory, "exit").await?;
        Ok(())
    })
}
