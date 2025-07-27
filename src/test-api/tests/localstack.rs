use async_trait::async_trait;
use test_api::localstack::{get_aws_config, spin_up_localstack_with_services};
use test_api::{IntegrationTestService, get_dynamodb_client};
use test_api_macros::localstack_test;

#[tokio::test]
#[serial_test::serial]
async fn should_expose_test_host_and_port() {
    let container = spin_up_localstack_with_services(&[]).await;

    let host_ip = container.get_host().await.ok();
    let host_port = container.get_host_port_ipv4(4566).await.ok();

    assert_eq!(host_ip.unwrap().to_string(), "localhost");
    assert_eq!(host_port.unwrap(), 4566);

    drop(container);
}

#[tokio::test]
#[serial_test::serial]
async fn should_spin_up_localstack() {
    let container = spin_up_localstack_with_services(&["dynamodb"]).await;

    get_dynamodb_client()
        .await
        .list_tables()
        .send()
        .await
        .unwrap();

    drop(container);
}

struct DynamoDB();
struct Sqs();
struct Lambda();
struct Combined();

#[async_trait]
impl IntegrationTestService for DynamoDB {
    const SERVICE_NAMES: &'static [&'static str] = &["dynamodb"];
    async fn set_up() {}
}

#[async_trait]
impl IntegrationTestService for Sqs {
    const SERVICE_NAMES: &'static [&'static str] = &["sqs"];
    async fn set_up() {}
}

#[async_trait]
impl IntegrationTestService for Lambda {
    const SERVICE_NAMES: &'static [&'static str] = &["lambda"];
    async fn set_up() {}
}

#[async_trait]
impl IntegrationTestService for Combined {
    const SERVICE_NAMES: &'static [&'static str] = &["lambda", "dynamodb", "combined"];
    async fn set_up() {}
}

#[localstack_test(services = [DynamoDB])]
async fn should_start_successfully_for_single_service() {
    get_dynamodb_client()
        .await
        .list_tables()
        .send()
        .await
        .unwrap();
}

#[localstack_test(services = [DynamoDB, Sqs, Lambda])]
async fn should_start_successfully_for_multiple_services() {
    let dynamodb_client = get_dynamodb_client().await;
    let sqs_client = aws_sdk_sqs::Client::new(get_aws_config().await);
    let lambda_client = aws_sdk_lambda::Client::new(get_aws_config().await);

    dynamodb_client.list_tables().send().await.unwrap();
    sqs_client.list_queues().send().await.unwrap();
    lambda_client.list_functions().send().await.unwrap();
}

#[localstack_test(services = [Combined])]
async fn should_start_successfully_for_combined_services() {
    get_dynamodb_client()
        .await
        .list_tables()
        .send()
        .await
        .unwrap();
}

#[localstack_test(services = [Combined, DynamoDB])]
async fn should_start_successfully_for_overlapping_services() {
    get_dynamodb_client()
        .await
        .list_tables()
        .send()
        .await
        .unwrap();
}
