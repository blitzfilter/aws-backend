use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use item_lambda_materialize_opensearch_update::handler;
use item_opensearch::repository::ItemOpenSearchRepositoryImpl;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use std::env;
use tracing::info;
use url::Url;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    let aws_config = aws_config::defaults(BehaviorVersion::v2025_08_07())
        .load()
        .await;

    let os_endpoint_url = Url::parse(&env::var("OPENSEARCH_ITEMS_DOMAIN_ENDPOINT_URL")?)?;
    let transport = TransportBuilder::new(SingleNodeConnectionPool::new(os_endpoint_url))
        .auth(aws_config.try_into()?)
        .service_name("es")
        .build()?;
    let client = opensearch::OpenSearch::new(transport);
    let repository = ItemOpenSearchRepositoryImpl::new(&client);

    info!("Lambda cold start completed, DynamoDB-Client initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&repository, event).await
    }))
    .await
}
