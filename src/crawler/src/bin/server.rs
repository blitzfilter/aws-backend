//! Production crawler CLI. No dotenv, bootstrap, migrations, or implicit mode fallback.
//!
//! `--check-config` validates environment configuration and both existing database histories.
//! It does not verify Google/AWS authentication, instantiate providers, or start workers.
//! No arguments runs the concrete daemon. Its legacy task/signal lifecycle is incomplete;
//! iteration04e2 owns graceful shutdown and process fencing.
//!
//! Build daemon/preflight artifacts with `COMMIT_SHA` set to the canonical 40-character,
//! lowercase Git SHA. This is build metadata, not a runtime environment override.

#[path = "server_runtime/config.rs"]
mod config;
#[path = "server_runtime/daemon.rs"]
mod daemon;
#[path = "server_runtime/preflight.rs"]
mod preflight;
#[path = "server_runtime/watchdog.rs"]
mod watchdog;

use config::{Mode, ServerConfig};
use std::io::{self, Write};
use std::process::ExitCode;

const HELP: &str = "Usage: server [--check-config | --help]

  (no arguments)  Run the crawler daemon with optional CloudWatch export.
                  Legacy lifecycle: signal/drain integration remains incomplete (04e2).
  --check-config  Read-only checks of both explicit PostgreSQL databases; then exit.
  --help          Show help without reading configuration or contacting dependencies.

No dotenv loading, Docker, bootstrap, DDL, migration execution, or history repair.
Daemon/check require build-time COMMIT_SHA (40 lowercase hex), LOCAL_DB_URL,
BUSINESS_DATABASE_URL, STAGE, POSTGRES_SSL_MODE, SPIDER_MAX_SIZE_BYTES,
VERTEX_AI_PROJECT_ID and VERTEX_AI_LOCATION. dev/prod require verify-full TLS
and POSTGRES_SSL_ROOT_CERT. Both URLs require explicit credentials/host/database.

SPIDER_MAX_SIZE_BYTES: 1048576..=8388608.
CRAWLER_LLM_MAX_CONCURRENT_REQUESTS: positive usize, at most Tokio semaphore
MAX_PERMITS (constructor resource ceiling); default 1.
CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS: 1..=u64::MAX; default 2000.
COMMIT_SHA is embedded at build time; runtime COMMIT_SHA overrides are ignored.
Review/model/logging settings are also validated before dependency activity.
Non-loopback CRAWLER_REVIEW_BIND_ADDR requires CRAWLER_REVIEW_AUTH_TOKEN.
Check never discovers credentials, verifies cloud authentication, creates CloudWatch
resources, constructs models, synchronizes ListingSources, or starts listeners.
DB checks plus cleanup/teardown are bounded to 60 seconds; incomplete cleanup fails.
";

#[derive(Debug, thiserror::Error)]
enum StartupError {
    #[error(transparent)]
    Arguments(#[from] config::ArgumentError),
    #[error(transparent)]
    Configuration(#[from] config::ConfigError),
    #[error(transparent)]
    Preflight(#[from] preflight::PreflightError),
    #[error(transparent)]
    Daemon(#[from] daemon::DaemonError),
    #[error("failed to create crawler runtime (details redacted)")]
    Runtime,
    #[error("failed to write CLI output (details redacted)")]
    Output,
}

fn run(mode: Mode) -> Result<(), StartupError> {
    if mode == Mode::Help {
        return io::stdout()
            .write_all(HELP.as_bytes())
            .map_err(|_| StartupError::Output);
    }
    let config = ServerConfig::from_lookup(option_env!("COMMIT_SHA"), std::env::var)?;
    if mode == Mode::Daemon {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|_| StartupError::Runtime)?;
        return runtime.block_on(daemon::run(config)).map_err(Into::into);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| StartupError::Runtime)?;
    let result = runtime.block_on(preflight::run(&config.databases));
    // Require actual destruction, not shutdown_timeout's potentially detached threads.
    // The one-shot process exits below before either watchdog could outlive its command.
    if watchdog::arm(preflight::RUNTIME_SHUTDOWN_TIMEOUT).is_err() {
        std::process::exit(1);
    }
    drop(runtime);
    result?;
    writeln!(
        io::stdout(),
        "Configuration and both database histories verified read-only; commit {}. Cloud authentication NOT verified. No daemon started.",
        config.commit_sha,
    )
    .map_err(|_| StartupError::Output)
}

fn main() -> ExitCode {
    // Parse OS strings before config, logging, providers, pools, or a Tokio runtime.
    let mode = Mode::parse(std::env::args_os().skip(1));
    let one_shot = !matches!(mode, Ok(Mode::Daemon));
    if one_shot && watchdog::arm(std::time::Duration::from_secs(60)).is_err() {
        std::process::exit(1);
    }
    let code = match mode.map_err(StartupError::from).and_then(run) {
        Ok(()) => 0_u8,
        Err(error) => {
            // Every exposed error/Debug/source is safe; never print raw input or provider bodies.
            let _ = writeln!(io::stderr(), "crawler server: {error}");
            1
        }
    };
    if one_shot {
        // Watchdog survives blocked output, all runtime destruction and the final exit boundary.
        std::process::exit(i32::from(code));
    }
    ExitCode::from(code)
}
