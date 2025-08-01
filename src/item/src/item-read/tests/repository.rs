use common::batch::Batch;
use common::currency::record::CurrencyRecord;
use common::event_id::EventId;
use common::has::HasKey;
use common::item_id::{ItemId, ItemKey};
use common::language::record::{LanguageRecord, TextRecord};
use common::price::record::PriceRecord;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::hash::{ItemHash, ItemSummaryHash};
use item_core::item::record::ItemRecord;
use item_core::item_event::record::ItemEventRecord;
use item_core::item_event_type::record::ItemEventTypeRecord;
use item_core::item_state::domain::ItemState;
use item_core::item_state::record::ItemStateRecord;
use item_read::repository::ReadItemRecords;
use std::time::Duration;
use test_api::tokio::time::sleep;
use test_api::*;
use time::OffsetDateTime;
use time::format_description::well_known;

#[localstack_test(services = [DynamoDB()])]
async fn should_return_nothing_for_get_item_record_when_table_is_empty() {
    let client = get_dynamodb_client().await;
    let actual = client
        .get_item_record(&ShopId::new(), &"non-existent".into())
        .await
        .unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_item_record_for_get_item_record_when_exists() {
    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&well_known::Rfc3339).unwrap();
    let shop_id = ShopId::new();
    let shops_item_id: ShopsItemId = "123465".into();
    let expected = ItemRecord {
        pk: format!(
            "item#shop_id#{}#shops_item_id#{shops_item_id}",
            shop_id.clone()
        ),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: format!("updated#{now_str}"),
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: "Foo".to_string(),
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: Some(PriceRecord {
            amount: 110,
            currency: CurrencyRecord::Eur,
        }),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateRecord::Available,
        url: "https:://foo.bar/123456".to_string(),
        images: vec!["https:://foo.bar/123456/image".to_string()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now,
        updated: now,
    };

    let client = get_dynamodb_client().await;
    client
        .put_item()
        .table_name("items")
        .set_item(serde_dynamo::to_item(&expected).ok())
        .send()
        .await
        .unwrap();

    let actual = client
        .get_item_record(&shop_id, &shops_item_id)
        .await
        .unwrap();

    assert!(actual.is_some());
    assert_eq!(expected, actual.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_nothing_for_get_item_record_when_only_others_exist() {
    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&well_known::Rfc3339).unwrap();
    let shop_id = ShopId::new();
    let shops_item_id: ShopsItemId = "123465".into();
    let other = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: format!("updated#{now_str}"),
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: "Foo".to_string(),
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: Some(PriceRecord {
            amount: 110,
            currency: CurrencyRecord::Eur,
        }),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateRecord::Available,
        url: "https:://foo.bar/123456".to_string(),
        images: vec!["https:://foo.bar/123456/image".to_string()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now,
        updated: now,
    };

    let client = get_dynamodb_client().await;
    client
        .put_item()
        .table_name("items")
        .set_item(serde_dynamo::to_item(&other).ok())
        .send()
        .await
        .unwrap();

    let actual = client
        .get_item_record(&ShopId::new(), &"non-existent".into())
        .await
        .unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_nothing_for_get_item_record_when_only_others_exist_mix() {
    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&well_known::Rfc3339).unwrap();
    let shop_id = ShopId::new();
    let shops_item_id: ShopsItemId = "123465".into();
    let other1 = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: format!("updated#{now_str}"),
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: "Foo".to_string(),
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: Some(PriceRecord {
            amount: 110,
            currency: CurrencyRecord::Eur,
        }),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateRecord::Available,
        url: "https:://foo.bar/123456".to_string(),
        images: vec!["https:://foo.bar/123456/image".to_string()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now,
        updated: now,
    };
    let other2 = ItemEventRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
        sk: format!("item#event#{now_str}"),
        item_id: ItemId::new(),
        event_id: EventId::new(),
        event_type: ItemEventTypeRecord::StateListed,
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: None,
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: None,
        state: Some(ItemStateRecord::Listed),
        url: None,
        images: Some(vec!["https:://foo.bar/123456/image".to_string()]),
        hash: ItemHash::new(&None, &ItemState::Listed),
        timestamp: OffsetDateTime::now_utc(),
    };

    let client = get_dynamodb_client().await;
    client
        .put_item()
        .table_name("items")
        .set_item(serde_dynamo::to_item(&other1).ok())
        .send()
        .await
        .unwrap();
    client
        .put_item()
        .table_name("items")
        .set_item(serde_dynamo::to_item(&other2).ok())
        .send()
        .await
        .unwrap();

    let actual = client
        .get_item_record(&ShopId::new(), &"non-existent".into())
        .await
        .unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_nothing_for_query_item_diff_records_when_table_is_empty() {
    let client = get_dynamodb_client().await;
    let actual = client
        .query_item_hashes(&ShopId::new(), true)
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_item_diff_record_for_query_item_diff_records_when_exists() {
    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&well_known::Rfc3339).unwrap();
    let shop_id = ShopId::new();
    let shops_item_id: ShopsItemId = "123465".into();
    let inserted = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: format!("updated#{now_str}"),
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: "Foo".to_string(),
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: Some(PriceRecord {
            amount: 110,
            currency: CurrencyRecord::Eur,
        }),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateRecord::Available,
        url: "https:://foo.bar/123456".to_string(),
        images: vec!["https:://foo.bar/123456/image".to_string()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now,
        updated: now,
    };

    let client = get_dynamodb_client().await;
    client
        .put_item()
        .table_name("items")
        .set_item(serde_dynamo::to_item(&inserted).ok())
        .send()
        .await
        .unwrap();

    // Wait for GSI
    sleep(Duration::from_secs(3)).await;

    let expected: ItemSummaryHash = inserted.into();
    let actual = client.query_item_hashes(&shop_id, true).await.unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_item_diff_records_for_query_item_diff_records_when_exists() {
    let now1 = OffsetDateTime::now_utc();
    let now1_str = now1.format(&well_known::Rfc3339).unwrap();
    let shop_id = ShopId::new();
    let shops_item_id_1: ShopsItemId = "123465".into();
    let inserted1 = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id_1}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: format!("updated#{now1_str}"),
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id_1.clone(),
        shop_name: "Foo".to_string(),
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: Some(PriceRecord {
            amount: 110,
            currency: CurrencyRecord::Eur,
        }),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateRecord::Available,
        url: "https:://foo.bar/123456".to_string(),
        images: vec!["https:://foo.bar/123456/image".to_string()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now1,
        updated: now1,
    };
    let shops_item_id_2: ShopsItemId = "abcdefg".into();
    let now2 = OffsetDateTime::now_utc();
    let now2_str = now2.format(&well_known::Rfc3339).unwrap();
    let inserted2 = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id_2}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: format!("updated#{now2_str}"),
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id_2.clone(),
        shop_name: "Foo".to_string(),
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: Some(PriceRecord {
            amount: 110,
            currency: CurrencyRecord::Eur,
        }),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateRecord::Available,
        url: "https:://foo.bar/123456".to_string(),
        images: vec!["https:://foo.bar/123456/image".to_string()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now2,
        updated: now2,
    };

    let client = get_dynamodb_client().await;
    client
        .put_item()
        .table_name("items")
        .set_item(serde_dynamo::to_item(&inserted1).ok())
        .send()
        .await
        .unwrap();
    client
        .put_item()
        .table_name("items")
        .set_item(serde_dynamo::to_item(&inserted2).ok())
        .send()
        .await
        .unwrap();

    // Wait for GSI
    sleep(Duration::from_secs(3)).await;

    let expected1: ItemSummaryHash = inserted1.into();
    let expected2: ItemSummaryHash = inserted2.into();
    let actual = client.query_item_hashes(&shop_id, true).await.unwrap();

    assert_eq!(2, actual.len());
    assert!(actual.contains(&expected1));
    assert!(actual.contains(&expected2));
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_item_diff_records_sorted_by_created_latest_for_query_item_diff_records_when_exists_and_scan_forward()
 {
    let now1 = OffsetDateTime::now_utc();
    let now1_str = now1.format(&well_known::Rfc3339).unwrap();
    let shop_id = ShopId::new();
    let shops_item_id_1: ShopsItemId = "123465".into();
    let inserted1 = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id_1}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: format!("updated#{now1_str}"),
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id_1.clone(),
        shop_name: "Foo".to_string(),
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: Some(PriceRecord {
            amount: 110,
            currency: CurrencyRecord::Eur,
        }),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateRecord::Available,
        url: "https:://foo.bar/123456".to_string(),
        images: vec!["https:://foo.bar/123456/image".to_string()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now1,
        updated: now1,
    };
    let shops_item_id_2: ShopsItemId = "abcdefg".into();
    let now2 = OffsetDateTime::now_utc();
    let now2_str = now2.format(&well_known::Rfc3339).unwrap();
    let inserted2 = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id_2}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: format!("updated#{now2_str}"),
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id_2.clone(),
        shop_name: "Foo".to_string(),
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: Some(PriceRecord {
            amount: 110,
            currency: CurrencyRecord::Eur,
        }),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateRecord::Available,
        url: "https:://foo.bar/123456".to_string(),
        images: vec!["https:://foo.bar/123456/image".to_string()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now2,
        updated: now2,
    };

    let client = get_dynamodb_client().await;
    client
        .put_item()
        .table_name("items")
        .set_item(serde_dynamo::to_item(&inserted1).ok())
        .send()
        .await
        .unwrap();
    client
        .put_item()
        .table_name("items")
        .set_item(serde_dynamo::to_item(&inserted2).ok())
        .send()
        .await
        .unwrap();

    // Wait for GSI
    sleep(Duration::from_secs(3)).await;

    let expected1: ItemSummaryHash = inserted1.into();
    let expected2: ItemSummaryHash = inserted2.into();
    let actual = client.query_item_hashes(&shop_id, true).await.unwrap();

    assert_eq!(vec![expected1, expected2], actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_nothing_for_query_item_diff_records_when_only_others_exist() {
    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&well_known::Rfc3339).unwrap();
    let shop_id = ShopId::new();
    let shops_item_id: ShopsItemId = "123465".into();
    let other = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: format!("updated#{now_str}"),
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: "Foo".to_string(),
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: Some(PriceRecord {
            amount: 110,
            currency: CurrencyRecord::Eur,
        }),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateRecord::Available,
        url: "https:://foo.bar/123456".to_string(),
        images: vec!["https:://foo.bar/123456/image".to_string()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now,
        updated: now,
    };

    let client = get_dynamodb_client().await;
    client
        .put_item()
        .table_name("items")
        .set_item(serde_dynamo::to_item(&other).ok())
        .send()
        .await
        .unwrap();

    // Wait for GSI
    sleep(Duration::from_secs(3)).await;

    let actual = client
        .query_item_hashes(&ShopId::new(), true)
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_nothing_for_query_item_diff_records_when_only_others_exist_mix() {
    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&well_known::Rfc3339).unwrap();
    let shop_id = ShopId::new();
    let shops_item_id: ShopsItemId = "123465".into();
    let other = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: format!("updated#{now_str}"),
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: "Foo".to_string(),
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: Some(PriceRecord {
            amount: 110,
            currency: CurrencyRecord::Eur,
        }),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateRecord::Available,
        url: "https:://foo.bar/123456".to_string(),
        images: vec!["https:://foo.bar/123456/image".to_string()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now,
        updated: now,
    };
    let other2 = ItemEventRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
        sk: format!("item#event#{now_str}"),
        item_id: ItemId::new(),
        event_id: EventId::new(),
        event_type: ItemEventTypeRecord::StateListed,
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: None,
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: None,
        state: Some(ItemStateRecord::Listed),
        url: None,
        images: Some(vec!["https:://foo.bar/123456/image".to_string()]),
        hash: ItemHash::new(&None, &ItemState::Listed),
        timestamp: OffsetDateTime::now_utc(),
    };

    let client = get_dynamodb_client().await;
    client
        .put_item()
        .table_name("items")
        .set_item(serde_dynamo::to_item(&other).ok())
        .send()
        .await
        .unwrap();
    client
        .put_item()
        .table_name("items")
        .set_item(serde_dynamo::to_item(&other2).ok())
        .send()
        .await
        .unwrap();

    // Wait for GSI
    sleep(Duration::from_secs(3)).await;

    let actual = client
        .query_item_hashes(&ShopId::new(), true)
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_item_records_for_batch_get_item_records_when_all_exist() {
    let client = get_dynamodb_client().await;
    let shop_id = ShopId::new();
    let mk_expected = |n: i32| {
        let now = OffsetDateTime::now_utc();
        let now_str = now.format(&well_known::Rfc3339).unwrap();
        let shops_item_id: ShopsItemId = n.to_string().into();
        ItemRecord {
            pk: format!(
                "item#shop_id#{}#shops_item_id#{shops_item_id}",
                shop_id.clone()
            ),
            sk: "item#materialized".to_string(),
            gsi_1_pk: format!("shop_id#{shop_id}"),
            gsi_1_sk: format!("updated#{now_str}"),
            item_id: ItemId::new(),
            event_id: EventId::new(),
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            shop_name: "Foo".to_string(),
            title: Some(TextRecord::new("Bar", LanguageRecord::De)),
            title_de: Some("Bar".to_string()),
            title_en: Some("Barr".to_string()),
            description: Some(TextRecord::new("Baz", LanguageRecord::De)),
            description_de: Some("Baz".to_string()),
            description_en: Some("Bazz".to_string()),
            price: Some(PriceRecord {
                amount: 110,
                currency: CurrencyRecord::Eur,
            }),
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: ItemStateRecord::Available,
            url: format!("https:://foo.bar/{n}"),
            images: vec![format!("https:://foo.bar/{n}/image")],
            hash: ItemHash::new(&None, &ItemState::Available),
            created: now,
            updated: now,
        }
    };
    let mut expecteds = Vec::with_capacity(100);
    for n in 1..=100 {
        let expected = mk_expected(n);
        client
            .put_item()
            .table_name("items")
            .set_item(serde_dynamo::to_item(&expected).ok())
            .send()
            .await
            .unwrap();
        expecteds.push(expected);
    }

    let mut actuals = client
        .get_item_records(
            &Batch::try_from(
                (1..=100)
                    .map(|n| ItemKey::new(shop_id.clone(), ShopsItemId::from(n.to_string())))
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    assert!(actuals.unprocessed.is_none());
    assert_eq!(actuals.items.len(), 100);

    expecteds.sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
    actuals
        .items
        .sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
    assert_eq!(actuals.items, expecteds);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_item_records_for_batch_get_item_records_when_some_do_not_exist() {
    let client = get_dynamodb_client().await;
    let shop_id = ShopId::new();
    let mk_expected = |n: i32| {
        let now = OffsetDateTime::now_utc();
        let now_str = now.format(&well_known::Rfc3339).unwrap();
        let shops_item_id: ShopsItemId = n.to_string().into();
        ItemRecord {
            pk: format!(
                "item#shop_id#{}#shops_item_id#{shops_item_id}",
                shop_id.clone()
            ),
            sk: "item#materialized".to_string(),
            gsi_1_pk: format!("shop_id#{shop_id}"),
            gsi_1_sk: format!("updated#{now_str}"),
            item_id: ItemId::new(),
            event_id: EventId::new(),
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            shop_name: "Foo".to_string(),
            title: Some(TextRecord::new("Bar", LanguageRecord::De)),
            title_de: Some("Bar".to_string()),
            title_en: Some("Barr".to_string()),
            description: Some(TextRecord::new("Baz", LanguageRecord::De)),
            description_de: Some("Baz".to_string()),
            description_en: Some("Bazz".to_string()),
            price: Some(PriceRecord {
                amount: 110,
                currency: CurrencyRecord::Eur,
            }),
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: ItemStateRecord::Available,
            url: format!("https:://foo.bar/{n}"),
            images: vec![format!("https:://foo.bar/{n}/image")],
            hash: ItemHash::new(&None, &ItemState::Available),
            created: now,
            updated: now,
        }
    };
    let mut expecteds = Vec::with_capacity(100);
    for n in 1..=10 {
        let expected = mk_expected(n);
        client
            .put_item()
            .table_name("items")
            .set_item(serde_dynamo::to_item(&expected).ok())
            .send()
            .await
            .unwrap();
        expecteds.push(expected);
    }

    let mut actuals = client
        .get_item_records(
            &Batch::try_from(
                (1..=14)
                    .map(|n| ItemKey::new(shop_id.clone(), ShopsItemId::from(n.to_string())))
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    assert!(actuals.unprocessed.is_none());
    assert_eq!(actuals.items.len(), 10);

    expecteds.sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
    actuals
        .items
        .sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
    assert_eq!(actuals.items, expecteds);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_item_records_for_batch_get_item_records_when_more_than_100_exist() {
    let client = get_dynamodb_client().await;
    let shop_id = ShopId::new();
    let mk_expected = |n: i32| {
        let now = OffsetDateTime::now_utc();
        let now_str = now.format(&well_known::Rfc3339).unwrap();
        let shops_item_id: ShopsItemId = n.to_string().into();
        ItemRecord {
            pk: format!(
                "item#shop_id#{}#shops_item_id#{shops_item_id}",
                shop_id.clone()
            ),
            sk: "item#materialized".to_string(),
            gsi_1_pk: format!("shop_id#{shop_id}"),
            gsi_1_sk: format!("updated#{now_str}"),
            item_id: ItemId::new(),
            event_id: EventId::new(),
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            shop_name: "Foo".to_string(),
            title: Some(TextRecord::new("Bar", LanguageRecord::De)),
            title_de: Some("Bar".to_string()),
            title_en: Some("Barr".to_string()),
            description: Some(TextRecord::new("Baz", LanguageRecord::De)),
            description_de: Some("Baz".to_string()),
            description_en: Some("Bazz".to_string()),
            price: Some(PriceRecord {
                amount: 110,
                currency: CurrencyRecord::Eur,
            }),
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: ItemStateRecord::Available,
            url: format!("https:://foo.bar/{n}"),
            images: vec![format!("https:://foo.bar/{n}/image")],
            hash: ItemHash::new(&None, &ItemState::Available),
            created: now,
            updated: now,
        }
    };
    let mut expecteds = Vec::with_capacity(100);
    for n in 1..=120 {
        let expected = mk_expected(n);
        client
            .put_item()
            .table_name("items")
            .set_item(serde_dynamo::to_item(&expected).ok())
            .send()
            .await
            .unwrap();
        if n <= 100 {
            expecteds.push(expected);
        }
    }

    let mut actuals = client
        .get_item_records(
            &Batch::try_from(
                (1..=100)
                    .map(|n| ItemKey::new(shop_id.clone(), ShopsItemId::from(n.to_string())))
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    assert!(actuals.unprocessed.is_none());
    assert_eq!(actuals.items.len(), 100);

    expecteds.sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
    actuals
        .items
        .sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
    assert_eq!(actuals.items, expecteds);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_item_keys_for_batch_exist_item_records_when_all_exist() {
    let client = get_dynamodb_client().await;
    let shop_id = ShopId::new();
    let mk_expected = |n: i32| {
        let now = OffsetDateTime::now_utc();
        let now_str = now.format(&well_known::Rfc3339).unwrap();
        let shops_item_id: ShopsItemId = n.to_string().into();
        ItemRecord {
            pk: format!(
                "item#shop_id#{}#shops_item_id#{shops_item_id}",
                shop_id.clone()
            ),
            sk: "item#materialized".to_string(),
            gsi_1_pk: format!("shop_id#{shop_id}"),
            gsi_1_sk: format!("updated#{now_str}"),
            item_id: ItemId::new(),
            event_id: EventId::new(),
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            shop_name: "Foo".to_string(),
            title: Some(TextRecord::new("Bar", LanguageRecord::De)),
            title_de: Some("Bar".to_string()),
            title_en: Some("Barr".to_string()),
            description: Some(TextRecord::new("Baz", LanguageRecord::De)),
            description_de: Some("Baz".to_string()),
            description_en: Some("Bazz".to_string()),
            price: Some(PriceRecord {
                amount: 110,
                currency: CurrencyRecord::Eur,
            }),
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: ItemStateRecord::Available,
            url: format!("https:://foo.bar/{n}"),
            images: vec![format!("https:://foo.bar/{n}/image")],
            hash: ItemHash::new(&None, &ItemState::Available),
            created: now,
            updated: now,
        }
    };
    let mut expecteds = Vec::with_capacity(100);
    for n in 1..=100 {
        let expected = mk_expected(n);
        client
            .put_item()
            .table_name("items")
            .set_item(serde_dynamo::to_item(&expected).ok())
            .send()
            .await
            .unwrap();
        expecteds.push(expected.key());
    }

    let mut actuals = client
        .exist_item_records(
            &Batch::try_from(
                (1..=100)
                    .map(|n| ItemKey::new(shop_id.clone(), ShopsItemId::from(n.to_string())))
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    assert!(actuals.unprocessed.is_none());
    assert_eq!(actuals.items.len(), 100);

    expecteds.sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
    actuals.items.sort();
    assert_eq!(actuals.items, expecteds);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_item_keys_for_batch_exist_item_records_when_some_do_not_exist() {
    let client = get_dynamodb_client().await;
    let shop_id = ShopId::new();
    let mk_expected = |n: i32| {
        let now = OffsetDateTime::now_utc();
        let now_str = now.format(&well_known::Rfc3339).unwrap();
        let shops_item_id: ShopsItemId = n.to_string().into();
        ItemRecord {
            pk: format!(
                "item#shop_id#{}#shops_item_id#{shops_item_id}",
                shop_id.clone()
            ),
            sk: "item#materialized".to_string(),
            gsi_1_pk: format!("shop_id#{shop_id}"),
            gsi_1_sk: format!("updated#{now_str}"),
            item_id: ItemId::new(),
            event_id: EventId::new(),
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            shop_name: "Foo".to_string(),
            title: Some(TextRecord::new("Bar", LanguageRecord::De)),
            title_de: Some("Bar".to_string()),
            title_en: Some("Barr".to_string()),
            description: Some(TextRecord::new("Baz", LanguageRecord::De)),
            description_de: Some("Baz".to_string()),
            description_en: Some("Bazz".to_string()),
            price: Some(PriceRecord {
                amount: 110,
                currency: CurrencyRecord::Eur,
            }),
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: ItemStateRecord::Available,
            url: format!("https:://foo.bar/{n}"),
            images: vec![format!("https:://foo.bar/{n}/image")],
            hash: ItemHash::new(&None, &ItemState::Available),
            created: now,
            updated: now,
        }
    };
    let mut expecteds = Vec::with_capacity(100);
    for n in 1..=10 {
        let expected = mk_expected(n);
        client
            .put_item()
            .table_name("items")
            .set_item(serde_dynamo::to_item(&expected).ok())
            .send()
            .await
            .unwrap();
        expecteds.push(expected.key());
    }

    let mut actuals = client
        .exist_item_records(
            &Batch::try_from(
                (1..=14)
                    .map(|n| ItemKey::new(shop_id.clone(), ShopsItemId::from(n.to_string())))
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    assert!(actuals.unprocessed.is_none());
    assert_eq!(actuals.items.len(), 10);

    expecteds.sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
    actuals.items.sort();
    assert_eq!(actuals.items, expecteds);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_item_keys_for_batch_exist_item_records_when_more_than_100_exist() {
    let client = get_dynamodb_client().await;
    let shop_id = ShopId::new();
    let mk_expected = |n: i32| {
        let now = OffsetDateTime::now_utc();
        let now_str = now.format(&well_known::Rfc3339).unwrap();
        let shops_item_id: ShopsItemId = n.to_string().into();
        ItemRecord {
            pk: format!(
                "item#shop_id#{}#shops_item_id#{shops_item_id}",
                shop_id.clone()
            ),
            sk: "item#materialized".to_string(),
            gsi_1_pk: format!("shop_id#{shop_id}"),
            gsi_1_sk: format!("updated#{now_str}"),
            item_id: ItemId::new(),
            event_id: EventId::new(),
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            shop_name: "Foo".to_string(),
            title: Some(TextRecord::new("Bar", LanguageRecord::De)),
            title_de: Some("Bar".to_string()),
            title_en: Some("Barr".to_string()),
            description: Some(TextRecord::new("Baz", LanguageRecord::De)),
            description_de: Some("Baz".to_string()),
            description_en: Some("Bazz".to_string()),
            price: Some(PriceRecord {
                amount: 110,
                currency: CurrencyRecord::Eur,
            }),
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: ItemStateRecord::Available,
            url: format!("https:://foo.bar/{n}"),
            images: vec![format!("https:://foo.bar/{n}/image")],
            hash: ItemHash::new(&None, &ItemState::Available),
            created: now,
            updated: now,
        }
    };
    let mut expecteds = Vec::with_capacity(100);
    for n in 1..=120 {
        let expected = mk_expected(n);
        client
            .put_item()
            .table_name("items")
            .set_item(serde_dynamo::to_item(&expected).ok())
            .send()
            .await
            .unwrap();
        if n <= 100 {
            expecteds.push(expected.key());
        }
    }

    let mut actuals = client
        .exist_item_records(
            &Batch::try_from(
                (1..=100)
                    .map(|n| ItemKey::new(shop_id.clone(), ShopsItemId::from(n.to_string())))
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    assert!(actuals.unprocessed.is_none());
    assert_eq!(actuals.items.len(), 100);

    expecteds.sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
    actuals.items.sort();
    assert_eq!(actuals.items, expecteds);
}
