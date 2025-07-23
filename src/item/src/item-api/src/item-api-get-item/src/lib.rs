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
        Ok(response) => Ok(ApiGatewayProxyResponseBuilder::json(200)
            .body(response)
            .cors()
            .build()),
        Err(err) => Ok(err.into()),
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayProxyRequest>,
    service: &impl ReadItem,
) -> Result<String, ApiError> {
    let languages = event
        .payload
        .headers
        .get("Accept-Language")
        .map(HeaderValue::to_str)
        .map(|header_value_res| {
            header_value_res.map_err(|_| {
                ApiError::bad_request(BAD_HEADER_VALUE).with_header_field("Accept-Language")
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
        .unwrap_or(Currency::Eur);
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
    Ok(response)
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use aws_lambda_events::apigw::ApiGatewayProxyRequest;
    use aws_lambda_events::encodings::Body::Text;
    use common::language::domain::Language;
    use common::shop_id::ShopId;
    use common::shops_item_id::ShopsItemId;
    use http::header::ACCEPT_LANGUAGE;
    use http::{HeaderMap, HeaderValue};
    use item_core::item::domain::Item;
    use item_core::item::hash::ItemHash;
    use item_core::item_state::domain::ItemState;
    use item_read::service::MockReadItem;
    use lambda_runtime::LambdaEvent;
    use serde_json::Value;
    use std::collections::HashMap;
    use time::OffsetDateTime;

    #[rstest::rstest]
    #[case::de_DE("de-DE", "German title")]
    #[case::de_AT("de-AT", "German title")]
    #[case::de_CH("de-CH", "German title")]
    #[case::de_LU("de-LU", "German title")]
    #[case::de_LI("de-LI", "German title")]
    #[case::en_US("en-US", "English title")]
    #[case::en_GB("en-GB", "English title")]
    #[case::en_AU("en-AU", "English title")]
    #[case::en_CA("en-CA", "English title")]
    #[case::en_NZ("en-NZ", "English title")]
    #[case::en_IE("en_IE", "English title")]
    #[case::fr_FR("fr-FR", "French title")]
    #[case::fr_CA("fr-CA", "French title")]
    #[case::fr_BE("fr-BE", "French title")]
    #[case::fr_CH("fr-CH", "French title")]
    #[case::fr_LU("fr-LU", "French title")]
    #[case::es_ES("es-ES", "Spanish title")]
    #[case::es_MX("es-MX", "Spanish title")]
    #[case::es_AR("es-AR", "Spanish title")]
    #[case::es_CO("es-CO", "Spanish title")]
    #[case::es_CL("es-CL", "Spanish title")]
    #[case::es_PE("es-PE", "Spanish title")]
    #[case::es_VE("es-VE", "Spanish title")]
    #[case::complex_de("de;q=0.95,en;q=0.9", "German title")]
    #[case::complex_en("en-US,en;q=0.9,de;q=0.8", "English title")]
    #[case::complex_de("fr-CA,fr;q=0.92,en;q=0.6", "French title")]
    #[case::complex_de("es-ES,es;q=0.91,en;q=0.7", "Spanish title")]
    #[case::edge_quality("en;q=1.0", "English title")]
    #[case::edge_format_1("fr-CH, de;q=0.9, en;q=0.8", "French title")]
    #[case::edge_format_2("es-AR;q=0.6, es;q=0.5, en;q=0.3", "Spanish title")]
    #[case::star("*", "German title")]
    #[case::star_overriden("en, *", "English title")]
    #[allow(non_snake_case)]
    #[tokio::test]
    async fn should_respect_accept_language_header_for_title(
        #[case] header_value: &str,
        #[case] expected_item_title: &str,
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
        if let Text(body) = handler(lambda_event, &service).await.unwrap().body.unwrap() {
            let item_data = serde_json::from_str::<Value>(&body).unwrap();
            assert_eq!(
                expected_item_title,
                item_data.get("title").unwrap().get("text").unwrap()
            );
        } else {
            panic!("Expected Text.");
        }
    }

    #[rstest::rstest]
    #[case::de_DE("de-DE", "German description")]
    #[case::de_AT("de-AT", "German description")]
    #[case::de_CH("de-CH", "German description")]
    #[case::de_LU("de-LU", "German description")]
    #[case::de_LI("de-LI", "German description")]
    #[case::en_US("en-US", "English description")]
    #[case::en_GB("en-GB", "English description")]
    #[case::en_AU("en-AU", "English description")]
    #[case::en_CA("en-CA", "English description")]
    #[case::en_NZ("en-NZ", "English description")]
    #[case::en_IE("en_IE", "English description")]
    #[case::fr_FR("fr-FR", "French description")]
    #[case::fr_CA("fr-CA", "French description")]
    #[case::fr_BE("fr-BE", "French description")]
    #[case::fr_CH("fr-CH", "French description")]
    #[case::fr_LU("fr-LU", "French description")]
    #[case::es_ES("es-ES", "Spanish description")]
    #[case::es_MX("es-MX", "Spanish description")]
    #[case::es_AR("es-AR", "Spanish description")]
    #[case::es_CO("es-CO", "Spanish description")]
    #[case::es_CL("es-CL", "Spanish description")]
    #[case::es_PE("es-PE", "Spanish description")]
    #[case::es_VE("es-VE", "Spanish description")]
    #[case::complex_de("de;q=0.95,en;q=0.9", "German description")]
    #[case::complex_en("en-US,en;q=0.9,de;q=0.8", "English description")]
    #[case::complex_de("fr-CA,fr;q=0.92,en;q=0.6", "French description")]
    #[case::complex_de("es-ES,es;q=0.91,en;q=0.7", "Spanish description")]
    #[case::edge_quality("en;q=1.0", "English description")]
    #[case::edge_format_1("fr-CH, de;q=0.9, en;q=0.8", "French description")]
    #[case::edge_format_2("es-AR;q=0.6, es;q=0.5, en;q=0.3", "Spanish description")]
    #[case::star("*", "German description")]
    #[case::star_overriden("en, *", "English description")]
    #[allow(non_snake_case)]
    #[tokio::test]
    async fn should_respect_accept_language_header_for_description(
        #[case] header_value: &str,
        #[case] expected_item_description: &str,
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
        if let Text(body) = handler(lambda_event, &service).await.unwrap().body.unwrap() {
            let item_data = serde_json::from_str::<Value>(&body).unwrap();
            assert_eq!(
                expected_item_description,
                item_data.get("description").unwrap().get("text").unwrap()
            );
        } else {
            panic!("Expected Text.");
        }
    }
}
