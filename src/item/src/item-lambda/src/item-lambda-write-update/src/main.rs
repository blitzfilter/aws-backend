use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_dynamodb::Client;
use item_lambda_write_update::handler;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use std::env;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    if dotenvy::from_filename(".env.localstack").is_ok() {
        info!("Successfully loaded '.env.localstack'.")
    }

    let mut aws_config_builder = aws_config::defaults(BehaviorVersion::v2025_01_17())
        .load()
        .await
        .into_builder();

    if let Ok(endpoint_url) = env::var("AWS_ENDPOINT_URL") {
        info!("Using environments custom AWS_ENDPOINT_URL '{endpoint_url}'");
        aws_config_builder.set_endpoint_url(Some(endpoint_url));
    }

    let ddb_client = &Client::new(&aws_config_builder.build());

    info!("Lambda cold start completed, DynamoDB-Client initialized.");

    run(service_fn(move |event: LambdaEvent<SqsEvent>| async move {
        handler(ddb_client, event).await
    }))
    .await
}
