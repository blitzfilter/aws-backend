use common::{item_state::domain::ItemState, language::data::LocalizedTextData};
use fake::{Fake, Faker};
use item_dynamodb::{
    item_record::ItemRecord,
    item_state_record::ItemStateRecord,
    repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl},
};
use item_opensearch::{
    item_document::ItemDocument,
    item_state_document::ItemStateDocument,
    repository::{ItemOpenSearchRepository, ItemOpenSearchRepositoryImpl},
};
use item_service::item_command_data::{CreateItemCommandData, UpdateItemCommandData};
use opensearch::{GetParts, IndexParts, params::Refresh};
use search_filter_core::search_filter::SearchFilter;
use serde::de::DeserializeOwned;
use staging_tests::{
    get_cfn_output, get_dynamodb_client, get_opensearch_client, get_sqs_client, staging_test,
};
use std::time::{Duration, Instant};

pub async fn read_by_id<T: DeserializeOwned>(index: &str, id: impl Into<String>) -> T {
    let get_response = get_opensearch_client()
        .await
        .get(GetParts::IndexId(index, &id.into()))
        .send()
        .await
        .unwrap();
    assert!(get_response.status_code().is_success());

    let response_doc: serde_json::Value = get_response.json().await.unwrap();
    serde_json::from_value(response_doc["_source"].clone()).unwrap()
}

pub async fn refresh_index(index: &str) {
    get_opensearch_client()
        .await
        .index(IndexParts::Index(index))
        .refresh(Refresh::True)
        .send()
        .await
        .unwrap();
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
    tokio::time::sleep(Duration::from_secs(3)).await;

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

#[staging_test]
async fn should_materialize_item_in_opensearch_for_create_item_command() {
    let stack = get_cfn_output();
    let sqs_client = get_sqs_client().await;
    let mut cmd: CreateItemCommandData = Faker.fake();
    cmd.native_title = LocalizedTextData {
        text: "Exactly the expected title".to_string(),
        language: common::language::data::LanguageData::En,
    };

    let _ = sqs_client
        .send_message()
        .queue_url(stack.item_write_new_queue_url.clone())
        .message_body(serde_json::to_string(&cmd).unwrap())
        .send()
        .await
        .unwrap();

    let opensearch_client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(opensearch_client);
    refresh_index("items").await;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .search_item_documents(
                &SearchFilter {
                    item_query: "Exactly the expected title".try_into().unwrap(),
                    shop_name_query: None,
                    price_query: None,
                    state_query: Default::default(),
                    created_query: None,
                    updated_query: None,
                },
                &common::language::domain::Language::En,
                &common::currency::domain::Currency::Eur,
                &None,
                &None,
            )
            .await
            .unwrap()
            .hits
            .hits
            .first()
            .cloned();

        if let Some(materialized) = materialized {
            assert_eq!(cmd.shop_id, materialized.source.shop_id);
            assert_eq!(cmd.shops_item_id, materialized.source.shops_item_id);
            assert_eq!(cmd.url, materialized.source.url);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ItemDocument with shop_id '{}' and shops_item_id '{}' not found in OpenSearch after 60 seconds",
                cmd.shop_id, cmd.shops_item_id
            );
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[staging_test]
async fn should_materialize_item_in_opensearch_for_update_item_command() {
    let stack = get_cfn_output();

    // we also need to ingest materialized into DynamoDB because item-write-lambda-update performs validity and existence checks in the primary data-store
    let dynamodb_client = get_dynamodb_client().await;
    let repository = ItemDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let mut materialized_ddb_old: ItemRecord = Faker.fake();
    materialized_ddb_old.title_en = Some("Exactly the expected title".to_string());
    let insert_res = repository
        .put_item_records([materialized_ddb_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

    let opensearch_client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(opensearch_client);
    let materialized_os_old: ItemDocument = materialized_ddb_old.into();
    let insert_res = repository
        .create_item_documents(vec![materialized_os_old.clone()])
        .await
        .unwrap();
    assert!(!insert_res.errors);
    tracing::info!(items = ?insert_res.items, itemId = %materialized_os_old._id() , "Indexed ItemDocument");
    refresh_index("items").await;
    tokio::time::sleep(Duration::from_secs(10)).await;

    let sqs_client = get_sqs_client().await;
    let new_state = match materialized_os_old.state {
        ItemStateDocument::Available => {
            item_service::item_state_command_data::ItemStateCommandData::Sold
        }
        _ => item_service::item_state_command_data::ItemStateCommandData::Available,
    };
    let cmd = UpdateItemCommandData {
        shop_id: materialized_os_old.shop_id,
        shops_item_id: materialized_os_old.shops_item_id,
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
        refresh_index("items").await;
        let materialized = repository
            .search_item_documents(
                &SearchFilter {
                    item_query: "Exactly the expected title".try_into().unwrap(),
                    shop_name_query: None,
                    price_query: None,
                    state_query: Default::default(),
                    created_query: None,
                    updated_query: None,
                },
                &common::language::domain::Language::En,
                &common::currency::domain::Currency::Usd,
                &None,
                &None,
            )
            .await
            .unwrap()
            .hits
            .hits
            .first()
            .cloned();

        if let Some(materialized) = materialized
            && ItemState::from(new_state) == ItemState::from(materialized.source.state)
        {
            assert_eq!(cmd.shop_id, materialized.source.shop_id);
            assert_eq!(cmd.shops_item_id, materialized.source.shops_item_id);
            assert_eq!(
                ItemState::from(new_state),
                ItemState::from(materialized.source.state)
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ItemDocument with shop_id '{}' and shops_item_id '{}' \
                    has not been updated in OpenSearch or been updated with expected state after 60 seconds",
                cmd.shop_id, cmd.shops_item_id
            );
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
