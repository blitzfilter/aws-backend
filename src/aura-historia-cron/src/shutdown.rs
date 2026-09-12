use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub(crate) const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// Independent of Tokio timers, blocked polls, destructors and logging locks.
pub(crate) struct FatalDeadline {
    cancel: Option<mpsc::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FatalDeadline {
    pub(crate) fn after_drain(drain: Duration) -> Self {
        Self::after(
            drain
                .checked_add(CLEANUP_TIMEOUT)
                .unwrap_or_else(|| fatal()),
        )
    }

    pub(crate) fn after(timeout: Duration) -> Self {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(|| fatal());
        Self::at(deadline)
    }

    pub(crate) fn at(deadline: Instant) -> Self {
        let (cancel, cancelled) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("cron-shutdown-deadline".into())
            .spawn(move || {
                if matches!(
                    cancelled.recv_timeout(deadline.saturating_duration_since(Instant::now())),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    fatal();
                }
            })
            .unwrap_or_else(|_| fatal());
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
            fatal();
        }
    }
}

pub(crate) fn fatal() -> ! {
    // Never claim stopped/released or wait on a possibly stalled logging subscriber.
    std::process::exit(1);
}

pub(crate) fn teardown(runtime: tokio::runtime::Runtime) {
    let _deadline = FatalDeadline::after(TEARDOWN_TIMEOUT);
    // shutdown_timeout could silently leave threads alive. Require actual destruction.
    drop(runtime);
}

#[cfg(unix)]
pub(crate) struct ShutdownSignals {
    observed: tokio::sync::watch::Receiver<bool>,
    finished: tokio::sync::watch::Sender<bool>,
    grace_ms: std::sync::Arc<std::sync::atomic::AtomicU64>,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl ShutdownSignals {
    pub(crate) fn register() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        let (observed_tx, observed) = tokio::sync::watch::channel(false);
        let (finished, mut finish) = tokio::sync::watch::channel(false);
        let grace_ms = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(300_000));
        let budget = std::sync::Arc::clone(&grace_ms);
        let (ready, registered) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("cron-signals".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _undelivered = ready.send(Err(error));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let handlers = signal(SignalKind::terminate()).and_then(|terminate| {
                        signal(SignalKind::interrupt()).map(|interrupt| (terminate, interrupt))
                    });
                    let (mut terminate, mut interrupt) = match handlers {
                        Ok(handlers) => handlers,
                        Err(error) => {
                            let _undelivered = ready.send(Err(error));
                            return;
                        }
                    };
                    if ready.send(Ok(())).is_err() {
                        return;
                    }
                    tokio::select! {
                        biased;
                        _ = finish.wait_for(|done| *done) => return,
                        _ = terminate.recv() => {},
                        _ = interrupt.recv() => {},
                    }
                    let _deadline = FatalDeadline::after_drain(Duration::from_millis(
                        budget.load(std::sync::atomic::Ordering::Acquire),
                    ));
                    observed_tx.send_replace(true);
                    // Retain both handlers and the watchdog through work and runtime cleanup.
                    let _closed = finish.wait_for(|done| *done).await;
                });
            })?;
        match registered.recv() {
            Ok(Ok(())) => Ok(Self {
                observed,
                finished,
                grace_ms,
                thread: Some(thread),
            }),
            result => {
                let _joined = thread.join();
                match result {
                    Ok(Err(error)) => Err(error),
                    _ => Err(std::io::Error::other("signal thread failed")),
                }
            }
        }
    }

    pub(crate) fn set_grace(&self, grace: Duration) {
        let millis = u64::try_from(grace.as_millis()).unwrap_or_else(|_| fatal());
        self.grace_ms
            .store(millis, std::sync::atomic::Ordering::Release);
    }

    pub(crate) async fn wait(&mut self) {
        let _closed = self.observed.wait_for(|observed| *observed).await;
    }
}

#[cfg(unix)]
impl Drop for ShutdownSignals {
    fn drop(&mut self) {
        self.finished.send_replace(true);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            fatal();
        }
    }
}
