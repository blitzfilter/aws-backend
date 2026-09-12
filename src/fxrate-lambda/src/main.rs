use aws_lambda_events::eventbridge::EventBridgeEvent;
use fxrate_fxratesapi::FxRatesApiQuoteProvider;
use fxrate_lambda::handler;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use fxrate_service::CaptureFxRateSnapshotHandler;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_observability::{LogLevel, LoggingConfig, init};
use platform_postgres::{PostgresPoolConfig, PostgresPoolConfigError, SqlxUnitOfWork};
use serde_json::Value;

use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    init(logging_config_from_env());

    let pool = postgres_config_from_env()?.connect().await?;
    let token = std::env::var("FXRATES_API_TOKEN")
        .map_err(|_| Error::from("missing required environment variable FXRATES_API_TOKEN"))?;
    let snapshots = CaptureFxRateSnapshotHandler::new(
        FxRatesApiQuoteProvider::new(reqwest::Client::new(), token),
        SqlxUnitOfWork::new(pool),
        SqlxFxRateSnapshotRepositoryFactory::new(),
    );

    debug!("FX rate Lambda initialized");
    run(service_fn(
        |event: LambdaEvent<EventBridgeEvent<Value>>| async { handler(event, &snapshots).await },
    ))
    .await
}

fn logging_config_from_env() -> LoggingConfig {
    let level = std::env::var("LOG_LEVEL")
        .ok()
        .as_deref()
        .and_then(LogLevel::parse)
        .unwrap_or_default();
    LoggingConfig::new(level)
}

fn postgres_config_from_env() -> Result<PostgresPoolConfig, PostgresPoolConfigError> {
    postgres_config(&mut |name| match std::env::var(name) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        // Preserve presence so malformed optional inputs cannot fall back to defaults.
        Err(std::env::VarError::NotUnicode(_)) => Some(String::new()),
    })
}

fn postgres_config(
    get: &mut impl FnMut(&'static str) -> Option<String>,
) -> Result<PostgresPoolConfig, PostgresPoolConfigError> {
    PostgresPoolConfig::from_lookup("fxrate-lambda", get)
}

#[cfg(test)]
mod postgres_config_tests;
