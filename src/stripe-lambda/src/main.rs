use aws_lambda_events::eventbridge::EventBridgeEvent;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_observability::{LogLevel, LoggingConfig, init};
use platform_postgres::{PostgresPoolConfig, PostgresPoolConfigError, SqlxUnitOfWork};
use serde_json::Value;

use stripe_lambda::{StripeProductTierMap, handler};
use user_postgres::{SqlxUserRepositoryFactory, SqlxUserTierEntitlementsFactory};
use user_service::use_cases::ApplyStripeSubscriptionHandler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    init(logging_config_from_env());

    let pool = postgres_config_from_env()?.connect().await?;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let pro_product_listing_id = required_env("STRIPE_PRO_PRODUCT_ID")?;
    let ultimate_product_listing_id = required_env("STRIPE_ULTIMATE_PRODUCT_ID")?;

    let subscriptions = ApplyStripeSubscriptionHandler::new(
        unit_of_work,
        SqlxUserRepositoryFactory::new(),
        SqlxUserTierEntitlementsFactory::new(),
    );
    let tier_map = StripeProductTierMap {
        pro_product_listing_id,
        ultimate_product_listing_id,
    };

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<EventBridgeEvent<Value>>| async {
            handler(event, &subscriptions, &tier_map).await
        },
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
    PostgresPoolConfig::from_lookup("stripe-lambda", get)
}

#[cfg(test)]
mod postgres_config_tests;

fn required_env(name: &str) -> Result<String, Error> {
    std::env::var(name).map_err(|error| config_error(format!("failed to read {name}: {error}")))
}

fn config_error(message: String) -> Error {
    std::io::Error::other(message).into()
}
