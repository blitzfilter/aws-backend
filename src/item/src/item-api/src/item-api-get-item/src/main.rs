use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use aws_sdk_dynamodb::Client;
use item_api_get_item::handler;
use item_read::repository::QueryItemRepositoryImpl;
use item_read::service::QueryItemServiceImpl;
use lambda_runtime::tracing::info;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use std::env;

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
        aws_config_builder.set_endpoint_url(Some(endpoint_url.clone()));
        info!("Using environments custom AWS_ENDPOINT_URL '{endpoint_url}'");
    }

    let client = Client::new(&aws_config_builder.build());
    let repository = QueryItemRepositoryImpl::new(&client);
    let service = QueryItemServiceImpl::new(&repository);

    info!("Lambda cold start completed, client initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async { handler(event, &service).await },
    ))
    .await
}
