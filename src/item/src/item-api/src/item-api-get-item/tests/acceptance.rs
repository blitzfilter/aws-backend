use aws_lambda_events::{apigw::ApiGatewayV2httpRequest, query_map::QueryMap};
use common::{
    currency::data::CurrencyData,
    language::data::{LanguageData, LocalizedTextData},
};
use common::{
    currency::record::CurrencyRecord,
    event_id::EventId,
    item_id::ItemId,
    language::record::{LanguageRecord, TextRecord},
    price::record::PriceRecord,
};
use common::{price::data::PriceData, shop_id::ShopId, shops_item_id::ShopsItemId};
use http::{HeaderMap, HeaderValue, header::ACCEPT_LANGUAGE};
use item_api_get_item::handler;
use item_core::item::record::ItemRecord;
use item_core::item_state::data::ItemStateData;
use item_core::{
    item::hash::ItemHash,
    item_state::{domain::ItemState, record::ItemStateRecord},
};
use item_write::repository::PersistItemRepository;
use lambda_runtime::LambdaEvent;
use serde_json::{Value, json};
use std::collections::HashMap;
use test_api::*;
use time::{OffsetDateTime, format_description::well_known};
use url::Url;

#[rstest::rstest]
#[test_attr(apply(test))]
#[case("abcdefg", "123456")]
#[case("boop", "bap")]
#[case("foo", "bar")]
#[case(&ShopId::new().to_string(), &ShopsItemId::new().to_string())]
#[localstack_test(services = [DynamoDB()])]
async fn should_return_404_when_item_does_not_exist(
    #[case] shop_id: &str,
    #[case] shops_item_id: &str,
) {
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequest {
            resource: None,
            http_method: Default::default(),
            headers: Default::default(),
            query_string_parameters: Default::default(),
            path_parameters: HashMap::from_iter([
                ("shopId".to_string(), shop_id.to_string()),
                ("shopsItemId".to_string(), shops_item_id.to_string()),
            ]),
            stage_variables: Default::default(),
            request_context: Default::default(),
            body: None,
            is_base64_encoded: false,
            kind: None,
            method_arn: None,
            identity_source: None,
            authorization_token: None,
            version: None,
            route_key: None,
            raw_path: None,
            raw_query_string: None,
            cookies: None,
        },
        context: Default::default(),
    };
    let service = get_dynamodb_client().await;

    let actual = handler(lambda_event, service).await.unwrap();

    assert_eq!(404, actual.status_code);
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case("abcdefg", "123456")]
#[case(&ShopId::new().to_string(), &ShopsItemId::new().to_string())]
#[localstack_test(services = [DynamoDB()])]
async fn should_return_200_when_item_does_exist(
    #[case] shop_id: &str,
    #[case] shops_item_id: &str,
) {
    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&well_known::Rfc3339).unwrap();
    let shop_id: ShopId = shop_id.into();
    let shops_item_id: ShopsItemId = shops_item_id.into();
    let item_id = ItemId::new();
    let event_id = EventId::new();

    let item_record = ItemRecord {
        pk: format!(
            "item#shop_id#{}#shops_item_id#{shops_item_id}",
            shop_id.clone()
        ),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: format!("updated#{now_str}"),
        item_id,
        event_id,
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
            amount: 10000,
            currency: CurrencyRecord::Nzd,
        }),
        price_eur: Some(10000),
        price_usd: Some(10000),
        price_gbp: Some(10000),
        price_aud: Some(10000),
        price_cad: Some(10000),
        price_nzd: Some(10000),
        state: ItemStateRecord::Available,
        url: Url::parse("https://foo.bar/").unwrap(),
        images: vec![Url::parse("https://foo.bar/image").unwrap()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now,
        updated: now,
    };
    let client = get_dynamodb_client().await;
    client.put_item_records([item_record].into()).await.unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequest {
            resource: None,
            http_method: Default::default(),
            headers: HeaderMap::from_iter([(
                ACCEPT_LANGUAGE,
                HeaderValue::from_str("en").unwrap(),
            )]),
            query_string_parameters: Default::default(),
            path_parameters: HashMap::from_iter([
                ("shopId".to_string(), shop_id.to_string()),
                ("shopsItemId".to_string(), shops_item_id.to_string()),
            ]),
            stage_variables: Default::default(),
            request_context: Default::default(),
            body: None,
            is_base64_encoded: false,
            kind: None,
            method_arn: None,
            identity_source: None,
            authorization_token: None,
            version: None,
            route_key: None,
            raw_path: None,
            raw_query_string: None,
            cookies: None,
        },
        context: Default::default(),
    };
    let response = handler(lambda_event, client).await.unwrap();

    assert_eq!(200, response.status_code);

    match response.body.unwrap() {
        aws_lambda_events::encodings::Body::Text(body) => {
            let expected = json!({
                "itemId": item_id,
                "eventId": event_id,
                "shopId": shop_id,
                "shopsItemId": shops_item_id,
                "shopName": "Foo",
                "title": LocalizedTextData::new("Barr", LanguageData::En),
                "description": Some(LocalizedTextData::new("Bazz", LanguageData::En)),
                "price": PriceData { currency: CurrencyData::Eur, amount: 10000 },
                "state": ItemStateData::Available,
                "url": "https://foo.bar/",
                "images": vec!["https://foo.bar/image"],
                "created": now_str,
                "updated": now_str,
            });
            let actual: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(expected, actual);
        }
        _ => panic!("Expected Body::Text"),
    }
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case("de", LanguageData::De)]
#[case("de-DE", LanguageData::De)]
#[case("en", LanguageData::En)]
#[case("en-US", LanguageData::En)]
#[case("en-GB", LanguageData::En)]
#[case("es", LanguageData::Es)]
#[case("es-ES", LanguageData::Es)]
#[case("de;q=0.9,en;q=0.8", LanguageData::De)]
#[case("en-GB,en;q=0.7,de;q=0.6", LanguageData::En)]
#[case("es-ES;q=0.9,en;q=0.8,de;q=0.7", LanguageData::Es)]
#[case("en,fr;q=0.5,de;q=0.3,es;q=0.2", LanguageData::En)]
#[case("it", LanguageData::De)] // fallback to German
#[case("pt-BR", LanguageData::De)]
#[case("ru", LanguageData::De)]
#[case("ja", LanguageData::De)]
#[case("zh-CN", LanguageData::De)]
#[case("ko-KR", LanguageData::De)]
#[case("*", LanguageData::De)] // wildcard fallback
#[case("fr-FR; q=0", LanguageData::De)] // not acceptable
#[case("", LanguageData::De)] // empty string
#[case("null", LanguageData::De)] // literal "null"
#[case("undefined", LanguageData::De)] // literal "undefined"
#[case("\"en-US\"", LanguageData::De)] // quotes
#[case("123", LanguageData::De)] // numeric
#[case("abcdefg", LanguageData::De)] // unrecognized
#[localstack_test(services = [DynamoDB()])]
async fn should_respect_accept_language_header(
    #[case] accept_language_header_value: &str,
    #[case] expected_language: LanguageData,
) {
    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&well_known::Rfc3339).unwrap();
    let shop_id: ShopId = ShopId::new();
    let shops_item_id = ShopsItemId::new();
    let item_id = ItemId::new();
    let event_id = EventId::new();

    let item_record = ItemRecord {
        pk: format!(
            "item#shop_id#{}#shops_item_id#{shops_item_id}",
            shop_id.clone()
        ),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: format!("updated#{now_str}"),
        item_id,
        event_id,
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: "Foo".to_string(),
        title_native: TextRecord::new("Bar", LanguageRecord::Es),
        title_de: Some("Bar".to_string()),
        title_en: Some("Bar".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::Es)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Baz".to_string()),
        price_native: Some(PriceRecord {
            amount: 10000,
            currency: CurrencyRecord::Nzd,
        }),
        price_eur: Some(10000),
        price_usd: Some(10000),
        price_gbp: Some(10000),
        price_aud: Some(10000),
        price_cad: Some(10000),
        price_nzd: Some(10000),
        state: ItemStateRecord::Available,
        url: Url::parse("https://foo.bar/").unwrap(),
        images: vec![Url::parse("https://foo.bar/image").unwrap()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now,
        updated: now,
    };
    let client = get_dynamodb_client().await;
    client.put_item_records([item_record].into()).await.unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequest {
            resource: None,
            http_method: Default::default(),
            headers: HeaderMap::from_iter([(
                ACCEPT_LANGUAGE,
                HeaderValue::from_str(accept_language_header_value).unwrap(),
            )]),
            query_string_parameters: Default::default(),
            path_parameters: HashMap::from_iter([
                ("shopId".to_string(), shop_id.to_string()),
                ("shopsItemId".to_string(), shops_item_id.to_string()),
            ]),
            stage_variables: Default::default(),
            request_context: Default::default(),
            body: None,
            is_base64_encoded: false,
            kind: None,
            method_arn: None,
            identity_source: None,
            authorization_token: None,
            version: None,
            route_key: None,
            raw_path: None,
            raw_query_string: None,
            cookies: None,
        },
        context: Default::default(),
    };
    let response = handler(lambda_event, client).await.unwrap();

    assert_eq!(200, response.status_code);

    match response.body.unwrap() {
        aws_lambda_events::encodings::Body::Text(body) => {
            let expected = json!({
                "itemId": item_id,
                "eventId": event_id,
                "shopId": shop_id,
                "shopsItemId": shops_item_id,
                "shopName": "Foo",
                "title": LocalizedTextData::new("Bar", expected_language),
                "description": Some(LocalizedTextData::new("Baz", expected_language)),
                "price": PriceData { currency: CurrencyData::Eur, amount: 10000 },
                "state": ItemStateData::Available,
                "url": "https://foo.bar/",
                "images": vec!["https://foo.bar/image"],
                "created": now_str,
                "updated": now_str,
            });
            let actual: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(expected, actual);
        }
        _ => panic!("Expected Body::Text"),
    }
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::eur("EUR", CurrencyData::Eur)]
#[case::gbp("GBP", CurrencyData::Gbp)]
#[case::usd("USD", CurrencyData::Usd)]
#[case::aud("AUD", CurrencyData::Aud)]
#[case::cad("CAD", CurrencyData::Cad)]
#[case::nzd("NZD", CurrencyData::Nzd)]
#[localstack_test(services = [DynamoDB()])]
async fn should_respect_currency_query_param(
    #[case] query_value: &str,
    #[case] expected_currency: CurrencyData,
) {
    let now = OffsetDateTime::now_utc();
    let now_str = now.format(&well_known::Rfc3339).unwrap();
    let shop_id: ShopId = ShopId::new();
    let shops_item_id = ShopsItemId::new();
    let item_id = ItemId::new();
    let event_id = EventId::new();

    let item_record = ItemRecord {
        pk: format!(
            "item#shop_id#{}#shops_item_id#{shops_item_id}",
            shop_id.clone()
        ),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: format!("updated#{now_str}"),
        item_id,
        event_id,
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: "Foo".to_string(),
        title_native: TextRecord::new("Bar", LanguageRecord::Es),
        title_de: Some("Bar".to_string()),
        title_en: Some("Bar".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::Es)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Baz".to_string()),
        price_native: Some(PriceRecord {
            amount: 10000,
            currency: CurrencyRecord::Nzd,
        }),
        price_eur: Some(10000),
        price_usd: Some(10000),
        price_gbp: Some(10000),
        price_aud: Some(10000),
        price_cad: Some(10000),
        price_nzd: Some(10000),
        state: ItemStateRecord::Available,
        url: Url::parse("https://foo.bar/").unwrap(),
        images: vec![Url::parse("https://foo.bar/image").unwrap()],
        hash: ItemHash::new(&None, &ItemState::Available),
        created: now,
        updated: now,
    };
    let client = get_dynamodb_client().await;
    client.put_item_records([item_record].into()).await.unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequest {
            resource: None,
            http_method: Default::default(),
            headers: Default::default(),
            query_string_parameters: QueryMap::from(HashMap::from_iter([(
                "currency".to_string(),
                query_value.to_string(),
            )])),
            path_parameters: HashMap::from_iter([
                ("shopId".to_string(), shop_id.to_string()),
                ("shopsItemId".to_string(), shops_item_id.to_string()),
            ]),
            stage_variables: Default::default(),
            request_context: Default::default(),
            body: None,
            is_base64_encoded: false,
            kind: None,
            method_arn: None,
            identity_source: None,
            authorization_token: None,
            version: None,
            route_key: None,
            raw_path: None,
            raw_query_string: None,
            cookies: None,
        },
        context: Default::default(),
    };
    let response = handler(lambda_event, client).await.unwrap();

    assert_eq!(200, response.status_code);

    match response.body.unwrap() {
        aws_lambda_events::encodings::Body::Text(body) => {
            let expected = json!({
                "itemId": item_id,
                "eventId": event_id,
                "shopId": shop_id,
                "shopsItemId": shops_item_id,
                "shopName": "Foo",
                "title": LocalizedTextData::new("Bar", LanguageData::De),
                "description": Some(LocalizedTextData::new("Baz", LanguageData::De)),
                "price": PriceData { currency: expected_currency, amount: 10000 },
                "state": ItemStateData::Available,
                "url": "https://foo.bar/",
                "images": vec!["https://foo.bar/image"],
                "created": now_str,
                "updated": now_str,
            });
            let actual: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(expected, actual);
        }
        _ => panic!("Expected Body::Text"),
    }
}
