use common::currency::record::CurrencyRecord;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::language::record::{LanguageRecord, TextRecord};
use common::price::record::PriceRecord;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::domain::Item;
use item_core::item::hash::ItemHash;
use item_core::item::record::ItemRecord;
use item_core::item_state::domain::ItemState;
use item_core::item_state::record::ItemStateRecord;
use item_read::service::{GetItemError, QueryItemService};
use test_api::*;
use time::OffsetDateTime;
use time::format_description::well_known;
use url::Url;

#[localstack_test(services = [DynamoDB()])]
async fn should_return_item_not_found_err_for_get_item_with_currency_when_table_is_empty() {
    let shop_id = ShopId::new();
    let shops_item_id = "non-existent".into();
    let client = get_dynamodb_client().await;
    let actual = client.find_item(&shop_id, &shops_item_id).await;

    assert!(actual.is_err());
    match actual.unwrap_err() {
        GetItemError::ItemNotFound(err_shop_id, err_shops_item_id) => {
            assert_eq!(err_shop_id, shop_id);
            assert_eq!(err_shops_item_id, shops_item_id);
        }
        _ => panic!("expected GetItemError::ItemNotFound"),
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_item_record_for_get_item_with_currency_when_exists() {
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
        title_native: TextRecord::new("Bar", LanguageRecord::De),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        price_native: Some(PriceRecord {
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
        url: Url::parse("https://foo.bar/123456").unwrap(),
        images: vec![Url::parse("https://foo.bar/123456/image").unwrap()],
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
    let actual = client.find_item(&shop_id, &shops_item_id).await.unwrap();

    assert_eq!(expected, actual);
}
