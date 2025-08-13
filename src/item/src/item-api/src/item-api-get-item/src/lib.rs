use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use aws_lambda_events::http::HeaderValue;
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{
    BAD_HEADER_VALUE, BAD_PARAMETER, BAD_QUERY_PARAMETER_VALUE, INTERNAL_SERVER_ERROR,
};
use common::currency::data::CurrencyData;
use common::currency::domain::Currency;
use common::language::data::LanguageData;
use common::language::domain::Language;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use http::header::ACCEPT_LANGUAGE;
use item_core::item::get_data::GetItemData;
use item_service::query_service::QueryItemService;
use lambda_runtime::LambdaEvent;
use tracing::error;

#[tracing::instrument(skip(event, service), fields(requestId = %event.context.request_id))]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl QueryItemService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayV2httpResponse::from(err)),
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl QueryItemService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let languages = event
        .payload
        .headers
        .get(ACCEPT_LANGUAGE)
        .map(HeaderValue::to_str)
        .map(|header_value_res| {
            header_value_res.map_err(|_| {
                ApiError::bad_request(BAD_HEADER_VALUE).with_header_field(ACCEPT_LANGUAGE.as_str())
            })
        })
        .transpose()?
        .map(accept_language::parse)
        .unwrap_or_default()
        .into_iter()
        .map(|accept_language| {
            serde_json::from_str::<LanguageData>(&format!(r#""{accept_language}""#))
        })
        .filter_map(Result::ok)
        .map(Language::from)
        .collect::<Vec<_>>();
    let currency = event
        .payload
        .query_string_parameters
        .first("currency")
        .filter(|str| !str.is_empty())
        .map(|currency| serde_json::from_str::<CurrencyData>(&format!(r#""{currency}""#)))
        .map(|currency_res| {
            currency_res.map_err(|err| {
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_query_field("currency")
                    .with_message(err.to_string())
            })
        })
        .transpose()?
        .map(Currency::from)
        .unwrap_or_default();
    let shop_id = event
        .payload
        .path_parameters
        .get("shopId")
        .filter(|str| !str.is_empty())
        .map(ShopId::from)
        .ok_or(ApiError::bad_request(BAD_PARAMETER).with_path_field("shopId"))?;
    let shops_item_id = event
        .payload
        .path_parameters
        .get("shopsItemId")
        .filter(|str| !str.is_empty())
        .map(ShopsItemId::from)
        .ok_or(ApiError::bad_request(BAD_PARAMETER).with_path_field("shopsItemId"))?;

    let item_data: GetItemData = service
        .view_item(&shop_id, &shops_item_id, languages.as_slice(), &currency)
        .await?
        .into();
    let response = serde_json::to_string(&item_data).map_err(|err| {
        error!(error = %err, payload = ?item_data, type = %std::any::type_name::<GetItemData>(), "Failed serializing GetItemData.");
        ApiError::internal_server_error(INTERNAL_SERVER_ERROR)
    })?;

    let content_language = item_data.title.language;

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body(response)
        .content_language(content_language)
        .e_tag(item_data.event_id.to_string().as_str())
        .last_modified(item_data.updated)
        .cors()
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use aws_lambda_events::encodings::Body::Text;
    use common::currency::data::CurrencyData;
    use common::event_id::EventId;
    use common::language::data::LanguageData;
    use common::language::domain::Language;
    use common::localized::Localized;
    use common::price::domain::Price;
    use common::shop_id::ShopId;
    use common::shops_item_id::ShopsItemId;
    use http::header::{ACCEPT_LANGUAGE, CONTENT_LANGUAGE, ETAG, LAST_MODIFIED};
    use item_core::item::domain::LocalizedItemView;
    use item_core::item::hash::ItemHash;
    use item_core::item_state::domain::ItemState;
    use item_service::query_service::{GetItemError, MockQueryItemService};
    use lambda_runtime::LambdaEvent;
    use test_api::{ApiGatewayV2httpRequestProxy, extract_apigw_response_json_body};
    use time::OffsetDateTime;
    use time::macros::datetime;
    use url::Url;

    #[tokio::test]
    #[rstest::rstest]
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
    #[case("pt-BR", LanguageData::De)]
    #[case("ru", LanguageData::De)]
    #[case("ja", LanguageData::De)]
    #[case("zh-CN", LanguageData::De)]
    #[case("ko-KR", LanguageData::De)]
    #[case("*", LanguageData::De)]
    #[case("fr-FR; q=0", LanguageData::De)] // not acceptable
    #[case("", LanguageData::De)] // empty string
    #[case("null", LanguageData::De)] // literal "null"
    #[case("undefined", LanguageData::De)] // literal "undefined"
    #[case("\"en-US\"", LanguageData::De)] // quotes
    #[case("123", LanguageData::De)] // numeric
    #[case("abcdefg", LanguageData::De)] // unrecognized
    async fn should_respect_accept_language_header(
        #[case] accept_language_header_value: &str,
        #[case] expected_language: LanguageData,
    ) {
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsItemId", shops_item_id)
                .header(ACCEPT_LANGUAGE.as_str(), accept_language_header_value)
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryItemService::default();
        service
            .expect_view_item()
            .return_once(move |shop_id, shops_item_id, _, _| {
                let item = LocalizedItemView {
                    item_id: Default::default(),
                    event_id: EventId::new(),
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".into(),
                    title: Localized::new(expected_language.into(), "Native title".into()),
                    description: None,
                    price: None,
                    state: ItemState::Listed,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    images: vec![],
                    hash: ItemHash::new(&None, &ItemState::Listed),
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                };
                Box::pin(async move { Ok(item) })
            });

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(
            expected_language,
            serde_json::from_value::<LanguageData>(json["title"]["language"].clone()).unwrap()
        );
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(LanguageData::De, "de")]
    #[case(LanguageData::En, "en")]
    #[case(LanguageData::Es, "es")]
    #[case(LanguageData::Fr, "fr")]
    async fn should_include_actual_language_as_header_content_language(
        #[case] language: LanguageData,
        #[case] expected_content_language: &str,
    ) {
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsItemId", shops_item_id)
                .header(ACCEPT_LANGUAGE.as_str(), expected_content_language)
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryItemService::default();
        service
            .expect_view_item()
            .return_once(move |shop_id, shops_item_id, _, _| {
                let item = LocalizedItemView {
                    item_id: Default::default(),
                    event_id: EventId::new(),
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".into(),
                    title: Localized::new(language.into(), "Native title".into()),
                    description: None,
                    price: None,
                    state: ItemState::Listed,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    images: vec![],
                    hash: ItemHash::new(&None, &ItemState::Listed),
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                };
                Box::pin(async move { Ok(item) })
            });

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        assert_eq!(
            expected_content_language,
            response
                .headers
                .get(CONTENT_LANGUAGE)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case::eur("EUR", CurrencyData::Eur)]
    #[case::gbp("GBP", CurrencyData::Gbp)]
    #[case::usd("USD", CurrencyData::Usd)]
    #[case::aud("AUD", CurrencyData::Aud)]
    #[case::cad("CAD", CurrencyData::Cad)]
    #[case::nzd("NZD", CurrencyData::Nzd)]
    async fn should_respect_currency_query_param(
        #[case] query_value: &str,
        #[case] expected_currency: CurrencyData,
    ) {
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsItemId", shops_item_id)
                .query_string_parameter("currency", query_value)
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryItemService::default();
        service
            .expect_view_item()
            .return_once(move |shop_id, shops_item_id, _, _| {
                let item = LocalizedItemView {
                    item_id: Default::default(),
                    event_id: EventId::new(),
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".into(),
                    title: Localized::new(Language::Es, "Native title".into()),
                    description: None,
                    price: Some(Price::new(50000u64.into(), expected_currency.into())),
                    state: ItemState::Listed,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    images: vec![],
                    hash: ItemHash::new(&None, &ItemState::Listed),
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                };
                Box::pin(async move { Ok(item) })
            });

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(query_value, json["price"]["currency"]);
    }

    #[tokio::test]
    async fn should_include_event_id_as_header_e_tag() {
        let event_id = EventId::new();
        let mut service = MockQueryItemService::default();
        service
            .expect_view_item()
            .return_once(move |shop_id, shops_item_id, _, _| {
                let item = LocalizedItemView {
                    item_id: Default::default(),
                    event_id,
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".into(),
                    title: Localized::new(Language::Es, "Native title".into()),
                    description: None,
                    price: None,
                    state: ItemState::Listed,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    images: vec![],
                    hash: ItemHash::new(&None, &ItemState::Listed),
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                };
                Box::pin(async move { Ok(item) })
            });
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsItemId", shops_item_id)
                .build(),
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        assert_eq!(
            event_id.to_string().as_str(),
            response.headers.get(ETAG).unwrap()
        );
    }

    #[tokio::test]
    async fn should_include_updated_timestamp_as_header_last_modified() {
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let event_id = EventId::new();
        let mut service = MockQueryItemService::default();
        service
            .expect_view_item()
            .return_once(move |shop_id, shops_item_id, _, _| {
                let item = LocalizedItemView {
                    item_id: Default::default(),
                    event_id,
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".into(),
                    title: Localized::new(Language::Es, "Native title".into()),
                    description: None,
                    price: None,
                    state: ItemState::Listed,
                    url: Url::parse("https://foo.com/boop").unwrap(),
                    images: vec![],
                    hash: ItemHash::new(&None, &ItemState::Listed),
                    created: timestamp,
                    updated: timestamp,
                };
                Box::pin(async move { Ok(item) })
            });
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsItemId", shops_item_id)
                .build(),
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        assert_eq!(
            "Wed, 01 Jan 2020 00:00:00 GMT",
            response.headers.get(LAST_MODIFIED).unwrap()
        );
    }

    #[tokio::test]
    async fn should_400_when_currency_query_param_is_invalid() {
        let mut service = MockQueryItemService::default();
        service.expect_view_item().never();

        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsItemId", shops_item_id)
                .query_string_parameter("currency", "invalid_currency")
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(400, json["status"]);
        assert_eq!("currency", json["source"]["field"]);
    }

    #[tokio::test]
    async fn should_400_when_path_param_shop_id_is_missing() {
        let mut service = MockQueryItemService::default();
        service.expect_view_item().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopsItemId", ShopsItemId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(400, json["status"]);
        assert_eq!("shopId", json["source"]["field"]);
    }

    #[tokio::test]
    async fn should_400_when_path_param_shops_item_id_is_missing() {
        let mut service = MockQueryItemService::default();
        service.expect_view_item().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", ShopId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(400, json["status"]);
        assert_eq!("shopsItemId", json["source"]["field"]);
    }

    #[tokio::test]
    async fn should_404_when_item_does_not_exist() {
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsItemId", shops_item_id)
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryItemService::default();
        service
            .expect_view_item()
            .return_once(move |shop_id, shops_item_id, _, _| {
                let shop_id = shop_id.clone();
                let shops_item_id = shops_item_id.clone();
                Box::pin(async move { Err(GetItemError::ItemNotFound(shop_id, shops_item_id)) })
            });

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(404, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(404, json["status"]);
    }
}
