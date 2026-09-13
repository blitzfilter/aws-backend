mod config;
mod error;
pub mod health;
pub mod jobs;
mod process;
pub mod scheduled_job;
pub mod scheduler;
mod shutdown;
pub mod wiring;

pub use config::{
    CRON_ENABLED_JOBS_ENV, CRON_HEALTH_BIND_ADDR_ENV, CRON_SHUTDOWN_GRACE_SECONDS_ENV,
    CRON_STOP_TIMEOUT_SECONDS_ENV, CronRuntimeConfig, CronRuntimeConfigError,
};
pub use error::CronErrorCause;
pub use process::run_process;
use scheduled_job::{ActiveExecutionTracker, CronJobExecutionOutcome, ScheduledJobRunner};
pub use scheduler::{CronSchedulerShutdownError, CronSchedulerTaskExit, JobRegistration};
use std::{future::Future, sync::Arc, time::Instant};
use tokio::{net::TcpListener, task::JoinSet};

pub async fn run_until_shutdown<S>(
    config: CronRuntimeConfig,
    registrations: Vec<JobRegistration>,
    shutdown: S,
) -> Result<(), CronRuntimeError>
where
    S: Future<Output = ()>,
{
    run_with_startup(config, async { Ok(registrations) }, false, shutdown).await
}

async fn run_with_startup<S, P>(
    config: CronRuntimeConfig,
    startup: P,
    run_once: bool,
    shutdown: S,
) -> Result<(), CronRuntimeError>
where
    S: Future<Output = ()>,
    P: Future<Output = Result<Vec<JobRegistration>, wiring::WiringError>> + Send + 'static,
{
    let listener = TcpListener::bind(config.health_bind_addr())
        .await
        .map_err(|error| CronRuntimeError::HealthBind(CronErrorCause::new(error)))?;
    let tracker = Arc::new(ActiveExecutionTracker::new());
    let health = Arc::new(health::RuntimeHealth::new(
        config.clone(),
        Arc::clone(&tracker),
    ));
    let server_health = Arc::clone(&health);
    let mut server = JoinSet::new();
    server.spawn(async move {
        axum::serve(listener, health::router(server_health))
            .await
            .map_err(|error| CronRuntimeError::HealthServer(CronErrorCause::new(error)))
    });
    let mut preparation = JoinSet::new();
    preparation.spawn(startup);
    tokio::pin!(shutdown);
    let registrations = tokio::select! {
        biased;
        () = &mut shutdown => Err(CronRuntimeError::StartupInterrupted),
        result = server.join_next() => Err(health_exit(result.unwrap_or_else(|| shutdown::fatal()))),
        result = preparation.join_next() => match result {
            Some(Ok(Ok(registrations))) => Ok(registrations),
            Some(Ok(Err(error))) => Err(CronRuntimeError::Wiring(CronErrorCause::new(error))),
            Some(Err(error)) => Err(CronRuntimeError::StartupTask(CronErrorCause::new(error))),
            None => shutdown::fatal(),
        },
    };
    let registrations = match registrations {
        Ok(registrations) => registrations,
        Err(error) => {
            let _deadline = shutdown::FatalDeadline::after(shutdown::CLEANUP_TIMEOUT);
            health.draining();
            let result =
                CronRuntimeError::combine(Err(error), stop_preparation(&mut preparation).await);
            let result = CronRuntimeError::combine(result, stop_health(&mut server).await);
            health.stopped();
            return result;
        }
    };
    if let Some(maximum) = registrations
        .iter()
        .filter_map(|registration| registration.max_run_duration)
        .max()
    {
        health.set_execution_seconds(maximum.as_secs());
    }
    let mut scheduler = None;
    let mut once = None;
    if run_once {
        if registrations.len() != 1 {
            shutdown::fatal();
        }
        for registration in registrations {
            let runner = ScheduledJobRunner::new(
                registration.job,
                Arc::clone(&tracker),
                registration.schedule,
                registration.max_run_duration,
            );
            once = Some(runner.submit());
        }
    } else {
        match scheduler::CronScheduler::start_with_tracker(registrations, Arc::clone(&tracker))
            .await
        {
            Ok(started) => scheduler = Some(started),
            Err(error) => {
                let _deadline = shutdown::FatalDeadline::after(shutdown::CLEANUP_TIMEOUT);
                health.draining();
                let result = CronRuntimeError::combine(
                    Err(CronRuntimeError::SchedulerStart(CronErrorCause::new(error))),
                    stop_health(&mut server).await,
                );
                health.stopped();
                return result;
            }
        }
    }
    health.ready();
    let result = tokio::select! {
        biased;
        () = &mut shutdown => Ok(()),
        result = server.join_next() => Err(health_exit(result.unwrap_or_else(|| shutdown::fatal()))),
        exit = async { match &mut scheduler { Some(scheduler) => scheduler.wait_for_exit().await, None => std::future::pending().await } } => Err(CronRuntimeError::SchedulerTask(exit)),
        outcome = async { match &mut once { Some(once) => once.await, None => std::future::pending().await } } => {
            once = None;
            outcome_result(outcome.unwrap_or_else(|_| shutdown::fatal()))
        },
    };
    // Arm before polling any shutdown path; retained through all task destruction.
    let _deadline = shutdown::FatalDeadline::after_drain(config.shutdown_grace());
    let started = Instant::now();
    health.draining();
    let drain = match scheduler {
        Some(scheduler) => scheduler.shutdown(config.shutdown_grace()).await,
        None => tracker
            .drain(config.shutdown_grace())
            .await
            .map_err(Into::into),
    };
    let result = if let Some(once) = once {
        let terminal = once.await.unwrap_or_else(|_| shutdown::fatal());
        // Active terminal failures are already retained by the drain. If the job
        // completed before stop-admission, run-once must still report its result.
        if drain.is_ok() {
            CronRuntimeError::combine(result, outcome_result(terminal))
        } else {
            result
        }
    } else {
        result
    };
    let result =
        CronRuntimeError::combine(result, drain.map_err(CronRuntimeError::SchedulerShutdown));
    health.stopped();
    let result = CronRuntimeError::combine(result, stop_health(&mut server).await);
    tracing::info!(
        source_sha = config.source_sha.as_deref(),
        stage = config.stage,
        duration_ms = started.elapsed().as_millis(),
        outcome = if result.is_ok() { "drained" } else { "failed" },
        "cron.runtime.stopped"
    );
    result
}

async fn stop_preparation(
    preparation: &mut JoinSet<Result<Vec<JobRegistration>, wiring::WiringError>>,
) -> Result<(), CronRuntimeError> {
    preparation.abort_all();
    let mut result = Ok(());
    while let Some(joined) = preparation.join_next().await {
        let terminal = match joined {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(CronRuntimeError::Wiring(CronErrorCause::new(error))),
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(CronRuntimeError::StartupTask(CronErrorCause::new(error))),
        };
        result = CronRuntimeError::combine(result, terminal);
    }
    result
}

async fn stop_health(
    server: &mut JoinSet<Result<(), CronRuntimeError>>,
) -> Result<(), CronRuntimeError> {
    server.abort_all();
    let mut result = Ok(());
    while let Some(joined) = server.join_next().await {
        if matches!(&joined, Err(error) if error.is_cancelled()) {
            continue;
        }
        result = CronRuntimeError::combine(result, Err(health_exit(joined)));
    }
    result
}

fn health_exit(
    result: Result<Result<(), CronRuntimeError>, tokio::task::JoinError>,
) -> CronRuntimeError {
    match result {
        Ok(Ok(())) => CronRuntimeError::HealthServerExited,
        Ok(Err(error)) => error,
        Err(error) => CronRuntimeError::HealthServerTask(CronErrorCause::new(error)),
    }
}

fn outcome_result(outcome: CronJobExecutionOutcome) -> Result<(), CronRuntimeError> {
    match outcome {
        CronJobExecutionOutcome::Succeeded | CronJobExecutionOutcome::SkippedLocalOverlap => Ok(()),
        CronJobExecutionOutcome::Failed(error) => Err(CronRuntimeError::Job(error)),
        CronJobExecutionOutcome::Panicked(error) => Err(CronRuntimeError::JobPanicked(error)),
        CronJobExecutionOutcome::TimedOut(error) => Err(CronRuntimeError::JobTimedOut(error)),
        CronJobExecutionOutcome::Cancelled | CronJobExecutionOutcome::SkippedShutdown => {
            Err(CronRuntimeError::JobCancelled)
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CronRuntimeError {
    #[error("failed to start cron scheduler")]
    SchedulerStart(#[source] CronErrorCause),
    #[error("failed to bind cron health listener")]
    HealthBind(#[source] CronErrorCause),
    #[error("cron health server failed")]
    HealthServer(#[source] CronErrorCause),
    #[error("cron health server exited unexpectedly")]
    HealthServerExited,
    #[error("cron health server task failed")]
    HealthServerTask(#[source] CronErrorCause),
    #[error("cron scheduler stopped unexpectedly")]
    SchedulerTask(#[source] CronSchedulerTaskExit),
    #[error("failed to drain cron executions")]
    SchedulerShutdown(#[source] CronSchedulerShutdownError),
    #[error("cron startup failed")]
    Wiring(#[source] CronErrorCause),
    #[error("cron startup interrupted")]
    StartupInterrupted,
    #[error("cron startup task failed")]
    StartupTask(#[source] CronErrorCause),
    #[error("cron job failed")]
    Job(#[source] scheduled_job::CronJobExecutionError),
    #[error("cron job panicked")]
    JobPanicked(#[source] CronErrorCause),
    #[error("cron job timed out")]
    JobTimedOut(#[source] Option<scheduled_job::CronJobExecutionError>),
    #[error("cron job cancelled")]
    JobCancelled,
    #[error("multiple cron runtime failures")]
    Multiple {
        #[source]
        primary: Box<CronRuntimeError>,
        additional: Box<CronRuntimeError>,
    },
}

impl CronRuntimeError {
    fn combine(primary: Result<(), Self>, additional: Result<(), Self>) -> Result<(), Self> {
        match (primary, additional) {
            (Ok(()), result) | (result, Ok(())) => result,
            (Err(primary), Err(additional)) => Err(Self::Multiple {
                primary: Box::new(primary),
                additional: Box::new(additional),
            }),
        }
    }
}
