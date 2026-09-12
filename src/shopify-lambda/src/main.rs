use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use listing_source_postgres::SqlxListingSourceReaders;
use platform_observability::{LogLevel, LoggingConfig, init};
use platform_postgres::{PostgresPoolConfig, PostgresPoolConfigError, SqlxUnitOfWork};
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingRawCaptureWriterFactory,
};
use product_listing_service::use_cases::CaptureProductListingRawObservationHandler;
use shopify_lambda::{ShopifyProductListingProcessor, handler};

#[tokio::main]
async fn main() -> Result<(), Error> {
    init(logging_config_from_env());

    let pool = postgres_config_from_env()?.connect().await?;
    let processor = ShopifyProductListingProcessor::new(
        SqlxListingSourceReaders::new(pool.clone()),
        CaptureProductListingRawObservationHandler::new(
            SqlxUnitOfWork::new(pool),
            SqlxProductListingRawCaptureWriterFactory::new(),
            SqlxPartnerProductListingAuthorizerFactory::new(),
        ),
    );

    debug!("Shopify Lambda initialized");
    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(event, &processor).await
    }))
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
    PostgresPoolConfig::from_lookup("shopify-lambda", get)
}

#[cfg(test)]
mod postgres_config_tests;
