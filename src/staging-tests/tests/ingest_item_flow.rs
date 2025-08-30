use common::item_state::domain::ItemState;
use fake::{Fake, Faker};
use item_dynamodb::{
    item_record::ItemRecord,
    item_state_record::ItemStateRecord,
    repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl},
};
use item_service::item_command_data::{CreateItemCommandData, UpdateItemCommandData};
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

#[staging_test]
async fn should_materialize_item_in_dynamodb_for_update_item_command() {
    let stack = get_cfn_output();
    let dynamodb_client = get_dynamodb_client().await;
    let repository = ItemDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let materialized_old: ItemRecord = Faker.fake();
    let insert_res = repository
        .put_item_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(10)).await;

    let sqs_client = get_sqs_client().await;
    let new_state = match materialized_old.state {
        ItemStateRecord::Available => {
            item_service::item_state_command_data::ItemStateCommandData::Sold
        }
        _ => item_service::item_state_command_data::ItemStateCommandData::Available,
    };
    let cmd = UpdateItemCommandData {
        shop_id: materialized_old.shop_id,
        shops_item_id: materialized_old.shops_item_id,
        price: None,
        state: Some(new_state),
    };

    let _ = sqs_client
        .send_message()
        .queue_url(stack.item_write_update_queue_url.clone())
        .message_body(serde_json::to_string(&cmd).unwrap())
        .send()
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_item_record(&cmd.shop_id, &cmd.shops_item_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && ItemState::from(new_state) == ItemState::from(materialized.state)
        {
            assert_eq!(cmd.shop_id, materialized.shop_id);
            assert_eq!(cmd.shops_item_id, materialized.shops_item_id);
            assert_eq!(
                ItemState::from(new_state),
                ItemState::from(materialized.state)
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ItemRecord with shop_id '{}' and shops_item_id '{}' \
                    has not been updated in DynamoDB or been updated with expected state after 60 seconds",
                cmd.shop_id, cmd.shops_item_id
            );
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
