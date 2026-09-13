use crate::{COMMIT_SHA_ENV, WorkerConfig, WorkerScope, WorkerStartupConfigError};
use serde::Serialize;
use std::time::Duration;

pub(crate) const DEFAULT_DRAIN_SECONDS: u64 = 270;
pub(crate) const DEFAULT_STOP_SECONDS: u64 = 300;
pub(crate) const MAX_SHUTDOWN_SECONDS: u64 = 3600;
// HTTP drains concurrently with execution. Leave 30s outside the supervisor ceiling
// for confirmed cancellation joins (bounded by 5s in main) and process teardown.
const STOP_HEADROOM_SECONDS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OperationalIdentity {
    pub(crate) schema_version: u8,
    component: &'static str,
    pub(crate) scope: &'static str,
    pub(crate) source_sha: Option<String>,
    local: bool,
}

impl OperationalIdentity {
    pub(crate) fn parse(
        scope: WorkerScope,
        stage: Option<&str>,
        sha: Option<String>,
        config: &WorkerConfig,
    ) -> Result<Self, WorkerStartupConfigError> {
        let local = crate::is_local_development_stage(stage);
        if !local {
            let drain = config.drain_timeout().as_secs();
            let stop = config.stop_timeout().as_secs();
            if drain < DEFAULT_DRAIN_SECONDS
                || stop < DEFAULT_STOP_SECONDS
                || drain
                    .checked_add(STOP_HEADROOM_SECONDS)
                    .is_none_or(|floor| stop < floor)
            {
                return Err(WorkerStartupConfigError::UnsafeDeploymentBudgets);
            }
        }
        let source_sha = match sha {
            Some(sha) if valid_sha(&sha) => Some(sha),
            None if local => None,
            None => {
                return Err(WorkerStartupConfigError::MissingEnv {
                    name: COMMIT_SHA_ENV,
                });
            }
            Some(_) => return Err(WorkerStartupConfigError::InvalidReleaseSha),
        };
        Ok(Self {
            schema_version: 1,
            component: "aura-historia-worker",
            scope: scope.as_str(),
            source_sha,
            local,
        })
    }
}

fn valid_sha(sha: &str) -> bool {
    sha.len() == 40
        && sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && !sha.bytes().all(|byte| Some(byte) == sha.bytes().next())
        && !sha.starts_with("0123456789abcdef")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationalConfig {
    pub(crate) identity: OperationalIdentity,
    pub(crate) drain: Duration,
    pub(crate) stop: Duration,
    pub(crate) execution: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WORKER_DRAIN_TIMEOUT_SECONDS_ENV, WORKER_STOP_TIMEOUT_SECONDS_ENV};
    use strum::IntoEnumIterator;

    const SHA: &str = "d5bd9ca854e713b0c587528f02037211b2020fd4";

    #[test]
    fn should_preserve_default_drain_http_execution_and_external_stop_relationship_for_all_scopes()
    {
        let config = WorkerConfig::from_getter(|_| None).unwrap();
        assert_eq!(Duration::from_secs(270), config.drain_timeout());
        assert_eq!(Duration::from_secs(300), config.stop_timeout());
        assert_eq!(Duration::from_secs(20), crate::http::CONNECTION_TIMEOUT);
        for scope in WorkerScope::iter() {
            let queue = crate::queue::SqsQueueConfig::new(
                scope,
                format!(
                    "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-{}-prod",
                    scope.as_str()
                )
                .parse()
                .unwrap(),
                "eu-central-1".into(),
                "prod".into(),
                None,
            )
            .unwrap();
            // Worst terminal settlement: three 5s deletes plus 1s + 2s backoff.
            assert!(config.drain_timeout() > queue.execution_budget() + Duration::from_secs(18));
            assert!(
                config.stop_timeout() > config.drain_timeout() + crate::http::CONNECTION_TIMEOUT
            );
            assert!(
                OperationalIdentity::parse(scope, Some("prod"), Some(SHA.into()), &config).is_ok()
            );
        }
    }

    #[test]
    fn should_reject_unsafe_real_stage_overrides_but_keep_explicit_local_short_tests() {
        for (drain, stop, valid) in [
            ("269", "300", false),
            ("270", "299", false),
            ("271", "300", false),
            ("270", "290", false),
            ("321", "351", true),
            ("3570", "3600", true),
        ] {
            let config = WorkerConfig::from_getter(|name| match name {
                WORKER_DRAIN_TIMEOUT_SECONDS_ENV => Some(drain.into()),
                WORKER_STOP_TIMEOUT_SECONDS_ENV => Some(stop.into()),
                _ => None,
            })
            .unwrap();
            for stage in ["dev", "prod"] {
                assert_eq!(
                    valid,
                    OperationalIdentity::parse(
                        WorkerScope::NotificationDelivery,
                        Some(stage),
                        Some(SHA.into()),
                        &config
                    )
                    .is_ok()
                );
            }
        }
        let config = WorkerConfig::from_getter(|name| {
            (name == WORKER_DRAIN_TIMEOUT_SECONDS_ENV).then(|| "1".into())
        })
        .unwrap();
        for stage in ["test", "local", "ephemeral"] {
            assert!(
                OperationalIdentity::parse(
                    WorkerScope::NotificationDelivery,
                    Some(stage),
                    None,
                    &config
                )
                .is_ok()
            );
        }
    }

    #[rstest::rstest]
    #[case::near_u64_max("18446744073709551585", "18446744073709551615")]
    #[case::max_drain("18446744073709551615", "300")]
    #[case::finite_drain_over_limit("3601", "3631")]
    #[case::finite_stop_over_limit("270", "3601")]
    fn should_reject_shutdown_budgets_above_one_hour_during_config_parsing(
        #[case] drain: &str,
        #[case] stop: &str,
    ) {
        let config = WorkerConfig::from_getter(|name| match name {
            WORKER_DRAIN_TIMEOUT_SECONDS_ENV => Some(drain.into()),
            WORKER_STOP_TIMEOUT_SECONDS_ENV => Some(stop.into()),
            _ => None,
        });
        assert!(matches!(
            config,
            Err(crate::WorkerConfigError::ShutdownBudgetTooLarge)
        ));
    }

    #[test]
    fn should_require_canonical_non_placeholder_release_sha_and_only_allow_explicit_local_fallback()
    {
        let config = WorkerConfig::from_getter(|_| None).unwrap();
        for invalid in [
            "",
            "main",
            "test-commit",
            "d5bd9ca",
            "D5BD9CA854E713B0C587528F02037211B2020FD4",
            "0000000000000000000000000000000000000000",
            "0123456789abcdef0123456789abcdef01234567",
        ] {
            for stage in ["prod", "test"] {
                let result = OperationalIdentity::parse(
                    WorkerScope::NotificationDelivery,
                    Some(stage),
                    Some(invalid.into()),
                    &config,
                );
                assert!(matches!(
                    result,
                    Err(WorkerStartupConfigError::InvalidReleaseSha)
                ));
                assert!(
                    !format!("{:?}", result.err().unwrap()).contains(invalid) || invalid.is_empty()
                );
            }
        }
        for stage in [None, Some("dev"), Some("prod")] {
            assert!(
                OperationalIdentity::parse(WorkerScope::NotificationDelivery, stage, None, &config)
                    .is_err()
            );
        }
    }
}
