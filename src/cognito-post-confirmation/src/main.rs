use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use cognito_post_confirmation::handler;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_observability::{LogLevel, LoggingConfig, init};
use platform_postgres::{PostgresPoolConfig, PostgresPoolConfigError, SqlxUnitOfWork};
use user_postgres::{SqlxUserCognitoIdentityRegistryFactory, SqlxUserRepositoryFactory};
use user_service::use_cases::RegisterCognitoUserHandler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    init(logging_config_from_env());

    let pool = postgres_config_from_env()?.connect().await?;
    let service = RegisterCognitoUserHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxUserRepositoryFactory::new(),
        SqlxUserCognitoIdentityRegistryFactory::new(),
    );

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<CognitoEventUserPoolsPostConfirmation>| async {
            handler(event, &service).await
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
    PostgresPoolConfig::from_lookup("cognito-post-confirmation", get)
}

#[cfg(test)]
mod postgres_config_tests;
