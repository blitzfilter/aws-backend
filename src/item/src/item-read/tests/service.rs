use common::currency::domain::Currency;
use common::currency::record::CurrencyRecord;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::language::domain::Language;
use common::language::record::{LanguageRecord, TextRecord};
use common::localized::Localized;
use common::price::domain::MonetaryAmount;
use common::price::record::PriceRecord;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::domain::Item;
use item_core::item::domain::description::Description;
use item_core::item::domain::title::Title;
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
async fn should_return_item_not_found_err_for_find_item_when_table_is_empty() {
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
async fn should_return_item_for_find_item_when_exists() {
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

#[localstack_test(services = [DynamoDB()])]
async fn should_return_item_not_found_err_for_view_item_when_table_is_empty() {
    let shop_id = ShopId::new();
    let shops_item_id = "non-existent".into();
    let client = get_dynamodb_client().await;
    let actual = client
        .view_item(&shop_id, &shops_item_id, &[Language::De], &Currency::Eur)
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

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(
    &[Language::De],
    Localized { localization: Language::De, payload: "German title".into() },
    Some(Localized { localization: Language::De, payload: "German description".into() }),
)]
#[case(
    &[Language::En],
    Localized { localization: Language::En, payload: "English title".into() },
    Some(Localized { localization: Language::En, payload: "English description".into() }),
)]
#[case(
    &[Language::Es],
    Localized { localization: Language::Es, payload: "Spanish title".into() },
    Some(Localized { localization: Language::Es, payload: "Spanish description".into() }),
)]
#[localstack_test(services = [DynamoDB()])]
async fn should_return_localized_item_view_for_view_item_respecting_preferred_languages(
    #[case] languages: &[Language],
    #[case] expected_title: Localized<Language, Title>,
    #[case] expected_description: Option<Localized<Language, Description>>,
) {
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
        title_native: TextRecord::new("Spanish title", LanguageRecord::Es),
        title_de: Some("German title".to_string()),
        title_en: Some("English title".to_string()),
        description_native: Some(TextRecord::new("Spanish description", LanguageRecord::Es)),
        description_de: Some("German description".to_string()),
        description_en: Some("English description".to_string()),
        price_native: None,
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

    let actual = client
        .view_item(&shop_id, &shops_item_id, languages, &Currency::Eur)
        .await
        .unwrap();

    assert_eq!(expected_title, actual.title);
    assert_eq!(expected_description, actual.description);
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(Currency::Eur, 10000u64.into())]
#[case(Currency::Usd, 11000u64.into())]
#[case(Currency::Gbp, 12000u64.into())]
#[case(Currency::Aud, 13000u64.into())]
#[case(Currency::Cad, 14000u64.into())]
#[case(Currency::Nzd, 15000u64.into())]
#[localstack_test(services = [DynamoDB()])]
async fn should_return_localized_item_view_for_view_item_respecting_currency(
    #[case] expected_currency: Currency,
    #[case] expected_monetary_amount: MonetaryAmount,
) {
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
        title_native: TextRecord::new("Spanish title", LanguageRecord::Es),
        title_de: Some("German title".to_string()),
        title_en: Some("English title".to_string()),
        description_native: Some(TextRecord::new("Spanish description", LanguageRecord::Es)),
        description_de: Some("German description".to_string()),
        description_en: Some("English description".to_string()),
        price_native: Some(PriceRecord {
            currency: CurrencyRecord::Eur,
            amount: 10000u64,
        }),
        price_eur: 10000u64.into(),
        price_usd: 11000u64.into(),
        price_gbp: 12000u64.into(),
        price_aud: 13000u64.into(),
        price_cad: 14000u64.into(),
        price_nzd: 15000u64.into(),
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

    let actual = client
        .view_item(&shop_id, &shops_item_id, &[], &expected_currency)
        .await
        .unwrap();

    assert_eq!(expected_currency, actual.price.unwrap().currency);
    assert_eq!(
        expected_monetary_amount,
        actual.price.unwrap().monetary_amount
    );
}
