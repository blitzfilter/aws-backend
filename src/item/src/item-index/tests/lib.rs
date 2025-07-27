use common::event_id::EventId;
use common::item_id::ItemId;
use common::shops_item_id::ShopsItemId;
use item_core::item::document::ItemDocument;
use item_core::item::update_document::ItemUpdateDocument;
use item_core::item_state::document::ItemStateDocument;
use item_index::IndexItemDocuments;
use opensearch::params::Refresh;
use opensearch::{GetParts, IndexParts};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use test_api::*;
use time::OffsetDateTime;

async fn read_by_id<T: DeserializeOwned>(id: impl Into<String>) -> T {
    let get_response = get_opensearch_client()
        .await
        .get(GetParts::IndexId("items", &id.into()))
        .send()
        .await
        .unwrap();
    assert!(get_response.status_code().is_success());

    let response_doc: serde_json::Value = get_response.json().await.unwrap();
    serde_json::from_value(response_doc["_source"].clone()).unwrap()
}

async fn refresh_index() {
    get_opensearch_client()
        .await
        .index(IndexParts::Index("items"))
        .refresh(Refresh::True)
        .send()
        .await
        .unwrap();
}

#[localstack_test(services = [OpenSearch, S3])]
async fn should_create_item_document() {
    let item_id = ItemId::new();
    let expected = ItemDocument {
        item_id,
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_item_id: ShopsItemId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateDocument::Listed,
        is_available: false,
        url: "https://foo.com/bar".to_string(),
        images: vec!["https://foo.com/bar".to_string()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let client = get_opensearch_client().await;
    let response = client
        .create_item_documents(vec![expected.clone()])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index().await;
    let actual = read_by_id(item_id).await;

    assert_eq!(expected, actual);
}

#[localstack_test(services = [OpenSearch, S3])]
async fn should_create_item_documents() {
    let item_id1 = ItemId::new();
    let expected1 = ItemDocument {
        item_id: item_id1,
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_item_id: ShopsItemId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateDocument::Listed,
        is_available: false,
        url: "https://foo.com/bar".to_string(),
        images: vec!["https://foo.com/bar".to_string()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let item_id2 = ItemId::new();
    let expected2 = ItemDocument {
        item_id: item_id2,
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_item_id: ShopsItemId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateDocument::Listed,
        is_available: false,
        url: "https://foo.com/bar".to_string(),
        images: vec!["https://foo.com/bar".to_string()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let client = get_opensearch_client().await;
    let response = client
        .create_item_documents(vec![expected1.clone(), expected2.clone()])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index().await;
    let actual1 = read_by_id(item_id1).await;
    let actual2 = read_by_id(item_id2).await;

    assert_eq!(expected1, actual1);
    assert_eq!(expected2, actual2);
}

#[localstack_test(services = [OpenSearch, S3])]
async fn should_update_item_document() {
    let item_id = ItemId::new();
    let initial = ItemDocument {
        item_id,
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_item_id: ShopsItemId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_de: Some("Bar".to_string()),
        title_en: Some("Baz".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateDocument::Listed,
        is_available: false,
        url: "https://foo.com/bar".to_string(),
        images: vec!["https://foo.com/bar".to_string()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let client = get_opensearch_client().await;
    let write_response = client
        .create_item_documents(vec![initial.clone()])
        .await
        .unwrap();
    assert!(!write_response.errors);
    refresh_index().await;

    let updated_event_id = EventId::new();
    let updated_update_ts = OffsetDateTime::now_utc();
    let update = ItemUpdateDocument {
        event_id: updated_event_id,
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: Some(ItemStateDocument::Sold),
        is_available: None,
        updated: updated_update_ts,
    };
    let update_response = client
        .update_item_documents(HashMap::from([(item_id, update)]))
        .await
        .unwrap();
    assert!(!update_response.errors);
    refresh_index().await;

    let mut expected = initial;
    expected.event_id = updated_event_id;
    expected.state = ItemStateDocument::Sold;
    expected.updated = updated_update_ts;

    let actual = read_by_id(item_id).await;

    assert_eq!(expected, actual);
}
