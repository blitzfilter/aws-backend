use aws_lambda_events::apigw::{ApiGatewayProxyRequest, ApiGatewayProxyResponse};
use aws_lambda_events::http::HeaderValue;
use common::api::api_gateway_proxy_response_builder::ApiGatewayProxyResponseBuilder;
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
use item_read::service::ReadItem;
use lambda_runtime::LambdaEvent;
use tracing::error;

#[tracing::instrument(skip(event, service), fields(requestId = %event.context.request_id))]
pub async fn handler(
    event: LambdaEvent<ApiGatewayProxyRequest>,
    service: &impl ReadItem,
) -> Result<ApiGatewayProxyResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayProxyResponse::from(err)),
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayProxyRequest>,
    service: &impl ReadItem,
) -> Result<ApiGatewayProxyResponse, ApiError> {
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
        .map(serde_json::from_str::<CurrencyData>)
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
        .map(ShopId::from)
        .ok_or(ApiError::bad_request(BAD_PARAMETER).with_path_field("shopId"))?;
    let shops_item_id = event
        .payload
        .path_parameters
        .get("shopsItemId")
        .map(ShopsItemId::from)
        .ok_or(ApiError::bad_request(BAD_PARAMETER).with_path_field("shopsItemId"))?;

    let item = service
        .get_item_with_currency(&shop_id, &shops_item_id, currency)
        .await?;
    let item_data = GetItemData::from_domain_localized(item, languages);
    let response = serde_json::to_string(&item_data).map_err(|err| {
        error!(error = %err, payload = ?item_data, "Failed serializing GetItemData.");
        ApiError::internal_server_error(INTERNAL_SERVER_ERROR)
    })?;

    let content_language = item_data
        .title
        .or(item_data.description)
        .map(|localized| localized.language);

    Ok(ApiGatewayProxyResponseBuilder::json(200)
        .body(response)
        .try_content_language(content_language)
        .e_tag(item_data.event_id.to_string().as_str())
        .last_modified(item_data.updated)
        .cors()
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use aws_lambda_events::apigw::ApiGatewayProxyRequest;
    use aws_lambda_events::encodings::Body::Text;
    use aws_lambda_events::query_map::QueryMap;
    use common::currency::domain::Currency;
    use common::event_id::EventId;
    use common::language::domain::Language;
    use common::price::domain::{MonetaryAmount, Price};
    use common::shop_id::ShopId;
    use common::shops_item_id::ShopsItemId;
    use http::header::{ACCEPT_LANGUAGE, CONTENT_LANGUAGE, ETAG, LAST_MODIFIED};
    use http::{HeaderMap, HeaderValue};
    use item_core::item::domain::Item;
    use item_core::item::hash::ItemHash;
    use item_core::item_state::domain::ItemState;
    use item_read::service::MockReadItem;
    use lambda_runtime::LambdaEvent;
    use serde_json::Value;
    use std::collections::HashMap;
    use time::OffsetDateTime;
    use time::macros::datetime;

    #[rstest::rstest]
    #[case::de_DE("de-DE", "German title", "de")]
    #[case::de_AT("de-AT", "German title", "de")]
    #[case::de_CH("de-CH", "German title", "de")]
    #[case::de_LU("de-LU", "German title", "de")]
    #[case::de_LI("de-LI", "German title", "de")]
    #[case::en_US("en-US", "English title", "en")]
    #[case::en_GB("en-GB", "English title", "en")]
    #[case::en_AU("en-AU", "English title", "en")]
    #[case::en_CA("en-CA", "English title", "en")]
    #[case::en_NZ("en-NZ", "English title", "en")]
    #[case::en_IE("en_IE", "English title", "en")]
    #[case::fr_FR("fr-FR", "French title", "fr")]
    #[case::fr_CA("fr-CA", "French title", "fr")]
    #[case::fr_BE("fr-BE", "French title", "fr")]
    #[case::fr_CH("fr-CH", "French title", "fr")]
    #[case::fr_LU("fr-LU", "French title", "fr")]
    #[case::es_ES("es-ES", "Spanish title", "es")]
    #[case::es_MX("es-MX", "Spanish title", "es")]
    #[case::es_AR("es-AR", "Spanish title", "es")]
    #[case::es_CO("es-CO", "Spanish title", "es")]
    #[case::es_CL("es-CL", "Spanish title", "es")]
    #[case::es_PE("es-PE", "Spanish title", "es")]
    #[case::es_VE("es-VE", "Spanish title", "es")]
    #[case::complex_de("de;q=0.95,en;q=0.9", "German title", "de")]
    #[case::complex_en("en-US,en;q=0.9,de;q=0.8", "English title", "en")]
    #[case::complex_de("fr-CA,fr;q=0.92,en;q=0.6", "French title", "fr")]
    #[case::complex_de("es-ES,es;q=0.91,en;q=0.7", "Spanish title", "es")]
    #[case::edge_quality("en;q=1.0", "English title", "en")]
    #[case::edge_format_1("fr-CH, de;q=0.9, en;q=0.8", "French title", "fr")]
    #[case::edge_format_2("es-AR;q=0.6, es;q=0.5, en;q=0.3", "Spanish title", "es")]
    #[case::star("*", "German title", "de")]
    #[case::star_overriden("en, *", "English title", "en")]
    #[case::empty("", "German title", "de")]
    #[allow(non_snake_case)]
    #[tokio::test]
    async fn should_respect_accept_language_header_for_title(
        #[case] header_value: &str,
        #[case] expected_item_title: &str,
        #[case] expected_content_language: &str,
    ) {
        let mut service = MockReadItem::default();
        service
            .expect_get_item_with_currency()
            .return_once(|shop_id, shops_item_id, _| {
                let title = HashMap::from([
                    (Language::De, "German title".to_string()),
                    (Language::En, "English title".to_string()),
                    (Language::Es, "Spanish title".to_string()),
                    (Language::Fr, "French title".to_string()),
                ]);
                let item = Item {
                    item_id: Default::default(),
                    event_id: Default::default(),
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".to_string(),
                    title,
                    description: Default::default(),
                    price: None,
                    state: ItemState::Listed,
                    url: "".to_string(),
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
            payload: ApiGatewayProxyRequest {
                resource: None,
                path: None,
                http_method: Default::default(),
                headers: HeaderMap::from_iter([(
                    ACCEPT_LANGUAGE,
                    HeaderValue::from_str(header_value).unwrap(),
                )]),
                multi_value_headers: Default::default(),
                query_string_parameters: Default::default(),
                multi_value_query_string_parameters: Default::default(),
                path_parameters: HashMap::from_iter([
                    ("shopId".to_string(), shop_id.to_string()),
                    ("shopsItemId".to_string(), shops_item_id.to_string()),
                ]),
                stage_variables: Default::default(),
                request_context: Default::default(),
                body: None,
                is_base64_encoded: false,
            },
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        assert_eq!(
            expected_content_language,
            response.headers.get(CONTENT_LANGUAGE).unwrap()
        );
        if let Text(body) = response.body.unwrap() {
            let item_data = serde_json::from_str::<Value>(&body).unwrap();
            assert_eq!(
                expected_item_title,
                item_data.get("title").unwrap().get("text").unwrap()
            );
        } else {
            panic!("Expected Text.");
        }
    }

    #[tokio::test]
    async fn should_default_to_german_when_accept_language_header_is_missing_for_title() {
        let mut service = MockReadItem::default();
        service
            .expect_get_item_with_currency()
            .return_once(|shop_id, shops_item_id, _| {
                let title = HashMap::from([
                    (Language::De, "German title".to_string()),
                    (Language::En, "English title".to_string()),
                    (Language::Es, "Spanish title".to_string()),
                    (Language::Fr, "French title".to_string()),
                ]);
                let item = Item {
                    item_id: Default::default(),
                    event_id: Default::default(),
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".to_string(),
                    title,
                    description: Default::default(),
                    price: None,
                    state: ItemState::Listed,
                    url: "".to_string(),
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
            payload: ApiGatewayProxyRequest {
                resource: None,
                path: None,
                http_method: Default::default(),
                headers: Default::default(),
                multi_value_headers: Default::default(),
                query_string_parameters: Default::default(),
                multi_value_query_string_parameters: Default::default(),
                path_parameters: HashMap::from_iter([
                    ("shopId".to_string(), shop_id.to_string()),
                    ("shopsItemId".to_string(), shops_item_id.to_string()),
                ]),
                stage_variables: Default::default(),
                request_context: Default::default(),
                body: None,
                is_base64_encoded: false,
            },
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        if let Text(body) = response.body.unwrap() {
            let item_data = serde_json::from_str::<Value>(&body).unwrap();
            assert_eq!(
                "German title",
                item_data.get("title").unwrap().get("text").unwrap()
            );
        } else {
            panic!("Expected Text.");
        }
    }

    #[rstest::rstest]
    #[case::de_DE("de-DE", "German description", "de")]
    #[case::de_AT("de-AT", "German description", "de")]
    #[case::de_CH("de-CH", "German description", "de")]
    #[case::de_LU("de-LU", "German description", "de")]
    #[case::de_LI("de-LI", "German description", "de")]
    #[case::en_US("en-US", "English description", "en")]
    #[case::en_GB("en-GB", "English description", "en")]
    #[case::en_AU("en-AU", "English description", "en")]
    #[case::en_CA("en-CA", "English description", "en")]
    #[case::en_NZ("en-NZ", "English description", "en")]
    #[case::en_IE("en_IE", "English description", "en")]
    #[case::fr_FR("fr-FR", "French description", "fr")]
    #[case::fr_CA("fr-CA", "French description", "fr")]
    #[case::fr_BE("fr-BE", "French description", "fr")]
    #[case::fr_CH("fr-CH", "French description", "fr")]
    #[case::fr_LU("fr-LU", "French description", "fr")]
    #[case::es_ES("es-ES", "Spanish description", "es")]
    #[case::es_MX("es-MX", "Spanish description", "es")]
    #[case::es_AR("es-AR", "Spanish description", "es")]
    #[case::es_CO("es-CO", "Spanish description", "es")]
    #[case::es_CL("es-CL", "Spanish description", "es")]
    #[case::es_PE("es-PE", "Spanish description", "es")]
    #[case::es_VE("es-VE", "Spanish description", "es")]
    #[case::complex_de("de;q=0.95,en;q=0.9", "German description", "de")]
    #[case::complex_en("en-US,en;q=0.9,de;q=0.8", "English description", "en")]
    #[case::complex_de("fr-CA,fr;q=0.92,en;q=0.6", "French description", "fr")]
    #[case::complex_de("es-ES,es;q=0.91,en;q=0.7", "Spanish description", "es")]
    #[case::edge_quality("en;q=1.0", "English description", "en")]
    #[case::edge_format_1("fr-CH, de;q=0.9, en;q=0.8", "French description", "fr")]
    #[case::edge_format_2("es-AR;q=0.6, es;q=0.5, en;q=0.3", "Spanish description", "es")]
    #[case::star("*", "German description", "de")]
    #[case::star_overriden("en, *", "English description", "en")]
    #[case::empty("", "German description", "de")]
    #[allow(non_snake_case)]
    #[tokio::test]
    async fn should_respect_accept_language_header_for_description(
        #[case] header_value: &str,
        #[case] expected_item_description: &str,
        #[case] expected_content_language: &str,
    ) {
        let mut service = MockReadItem::default();
        service
            .expect_get_item_with_currency()
            .return_once(|shop_id, shops_item_id, _| {
                let description = HashMap::from([
                    (Language::De, "German description".to_string()),
                    (Language::En, "English description".to_string()),
                    (Language::Es, "Spanish description".to_string()),
                    (Language::Fr, "French description".to_string()),
                ]);
                let item = Item {
                    item_id: Default::default(),
                    event_id: Default::default(),
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".to_string(),
                    title: Default::default(),
                    description,
                    price: None,
                    state: ItemState::Listed,
                    url: "".to_string(),
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
            payload: ApiGatewayProxyRequest {
                resource: None,
                path: None,
                http_method: Default::default(),
                headers: HeaderMap::from_iter([(
                    ACCEPT_LANGUAGE,
                    HeaderValue::from_str(header_value).unwrap(),
                )]),
                multi_value_headers: Default::default(),
                query_string_parameters: Default::default(),
                multi_value_query_string_parameters: Default::default(),
                path_parameters: HashMap::from_iter([
                    ("shopId".to_string(), shop_id.to_string()),
                    ("shopsItemId".to_string(), shops_item_id.to_string()),
                ]),
                stage_variables: Default::default(),
                request_context: Default::default(),
                body: None,
                is_base64_encoded: false,
            },
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        assert_eq!(
            expected_content_language,
            response.headers.get(CONTENT_LANGUAGE).unwrap()
        );
        if let Text(body) = response.body.unwrap() {
            let item_data = serde_json::from_str::<Value>(&body).unwrap();
            assert_eq!(
                expected_item_description,
                item_data.get("description").unwrap().get("text").unwrap()
            );
        } else {
            panic!("Expected Text.");
        }
    }

    #[tokio::test]
    async fn should_default_to_german_when_accept_language_header_is_missing_for_description() {
        let mut service = MockReadItem::default();
        service
            .expect_get_item_with_currency()
            .return_once(|shop_id, shops_item_id, _| {
                let description = HashMap::from([
                    (Language::De, "German description".to_string()),
                    (Language::En, "English description".to_string()),
                    (Language::Es, "Spanish description".to_string()),
                    (Language::Fr, "French description".to_string()),
                ]);
                let item = Item {
                    item_id: Default::default(),
                    event_id: Default::default(),
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".to_string(),
                    title: Default::default(),
                    description,
                    price: None,
                    state: ItemState::Listed,
                    url: "".to_string(),
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
            payload: ApiGatewayProxyRequest {
                resource: None,
                path: None,
                http_method: Default::default(),
                headers: Default::default(),
                multi_value_headers: Default::default(),
                query_string_parameters: Default::default(),
                multi_value_query_string_parameters: Default::default(),
                path_parameters: HashMap::from_iter([
                    ("shopId".to_string(), shop_id.to_string()),
                    ("shopsItemId".to_string(), shops_item_id.to_string()),
                ]),
                stage_variables: Default::default(),
                request_context: Default::default(),
                body: None,
                is_base64_encoded: false,
            },
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        if let Text(body) = response.body.unwrap() {
            let item_data = serde_json::from_str::<Value>(&body).unwrap();
            assert_eq!(
                "German description",
                item_data.get("description").unwrap().get("text").unwrap()
            );
        } else {
            panic!("Expected Text.");
        }
    }

    #[tokio::test]
    async fn should_default_to_german_when_accept_language_header_is_invalid_for_title() {
        let mut service = MockReadItem::default();
        service
            .expect_get_item_with_currency()
            .return_once(|shop_id, shops_item_id, _| {
                let title = HashMap::from([
                    (Language::De, "German description".to_string()),
                    (Language::En, "English description".to_string()),
                    (Language::Es, "Spanish description".to_string()),
                    (Language::Fr, "French description".to_string()),
                ]);
                let item = Item {
                    item_id: Default::default(),
                    event_id: Default::default(),
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".to_string(),
                    title,
                    description: Default::default(),
                    price: None,
                    state: ItemState::Listed,
                    url: "".to_string(),
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
            payload: ApiGatewayProxyRequest {
                resource: None,
                path: None,
                http_method: Default::default(),
                headers: HeaderMap::from_iter([(
                    ACCEPT_LANGUAGE,
                    HeaderValue::from_str("invalid header value").unwrap(),
                )]),
                multi_value_headers: Default::default(),
                query_string_parameters: Default::default(),
                multi_value_query_string_parameters: Default::default(),
                path_parameters: HashMap::from_iter([
                    ("shopId".to_string(), shop_id.to_string()),
                    ("shopsItemId".to_string(), shops_item_id.to_string()),
                ]),
                stage_variables: Default::default(),
                request_context: Default::default(),
                body: None,
                is_base64_encoded: false,
            },
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        if let Text(body) = response.body.unwrap() {
            let item_data = serde_json::from_str::<Value>(&body).unwrap();
            assert_eq!(
                "German description",
                item_data.get("title").unwrap().get("text").unwrap()
            );
        } else {
            panic!("Expected Text.");
        }
    }

    #[tokio::test]
    async fn should_default_to_german_when_accept_language_header_is_invalid_for_description() {
        let mut service = MockReadItem::default();
        service
            .expect_get_item_with_currency()
            .return_once(|shop_id, shops_item_id, _| {
                let description = HashMap::from([
                    (Language::De, "German description".to_string()),
                    (Language::En, "English description".to_string()),
                    (Language::Es, "Spanish description".to_string()),
                    (Language::Fr, "French description".to_string()),
                ]);
                let item = Item {
                    item_id: Default::default(),
                    event_id: Default::default(),
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".to_string(),
                    title: Default::default(),
                    description,
                    price: None,
                    state: ItemState::Listed,
                    url: "".to_string(),
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
            payload: ApiGatewayProxyRequest {
                resource: None,
                path: None,
                http_method: Default::default(),
                headers: HeaderMap::from_iter([(
                    ACCEPT_LANGUAGE,
                    HeaderValue::from_str("invalid header value").unwrap(),
                )]),
                multi_value_headers: Default::default(),
                query_string_parameters: Default::default(),
                multi_value_query_string_parameters: Default::default(),
                path_parameters: HashMap::from_iter([
                    ("shopId".to_string(), shop_id.to_string()),
                    ("shopsItemId".to_string(), shops_item_id.to_string()),
                ]),
                stage_variables: Default::default(),
                request_context: Default::default(),
                body: None,
                is_base64_encoded: false,
            },
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        if let Text(body) = response.body.unwrap() {
            let item_data = serde_json::from_str::<Value>(&body).unwrap();
            assert_eq!(
                "German description",
                item_data.get("description").unwrap().get("text").unwrap()
            );
        } else {
            panic!("Expected Text.");
        }
    }

    #[tokio::test]
    async fn should_400_when_currency_query_param_is_invalid() {
        let mut service = MockReadItem::default();
        service.expect_get_item_with_currency().never();
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayProxyRequest {
                resource: None,
                path: None,
                http_method: Default::default(),
                headers: Default::default(),
                multi_value_headers: Default::default(),
                query_string_parameters: QueryMap::from(HashMap::from([(
                    "currency".to_string(),
                    "invalid_currency".to_string(),
                )])),
                multi_value_query_string_parameters: Default::default(),
                path_parameters: HashMap::from_iter([
                    ("shopId".to_string(), shop_id.to_string()),
                    ("shopsItemId".to_string(), shops_item_id.to_string()),
                ]),
                stage_variables: Default::default(),
                request_context: Default::default(),
                body: None,
                is_base64_encoded: false,
            },
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);
        if let Text(body) = response.body.unwrap() {
            let item_data = serde_json::from_str::<Value>(&body).unwrap();
            assert_eq!(400, item_data.get("status").unwrap().as_u64().unwrap());
            assert_eq!(
                "currency",
                item_data.get("source").unwrap().get("field").unwrap()
            );
        } else {
            panic!("Expected Text.");
        }
    }

    #[tokio::test]
    async fn should_default_to_euro_when_currency_query_param_is_missing() {
        let mut service = MockReadItem::default();
        service
            .expect_get_item_with_currency()
            .return_once(|shop_id, shops_item_id, _| {
                let description = HashMap::from([
                    (Language::De, "German description".to_string()),
                    (Language::En, "English description".to_string()),
                    (Language::Es, "Spanish description".to_string()),
                    (Language::Fr, "French description".to_string()),
                ]);
                let item = Item {
                    item_id: Default::default(),
                    event_id: Default::default(),
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".to_string(),
                    title: Default::default(),
                    description,
                    price: Some(Price {
                        currency: Currency::Eur,
                        monetary_amount: MonetaryAmount::try_from(100f32).unwrap(),
                    }),
                    state: ItemState::Listed,
                    url: "".to_string(),
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
            payload: ApiGatewayProxyRequest {
                resource: None,
                path: None,
                http_method: Default::default(),
                headers: Default::default(),
                multi_value_headers: Default::default(),
                query_string_parameters: Default::default(),
                multi_value_query_string_parameters: Default::default(),
                path_parameters: HashMap::from_iter([
                    ("shopId".to_string(), shop_id.to_string()),
                    ("shopsItemId".to_string(), shops_item_id.to_string()),
                ]),
                stage_variables: Default::default(),
                request_context: Default::default(),
                body: None,
                is_base64_encoded: false,
            },
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        if let Text(body) = response.body.unwrap() {
            let item_data = serde_json::from_str::<Value>(&body).unwrap();
            assert_eq!(
                "EUR",
                item_data.get("price").unwrap().get("currency").unwrap()
            );
        } else {
            panic!("Expected Text.");
        }
    }

    #[tokio::test]
    async fn should_400_when_path_param_shop_id_is_missing() {
        let mut service = MockReadItem::default();
        service.expect_get_item_with_currency().never();
        let shops_item_id = ShopsItemId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayProxyRequest {
                resource: None,
                path: None,
                http_method: Default::default(),
                headers: Default::default(),
                multi_value_headers: Default::default(),
                query_string_parameters: Default::default(),
                multi_value_query_string_parameters: Default::default(),
                path_parameters: HashMap::from_iter([(
                    "shopsItemId".to_string(),
                    shops_item_id.to_string(),
                )]),
                stage_variables: Default::default(),
                request_context: Default::default(),
                body: None,
                is_base64_encoded: false,
            },
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);
        if let Text(body) = response.body.unwrap() {
            let item_data = serde_json::from_str::<Value>(&body).unwrap();
            assert_eq!(400, item_data.get("status").unwrap().as_u64().unwrap());
            assert_eq!(
                "shopId",
                item_data.get("source").unwrap().get("field").unwrap()
            );
        } else {
            panic!("Expected Text.");
        }
    }

    #[tokio::test]
    async fn should_400_when_path_param_shops_item_id_is_missing() {
        let mut service = MockReadItem::default();
        service.expect_get_item_with_currency().never();
        let shop_id = ShopId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayProxyRequest {
                resource: None,
                path: None,
                http_method: Default::default(),
                headers: Default::default(),
                multi_value_headers: Default::default(),
                query_string_parameters: Default::default(),
                multi_value_query_string_parameters: Default::default(),
                path_parameters: HashMap::from_iter([("shopId".to_string(), shop_id.to_string())]),
                stage_variables: Default::default(),
                request_context: Default::default(),
                body: None,
                is_base64_encoded: false,
            },
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);
        if let Text(body) = response.body.unwrap() {
            let item_data = serde_json::from_str::<Value>(&body).unwrap();
            assert_eq!(400, item_data.get("status").unwrap().as_u64().unwrap());
            assert_eq!(
                "shopsItemId",
                item_data.get("source").unwrap().get("field").unwrap()
            );
        } else {
            panic!("Expected Text.");
        }
    }

    #[tokio::test]
    async fn should_include_event_id_as_header_e_tag() {
        let event_id = EventId::new();
        let mut service = MockReadItem::default();
        service
            .expect_get_item_with_currency()
            .return_once(move |shop_id, shops_item_id, _| {
                let title = HashMap::from([
                    (Language::De, "German title".to_string()),
                    (Language::En, "English title".to_string()),
                    (Language::Es, "Spanish title".to_string()),
                    (Language::Fr, "French title".to_string()),
                ]);
                let item = Item {
                    item_id: Default::default(),
                    event_id,
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".to_string(),
                    title,
                    description: Default::default(),
                    price: None,
                    state: ItemState::Listed,
                    url: "".to_string(),
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
            payload: ApiGatewayProxyRequest {
                resource: None,
                path: None,
                http_method: Default::default(),
                headers: Default::default(),
                multi_value_headers: Default::default(),
                query_string_parameters: Default::default(),
                multi_value_query_string_parameters: Default::default(),
                path_parameters: HashMap::from_iter([
                    ("shopId".to_string(), shop_id.to_string()),
                    ("shopsItemId".to_string(), shops_item_id.to_string()),
                ]),
                stage_variables: Default::default(),
                request_context: Default::default(),
                body: None,
                is_base64_encoded: false,
            },
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
        let mut service = MockReadItem::default();
        service
            .expect_get_item_with_currency()
            .return_once(move |shop_id, shops_item_id, _| {
                let title = HashMap::from([
                    (Language::De, "German title".to_string()),
                    (Language::En, "English title".to_string()),
                    (Language::Es, "Spanish title".to_string()),
                    (Language::Fr, "French title".to_string()),
                ]);
                let item = Item {
                    item_id: Default::default(),
                    event_id,
                    shop_id: shop_id.clone(),
                    shops_item_id: shops_item_id.clone(),
                    shop_name: "".to_string(),
                    title,
                    description: Default::default(),
                    price: None,
                    state: ItemState::Listed,
                    url: "".to_string(),
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
            payload: ApiGatewayProxyRequest {
                resource: None,
                path: None,
                http_method: Default::default(),
                headers: Default::default(),
                multi_value_headers: Default::default(),
                query_string_parameters: Default::default(),
                multi_value_query_string_parameters: Default::default(),
                path_parameters: HashMap::from_iter([
                    ("shopId".to_string(), shop_id.to_string()),
                    ("shopsItemId".to_string(), shops_item_id.to_string()),
                ]),
                stage_variables: Default::default(),
                request_context: Default::default(),
                body: None,
                is_base64_encoded: false,
            },
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        assert_eq!(
            "Wed, 01 Jan 2020 00:00:00 GMT",
            response.headers.get(LAST_MODIFIED).unwrap()
        );
    }
}
