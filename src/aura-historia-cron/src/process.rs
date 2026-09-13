use crate::{
    CronErrorCause, CronRuntimeConfig, CronRuntimeError, JobRegistration,
    shutdown::{self, FatalDeadline, ShutdownSignals},
};
use platform_observability::{LogLevel, LoggingConfig, init};
use std::{ffi::OsString, future::Future, process::ExitCode};
use tokio::{sync::watch, task::JoinSet};

const JOB: &str = "search-filter-periodic-match";

/// Production binary boundary. Library embedders own their runtime teardown.
pub fn run_process() -> ExitCode {
    std::panic::set_hook(Box::new(|_| tracing::error!("cron.task.panicked")));
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return ExitCode::FAILURE,
    };
    let result = runtime.block_on(run());
    shutdown::teardown(runtime);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Error chains remain typed for callers, never dumped by the process boundary.
            tracing::error!(category = error.category(), "cron.process.failed");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Daemon,
    Once,
    CheckConfig,
}
impl RunMode {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, MainError> {
        let args = args.into_iter().collect::<Vec<_>>();
        if args.is_empty() {
            return Ok(Self::Daemon);
        }
        if args == ["--check-config"] {
            return Ok(Self::CheckConfig);
        }
        if args == ["--run-once", JOB] {
            return Ok(Self::Once);
        }
        Err(MainError::Arguments)
    }
}

async fn run() -> Result<(), MainError> {
    // Both handlers are installed before parsing configuration or composing dependencies.
    let signals = ShutdownSignals::register()
        .map_err(|error| MainError::Signal(CronErrorCause::new(error)))?;
    init(LoggingConfig::new(
        std::env::var("LOG_LEVEL")
            .ok()
            .as_deref()
            .and_then(LogLevel::parse)
            .unwrap_or_default(),
    ));
    let mode = RunMode::parse(std::env::args_os().skip(1))?;
    let config = CronRuntimeConfig::from_env(&[JOB])
        .map_err(|error| MainError::Config(CronErrorCause::new(error)))?;
    if mode != RunMode::Once && !config.enabled_jobs().iter().any(|name| name == JOB) {
        return Err(MainError::NoJobs);
    }
    let (stop, stopped) = watch::channel(false);
    let grace = config.shutdown_grace();
    let work = run_mode(config, mode, stopped);
    supervise_signals(signals, stop, grace, work).await
}

pub(crate) async fn supervise_signals<T>(
    mut signals: ShutdownSignals,
    stop: watch::Sender<bool>,
    grace: std::time::Duration,
    work: impl Future<Output = T>,
) -> T {
    signals.set_grace(grace);
    tokio::pin!(work);
    tokio::select! {
        biased;
        () = signals.wait() => {
            let _deadline = FatalDeadline::after_drain(grace);
            stop.send_replace(true);
            // Retain both signal registrations. Further signals neither re-open
            // admission nor shorten the owned cancellation/lease cleanup.
            work.await
        }
        result = &mut work => result,
    }
}

async fn run_mode(
    config: CronRuntimeConfig,
    mode: RunMode,
    mut stopped: watch::Receiver<bool>,
) -> Result<(), MainError> {
    if mode == RunMode::CheckConfig {
        let mut preflight = JoinSet::new();
        preflight.spawn(async { crate::wiring::check_from_env().await.map(|()| Vec::new()) });
        let result = tokio::select! {
            biased;
            _ = stopped.wait_for(|stop| *stop) => Err(CronRuntimeError::StartupInterrupted),
            result = preflight.join_next() => match result {
                Some(Ok(result)) => result.map(|_| ()).map_err(|error| CronRuntimeError::Wiring(CronErrorCause::new(error))),
                Some(Err(error)) => Err(CronRuntimeError::StartupTask(CronErrorCause::new(error))),
                None => shutdown::fatal(),
            },
        };
        let _deadline = FatalDeadline::after(shutdown::CLEANUP_TIMEOUT);
        CronRuntimeError::combine(result, crate::stop_preparation(&mut preflight).await)
            .map_err(MainError::Preflight)?;
        tracing::info!("cron.config.checked");
        return Ok(());
    }
    crate::run_with_startup(
        config,
        async {
            let (job, schedule, maximum) = crate::wiring::build_from_env().await?;
            Ok(vec![JobRegistration {
                name: JOB,
                schedule,
                max_run_duration: Some(maximum),
                job,
            }])
        },
        mode == RunMode::Once,
        async move {
            let _closed = stopped.wait_for(|stop| *stop).await;
        },
    )
    .await?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum MainError {
    #[error("usage: aura-historia-cron [--check-config | --run-once search-filter-periodic-match]")]
    Arguments,
    #[error("cron job must be enabled")]
    NoJobs,
    #[error("cron signal registration failed")]
    Signal(#[source] CronErrorCause),
    #[error("cron configuration failed")]
    Config(#[source] CronErrorCause),
    #[error("cron preflight failed")]
    Preflight(#[source] crate::CronRuntimeError),
    #[error("cron runtime failed")]
    Runtime(#[from] crate::CronRuntimeError),
}
impl MainError {
    fn category(&self) -> &'static str {
        match self {
            Self::Arguments => "ARGUMENTS",
            Self::NoJobs => "NO_JOBS",
            Self::Signal(_) => "SIGNALS",
            Self::Config(_) => "CONFIG",
            Self::Preflight(CronRuntimeError::StartupInterrupted) => "INTERRUPTED",
            Self::Preflight(_) => "PREFLIGHT",
            Self::Runtime(_) => "RUNTIME",
        }
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
