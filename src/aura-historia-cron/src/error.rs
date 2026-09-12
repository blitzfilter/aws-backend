use std::{error::Error, fmt, sync::Arc};

/// Retains the typed cause without exposing provider data or panic payloads in formatting.
/// Source traversal deliberately stops here, as at the PostgreSQL error boundary.
#[derive(Clone)]
#[doc(hidden)]
pub struct CronErrorCause {
    _original: Arc<dyn Error + Send + Sync>,
}

impl CronErrorCause {
    pub(crate) fn new(source: impl Error + Send + Sync + 'static) -> Self {
        Self {
            _original: Arc::new(source),
        }
    }
}

impl fmt::Display for CronErrorCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cron failure cause (redacted)")
    }
}

impl fmt::Debug for CronErrorCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Error for CronErrorCause {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CronRuntimeConfig, CronRuntimeError, JobRegistration,
        scheduled_job::{CronDrainError, CronJobExecutionError, CronJobExecutionOutcome},
        scheduler::CronSchedulerShutdownError,
    };
    use tokio::{sync::Notify, task::JoinError};

    const PRIVATE: &str = "provider-password-panic-payload";

    fn assert_redacted(error: &(dyn Error + 'static)) {
        let mut source = Some(error);
        while let Some(error) = source {
            assert!(!format!("{error} {error:?}").contains(PRIVATE));
            source = error.source();
        }
    }

    fn config() -> CronRuntimeConfig {
        CronRuntimeConfig::from_getter(
            |name| match name {
                "STAGE" => Some("test".into()),
                crate::CRON_HEALTH_BIND_ADDR_ENV => Some("127.0.0.1:0".into()),
                _ => None,
            },
            &[],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn should_retain_opaque_typed_startup_join_error_without_exposing_panic() {
        let error = crate::run_with_startup(
            config(),
            async {
                std::panic::panic_any(PRIVATE);
            },
            false,
            std::future::pending(),
        )
        .await
        .unwrap_err();
        assert_redacted(&error);
        let CronRuntimeError::StartupTask(cause) = error else {
            panic!("wrong startup outcome")
        };
        assert!(
            cause
                ._original
                .downcast_ref::<JoinError>()
                .unwrap()
                .is_panic()
        );
        assert!(cause.source().is_none());
    }

    #[tokio::test]
    async fn should_preserve_interruption_and_startup_cleanup_panic_together() {
        struct PanicOnDrop;
        impl Drop for PanicOnDrop {
            fn drop(&mut self) {
                std::panic::panic_any(PRIVATE);
            }
        }
        let started = Arc::new(Notify::new());
        let startup_started = Arc::clone(&started);
        let error = crate::run_with_startup(
            config(),
            async move {
                let _drop = PanicOnDrop;
                startup_started.notify_one();
                std::future::pending::<()>().await;
                Ok(Vec::<JobRegistration>::new())
            },
            false,
            async { started.notified().await },
        )
        .await
        .unwrap_err();
        assert_redacted(&error);
        let CronRuntimeError::Multiple {
            primary,
            additional,
        } = error
        else {
            panic!("failure history lost")
        };
        assert!(matches!(*primary, CronRuntimeError::StartupInterrupted));
        let CronRuntimeError::StartupTask(cause) = *additional else {
            panic!("cleanup cause lost")
        };
        assert!(
            cause
                ._original
                .downcast_ref::<JoinError>()
                .unwrap()
                .is_panic()
        );
    }

    #[tokio::test]
    async fn should_retain_timeout_and_typed_cancellation_drop_panic_together() {
        struct PanicOnDrop;
        impl Drop for PanicOnDrop {
            fn drop(&mut self) {
                std::panic::panic_any(PRIVATE);
            }
        }
        struct PendingJob;
        #[async_trait::async_trait]
        impl crate::scheduled_job::CronJob for PendingJob {
            fn name(&self) -> &'static str {
                "panic-on-cancel"
            }
            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                let _drop = PanicOnDrop;
                std::future::pending().await
            }
        }
        let runner = crate::scheduled_job::ScheduledJobRunner::new(
            Arc::new(PendingJob),
            Arc::new(crate::scheduled_job::ActiveExecutionTracker::new()),
            "test".into(),
            Some(std::time::Duration::from_millis(20)),
        );
        let error = crate::outcome_result(runner.execute_once().await).unwrap_err();
        assert_redacted(&error);
        let CronRuntimeError::JobTimedOut(Some(CronJobExecutionError::Failed(cause))) = error
        else {
            panic!("cleanup history lost")
        };
        assert!(
            cause
                ._original
                .downcast_ref::<JoinError>()
                .unwrap()
                .is_panic()
        );
    }

    #[test]
    fn should_preserve_prior_runtime_cause_and_all_drain_failures_without_leaking_sources() {
        let error = CronRuntimeError::combine(
            Err(CronRuntimeError::HealthServer(CronErrorCause::new(
                std::io::Error::other(PRIVATE),
            ))),
            Err(CronRuntimeError::SchedulerShutdown(
                CronSchedulerShutdownError::Drain(CronDrainError {
                    timeout_active: Some(1),
                    failures: vec![
                        CronJobExecutionOutcome::Failed(CronJobExecutionError::from_source(
                            std::io::Error::other(PRIVATE),
                        )),
                        CronJobExecutionOutcome::Failed(CronJobExecutionError::from_source(
                            std::io::Error::other(PRIVATE),
                        )),
                        CronJobExecutionOutcome::Cancelled,
                    ],
                }),
            )),
        )
        .unwrap_err();
        assert_redacted(&error);
        let CronRuntimeError::Multiple {
            primary,
            additional,
        } = error
        else {
            panic!("failure history lost")
        };
        let CronRuntimeError::HealthServer(cause) = *primary else {
            panic!("primary cause lost")
        };
        assert_eq!(
            cause
                ._original
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .kind(),
            std::io::ErrorKind::Other
        );
        let CronRuntimeError::SchedulerShutdown(CronSchedulerShutdownError::Drain(drain)) =
            *additional
        else {
            panic!("drain history lost")
        };
        assert_eq!(drain.timeout_active, Some(1));
        assert_eq!(drain.failures.len(), 3);
        for failure in &drain.failures[..2] {
            let CronJobExecutionOutcome::Failed(CronJobExecutionError::Failed(cause)) = failure
            else {
                panic!("job source lost")
            };
            assert!(cause._original.downcast_ref::<std::io::Error>().is_some());
        }
    }

    #[test]
    fn should_keep_existing_wiring_error_interface_behind_redacted_runtime_boundary() {
        let error = CronRuntimeError::Wiring(CronErrorCause::new(
            crate::wiring::WiringError::InvalidPolicy,
        ));
        assert_redacted(&error);
        let CronRuntimeError::Wiring(cause) = error else {
            unreachable!()
        };
        assert!(
            cause
                ._original
                .downcast_ref::<crate::wiring::WiringError>()
                .is_some()
        );
    }
}
