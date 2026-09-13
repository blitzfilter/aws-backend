use super::config::LifecycleConfig;
use super::lifecycle::Lifecycle;
use std::{
    io::{self, Write},
    sync::mpsc,
    thread,
    time::Duration,
};

/// Terminal process fence. Callers must finish any explicit output/cleanup BEFORE this call.
#[cfg(unix)]
pub(super) fn terminal_exit(code: i32) -> ! {
    // SAFETY: POSIX _exit accepts any integer and no pointers. It terminates the process
    // without Rust stdout cleanup, libc atexit handlers, unwinding, or shared cleanup locks.
    // This narrow runtime-boundary exception is required even when another thread is
    // already stuck inside std::process::exit. No Rust-owned value is accessed afterward.
    unsafe { libc::_exit(code) }
}

#[cfg(not(unix))]
pub(super) fn terminal_exit(code: i32) -> ! {
    // Preserve ordinary successful CLI/help termination off Unix. If Rust cleanup stalls,
    // the one-shot watchdog still aborts; only Unix has the daemon's terminal-fence contract.
    if code == 0 {
        std::process::exit(0);
    }
    std::process::abort()
}

pub(super) fn fatal() -> ! {
    terminal_exit(1)
}

pub(super) fn flush_output() -> io::Result<()> {
    io::stdout().flush()?;
    io::stderr().flush()
}

/// Process-lived by design: neither registration nor watchdog can be disarmed by Drop.
/// Main retains this owner through pools, CloudWatch, actual runtime destruction and output.
pub(super) struct ProcessShutdown {
    pub(super) lifecycle: Lifecycle,
}

impl ProcessShutdown {
    pub(super) fn install(config: LifecycleConfig) -> io::Result<Self> {
        let lifecycle = Lifecycle::new(config);
        let watchdog = lifecycle.clone();
        thread::Builder::new()
            .name("crawler-process-deadline".into())
            .spawn(move || watchdog.watchdog())?;
        let panic_lifecycle = lifecycle.clone();
        // Never chain the default hook: payloads, locations and backtraces may contain secrets
        // or block on stderr. Arm the deadline before unwinding can run any destructor.
        std::panic::set_hook(Box::new(move |_| panic_lifecycle.fail_early()));
        register_signals(lifecycle.clone())?;
        Ok(Self { lifecycle })
    }

    pub(super) fn exit(self, successful: bool) -> ! {
        let output = flush_output();
        terminal_exit(self.lifecycle.exit_code(successful && output.is_ok()))
    }
}

#[cfg(unix)]
fn register_signals(lifecycle: Lifecycle) -> io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let (ready, registered) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("crawler-signals".into())
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
                let handlers = signal(SignalKind::terminate())
                    .and_then(|term| signal(SignalKind::interrupt()).map(|int| (term, int)));
                let (mut term, mut int) = match handlers {
                    Ok(handlers) => handlers,
                    Err(error) => {
                        let _undelivered = ready.send(Err(error));
                        return;
                    }
                };
                let mut failure = lifecycle.failure_sender().subscribe();
                let mut failure_observed = false;
                if ready.send(Ok(())).is_err() {
                    fatal();
                }
                loop {
                    tokio::select! {
                        biased;
                        _ = lifecycle.wait_for_stop_publication() => lifecycle.publish_stop(),
                        _ = failure.wait_for(|failed| *failed), if !failure_observed => {
                            lifecycle.stop(true);
                            failure_observed = true;
                        }
                        value = term.recv() => {
                            if value.is_none() { fatal(); }
                            lifecycle.stop(false);
                        }
                        value = int.recv() => {
                            if value.is_none() { fatal(); }
                            lifecycle.stop(false);
                        }
                    }
                }
            });
        })?;
    registered
        .recv_timeout(Duration::from_secs(1))
        .map_err(|_| io::Error::other("crawler signal registration failed (details redacted)"))?
}

#[cfg(not(unix))]
fn register_signals(_lifecycle: Lifecycle) -> io::Result<()> {
    Err(io::Error::other(
        "crawler daemon requires Unix SIGINT/SIGTERM support",
    ))
}
