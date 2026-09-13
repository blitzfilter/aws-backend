use crate::{
    CronErrorCause,
    shutdown::{CLEANUP_TIMEOUT, FatalDeadline, fatal},
};
use async_trait::async_trait;
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use tokio::sync::{Mutex, Notify, oneshot, watch};
use tokio::task::JoinSet;
use tracing::info;

#[async_trait]
#[doc(hidden)]
pub trait CronJob: Send + Sync {
    fn name(&self) -> &'static str;
    async fn execute(&self) -> Result<(), CronJobExecutionError>;
}

#[derive(Debug, Clone, thiserror::Error)]
#[doc(hidden)]
pub enum CronJobExecutionError {
    #[error("cron job execution failed")]
    Failed(#[source] CronErrorCause),
}

impl CronJobExecutionError {
    pub fn from_source(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Failed(CronErrorCause::new(source))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum CronJobStatus {
    NeverRun,
    Succeeded,
    Failed,
    Panicked,
    TimedOut,
    SkippedLocalOverlap,
    SkippedShutdown,
    Cancelled,
}

#[doc(hidden)]
pub struct ActiveExecutionTracker {
    accepting: AtomicBool,
    active: AtomicUsize,
    drained: Notify,
    tasks: std::sync::Mutex<TrackedExecutions>,
    cancel: watch::Sender<bool>,
}

#[derive(Default)]
struct TrackedExecutions {
    tasks: JoinSet<CompletedExecution>,
    stopped: bool,
}

struct CompletedExecution {
    during_drain: bool,
    outcome: CronJobExecutionOutcome,
}

impl Default for ActiveExecutionTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ActiveExecutionTracker {
    pub fn new() -> Self {
        Self {
            accepting: AtomicBool::new(true),
            active: AtomicUsize::new(0),
            drained: Notify::new(),
            tasks: std::sync::Mutex::new(TrackedExecutions::default()),
            cancel: watch::channel(false).0,
        }
    }

    pub fn stop_accepting(&self) {
        let mut tracked = self.tasks.lock().unwrap_or_else(|_| fatal());
        tracked.stopped = true;
        self.accepting.store(false, Ordering::Release);
    }

    pub(crate) fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    fn try_track(self: &Arc<Self>) -> Option<ActiveExecutionGuard> {
        if !self.accepting.load(Ordering::Acquire) {
            return None;
        }
        self.active.fetch_add(1, Ordering::AcqRel);
        if self.accepting.load(Ordering::Acquire) {
            Some(ActiveExecutionGuard {
                tracker: Arc::clone(self),
            })
        } else {
            self.release();
            None
        }
    }

    fn release(&self) {
        if self.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.drained.notify_waiters();
        }
    }

    pub async fn drain(&self, timeout: Duration) -> Result<(), CronDrainError> {
        let _deadline = FatalDeadline::after_drain(timeout);
        self.stop_accepting();
        let mut tasks = {
            let mut tracked = self.tasks.lock().unwrap_or_else(|_| fatal());
            std::mem::take(&mut tracked.tasks)
        };
        let mut failures = Vec::new();
        let mut collect = |completed: CompletedExecution| {
            // Earlier terminal failures were already reported by the job boundary.
            // Only attempts still owned at stop-admission affect this drain.
            if completed.during_drain && completed.outcome.is_failure() {
                failures.push(completed.outcome);
            }
        };
        let wait = async {
            while let Some(result) = tasks.join_next().await {
                collect(result.unwrap_or_else(|_| fatal()));
            }
            loop {
                let notified = self.drained.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.active.load(Ordering::Acquire) == 0 {
                    return;
                }
                notified.await;
            }
        };
        if tokio::time::timeout(timeout, wait).await.is_ok() {
            return if failures.is_empty() {
                Ok(())
            } else {
                Err(CronDrainError {
                    timeout_active: None,
                    failures,
                })
            };
        }
        let active = self.active();
        self.cancel.send_replace(true);
        let cleanup = async {
            while let Some(result) = tasks.join_next().await {
                collect(result.unwrap_or_else(|_| fatal()));
            }
        };
        if tokio::time::timeout(CLEANUP_TIMEOUT, cleanup)
            .await
            .is_err()
        {
            fatal();
        }
        Err(CronDrainError {
            timeout_active: Some(active),
            failures,
        })
    }
}

struct ActiveExecutionGuard {
    tracker: Arc<ActiveExecutionTracker>,
}
impl Drop for ActiveExecutionGuard {
    fn drop(&mut self) {
        self.tracker.release();
    }
}

#[derive(Debug)]
#[doc(hidden)]
pub struct CronDrainError {
    pub timeout_active: Option<usize>,
    pub failures: Vec<CronJobExecutionOutcome>,
}

impl std::fmt::Display for CronDrainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cron execution drain failed")
    }
}

impl Error for CronDrainError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.failures.iter().find_map(|outcome| match outcome {
            CronJobExecutionOutcome::Failed(error)
            | CronJobExecutionOutcome::TimedOut(Some(error)) => Some(error as &dyn Error),
            CronJobExecutionOutcome::Panicked(error) => Some(error as &dyn Error),
            _ => None,
        })
    }
}

#[derive(Debug, Clone)]
#[doc(hidden)]
pub enum CronJobExecutionOutcome {
    Succeeded,
    Failed(CronJobExecutionError),
    Panicked(CronErrorCause),
    TimedOut(Option<CronJobExecutionError>),
    SkippedLocalOverlap,
    SkippedShutdown,
    Cancelled,
}

impl CronJobExecutionOutcome {
    fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::Failed(_) | Self::Panicked(_) | Self::TimedOut(_) | Self::Cancelled
        )
    }

    pub const fn status(&self) -> CronJobStatus {
        match self {
            Self::Succeeded => CronJobStatus::Succeeded,
            Self::Failed(_) => CronJobStatus::Failed,
            Self::Panicked(_) => CronJobStatus::Panicked,
            Self::TimedOut(_) => CronJobStatus::TimedOut,
            Self::SkippedLocalOverlap => CronJobStatus::SkippedLocalOverlap,
            Self::SkippedShutdown => CronJobStatus::SkippedShutdown,
            Self::Cancelled => CronJobStatus::Cancelled,
        }
    }
}

#[doc(hidden)]
pub struct ScheduledJobRunner {
    job: Arc<dyn CronJob>,
    running: Arc<AtomicBool>,
    tracker: Arc<ActiveExecutionTracker>,
    schedule: String,
    max_run_duration: Option<Duration>,
    status: Arc<Mutex<CronJobStatus>>,
}

impl ScheduledJobRunner {
    pub fn new(
        job: Arc<dyn CronJob>,
        tracker: Arc<ActiveExecutionTracker>,
        schedule: String,
        max_run_duration: Option<Duration>,
    ) -> Self {
        Self {
            job,
            running: Arc::new(AtomicBool::new(false)),
            tracker,
            schedule,
            max_run_duration,
            status: Arc::new(Mutex::new(CronJobStatus::NeverRun)),
        }
    }

    pub async fn run(&self) {
        let _outcome = self.execute_once().await;
    }

    pub async fn execute_once(&self) -> CronJobExecutionOutcome {
        let outcome = self.submit().await.unwrap_or_else(|_| fatal());
        self.complete(outcome).await
    }

    // Registration and stop-admission share one lock. The receiver is notification only;
    // dropping a caller or a cron_tab trigger cannot detach the owned execution.
    pub(crate) fn submit(&self) -> oneshot::Receiver<CronJobExecutionOutcome> {
        let (sender, receiver) = oneshot::channel();
        let mut tracked = self.tracker.tasks.lock().unwrap_or_else(|_| fatal());
        if !tracked.stopped {
            while let Some(result) = tracked.tasks.try_join_next() {
                if result.is_err() {
                    fatal();
                }
            }
        }
        let Some(active) = self.tracker.try_track() else {
            let _undelivered = sender.send(CronJobExecutionOutcome::SkippedShutdown);
            return receiver;
        };
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            let _undelivered = sender.send(CronJobExecutionOutcome::SkippedLocalOverlap);
            return receiver;
        }
        let running = RunningGuard {
            running: Arc::clone(&self.running),
        };
        let job = Arc::clone(&self.job);
        let schedule = self.schedule.clone();
        let status = Arc::clone(&self.status);
        let maximum = self.max_run_duration;
        let mut cancelled = self.tracker.cancel.subscribe();
        let ownership = Arc::new((active, running));
        let tracker = Arc::clone(&self.tracker);
        tracked.tasks.spawn(async move {
            let started = Instant::now();
            let deadline = maximum.map(|duration| started.checked_add(duration).unwrap_or_else(|| fatal()));
            let _execution_deadline = deadline.map(|deadline| {
                FatalDeadline::at(deadline.checked_add(CLEANUP_TIMEOUT).unwrap_or_else(|| fatal()))
            });
            let _ownership = ownership;
            let name = job.name();
            info!(job = name, schedule, actual_started_at = %OffsetDateTime::now_utc(), "cron.job.started");
            let mut child = JoinSet::new();
            let completed_at = Arc::new(std::sync::Mutex::new(None));
            let completion = CompletionTime(Arc::clone(&completed_at));
            let child_ownership = Arc::clone(&_ownership);
            child.spawn(async move {
                // Both tasks retain admission: even an unexpected owner drop cannot
                // release it while the job future is still being destroyed.
                let _ownership = child_ownership;
                let _completion = completion;
                job.execute().await
            });
            let ceiling = async {
                match deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                    None => std::future::pending().await,
                }
            };
            let mut outcome = tokio::select! {
                biased;
                result = child.join_next() => {
                    let completed_at = completed_at.lock().unwrap_or_else(|_| fatal()).unwrap_or_else(|| fatal());
                    let result = result.unwrap_or_else(|| fatal());
                    // Selection order is not completion time: a blocking poll can
                    // make both the join and the absolute timer ready together.
                    if deadline.is_some_and(|deadline| completed_at >= deadline) {
                        let source = match result {
                            Ok(Ok(())) => None,
                            Ok(Err(error)) => Some(error),
                            Err(error) => Some(CronJobExecutionError::from_source(error)),
                        };
                        CronJobExecutionOutcome::TimedOut(source)
                    } else {
                        match result {
                            Ok(Ok(())) => CronJobExecutionOutcome::Succeeded,
                            Ok(Err(error)) => CronJobExecutionOutcome::Failed(error),
                            Err(error) if error.is_panic() => CronJobExecutionOutcome::Panicked(CronErrorCause::new(error)),
                            Err(error) => CronJobExecutionOutcome::Failed(CronJobExecutionError::from_source(error)),
                        }
                    }
                },
                _ = cancelled.wait_for(|cancel| *cancel) => CronJobExecutionOutcome::Cancelled,
                () = ceiling => CronJobExecutionOutcome::TimedOut(None),
            };
            if !child.is_empty() {
                let _deadline = FatalDeadline::after(CLEANUP_TIMEOUT);
                child.abort_all();
                while let Some(result) = child.join_next().await {
                    let failure = match result {
                        Ok(Ok(())) => None,
                        Ok(Err(error)) => Some(error),
                        Err(error) if error.is_cancelled() => None,
                        Err(error) => Some(CronJobExecutionError::from_source(error)),
                    };
                    if let Some(error) = failure {
                        match &mut outcome {
                            CronJobExecutionOutcome::TimedOut(source) => *source = Some(error),
                            CronJobExecutionOutcome::Cancelled => outcome = CronJobExecutionOutcome::Failed(error),
                            _ => fatal(),
                        }
                    }
                }
            }
            *status.lock().await = outcome.status();
            let label = match &outcome {
                CronJobExecutionOutcome::Succeeded => "succeeded",
                CronJobExecutionOutcome::Failed(_) => "failed",
                CronJobExecutionOutcome::Panicked(_) => "panicked",
                CronJobExecutionOutcome::TimedOut(_) => "timed_out",
                CronJobExecutionOutcome::Cancelled => "cancelled",
                CronJobExecutionOutcome::SkippedLocalOverlap => "skipped_local_overlap",
                CronJobExecutionOutcome::SkippedShutdown => "skipped_shutdown",
            };
            info!(job = name, outcome = label, duration_ms = started.elapsed().as_millis(), "cron.job.completed");
            let completed = {
                // Terminal publication and admission release share the stop lock:
                // no active attempt can fall between the drain cutoff and its result.
                let tracked = tracker.tasks.lock().unwrap_or_else(|_| fatal());
                let completed = CompletedExecution { during_drain: tracked.stopped, outcome };
                drop(_ownership);
                completed
            };
            let _undelivered = sender.send(completed.outcome.clone());
            completed
        });
        receiver
    }

    async fn complete(&self, outcome: CronJobExecutionOutcome) -> CronJobExecutionOutcome {
        self.set_status(outcome.status()).await;
        outcome
    }

    pub async fn status(&self) -> CronJobStatus {
        *self.status.lock().await
    }
    async fn set_status(&self, status: CronJobStatus) {
        *self.status.lock().await = status;
    }
}

struct CompletionTime(Arc<std::sync::Mutex<Option<Instant>>>);

impl Drop for CompletionTime {
    fn drop(&mut self) {
        *self.0.lock().unwrap_or_else(|_| fatal()) = Some(Instant::now());
    }
}

struct RunningGuard {
    running: Arc<AtomicBool>,
}
impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    #[derive(Debug, thiserror::Error)]
    #[error("test job failure")]
    struct TestJobError;

    struct BlockingJob {
        started: Arc<Notify>,
        release: Arc<Notify>,
        runs: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl CronJob for BlockingJob {
        fn name(&self) -> &'static str {
            "test"
        }
        async fn execute(&self) -> Result<(), CronJobExecutionError> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            self.release.notified().await;
            Ok(())
        }
    }

    #[test]
    fn should_time_out_late_completion_when_single_worker_timers_are_stalled()
    -> Result<(), Box<dyn Error>> {
        struct BlockingPoll {
            started: std::sync::mpsc::Sender<()>,
            release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        }
        #[async_trait]
        impl CronJob for BlockingPoll {
            fn name(&self) -> &'static str {
                "blocking-poll"
            }
            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                self.started.send(()).unwrap();
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
                Ok(())
            }
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let (started, start) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let tracker = Arc::new(ActiveExecutionTracker::new());
        let runner = Arc::new(ScheduledJobRunner::new(
            Arc::new(BlockingPoll {
                started,
                release: std::sync::Mutex::new(released),
            }),
            Arc::clone(&tracker),
            "test".into(),
            Some(Duration::from_millis(20)),
        ));
        let controller_runner = Arc::clone(&runner);
        // An OS controller, not a Tokio sleep: the only worker is blocked inside poll.
        let controller = std::thread::spawn(move || {
            start.recv_timeout(Duration::from_secs(2)).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            assert_eq!(controller_runner.tracker.active(), 1);
            assert!(controller_runner.running.load(Ordering::Acquire));
            release.send(()).unwrap();
        });
        runtime.block_on(async {
            let started = Instant::now();
            let outcome = runner.execute_once().await;
            assert!(started.elapsed() >= Duration::from_millis(200));
            assert!(matches!(outcome, CronJobExecutionOutcome::TimedOut(None)));
            assert_eq!(runner.status().await, CronJobStatus::TimedOut);
            assert!(tracker.drain(Duration::from_secs(1)).await.is_ok());
        });
        controller.join().map_err(|_| "OS controller panicked")?;
        crate::shutdown::teardown(runtime);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn should_retain_admission_until_cancelled_future_drop_is_joined() {
        struct DropGate {
            dropping: std::sync::mpsc::Sender<()>,
            release: std::sync::mpsc::Receiver<()>,
        }
        impl Drop for DropGate {
            fn drop(&mut self) {
                self.dropping.send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(2)).unwrap();
            }
        }
        struct CancelJob(std::sync::Mutex<Option<DropGate>>);
        #[async_trait]
        impl CronJob for CancelJob {
            fn name(&self) -> &'static str {
                "cancel-drop"
            }
            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                let gate = self.0.lock().unwrap().take();
                if let Some(_gate) = gate {
                    std::future::pending::<()>().await;
                }
                Ok(())
            }
        }
        let (dropping, drop_started) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let tracker = Arc::new(ActiveExecutionTracker::new());
        let runner = Arc::new(ScheduledJobRunner::new(
            Arc::new(CancelJob(std::sync::Mutex::new(Some(DropGate {
                dropping,
                release: released,
            })))),
            Arc::clone(&tracker),
            "test".into(),
            Some(Duration::from_millis(20)),
        ));
        let controller_runner = Arc::clone(&runner);
        let controller = std::thread::spawn(move || {
            drop_started.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(controller_runner.tracker.active(), 1);
            let mut overlapping = controller_runner.submit();
            assert!(matches!(
                overlapping.try_recv(),
                Ok(CronJobExecutionOutcome::SkippedLocalOverlap)
            ));
            release.send(()).unwrap();
        });
        assert!(matches!(
            runner.execute_once().await,
            CronJobExecutionOutcome::TimedOut(None)
        ));
        controller.join().unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while tracker.active() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            runner.execute_once().await,
            CronJobExecutionOutcome::Succeeded
        ));
        assert!(tracker.drain(Duration::from_secs(1)).await.is_ok());
    }

    #[tokio::test]
    async fn should_retain_active_failure_even_when_late_trigger_drops_receiver() {
        struct FailAfterRelease(Arc<Notify>);
        #[async_trait]
        impl CronJob for FailAfterRelease {
            fn name(&self) -> &'static str {
                "fail-after-stop"
            }
            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                self.0.notified().await;
                Err(CronJobExecutionError::from_source(TestJobError))
            }
        }
        let tracker = Arc::new(ActiveExecutionTracker::new());
        let release = Arc::new(Notify::new());
        let runner = ScheduledJobRunner::new(
            Arc::new(FailAfterRelease(Arc::clone(&release))),
            Arc::clone(&tracker),
            "test".into(),
            None,
        );
        let terminal = runner.submit();
        tracker.stop_accepting();
        release.notify_one();
        assert!(matches!(
            terminal.await.unwrap(),
            CronJobExecutionOutcome::Failed(_)
        ));
        drop(runner.submit());
        let error = tracker.drain(Duration::from_secs(1)).await.unwrap_err();
        assert_eq!(error.failures.len(), 1);
        assert!(matches!(
            error.failures[0],
            CronJobExecutionOutcome::Failed(_)
        ));
        assert!(error.source().is_some());
        assert_eq!(tracker.active(), 0);
    }

    #[tokio::test]
    async fn should_not_fail_later_daemon_drain_for_an_already_terminal_failure() {
        struct Fail;
        #[async_trait]
        impl CronJob for Fail {
            fn name(&self) -> &'static str {
                "historic-failure"
            }
            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                Err(CronJobExecutionError::from_source(TestJobError))
            }
        }
        let tracker = Arc::new(ActiveExecutionTracker::new());
        let runner =
            ScheduledJobRunner::new(Arc::new(Fail), Arc::clone(&tracker), "test".into(), None);
        assert!(matches!(
            runner.execute_once().await,
            CronJobExecutionOutcome::Failed(_)
        ));
        assert!(tracker.drain(Duration::from_secs(1)).await.is_ok());
    }

    #[tokio::test]
    async fn should_record_failure_with_its_source() {
        struct FailingJob;
        #[async_trait]
        impl CronJob for FailingJob {
            fn name(&self) -> &'static str {
                "failing"
            }

            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                Err(CronJobExecutionError::from_source(TestJobError))
            }
        }

        let error = CronJobExecutionError::from_source(TestJobError);
        assert!(error.source().is_some());
        let runner = ScheduledJobRunner::new(
            Arc::new(FailingJob),
            Arc::new(ActiveExecutionTracker::new()),
            "test schedule".to_owned(),
            None,
        );
        let outcome = runner.execute_once().await;
        assert!(matches!(outcome, CronJobExecutionOutcome::Failed(_)));
        assert_eq!(CronJobStatus::Failed, runner.status().await);
    }

    #[tokio::test]
    async fn should_record_panic() {
        struct PanickingJob;
        #[async_trait]
        impl CronJob for PanickingJob {
            fn name(&self) -> &'static str {
                "panicking"
            }

            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                std::panic::panic_any("test job panic");
            }
        }

        let runner = ScheduledJobRunner::new(
            Arc::new(PanickingJob),
            Arc::new(ActiveExecutionTracker::new()),
            "test schedule".to_owned(),
            None,
        );
        let outcome = runner.execute_once().await;
        assert!(matches!(outcome, CronJobExecutionOutcome::Panicked(_)));
        assert_eq!(CronJobStatus::Panicked, runner.status().await);
    }

    #[tokio::test]
    async fn should_record_timeout() {
        struct SlowJob;
        #[async_trait]
        impl CronJob for SlowJob {
            fn name(&self) -> &'static str {
                "slow"
            }

            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(())
            }
        }

        let runner = ScheduledJobRunner::new(
            Arc::new(SlowJob),
            Arc::new(ActiveExecutionTracker::new()),
            "test schedule".to_owned(),
            Some(Duration::from_millis(10)),
        );
        let outcome = runner.execute_once().await;
        assert!(matches!(outcome, CronJobExecutionOutcome::TimedOut(_)));
        assert_eq!(CronJobStatus::TimedOut, runner.status().await);
    }

    #[tokio::test]
    async fn should_drain_after_active_execution_finishes() {
        let tracker = Arc::new(ActiveExecutionTracker::new());
        let active = tracker.try_track();
        assert!(active.is_some());
        let Some(active) = active else {
            return;
        };
        let drain = tokio::spawn({
            let tracker = Arc::clone(&tracker);
            async move { tracker.drain(Duration::from_secs(1)).await }
        });
        tokio::task::yield_now().await;
        drop(active);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), drain)
                .await
                .is_ok_and(|result| matches!(result, Ok(Ok(()))))
        );
    }

    #[tokio::test]
    async fn should_skip_overlapping_execution() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runs = Arc::new(AtomicUsize::new(0));
        let runner = Arc::new(ScheduledJobRunner::new(
            Arc::new(BlockingJob {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                runs: Arc::clone(&runs),
            }),
            Arc::new(ActiveExecutionTracker::new()),
            "test schedule".to_owned(),
            None,
        ));
        let first = tokio::spawn({
            let runner = Arc::clone(&runner);
            async move { runner.run().await }
        });
        started.notified().await;
        let outcome = runner.execute_once().await;
        assert!(matches!(
            outcome,
            CronJobExecutionOutcome::SkippedLocalOverlap
        ));
        assert_eq!(CronJobStatus::SkippedLocalOverlap, runner.status().await);
        assert_eq!(1, runs.load(Ordering::SeqCst));
        release.notify_one();
        let _ = first.await;
    }

    #[tokio::test]
    async fn should_return_shutdown_skip_outcome() {
        struct SuccessJob;
        #[async_trait]
        impl CronJob for SuccessJob {
            fn name(&self) -> &'static str {
                "success"
            }

            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                Ok(())
            }
        }

        let tracker = Arc::new(ActiveExecutionTracker::new());
        tracker.stop_accepting();
        let runner = ScheduledJobRunner::new(
            Arc::new(SuccessJob),
            tracker,
            "test schedule".to_owned(),
            None,
        );
        let outcome = runner.execute_once().await;
        assert!(matches!(outcome, CronJobExecutionOutcome::SkippedShutdown));
        assert_eq!(CronJobStatus::SkippedShutdown, runner.status().await);
    }

    #[tokio::test]
    async fn should_return_successful_execution_outcome() {
        struct SuccessJob;
        #[async_trait]
        impl CronJob for SuccessJob {
            fn name(&self) -> &'static str {
                "success"
            }

            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                Ok(())
            }
        }

        let runner = ScheduledJobRunner::new(
            Arc::new(SuccessJob),
            Arc::new(ActiveExecutionTracker::new()),
            "test schedule".to_owned(),
            None,
        );
        let outcome = runner.execute_once().await;
        assert!(matches!(outcome, CronJobExecutionOutcome::Succeeded));
        assert_eq!(CronJobStatus::Succeeded, runner.status().await);
    }
}
