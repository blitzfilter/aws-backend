use common::currency::domain::Currency;
use common::currency::record::CurrencyRecord;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::domain::Item;
use item_core::item::hash::ItemHash;
use item_core::item::record::ItemRecord;
use item_core::item_state::domain::ItemState;
use item_core::item_state::record::ItemStateRecord;
use item_read::service::{GetItemError, ReadItem};
use test_api::*;
use time::OffsetDateTime;
use time::format_description::well_known;

#[localstack_test(services = [DynamoDB])]
async fn should_return_item_not_found_err_for_get_item_with_currency_when_table_is_empty() {
    let shop_id = ShopId::new();
    let shops_item_id = "non-existent".into();
    let client = get_dynamodb_client().await;
    let actual = client
        .get_item_with_currency(&shop_id, &shops_item_id, Currency::Eur)
        .await;

    assert!(actual.is_err());
    match actual.unwrap_err() {
        GetItemError::ItemNotFound(err_shop_id, err_shops_item_id) => {
            assert_eq!(err_shop_id, shop_id);
            assert_eq!(err_shops_item_id, shops_item_id);
        }
        _ => panic!("expected GetItemError::ItemNotFound"),
    }
}

#[localstack_test(services = [DynamoDB])]
async fn should_return_item_record_for_get_item_with_currency_when_exists() {
    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&well_known::Rfc3339).unwrap();
    let shop_id = ShopId::new();
    let shops_item_id: ShopsItemId = "123465".into();
    let inserted = ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
        sk: "item#materialized".to_string(),
        gsi_1_pk: shop_id.clone().into(),
        gsi_1_sk: now_str.clone(),
        item_id: ItemId::now(),
        event_id: EventId::new(),
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: "Foo".to_string(),
        title: Some("Bar".to_string()),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description: Some("Baz".to_string()),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price_currency: Some(CurrencyRecord::Eur),
        price_amount: Some(110.5),
        price_eur: Some(110.5),
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

    let expected: Item = inserted.into();
    let actual = client
        .get_item_with_currency(&shop_id, &shops_item_id, Currency::Eur)
        .await
        .unwrap();

    assert_eq!(expected, actual);
}
