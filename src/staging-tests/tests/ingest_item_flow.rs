use aws_sdk_dynamodb::{Client, types::AttributeValue};
use fake::{Fake, Faker};
use item_dynamodb::repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl, mk_pk};
use item_service::item_command_data::CreateItemCommandData;
use staging_tests::{get_cfn_output, get_dynamodb_client, get_sqs_client, staging_test};
use std::time::{Duration, Instant};

pub async fn wait_for_ddb_partition_to_exist(
    client: &Client,
    table: &str,
    key_name: &str,
    key_value: &str,
    timeout_secs: u64,
) -> Result<(), aws_sdk_dynamodb::Error> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);

    loop {
        let resp = client
            .get_item()
            .table_name(table)
            .key(key_name, AttributeValue::S(key_value.to_string()))
            .consistent_read(true) // strong read
            .send()
            .await?;

        if resp.item().is_some() {
            return Ok(());
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: partition with key '{key_value}' not found in DynamoDB after '{timeout_secs}' seconds"
            );
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[staging_test]
async fn should_materialize_item_in_dynamodb_for_create_item_command() {
    let stack = get_cfn_output();
    let sqs_client = get_sqs_client().await;
    let cmd: CreateItemCommandData = Faker.fake();

    let _ = sqs_client
        .send_message()
        .queue_url(stack.item_write_new_queue_url.clone())
        .message_body(serde_json::to_string(&cmd).unwrap())
        .send()
        .await
        .unwrap();

    let dynamodb_client = get_dynamodb_client().await;
    wait_for_ddb_partition_to_exist(
        dynamodb_client,
        &stack.dynamodb_table_1_name,
        "pk",
        &mk_pk(&cmd.shop_id, &cmd.shops_item_id),
        60,
    )
    .await
    .unwrap();

    let repository = ItemDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let materialized = repository
        .get_item_record(&cmd.shop_id, &cmd.shops_item_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(cmd.shop_id, materialized.shop_id);
    assert_eq!(cmd.shops_item_id, materialized.shops_item_id);
    assert_eq!(cmd.url, materialized.url);
}
