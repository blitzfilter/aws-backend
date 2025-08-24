use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_dynamodb::Client;
use item_dynamodb::repository::ItemDynamoDbRepositoryImpl;
use item_lambda_materialize_dynamodb_update::handler;
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

    let client = Client::new(&aws_config);
    let repository = ItemDynamoDbRepositoryImpl::new(&client);

    info!("Lambda cold start completed, DynamoDB-Client initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&repository, event).await
    }))
    .await
}
