use crate::scheduled_job::{ActiveExecutionTracker, CronDrainError, CronJob, ScheduledJobRunner};
use chrono::Utc;
use cron_tab::AsyncCron;
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::task::JoinHandle;
use tracing::info;

#[doc(hidden)]
pub struct JobRegistration {
    pub name: &'static str,
    pub schedule: String,
    pub max_run_duration: Option<Duration>,
    pub job: Arc<dyn CronJob>,
}

#[doc(hidden)]
pub struct CronScheduler {
    tracker: Arc<ActiveExecutionTracker>,
    scheduler_task: Option<JoinHandle<()>>,
}

impl CronScheduler {
    pub async fn start(
        registrations: Vec<JobRegistration>,
    ) -> Result<Self, CronSchedulerStartError> {
        Self::start_with_tracker(registrations, Arc::new(ActiveExecutionTracker::new())).await
    }

    pub(crate) async fn start_with_tracker(
        registrations: Vec<JobRegistration>,
        tracker: Arc<ActiveExecutionTracker>,
    ) -> Result<Self, CronSchedulerStartError> {
        if registrations.is_empty() {
            return Err(CronSchedulerStartError::NoJobs);
        }
        let mut names = HashSet::new();
        let mut cron = AsyncCron::new(Utc);
        for registration in registrations {
            if !names.insert(registration.name) {
                return Err(CronSchedulerStartError::DuplicateJob {
                    name: registration.name,
                });
            }
            let runner = Arc::new(ScheduledJobRunner::new(
                registration.job,
                Arc::clone(&tracker),
                registration.schedule.clone(),
                registration.max_run_duration,
            ));
            cron.add_fn(&registration.schedule, move || {
                let runner = Arc::clone(&runner);
                // cron_tab's unowned task is only a trigger. All execution joins belong
                // to the tracker; a late trigger cannot pass its closed admission gate.
                async move {
                    drop(runner.submit());
                }
            })
            .await
            .map_err(|error| CronSchedulerStartError::InvalidSchedule {
                name: registration.name,
                detail: error.to_string(),
            })?;
        }
        let scheduler_task = tokio::spawn(async move { cron.start_blocking().await });
        info!(job_count = names.len(), "cron.scheduler.started");
        Ok(Self {
            tracker,
            scheduler_task: Some(scheduler_task),
        })
    }

    pub async fn wait_for_exit(&mut self) -> CronSchedulerTaskExit {
        let Some(task) = self.scheduler_task.as_mut() else {
            return CronSchedulerTaskExit::ObserverLost;
        };
        // Borrow, do not take: select cancellation must retain the shutdown join.
        let outcome = match task.await {
            Ok(()) => CronSchedulerTaskExit::Exited,
            Err(error) if error.is_panic() => {
                CronSchedulerTaskExit::Panicked(crate::CronErrorCause::new(error))
            }
            Err(error) => CronSchedulerTaskExit::Cancelled(crate::CronErrorCause::new(error)),
        };
        self.scheduler_task = None;
        outcome
    }

    pub async fn shutdown(mut self, grace: Duration) -> Result<(), CronSchedulerShutdownError> {
        let _deadline = crate::shutdown::FatalDeadline::after_drain(grace);
        let started = Instant::now();
        self.tracker.stop_accepting();
        let mut scheduler_failure = None;
        if let Some(task) = self.scheduler_task.take() {
            task.abort();
            match task.await {
                Ok(()) => scheduler_failure = Some(CronSchedulerTaskExit::Exited),
                Err(error) if error.is_cancelled() => {}
                Err(error) => {
                    scheduler_failure = Some(CronSchedulerTaskExit::Panicked(
                        crate::CronErrorCause::new(error),
                    ))
                }
            }
        }
        let result = self
            .tracker
            .drain(grace.saturating_sub(started.elapsed()))
            .await;
        info!(
            duration_ms = started.elapsed().as_millis(),
            outcome = if result.is_ok() {
                "drained"
            } else {
                "cancelled"
            },
            "cron.scheduler.drained"
        );
        match (scheduler_failure, result) {
            (None, result) => result.map_err(Into::into),
            (Some(task), drain) => Err(CronSchedulerShutdownError::Task {
                task,
                drain: drain.err(),
            }),
        }
    }
}

impl Drop for CronScheduler {
    fn drop(&mut self) {
        self.tracker.stop_accepting();
        if let Some(task) = &self.scheduler_task {
            task.abort();
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[doc(hidden)]
pub enum CronSchedulerStartError {
    #[error("at least one cron job must be registered")]
    NoJobs,
    #[error("duplicate cron job registration: {name}")]
    DuplicateJob { name: &'static str },
    #[error("invalid cron schedule for {name}: {detail}")]
    InvalidSchedule { name: &'static str, detail: String },
}

#[derive(Debug, thiserror::Error)]
#[doc(hidden)]
pub enum CronSchedulerShutdownError {
    #[error(transparent)]
    Drain(#[from] CronDrainError),
    #[error("scheduler task failed during shutdown")]
    Task {
        #[source]
        task: CronSchedulerTaskExit,
        drain: Option<CronDrainError>,
    },
}

#[derive(Debug, Clone, thiserror::Error)]
#[doc(hidden)]
pub enum CronSchedulerTaskExit {
    #[error("scheduler task exited")]
    Exited,
    #[error("scheduler task panicked")]
    Panicked(#[source] crate::CronErrorCause),
    #[error("scheduler task was cancelled")]
    Cancelled(#[source] crate::CronErrorCause),
    #[error("scheduler task observer stopped")]
    ObserverLost,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduled_job::CronJobExecutionError;
    use async_trait::async_trait;

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

    #[tokio::test]
    async fn should_reject_invalid_seven_field_schedule() {
        let result = CronScheduler::start(vec![JobRegistration {
            name: "success",
            schedule: "invalid".to_owned(),
            max_run_duration: None,
            job: Arc::new(SuccessJob),
        }])
        .await;
        assert!(matches!(
            result,
            Err(CronSchedulerStartError::InvalidSchedule { .. })
        ));
    }

    #[tokio::test]
    async fn should_report_unexpected_scheduler_exit() {
        let mut scheduler = scheduler_for_task(tokio::spawn(async {}));
        assert!(matches!(
            scheduler.wait_for_exit().await,
            CronSchedulerTaskExit::Exited
        ));
    }

    #[tokio::test]
    async fn should_report_scheduler_panic() {
        let mut scheduler = scheduler_for_task(tokio::spawn(async {
            std::panic::panic_any("scheduler test panic");
        }));
        assert!(matches!(
            scheduler.wait_for_exit().await,
            CronSchedulerTaskExit::Panicked(_)
        ));
    }

    #[tokio::test]
    async fn should_retain_scheduler_join_when_exit_wait_is_cancelled() {
        let mut scheduler = scheduler_for_task(tokio::spawn(std::future::pending()));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), scheduler.wait_for_exit())
                .await
                .is_err()
        );
        assert!(scheduler.scheduler_task.is_some());
        assert!(scheduler.shutdown(Duration::from_secs(1)).await.is_ok());
    }

    fn scheduler_for_task(task: JoinHandle<()>) -> CronScheduler {
        CronScheduler {
            tracker: Arc::new(ActiveExecutionTracker::new()),
            scheduler_task: Some(task),
        }
    }
}
