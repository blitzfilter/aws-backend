use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use item_api_search_items::handler;
use item_opensearch::repository::ItemOpenSearchRepositoryImpl;
use item_service::query_service::QueryItemServiceImpl;
use lambda_runtime::tracing::info;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use opensearch::http::Url;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
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

    let aws_config = aws_config::defaults(BehaviorVersion::v2025_01_17())
        .load()
        .await;

    let os_endpoint_url = Url::parse(&env::var("OPENSEARCH_ENDPOINT_URL")?)?;
    let transport = TransportBuilder::new(SingleNodeConnectionPool::new(os_endpoint_url))
        .auth(aws_config.try_into()?)
        .service_name("es")
        .build()?;
    let client = opensearch::OpenSearch::new(transport);
    let repository = ItemOpenSearchRepositoryImpl::new(&client);
    let service = QueryItemServiceImpl::new(&repository);

    info!("Lambda cold start completed, DynamoDB-Client initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async { handler(event, &service).await },
    ))
    .await
}
