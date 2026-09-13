use std::collections::HashSet;
use std::net::{AddrParseError, SocketAddr};
use std::time::Duration;

pub const CRON_HEALTH_BIND_ADDR_ENV: &str = "AURA_HISTORIA_CRON_HEALTH_BIND_ADDR";
pub const CRON_SHUTDOWN_GRACE_SECONDS_ENV: &str = "AURA_HISTORIA_CRON_SHUTDOWN_GRACE_SECONDS";
pub const CRON_ENABLED_JOBS_ENV: &str = "AURA_HISTORIA_CRON_ENABLED_JOBS";
pub const STAGE_ENV: &str = "STAGE";
pub const CRON_STOP_TIMEOUT_SECONDS_ENV: &str = "AURA_HISTORIA_CRON_STOP_TIMEOUT_SECONDS";
const COMMIT_SHA_ENV: &str = "COMMIT_SHA";
const MAX_SHUTDOWN_SECONDS: u64 = 3600;

const DEFAULT_HEALTH_BIND_ADDR: &str = "127.0.0.1:8082";
const DEFAULT_SHUTDOWN_GRACE_SECONDS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronRuntimeConfig {
    health_bind_addr: SocketAddr,
    shutdown_grace: Duration,
    enabled_jobs: Vec<String>,
    stop_timeout: Duration,
    pub(crate) stage: String,
    pub(crate) source_sha: Option<String>,
}

impl CronRuntimeConfig {
    pub fn from_env(known_jobs: &[&str]) -> Result<Self, CronRuntimeConfigError> {
        Self::from_getter(
            |name| match std::env::var(name) {
                Ok(value) => Some(value),
                Err(std::env::VarError::NotPresent) => None,
                Err(std::env::VarError::NotUnicode(_)) => Some(String::new()),
            },
            known_jobs,
        )
    }

    pub(crate) fn from_getter<F>(
        mut get: F,
        known_jobs: &[&str],
    ) -> Result<Self, CronRuntimeConfigError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let health_bind_addr_raw =
            get(CRON_HEALTH_BIND_ADDR_ENV).unwrap_or_else(|| DEFAULT_HEALTH_BIND_ADDR.to_owned());
        let health_bind_addr: SocketAddr = health_bind_addr_raw.parse().map_err(|source| {
            CronRuntimeConfigError::InvalidHealthBindAddr {
                value: health_bind_addr_raw,
                source,
            }
        })?;
        if !health_bind_addr.ip().is_loopback() {
            return Err(CronRuntimeConfigError::NonLoopbackHealthBindAddr);
        }
        let shutdown_grace_seconds = parse_positive_u64(
            get(CRON_SHUTDOWN_GRACE_SECONDS_ENV),
            CRON_SHUTDOWN_GRACE_SECONDS_ENV,
            DEFAULT_SHUTDOWN_GRACE_SECONDS,
        )?;
        let stage = get(STAGE_ENV);
        let enabled_jobs = parse_enabled_jobs(get(CRON_ENABLED_JOBS_ENV), known_jobs)?;
        if enabled_jobs.is_empty() && !is_local_stage(stage.as_deref()) {
            return Err(CronRuntimeConfigError::NoEnabledJobs);
        }

        let stage = stage
            .filter(|stage| {
                matches!(
                    stage.as_str(),
                    "dev" | "prod" | "local" | "test" | "ephemeral"
                )
            })
            .ok_or(CronRuntimeConfigError::InvalidStage)?;
        let local = is_local_stage(Some(&stage));
        let stop_seconds = parse_positive_u64(
            get(CRON_STOP_TIMEOUT_SECONDS_ENV),
            CRON_STOP_TIMEOUT_SECONDS_ENV,
            330,
        )?;
        if shutdown_grace_seconds > MAX_SHUTDOWN_SECONDS || stop_seconds > MAX_SHUTDOWN_SECONDS {
            return Err(CronRuntimeConfigError::ShutdownBudgetTooLarge);
        }
        if stop_seconds < shutdown_grace_seconds + 30
            || (!local && shutdown_grace_seconds < DEFAULT_SHUTDOWN_GRACE_SECONDS)
        {
            return Err(CronRuntimeConfigError::UnsafeShutdownBudget);
        }
        let source_sha = match get(COMMIT_SHA_ENV) {
            Some(sha) if valid_sha(&sha) => Some(sha),
            None if local => None,
            _ => return Err(CronRuntimeConfigError::InvalidReleaseSha),
        };
        Ok(Self {
            health_bind_addr,
            shutdown_grace: Duration::from_secs(shutdown_grace_seconds),
            enabled_jobs,
            stop_timeout: Duration::from_secs(stop_seconds),
            stage,
            source_sha,
        })
    }

    pub const fn health_bind_addr(&self) -> SocketAddr {
        self.health_bind_addr
    }
    pub const fn shutdown_grace(&self) -> Duration {
        self.shutdown_grace
    }
    pub const fn stop_timeout(&self) -> Duration {
        self.stop_timeout
    }
    pub fn enabled_jobs(&self) -> &[String] {
        &self.enabled_jobs
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

fn is_local_stage(stage: Option<&str>) -> bool {
    matches!(stage, Some("ephemeral" | "local" | "test"))
}

fn parse_positive_u64(
    value: Option<String>,
    name: &'static str,
    default: u64,
) -> Result<u64, CronRuntimeConfigError> {
    let value = match value {
        Some(value) => value,
        None => return Ok(default),
    };
    let parsed = value
        .parse()
        .map_err(|_| CronRuntimeConfigError::InvalidPositiveInteger {
            name,
            value: value.clone(),
        })?;
    if parsed == 0 {
        return Err(CronRuntimeConfigError::InvalidPositiveInteger { name, value });
    }
    Ok(parsed)
}

fn parse_enabled_jobs(
    raw: Option<String>,
    known_jobs: &[&str],
) -> Result<Vec<String>, CronRuntimeConfigError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();
    for entry in raw.split(',') {
        let job = entry.trim();
        if job.is_empty() {
            return Err(CronRuntimeConfigError::EmptyEnabledJob);
        }
        if !known_jobs.contains(&job) {
            return Err(CronRuntimeConfigError::UnknownEnabledJob {
                name: job.to_owned(),
            });
        }
        if !seen.insert(job) {
            return Err(CronRuntimeConfigError::DuplicateEnabledJob {
                name: job.to_owned(),
            });
        }
        jobs.push(job.to_owned());
    }
    Ok(jobs)
}

#[derive(Debug, thiserror::Error)]
pub enum CronRuntimeConfigError {
    #[error("invalid {CRON_HEALTH_BIND_ADDR_ENV}: {value}")]
    InvalidHealthBindAddr {
        value: String,
        source: AddrParseError,
    },
    #[error("{CRON_HEALTH_BIND_ADDR_ENV} must use a loopback IP address")]
    NonLoopbackHealthBindAddr,
    #[error("{name} must be a positive integer, got {value}")]
    InvalidPositiveInteger { name: &'static str, value: String },
    #[error("{CRON_ENABLED_JOBS_ENV} contains an empty job name")]
    EmptyEnabledJob,
    #[error("unknown enabled cron job: {name}")]
    UnknownEnabledJob { name: String },
    #[error("duplicate enabled cron job: {name}")]
    DuplicateEnabledJob { name: String },
    #[error("at least one cron job must be enabled outside local, test, and ephemeral stages")]
    NoEnabledJobs,
    #[error("STAGE must explicitly be dev, prod, local, test, or ephemeral")]
    InvalidStage,
    #[error("cron drain and stop budgets must not exceed 3600 seconds")]
    ShutdownBudgetTooLarge,
    #[error(
        "cron stop must be at least drain + 30 seconds; real-stage drain must be at least 300 seconds"
    )]
    UnsafeShutdownBudget,
    #[error(
        "COMMIT_SHA must be a canonical non-placeholder 40-character lowercase commit SHA; required outside local stages"
    )]
    InvalidReleaseSha,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const SHA: &str = "b0dd18439ba6492029b8781cc01ebdf894df1fd2";

    fn parsed(
        stage: Option<&str>,
        drain: Option<&str>,
        stop: Option<&str>,
        sha: Option<&str>,
    ) -> Result<CronRuntimeConfig, CronRuntimeConfigError> {
        CronRuntimeConfig::from_getter(
            |name| match name {
                STAGE_ENV => stage.map(str::to_owned),
                CRON_SHUTDOWN_GRACE_SECONDS_ENV => drain.map(str::to_owned),
                CRON_STOP_TIMEOUT_SECONDS_ENV => stop.map(str::to_owned),
                COMMIT_SHA_ENV => sha.map(str::to_owned),
                CRON_ENABLED_JOBS_ENV => Some("known".into()),
                _ => None,
            },
            &["known"],
        )
    }

    #[test]
    fn should_preserve_default_budgets_and_enforce_finite_headroom() {
        let config = parsed(Some("prod"), None, None, Some(SHA)).unwrap();
        assert_eq!(config.shutdown_grace().as_secs(), 300);
        assert_eq!(config.stop_timeout().as_secs(), 330);
        for stage in ["prod", "dev", "local", "test", "ephemeral"] {
            assert!(parsed(Some(stage), Some("3570"), Some("3600"), Some(SHA)).is_ok());
            for (drain, stop) in [
                ("300", "329"),
                ("330", "330"),
                ("3571", "3600"),
                ("3600", "3600"),
                ("3601", "3631"),
                ("300", "3601"),
                ("18446744073709551585", "18446744073709551615"),
                ("0", "330"),
                ("-1", "330"),
                (" 300", "330"),
            ] {
                assert!(parsed(Some(stage), Some(drain), Some(stop), Some(SHA)).is_err());
            }
        }
        assert!(parsed(Some("prod"), Some("299"), Some("330"), Some(SHA)).is_err());
        assert!(parsed(Some("test"), Some("1"), Some("31"), None).is_ok());
    }

    #[test]
    fn should_allow_only_private_loopback_listeners() {
        let default = parsed(Some("test"), None, None, None).unwrap();
        assert_eq!(default.health_bind_addr().to_string(), "127.0.0.1:8082");
        for (address, allowed) in [
            ("127.0.0.1:8082", true),
            ("127.0.0.2:0", true),
            ("[::1]:8082", true),
            ("0.0.0.0:8082", false),
            ("[::]:8082", false),
            ("192.0.2.1:8082", false),
            ("10.0.0.1:8082", false),
            ("[2001:db8::1]:8082", false),
            ("[fe80::1]:8082", false),
            ("[::ffff:127.0.0.1]:8082", false),
            ("localhost:8082", false),
        ] {
            let result = CronRuntimeConfig::from_getter(
                |name| match name {
                    STAGE_ENV => Some("test".into()),
                    CRON_HEALTH_BIND_ADDR_ENV => Some(address.into()),
                    _ => None,
                },
                &[],
            );
            assert_eq!(result.is_ok(), allowed, "{address}");
        }
    }

    #[test]
    fn should_require_explicit_stage_and_real_release_identity() {
        for stage in [None, Some("production"), Some(""), Some(" local ")] {
            assert!(parsed(stage, None, None, Some(SHA)).is_err());
        }
        for stage in ["dev", "prod"] {
            assert!(parsed(Some(stage), None, None, None).is_err());
            assert!(parsed(Some(stage), None, None, Some(SHA)).is_ok());
        }
        for invalid in [
            "main",
            "B0DD18439BA6492029B8781CC01EBDF894DF1FD2",
            "0000000000000000000000000000000000000000",
            "0123456789abcdef0123456789abcdef01234567",
            "",
        ] {
            assert!(parsed(Some("local"), None, None, Some(invalid)).is_err());
        }
    }

    #[test]
    fn should_reject_unknown_enabled_job() {
        let values = HashMap::from([(CRON_ENABLED_JOBS_ENV, "unknown".to_owned())]);
        let result = CronRuntimeConfig::from_getter(|name| values.get(name).cloned(), &["known"]);
        assert!(matches!(
            result,
            Err(CronRuntimeConfigError::UnknownEnabledJob { .. })
        ));
    }

    #[test]
    fn should_reject_duplicate_enabled_job() {
        let values = HashMap::from([(CRON_ENABLED_JOBS_ENV, "known, known".to_owned())]);
        let result = CronRuntimeConfig::from_getter(|name| values.get(name).cloned(), &["known"]);
        assert!(matches!(
            result,
            Err(CronRuntimeConfigError::DuplicateEnabledJob { .. })
        ));
    }

    #[test]
    fn should_reject_empty_production_job_set() {
        let values = HashMap::from([(STAGE_ENV, "production".to_owned())]);
        let result = CronRuntimeConfig::from_getter(|name| values.get(name).cloned(), &["known"]);
        assert!(matches!(result, Err(CronRuntimeConfigError::NoEnabledJobs)));
    }
}
