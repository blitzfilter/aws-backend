use aws_tests_common::get_cfn_output;
use common::{event_id::EventId, item_id::ItemId, shop_id::ShopId, shops_item_id::ShopsItemId};
use item_opensearch::{
    item_document::ItemDocument,
    item_state_document::ItemStateDocument,
    repository::{ItemOpenSearchRepository, ItemOpenSearchRepositoryImpl},
};
use opensearch::{IndexParts, params::Refresh};
use staging_tests::{get_opensearch_client, staging_test};
use std::{
    time::{Duration, SystemTime},
    vec,
};
use url::Url;

#[staging_test]
async fn should_respond_200_when_hits() {
    let os_client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(os_client);
    let expected = ItemDocument {
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: ShopId::new(),
        shops_item_id: ShopsItemId::new(),
        shop_name: "Hans Volkers Shop".into(),
        title_de: None,
        title_en: Some("Chopin Etudes Op.10 1833".to_string()),
        description_de: None,
        description_en: None,
        price_eur: Some(1400000),
        price_usd: Some(1500000),
        price_gbp: Some(1600000),
        price_aud: Some(1700000),
        price_cad: Some(1800000),
        price_nzd: Some(1990000),
        state: ItemStateDocument::Available,
        is_available: true,
        url: Url::parse("https://hans-volker.com/chopin-etudes-op10-1833").unwrap(),
        images: vec![],
        created: SystemTime::now().into(),
        updated: SystemTime::now().into(),
    };
    let mut all = fake::vec![ItemDocument; 10];
    all.push(expected.clone());

    let insert_res = repository.create_item_documents(all).await.unwrap();
    assert!(!insert_res.errors);
    os_client
        .index(IndexParts::Index("items"))
        .refresh(Refresh::True)
        .send()
        .await
        .unwrap();
    os_client
        .index(IndexParts::Index("items"))
        .refresh(Refresh::True)
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(30)).await;

    let response = reqwest::get(format!(
        "{}/api/v1/items?q=Chopin%20Etudes&language=en&currency=EUR&sort=price&order=asc&from=0&size=5",
        get_cfn_output().api_gateway_endpoint_url
    ))
    .await
    .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(0, body["pagination"]["from"]);
    assert_eq!(5, body["pagination"]["size"]);
    assert_eq!(1, body["pagination"]["total"]);

    let item = body["items"].as_array().unwrap()[0].clone();
    assert_eq!(expected.shop_id.to_string(), item["shopId"]);
    assert_eq!(expected.shops_item_id.to_string(), item["shopsItemId"]);
    assert_eq!(expected.item_id.to_string(), item["itemId"]);
    assert_eq!(expected.event_id.to_string(), item["eventId"]);
    assert_eq!(expected.url.to_string(), item["url"]);
    assert_eq!(expected.price_eur.unwrap(), item["price"]["amount"]);
    assert_eq!("EUR", item["price"]["currency"]);
}

#[staging_test]
async fn should_respond_200_when_no_hits() {
    let response = reqwest::get(format!(
        "{}/api/v1/items?q=Sergei%20Rachmaninow",
        get_cfn_output().api_gateway_endpoint_url
    ))
    .await
    .unwrap();
    assert_eq!(200, response.status());
}
