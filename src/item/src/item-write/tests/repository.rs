use common::currency::record::CurrencyRecord;
use common::dynamodb_batch::DynamoDbBatch;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::language::record::{LanguageRecord, TextRecord};
use common::price::record::PriceRecord;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::hash::ItemHash;
use item_core::item::record::ItemRecord;
use item_core::item::update_record::ItemUpdateRecord;
use item_core::item_event::record::ItemEventRecord;
use item_core::item_event_type::record::ItemEventTypeRecord;
use item_core::item_state::domain::ItemState;
use item_core::item_state::record::ItemStateRecord;
use item_read::repository::ReadItemRecords;
use item_write::repository::WriteItemRecords;
use test_api::*;
use time::OffsetDateTime;
use time::format_description::well_known;

#[localstack_test(services = [DynamoDB])]
async fn should_put_item_records_for_single_record() {
    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&well_known::Rfc3339).unwrap();
    let shop_id = ShopId::new();
    let shops_item_id: ShopsItemId = "123465".into();
    let expected = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{}", shop_id.clone()),
        gsi_1_sk: now_str.clone(),
        item_id: ItemId::now(),
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
            amount: 110.5,
            currency: CurrencyRecord::Eur,
        }),
        state: ItemStateRecord::Available,
        url: "https:://foo.bar/123456".to_string(),
        images: vec!["https:://foo.bar/123456/image".to_string()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now,
        updated: now,
    };

    let client = get_dynamodb_client().await;
    client
        .put_item_records(DynamoDbBatch::singleton(expected.clone()))
        .await
        .unwrap();

    let actual = client
        .get_item_record(&shop_id, &shops_item_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB])]
async fn should_put_item_records_for_multiple_records() {
    let now1 = OffsetDateTime::now_utc();
    let now1_str = now1.format(&well_known::Rfc3339).unwrap();
    let shop_id = ShopId::new();
    let shops_item_id_1: ShopsItemId = "123465".into();
    let expected1 = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id_1}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{}", shop_id.clone()),
        gsi_1_sk: format!("updated#{now1_str}"),
        item_id: ItemId::now(),
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
            amount: 110.5,
            currency: CurrencyRecord::Eur,
        }),
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
    let expected2 = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id_2}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{}", shop_id.clone()),
        gsi_1_sk: format!("updated#{now2_str}"),
        item_id: ItemId::now(),
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
            amount: 110.5,
            currency: CurrencyRecord::Eur,
        }),
        state: ItemStateRecord::Available,
        url: "https:://foo.bar/123456".to_string(),
        images: vec!["https:://foo.bar/123456/image".to_string()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now2,
        updated: now2,
    };

    let client = get_dynamodb_client().await;

    client
        .put_item_records([expected1.clone(), expected2.clone()].into())
        .await
        .unwrap();

    let actual1 = client
        .get_item_record(&shop_id, &shops_item_id_1)
        .await
        .unwrap()
        .unwrap();
    let actual2 = client
        .get_item_record(&shop_id, &shops_item_id_2)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected1, actual1);
    assert_eq!(expected2, actual2);
}

#[localstack_test(services = [DynamoDB])]
async fn should_put_item_event_records_for_single_record() {
    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&well_known::Rfc3339).unwrap();
    let shop_id = ShopId::new();
    let shops_item_id: ShopsItemId = "123465".into();
    let expected = ItemEventRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
        sk: format!("item#event#{now_str}"),
        item_id: ItemId::now(),
        event_id: EventId::new(),
        event_type: ItemEventTypeRecord::Created,
        shop_id,
        shops_item_id: shops_item_id.clone(),
        shop_name: Some("Foo".to_string()),
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: Some(PriceRecord {
            amount: 110.5,
            currency: CurrencyRecord::Eur,
        }),
        state: Some(ItemStateRecord::Available),
        url: Some("https:://foo.bar/123456".to_string()),
        images: Some(vec!["https:://foo.bar/123456/image".to_string()]),
        timestamp: now,
    };

    let client = get_dynamodb_client().await;
    client
        .put_item_event_records(DynamoDbBatch::singleton(expected.clone()))
        .await
        .unwrap();

    let actual = client
        .scan()
        .table_name("items")
        .send()
        .await
        .unwrap()
        .items
        .unwrap()
        .into_iter()
        .map(serde_dynamo::from_item)
        .collect::<Result<Vec<ItemEventRecord>, _>>()
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [DynamoDB])]
async fn should_put_item_event_records_for_multiple_records() {
    let shop_id = ShopId::new();
    let now1 = OffsetDateTime::now_utc();
    let now_str1 = now1.format(&well_known::Rfc3339).unwrap();
    let shops_item_id1: ShopsItemId = "123465".into();
    let price = PriceRecord {
        amount: 110.5,
        currency: CurrencyRecord::Eur,
    };
    let expected1 = ItemEventRecord {
        pk: format!(
            "item#shop_id#{}#shops_item_id#{shops_item_id1}",
            shop_id.clone()
        ),
        sk: format!("item#event#{now_str1}"),
        item_id: ItemId::now(),
        event_id: EventId::new(),
        event_type: ItemEventTypeRecord::Created,
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id1.clone(),
        shop_name: Some("Foo".to_string()),
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: Some(price),
        state: Some(ItemStateRecord::Available),
        url: Some("https:://foo.bar/123456".to_string()),
        images: Some(vec!["https:://foo.bar/123456/image".to_string()]),
        timestamp: now1,
    };

    let now2 = OffsetDateTime::now_utc();
    let now_str2 = now2.format(&well_known::Rfc3339).unwrap();
    let shops_item_id2: ShopsItemId = "123465".into();
    let expected2 = ItemEventRecord {
        pk: format!(
            "item#shop_id#{}#shops_item_id#{shops_item_id2}",
            shop_id.clone()
        ),
        sk: format!("item#event#{now_str2}"),
        item_id: ItemId::now(),
        event_id: EventId::new(),
        event_type: ItemEventTypeRecord::Created,
        shop_id,
        shops_item_id: shops_item_id2.clone(),
        shop_name: Some("Foo".to_string()),
        title: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price: Some(PriceRecord {
            amount: 110.5,
            currency: CurrencyRecord::Eur,
        }),
        state: Some(ItemStateRecord::Available),
        url: Some("https:://foo.bar/123456".to_string()),
        images: Some(vec!["https:://foo.bar/123456/image".to_string()]),
        timestamp: now2,
    };

    let client = get_dynamodb_client().await;
    client
        .put_item_event_records(DynamoDbBatch::from([expected1.clone(), expected2.clone()]))
        .await
        .unwrap();

    let actual = client
        .scan()
        .table_name("items")
        .send()
        .await
        .unwrap()
        .items
        .unwrap()
        .into_iter()
        .map(serde_dynamo::from_item)
        .collect::<Result<Vec<ItemEventRecord>, _>>()
        .unwrap();

    assert_eq!(vec![expected1, expected2], actual);
}

#[localstack_test(services = [DynamoDB])]
async fn should_update_item_record() {
    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&well_known::Rfc3339).unwrap();
    let shop_id = ShopId::new();
    let shops_item_id: ShopsItemId = "123465".into();
    let price = PriceRecord {
        amount: 110.5,
        currency: CurrencyRecord::Eur,
    };
    let initial = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{}", shop_id.clone()),
        gsi_1_sk: format!("updated#{now_str}"),
        item_id: ItemId::now(),
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
        price: Some(price),
        state: ItemStateRecord::Available,
        url: "https:://foo.bar/123456".to_string(),
        images: vec!["https:://foo.bar/123456/image".to_string()],
        hash: ItemHash::new(&Some(price.into()), &ItemState::Available),
        created: now,
        updated: now,
    };
    let now2 = OffsetDateTime::now_utc();
    let event_id2 = EventId::new();
    let update = ItemUpdateRecord {
        event_id: Some(event_id2),
        shop_name: None,
        title: None,
        title_de: None,
        title_en: None,
        description: None,
        description_de: None,
        description_en: None,
        price: None,
        state: Some(ItemStateRecord::Sold),
        url: None,
        images: None,
        hash: None,
        updated: now2,
    };
    let mut expected = initial.clone();
    expected.event_id = event_id2;
    expected.state = ItemStateRecord::Sold;
    expected.updated = now2;

    let client = get_dynamodb_client().await;
    client
        .put_item_records(DynamoDbBatch::singleton(initial.clone()))
        .await
        .unwrap();
    client
        .update_item_record(&shop_id, &shops_item_id, update)
        .await
        .unwrap();

    let actual = client
        .get_item_record(&shop_id, &shops_item_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}
