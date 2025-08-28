use fake::{Fake, Faker};
use item_dynamodb::{
    item_record::ItemRecord,
    repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl},
};
use staging_tests::{get_cfn_output, get_dynamodb_client, staging_test};

#[staging_test]
async fn should_respond_200_when_item_does_exist() {
    let ddb_client = get_dynamodb_client().await;
    let repository =
        ItemDynamoDbRepositoryImpl::new(ddb_client, &get_cfn_output().dynamodb_table_1_name);
    let record = Faker.fake::<ItemRecord>();
    let insert_res = repository
        .put_item_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

    let response = reqwest::get(format!(
        "{}/api/v1/items/{}/{}?currency=GBP",
        get_cfn_output().api_gateway_endpoint_url,
        record.shop_id,
        record.shops_item_id
    ))
    .await
    .unwrap();

    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(record.shop_id.to_string(), body["shopId"]);
    assert_eq!(record.shops_item_id.to_string(), body["shopsItemId"]);
    assert_eq!(record.item_id.to_string(), body["itemId"]);
    assert_eq!(record.event_id.to_string(), body["eventId"]);
    assert_eq!(record.url.to_string(), body["url"]);
    assert_eq!(record.price_gbp.unwrap(), body["price"]["amount"]);
    assert_eq!("GBP", body["price"]["currency"]);
}

#[staging_test]
async fn should_respond_404_when_item_does_not_exist() {
    let response = reqwest::get(format!(
        "{}/api/v1/items/foo/bar",
        get_cfn_output().api_gateway_endpoint_url
    ))
    .await
    .unwrap();
    assert_eq!(404, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(404, body["status"]);
    assert_eq!("ITEM_NOT_FOUND", body["error"]);
}
