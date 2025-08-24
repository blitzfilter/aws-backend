use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_dynamodb::Client;
use common::price::domain::FixedFxRate;
use item_dynamodb::repository::ItemDynamoDbRepositoryImpl;
use item_lambda_write_new::handler;
use item_service::command_service::CommandItemServiceImpl;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
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

    let aws_config = aws_config::defaults(BehaviorVersion::v2025_01_17())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")?;
    let client = Client::new(&aws_config);
    let dynamodb_repository = ItemDynamoDbRepositoryImpl::new(&client, &table_name);
    let fx_rate = FixedFxRate::default();
    let service = CommandItemServiceImpl::new(&dynamodb_repository, &fx_rate);

    info!(
        dynamoDbTableName = %table_name,
        "Lambda cold start completed, client initialized."
    );

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&service, event).await
    }))
    .await
}
