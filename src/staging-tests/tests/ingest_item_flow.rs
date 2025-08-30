use fake::{Fake, Faker};
use item_dynamodb::repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl};
use item_service::item_command_data::CreateItemCommandData;
use staging_tests::{get_cfn_output, get_dynamodb_client, get_sqs_client, staging_test};
use std::time::{Duration, Instant};

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
    let repository = ItemDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_item_record(&cmd.shop_id, &cmd.shops_item_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized {
            assert_eq!(cmd.shop_id, materialized.shop_id);
            assert_eq!(cmd.shops_item_id, materialized.shops_item_id);
            assert_eq!(cmd.url, materialized.url);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ItemRecord with shop_id '{}' and shops_item_id '{}' not found in DynamoDB after 60 seconds",
                cmd.shop_id, cmd.shops_item_id
            );
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
