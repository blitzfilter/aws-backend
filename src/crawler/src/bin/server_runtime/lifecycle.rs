use super::config::LifecycleConfig;
use super::daemon::RedactedDaemonCause;
use super::shutdown::fatal;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

pub(super) const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(1);
const WATCHDOG_POLL: Duration = Duration::from_millis(10);
const NO_FAILURE: u64 = u64::MAX;

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or_else(|_| fatal())
}

fn process_budget(config: LifecycleConfig) -> Duration {
    config
        .stop_timeout
        .min(config.shutdown_grace + CLEANUP_TIMEOUT + TEARDOWN_TIMEOUT)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CrawlerState {
    Starting,
    Ready,
    Draining,
    Stopped,
}

impl CrawlerState {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "STARTING",
            Self::Ready => "READY",
            Self::Draining => "DRAINING",
            Self::Stopped => "STOPPED",
        }
    }
}

struct Timeline {
    state: CrawlerState,
    failed: bool,
    born: Instant,
    config: LifecycleConfig,
    startup: Option<Instant>,
    stopped_at: Option<Instant>,
    drain: Option<Instant>,
    process: Option<Instant>,
    cleanup: Option<Instant>,
    teardown: bool,
}

struct Shared {
    timeline: Mutex<Timeline>,
    born: Instant,
    failure_deadline: AtomicU64,
    failure_budget: AtomicU64,
    process_deadline: AtomicU64,
    stop_publication: Notify,
    stop: watch::Sender<bool>,
    failure: watch::Sender<bool>,
}

#[derive(Clone)]
pub(super) struct Lifecycle(Arc<Shared>);

impl Lifecycle {
    pub(super) fn new(config: LifecycleConfig) -> Self {
        let born = Instant::now();
        let startup = born + config.startup_timeout;
        Self(Arc::new(Shared {
            timeline: Mutex::new(Timeline {
                state: CrawlerState::Starting,
                failed: false,
                born,
                config,
                startup: Some(startup),
                stopped_at: None,
                drain: None,
                process: Some(startup + CLEANUP_TIMEOUT + TEARDOWN_TIMEOUT),
                cleanup: None,
                teardown: false,
            }),
            born,
            failure_deadline: AtomicU64::new(NO_FAILURE),
            failure_budget: AtomicU64::new(nanos(process_budget(config))),
            process_deadline: AtomicU64::new(nanos(
                config.startup_timeout + CLEANUP_TIMEOUT + TEARDOWN_TIMEOUT,
            )),
            stop_publication: Notify::new(),
            stop: watch::channel(false).0,
            failure: watch::channel(false).0,
        }))
    }

    fn lock(&self) -> MutexGuard<'_, Timeline> {
        self.0.timeline.lock().unwrap_or_else(|_| fatal())
    }

    fn publish_deadline(&self, t: &Timeline) {
        let deadline = [t.process, t.cleanup]
            .into_iter()
            .flatten()
            .min()
            .map_or(NO_FAILURE, |end| nanos(end.duration_since(self.0.born)));
        self.0.process_deadline.store(deadline, Ordering::Release);
    }

    pub(super) fn configure(&self, config: LifecycleConfig) {
        let mut t = self.lock();
        self.expire(&mut t);
        t.config = config;
        self.0
            .failure_budget
            .store(nanos(process_budget(config)), Ordering::Release);
        let startup = t.born + config.startup_timeout;
        t.startup = Some(startup);
        let limit = startup + CLEANUP_TIMEOUT + TEARDOWN_TIMEOUT;
        if let Some(at) = t.stopped_at {
            // A signal received during configuration can shorten, never reset, its budget.
            t.drain = Some(t.drain.unwrap_or(at).min(at + config.shutdown_grace));
            t.process = Some(
                t.process
                    .unwrap_or(limit)
                    .min(limit)
                    .min(at + config.stop_timeout)
                    .min(t.drain.unwrap_or(at) + CLEANUP_TIMEOUT + TEARDOWN_TIMEOUT),
            );
        } else {
            t.process = Some(limit);
        }
        self.expire(&mut t);
    }

    fn stop_locked(&self, t: &mut Timeline, failure: bool) {
        t.failed |= failure;
        if t.stopped_at.is_none() {
            let at = Instant::now();
            let drain = at + t.config.shutdown_grace;
            let end = (at + t.config.stop_timeout).min(drain + CLEANUP_TIMEOUT + TEARDOWN_TIMEOUT);
            t.stopped_at = Some(at);
            t.drain = Some(drain);
            t.process = Some(t.process.map_or(end, |previous| previous.min(end)));
        }
        t.state = CrawlerState::Draining;
        self.publish_deadline(t);
    }

    pub(super) fn stop(&self, failure: bool) {
        self.stop_locked(&mut self.lock(), failure);
        // A workload can retain a watch borrow. Publish only AFTER releasing the deadline
        // mutex; the OS watchdog must never wait on a workload-owned channel lock.
        self.publish_stop();
    }

    /// Panic hook / early adapter failure path: publish before any lock, output or unwind.
    pub(super) fn fail_early(&self) {
        let deadline = nanos(self.0.born.elapsed())
            .saturating_add(self.0.failure_budget.load(Ordering::Acquire));
        // One atomic publishes both failure and its absolute process fence. Contention cannot
        // kill accepted work, and repeated failures/configuration cannot extend this fence.
        self.0
            .failure_deadline
            .fetch_min(deadline, Ordering::AcqRel);
        self.0.stop_publication.notify_one();
    }

    pub(super) async fn wait_for_stop_publication(&self) {
        self.0.stop_publication.notified().await;
    }

    pub(super) fn publish_stop(&self) {
        // Apply the already-published emergency deadline before touching the watch channel.
        self.expire(&mut self.lock());
        self.0.stop.send_replace(true);
    }

    // All success transitions check the OS clock, not whether Tokio happened to poll a timer.
    fn expire(&self, t: &mut Timeline) {
        let failure = self.0.failure_deadline.load(Ordering::Acquire);
        if failure != NO_FAILURE {
            self.stop_locked(t, true);
            let end = self.0.born + Duration::from_nanos(failure);
            t.process = Some(t.process.map_or(end, |previous| previous.min(end)));
        }
        let now = Instant::now();
        if t.process.is_some_and(|end| now >= end) || t.cleanup.is_some_and(|end| now >= end) {
            fatal();
        }
        if t.state == CrawlerState::Starting && t.startup.is_some_and(|end| now >= end) {
            self.stop_locked(t, true);
        }
        if t.cleanup.is_none() && t.drain.is_some_and(|end| now >= end) {
            t.failed = true;
        }
        self.publish_deadline(t);
    }

    fn observe_failure(&self) {
        let failed = *self.0.failure.borrow();
        if failed {
            self.stop(true);
        }
    }

    pub(super) fn state(&self) -> CrawlerState {
        self.observe_failure();
        let mut t = self.lock();
        self.expire(&mut t);
        t.state
    }

    pub(super) fn ready(&self) -> bool {
        self.observe_failure();
        let mut t = self.lock();
        self.expire(&mut t);
        if t.state != CrawlerState::Starting {
            return false;
        }
        t.state = CrawlerState::Ready;
        t.startup = None;
        t.process = None;
        self.publish_deadline(&t);
        true
    }

    pub(super) fn stopping(&self) -> bool {
        matches!(self.state(), CrawlerState::Draining | CrawlerState::Stopped)
    }

    pub(super) fn failed(&self) -> bool {
        self.observe_failure();
        let mut t = self.lock();
        self.expire(&mut t);
        t.failed
    }

    pub(super) fn stop_receiver(&self) -> watch::Receiver<bool> {
        self.0.stop.subscribe()
    }

    pub(super) fn failure_sender(&self) -> watch::Sender<bool> {
        self.0.failure.clone()
    }

    pub(super) async fn wait(&self) {
        let mut stop = self.stop_receiver();
        let _closed = stop.wait_for(|stop| *stop).await;
    }

    pub(super) fn startup_deadline(&self) -> Instant {
        self.lock().startup.unwrap_or_else(|| fatal())
    }

    pub(super) fn drain_deadline(&self) -> Instant {
        self.lock().drain.unwrap_or_else(|| fatal())
    }

    pub(super) fn begin_cleanup(&self) -> Instant {
        self.stop(false);
        let mut t = self.lock();
        self.expire(&mut t);
        if let Some(end) = t.cleanup {
            return end;
        }
        let end = (Instant::now() + CLEANUP_TIMEOUT)
            .min(t.process.unwrap_or_else(|| fatal()) - TEARDOWN_TIMEOUT);
        t.cleanup = Some(end);
        self.publish_deadline(&t);
        end
    }

    pub(super) fn begin_teardown(&self) {
        let mut t = self.lock();
        self.expire(&mut t);
        if t.teardown {
            return;
        }
        t.cleanup =
            Some((Instant::now() + TEARDOWN_TIMEOUT).min(t.process.unwrap_or_else(|| fatal())));
        t.teardown = true;
        self.publish_deadline(&t);
    }

    pub(super) fn exit_code(&self, successful: bool) -> i32 {
        self.observe_failure();
        let mut t = self.lock();
        self.expire(&mut t);
        if successful && !t.failed && t.teardown {
            // No output or destructors may follow this transition, only actual process exit.
            t.state = CrawlerState::Stopped;
            0
        } else {
            1
        }
    }

    pub(super) fn watchdog(&self) -> ! {
        loop {
            // Never reacquire a condition-variable mutex: a panic can hold it throughout a
            // blocked unwind. The atomic emergency fence remains enforceable in that case.
            let deadline = self
                .0
                .failure_deadline
                .load(Ordering::Acquire)
                .min(self.0.process_deadline.load(Ordering::Acquire));
            if nanos(self.0.born.elapsed()) >= deadline {
                fatal();
            }
            match self.0.timeline.try_lock() {
                Ok(mut t) => self.expire(&mut t),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Poisoned(_)) => fatal(),
            }
            std::thread::sleep(WATCHDOG_POLL);
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ChildError {
    #[error("owned task returned a failure")]
    Run(#[source] RedactedDaemonCause),
    #[error("owned task could not be joined")]
    Join(#[source] RedactedDaemonCause),
    #[error("owned task stopped without a shutdown request")]
    UnexpectedStop,
}

#[derive(Debug, thiserror::Error)]
#[error("crawler lifecycle failed (child outcomes retained; details redacted)")]
pub(super) struct RunError {
    pub(super) cron: Result<(), ChildError>,
    pub(super) review: Result<(), ChildError>,
    pub(super) operations: Result<(), ChildError>,
}

fn spawn_owned(
    lifecycle: Lifecycle,
    work: impl Future<Output = Result<(), RedactedDaemonCause>> + Send + 'static,
) -> JoinHandle<Result<(), ChildError>> {
    tokio::spawn(async move {
        // Observe the result and arm failure BEFORE dropping the completed top-level future.
        // A custom Future's destructor may block even though its final poll returned promptly.
        let mut work = Box::pin(work);
        let result = work.as_mut().await.map_err(ChildError::Run);
        let result = if result.is_ok() && !lifecycle.stopping() {
            Err(ChildError::UnexpectedStop)
        } else {
            result
        };
        if result.is_err() {
            lifecycle.stop(true);
        }
        drop(work);
        result
    })
}

async fn join_owned(
    lifecycle: &Lifecycle,
    handle: JoinHandle<Result<(), ChildError>>,
) -> Result<(), ChildError> {
    let result = handle
        .await
        .unwrap_or_else(|error| Err(ChildError::Join(RedactedDaemonCause::new(error))));
    if result.is_err() {
        lifecycle.stop(true);
    }
    result
}

pub(super) async fn run_owned(
    lifecycle: &Lifecycle,
    cron: impl Future<Output = Result<(), RedactedDaemonCause>> + Send + 'static,
    review: impl Future<Output = Result<(), RedactedDaemonCause>> + Send + 'static,
    operations: impl Future<Output = Result<(), RedactedDaemonCause>> + Send + 'static,
    operations_stop: watch::Sender<bool>,
) -> Result<(), RunError> {
    // Both listeners and concrete dependencies are already constructed by the caller.
    if !lifecycle.ready() {
        lifecycle.stop(false);
    }
    let cron = spawn_owned(lifecycle.clone(), cron);
    let review = spawn_owned(lifecycle.clone(), review);
    let operations = spawn_owned(lifecycle.clone(), operations);
    // Retain every top-level handle. Ordinary signal/failure NEVER drops these futures.
    let joins = async {
        let ((cron, review), operations) = tokio::join!(
            async {
                let outcomes =
                    tokio::join!(join_owned(lifecycle, cron), join_owned(lifecycle, review));
                operations_stop.send_replace(true);
                outcomes
            },
            join_owned(lifecycle, operations),
        );
        RunError {
            cron,
            review,
            operations,
        }
    };
    tokio::pin!(joins);
    let outcome = tokio::select! {
        biased;
        _ = lifecycle.wait() => {
            let end = lifecycle.drain_deadline();
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(end.into()) => fatal(),
                result = &mut joins => {
                    if Instant::now() >= end { fatal(); }
                    result
                }
            }
        }
        result = &mut joins => result,
    };
    if lifecycle.failed()
        || outcome.cron.is_err()
        || outcome.review.is_err()
        || outcome.operations.is_err()
    {
        Err(outcome)
    } else {
        Ok(())
    }
}

#[cfg(all(test, unix))]
#[path = "lifecycle_tests.rs"]
mod tests;
