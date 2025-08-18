use common::currency::domain::Currency;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::language::domain::Language;
use common::shops_item_id::ShopsItemId;
use item_opensearch::item_document::ItemDocument;
use item_opensearch::item_state_document::ItemStateDocument;
use item_opensearch::item_update_document::ItemUpdateDocument;
use item_opensearch::repository::{ItemOpenSearchRepository, ItemOpenSearchRepositoryImpl};
use opensearch::http::Url;
use search_filter_core::search_filter::SearchFilter;
use std::collections::HashMap;
use std::time::Duration;
use test_api::*;
use time::OffsetDateTime;

#[localstack_test(services = [OpenSearch()])]
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
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: vec![Url::parse("https://foo.com/bar").unwrap()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_item_documents(vec![expected.clone()])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("items").await;
    let actual = read_by_id("items", item_id).await;

    assert_eq!(expected, actual);
}

#[localstack_test(services = [OpenSearch()])]
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
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: vec![Url::parse("https://foo.com/bar").unwrap()],
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
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: vec![Url::parse("https://foo.com/bar").unwrap()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_item_documents(vec![expected1.clone(), expected2.clone()])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("items").await;
    let actual1 = read_by_id("items", item_id1).await;
    let actual2 = read_by_id("items", item_id2).await;

    assert_eq!(expected1, actual1);
    assert_eq!(expected2, actual2);
}

#[localstack_test(services = [OpenSearch()])]
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
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: vec![Url::parse("https://foo.com/bar").unwrap()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let write_response = repository
        .create_item_documents(vec![initial.clone()])
        .await
        .unwrap();
    assert!(!write_response.errors);
    refresh_index("items").await;

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
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let update_response = repository
        .update_item_documents(HashMap::from([(item_id, update)]))
        .await
        .unwrap();
    assert!(!update_response.errors);
    refresh_index("items").await;

    let mut expected = initial;
    expected.event_id = updated_event_id;
    expected.state = ItemStateDocument::Sold;
    expected.updated = updated_update_ts;

    let actual = read_by_id("items", item_id).await;

    assert_eq!(expected, actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_search_item_documents() {
    let expected = ItemDocument {
        item_id: Default::default(),
        event_id: Default::default(),
        shop_id: Default::default(),
        shops_item_id: ShopsItemId::from("abcdefgh"),
        shop_name: "Foo".to_string(),
        title_de: Some("Hallo Welt".to_string()),
        title_en: Some("Baz".to_string()),
        description_de: Some("Lorem ipsum dolor sit amet".to_string()),
        description_en: Some("Lorem ipsum dolor sit amet".to_string()),
        price_eur: Some(99),
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateDocument::Available,
        is_available: false,
        url: Url::parse("https://foo.com/bar").unwrap(),
        images: vec![Url::parse("https://foo.com/bar").unwrap()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let response = repository
        .create_item_documents(vec![expected.clone()])
        .await
        .unwrap();
    assert!(!response.errors);
    refresh_index("items").await;

    tokio::time::sleep(Duration::from_millis(3000)).await;

    let search_filter = SearchFilter {
        item_query: "Hallo Welt".into(),
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
    };
    let response = repository
        .search_item_documents(&search_filter, &Language::De, &Currency::Eur, &None, &None)
        .await
        .unwrap();

    println!("{:?}", response.hits);
}
