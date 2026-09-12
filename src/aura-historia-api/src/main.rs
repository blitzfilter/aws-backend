mod signals;

use aura_historia_api::{ApiConfig, ApiConfigError, ApiRunError, check_config, run_until_shutdown};
use platform_observability::{LogLevel, LoggingConfig, init};
use std::process::ExitCode;

fn main() -> ExitCode {
    init(logging_config_from_env());
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            tracing::error!(
                reason = "RUNTIME_INITIALIZATION_FAILED",
                "API process failed"
            );
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(execute());
    // Critical request tasks and the pool were joined/closed above. Bound SDK/blocking-task teardown too.
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Main's Result/Debug termination would recursively print config/provider secrets.
            tracing::error!(reason = error.code(), "API process failed");
            ExitCode::FAILURE
        }
    }
}

async fn execute() -> Result<(), MainError> {
    let mode = command_mode(std::env::args_os().skip(1))?;
    let config = ApiConfig::from_env()?;
    match mode {
        CommandMode::CheckConfig => {
            check_config(config).await?;
            tracing::info!(outcome = "read_only_checks_passed", "API preflight");
        }
        CommandMode::Serve => {
            let signals = signals::ShutdownSignals::install().map_err(MainError::Signal)?;
            run_until_shutdown(config, signals.wait()).await?;
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum CommandMode {
    Serve,
    CheckConfig,
}

fn command_mode(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<CommandMode, MainError> {
    let args: Vec<_> = args.into_iter().collect();
    match args.as_slice() {
        [] => Ok(CommandMode::Serve),
        [arg] if arg == "--check-config" => Ok(CommandMode::CheckConfig),
        _ => Err(MainError::Arguments),
    }
}

fn logging_config_from_env() -> LoggingConfig {
    let level = match std::env::var("LOG_LEVEL") {
        Ok(value) => LogLevel::parse(&value).unwrap_or_default(),
        Err(_) => LogLevel::default(),
    };
    LoggingConfig::new(level)
}

#[derive(thiserror::Error, Debug)]
enum MainError {
    #[error("expected no arguments or --check-config")]
    Arguments,
    #[error("signal registration failed")]
    Signal(#[source] std::io::Error),
    #[error("configuration rejected")]
    Config(#[from] ApiConfigError),
    #[error("API runtime failed")]
    Run(#[from] ApiRunError),
}

impl MainError {
    fn code(&self) -> &'static str {
        match self {
            Self::Arguments => "INVALID_ARGUMENTS",
            Self::Signal(_) => "SIGNAL_REGISTRATION_FAILED",
            Self::Config(_) => "INVALID_API_CONFIG",
            Self::Run(ApiRunError::State(_)) => "STARTUP_OR_PREFLIGHT_FAILED",
            Self::Run(ApiRunError::DrainDeadline | ApiRunError::CleanupDeadline) => {
                "SHUTDOWN_DEADLINE_EXCEEDED"
            }
            Self::Run(_) => "API_RUNTIME_FAILED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_only_supported_commands_without_environment_or_provider_access() {
        assert!(matches!(command_mode([]), Ok(CommandMode::Serve)));
        assert!(matches!(
            command_mode(["--check-config".into()]),
            Ok(CommandMode::CheckConfig)
        ));
        for args in [
            vec!["--migrate"],
            vec!["--check-config", "--serve"],
            vec!["--check-config=true"],
        ] {
            assert!(command_mode(args.into_iter().map(Into::into)).is_err());
        }
    }
}
