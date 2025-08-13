use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_dynamodb::Client;
use common::price::domain::FixedFxRate;
use item_dynamodb::repository::ItemDynamoDbRepositoryImpl;
use item_lambda_write_update::handler;
use item_service::command_service::CommandItemServiceImpl;
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

    let client = Client::new(&aws_config_builder.build());
    let dynamodb_repository = ItemDynamoDbRepositoryImpl::new(&client);
    let fx_rate = FixedFxRate::default();
    let service = CommandItemServiceImpl::new(&dynamodb_repository, &fx_rate);

    info!("Lambda cold start completed, DynamoDB-Client initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&service, event).await
    }))
    .await
}
