//! Private, fresh-only local operator tool; not a migration service or public test API.
//! Administrators are trusted. Operator must hold exclusive target custody and connect
//! directly to PostgreSQL (no transaction pool/proxy). Advisory locks only coordinate
//! cooperating initializers/migrators; they cannot fence arbitrary SQL writers.
//! No schema/history adoption, repair, reset, retry, backfill or cross-target atomicity.
mod config;
mod error;
mod initialize;
#[path = "../server_runtime/terminal.rs"]
mod terminal;

use error::{Code, Failure};
use platform_postgres::PostgresPoolConfig;
use std::{ffi::OsString, io::Write, process::ExitCode, time::Duration};
use tokio::time::timeout;

#[cfg(test)]
mod tests;

const WORK_TIMEOUT: Duration = Duration::from_secs(60);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
const PROCESS_TIMEOUT: Duration = Duration::from_secs(90);

const HELP: &str = "HELP
bootstrap-local [--initialize-fresh business|crawler | --verify business|crawler | --help]
No arguments: LEGACY non-real Docker bootstrap; creates four fixed crawler databases
and runs existing crawler helpers. Legacy alone loads dotenv. No business setup.
New modes: connect only; no Docker, database creation, dotenv, cloud or provider calls.
Required: STAGE=local|ephemeral|test and shared POSTGRES_SSL_MODE=disable|verify-full.
verify-full also requires POSTGRES_SSL_ROOT_CERT (readable CA PEM).
Selected URL only: BUSINESS_DATABASE_URL (business) or LOCAL_DB_URL (crawler),
with explicit username, password, TCP host and database. Shared URL/TLS policy applies;
PGSSLCERT/PGSSLKEY/PGSSLROOTCERT/PGOPTIONS are forbidden, even when empty.
New modes require Unix. All real/unknown/missing stages fail. Initialization also requires literal loopback
or exact localhost. Stage/host are NOT proof of disposability.
Operator prerequisites: trusted administrators, exclusive disposable-target custody,
direct PostgreSQL session, no transaction pool/proxy. Locks fence cooperating tools only.
Fresh means no ledger (even empty/exact), application object or unknown schema/extension.
Only standard built-ins/public schema and selected extension-owned/internal objects
are allowed. Extra auto-dependent objects refuse. Catalog access failures refuse.
Business: provision/preload pg_ttl_index in public first; shipped SQL creates pg_trgm
and unaccent if absent. Crawler: shipped SQL creates pgcrypto if absent.
Apply only embedded business baseline (1) or crawler baseline (6); all transactional.
No incremental upgrades, stamping, adoption, skipping, repair, deletion, reset or retry.
Verify uses existing read-only exact-history/availability gates; not full DDL drift,
TTL-worker health, provider readiness or artifact provenance proof.
Targets are separate, NOT atomic. On UNKNOWN_OUTCOME or forced termination after
possible writes: stop; operator must inspect independently. No automatic retry/cleanup
of effects. Session closure is not proof of rollback or absence of committed effects.
Bounds: work60s, close5s, process90s (includes config/runtime/output); session statement30s,
lock2s, idle/idle-in-transaction10s. Unix watchdog exits8 without logging/core collection;
any forced/abnormal exit during initialization has unknown outcome, never permits retry.
Results: HELP, INITIALIZED_BUSINESS, INITIALIZED_CRAWLER, VERIFIED_BUSINESS,
VERIFIED_CRAWLER; legacy retains its readiness message. Success exit0.
Errors: USAGE_ERROR=2; CONFIG_ERROR/UNSUPPORTED_STAGE/UNSUPPORTED_PLATFORM/NONLOCAL_ENDPOINT=3;
NOT_FRESH/PREREQUISITE_MISSING/UNSUPPORTED_SOURCE=4;
DEPENDENCY_FAILED/VERIFICATION_FAILED=5; DEADLINE_EXCEEDED/CLEANUP_UNCONFIRMED=6;
UNKNOWN_OUTCOME=7; RUNTIME_FAILED/OUTPUT_FAILED/LEGACY_FAILED=8.
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Business,
    Crawler,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Legacy,
    Help,
    Initialize(Target),
    Verify(Target),
}

fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, Failure> {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(Command::Legacy);
    };
    let first = first.to_str().ok_or_else(|| Failure::new(Code::Usage))?;
    if first == "--help" && args.next().is_none() {
        return Ok(Command::Help);
    }
    let initialize = match first {
        "--initialize-fresh" => true,
        "--verify" => false,
        _ => return Err(Failure::new(Code::Usage)),
    };
    let target = match args.next().as_deref().and_then(|value| value.to_str()) {
        Some("business") => Target::Business,
        Some("crawler") => Target::Crawler,
        _ => return Err(Failure::new(Code::Usage)),
    };
    if args.next().is_some() {
        return Err(Failure::new(Code::Usage));
    }
    Ok(if initialize {
        Command::Initialize(target)
    } else {
        Command::Verify(target)
    })
}

// One detached OS thread remains armed through actual runtime destruction, output and
// Rust's implicit terminal cleanup. process::exit can stall in that shared cleanup.
// Shared Unix _exit fence emits no payload and does not invoke core-dump collection.
// Non-Unix new modes are refused; legacy/help keep their previous platform limitations.
fn watchdog(duration: Duration) {
    if std::thread::Builder::new()
        .name("bootstrap-deadline".into())
        .spawn(move || {
            std::thread::sleep(duration);
            terminal::terminal_exit(i32::from(Code::Runtime.exit()));
        })
        .is_err()
    {
        terminal::terminal_exit(i32::from(Code::Runtime.exit()));
    }
}

fn install_panic_hook() {
    // A dependency panic may contain secrets; also never unwind into an apparent success.
    std::panic::set_hook(Box::new(|_| {
        terminal::terminal_exit(i32::from(Code::Runtime.exit()))
    }));
}

pub(super) fn main() -> ExitCode {
    watchdog(PROCESS_TIMEOUT);
    install_panic_hook();
    let mut possible_writes = false;
    let result = dispatch(std::env::args_os().skip(1), &mut possible_writes);
    emit(result, possible_writes)
}

fn dispatch(
    args: impl IntoIterator<Item = OsString>,
    possible_writes: &mut bool,
) -> Result<&'static str, Failure> {
    let command = parse(args)?; // Before env, runtime, dotenv or any dependency.
    if command == Command::Help {
        return Ok(HELP);
    }
    let selected = match command {
        Command::Initialize(target) => {
            Some((target, true, config::load(target, true, std::env::var)?))
        }
        Command::Verify(target) => {
            Some((target, false, config::load(target, false, std::env::var)?))
        }
        _ => None,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Failure::caused(Code::Runtime, error))?;
    let result = runtime.block_on(async {
        match selected {
            Some((target, true, config)) => {
                initialize::run(target, &config, possible_writes).await?;
                Ok(match target {
                    Target::Business => "INITIALIZED_BUSINESS\n",
                    Target::Crawler => "INITIALIZED_CRAWLER\n",
                })
            }
            Some((target, false, config)) => {
                verify(target, &config).await?;
                Ok(match target {
                    Target::Business => "VERIFIED_BUSINESS\n",
                    Target::Crawler => "VERIFIED_CRAWLER\n",
                })
            }
            None => legacy(possible_writes).await,
        }
    });
    // Actual teardown, not detached shutdown_timeout. OS deadline remains active.
    drop(runtime);
    result
}

fn emit(result: Result<&str, Failure>, possible_writes: bool) -> ExitCode {
    let (text, code) = match result {
        Ok(text) => (text.to_owned(), 0),
        Err(error) => {
            let error = error.after_writes(possible_writes);
            (format!("{error}\n"), error.code.exit())
        }
    };
    let mut output = std::io::stdout().lock();
    if output
        .write_all(text.as_bytes())
        .and_then(|()| output.flush())
        .is_err()
    {
        return ExitCode::from(if possible_writes {
            Code::UnknownOutcome.exit()
        } else {
            Code::Output.exit()
        });
    }
    ExitCode::from(code)
}

async fn verify(target: Target, config: &PostgresPoolConfig) -> Result<(), Failure> {
    let pool = config
        .pool_options()
        .connect_lazy_with(config.connect_options());
    let result = timeout(WORK_TIMEOUT, verify_schema(target, &pool))
        .await
        .unwrap_or_else(|error| Err(Failure::caused(Code::Deadline, error)));
    timeout(CLOSE_TIMEOUT, pool.close())
        .await
        .map_err(|error| Failure::caused(Code::Cleanup, error))?;
    result
}

async fn verify_schema(target: Target, pool: &sqlx::PgPool) -> Result<(), Failure> {
    match target {
        Target::Business => platform_postgres::verify_business_schema(pool)
            .await
            .map_err(|error| Failure::caused(Code::Verification, error)),
        Target::Crawler => crawler::local_db::verify_crawler_schema(pool)
            .await
            .map_err(|error| Failure::caused(Code::Verification, error)),
    }
}

async fn legacy(possible_writes: &mut bool) -> Result<&'static str, Failure> {
    use crawler::local_db::{
        LocalDevelopmentConfig, bootstrap_all_local_databases, parse_postgres_environment,
    };
    if let Err(error) = dotenvy::dotenv()
        && !error.not_found()
    {
        return Err(Failure::caused(Code::Config, error));
    }
    let local = parse_postgres_environment(std::env::var, |get| {
        LocalDevelopmentConfig::from_lookup("crawler-bootstrap-local", get)
    })
    .map_err(|error| Failure::caused(Code::Legacy, error))?;
    *possible_writes = true;
    bootstrap_all_local_databases(&local)
        .await
        .map_err(|error| Failure::caused(Code::Legacy, error))?;
    Ok("Local crawler databases ready; migrations applied.\n")
}
