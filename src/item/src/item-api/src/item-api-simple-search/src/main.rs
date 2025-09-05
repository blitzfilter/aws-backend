use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use item_api_simple_search::handler;
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

    let aws_config = aws_config::defaults(BehaviorVersion::v2025_01_17())
        .load()
        .await;

    let item_domain_endpoint = env::var("OPENSEARCH_ITEMS_DOMAIN_ENDPOINT_URL")?;
    let item_domain_endpoint_url = Url::parse(&item_domain_endpoint)?;
    let transport = TransportBuilder::new(SingleNodeConnectionPool::new(item_domain_endpoint_url))
        .auth(aws_config.try_into()?)
        .service_name("es")
        .build()?;
    let client = opensearch::OpenSearch::new(transport);
    let repository = ItemOpenSearchRepositoryImpl::new(&client);
    let service = QueryItemServiceImpl::new(&repository);

    info!(
        domainEndpointUrl = %item_domain_endpoint,
        "Lambda cold start completed, client initialized."
    );

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async { handler(event, &service).await },
    ))
    .await
}
