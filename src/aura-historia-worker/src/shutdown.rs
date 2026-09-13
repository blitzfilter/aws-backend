use std::{
    future::Future,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub(super) const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const RUNTIME_TEARDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// Last resort independent of Tokio scheduling, timer progress and cancellation destructors.
pub(super) struct FatalDeadline {
    cancel: Option<mpsc::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FatalDeadline {
    pub(super) fn for_runtime(drain: Duration) -> Self {
        Self::after_drain(drain.max(aura_historia_worker::WORKER_HTTP_DRAIN_TIMEOUT))
    }

    pub(super) fn after_drain(drain: Duration) -> Self {
        let timeout = drain
            .checked_add(CLEANUP_TIMEOUT)
            .unwrap_or_else(|| fatal("SHUTDOWN_BUDGET_OVERFLOW"));
        Self::after(timeout)
    }

    pub(super) fn after(timeout: Duration) -> Self {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(|| fatal("SHUTDOWN_DEADLINE_OVERFLOW"));
        let (cancel, cancelled) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("worker-shutdown-deadline".into())
            .spawn(move || {
                if matches!(
                    cancelled.recv_timeout(deadline.saturating_duration_since(Instant::now())),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    // No logging here: the stalled thread might hold the subscriber's lock.
                    std::process::exit(1);
                }
            })
            .unwrap_or_else(|_| fatal("SHUTDOWN_WATCHDOG_STARTUP"));
        Self {
            cancel: Some(cancel),
            thread: Some(thread),
        }
    }
}

impl Drop for FatalDeadline {
    fn drop(&mut self) {
        drop(self.cancel.take());
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            fatal("SHUTDOWN_WATCHDOG_FAILED");
        }
    }
}

pub(super) async fn join_cancelled(
    parent: impl Future<Output = ()>,
    children: impl Future<Output = ()>,
) {
    let _deadline = FatalDeadline::after(CLEANUP_TIMEOUT);
    let cleanup = async {
        // Parent destruction registers children; neither join gets a fresh allowance.
        parent.await;
        children.await;
    };
    if tokio::time::timeout(CLEANUP_TIMEOUT, cleanup)
        .await
        .is_err()
    {
        fatal("CLEANUP_DEADLINE");
    }
}

pub(super) fn teardown(runtime: tokio::runtime::Runtime) {
    let _deadline = FatalDeadline::after(RUNTIME_TEARDOWN_TIMEOUT);
    // shutdown_timeout can silently leave threads running. Confirm teardown instead, or exit
    // nonzero from the independent deadline thread without running any more Rust destructors.
    drop(runtime);
}

pub(super) fn fatal(category: &'static str) -> ! {
    tracing::error!(
        category,
        outcome = "worker_stopped",
        "worker cleanup failed"
    );
    std::process::exit(1);
}
