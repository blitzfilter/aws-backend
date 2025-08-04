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
use item_read::service::QueryItemService;
use lambda_runtime::LambdaEvent;
use tracing::error;

#[tracing::instrument(skip(event, service), fields(requestId = %event.context.request_id))]
pub async fn handler(
    event: LambdaEvent<ApiGatewayProxyRequest>,
    service: &impl QueryItemService,
) -> Result<ApiGatewayProxyResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayProxyResponse::from(err)),
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayProxyRequest>,
    service: &impl QueryItemService,
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

    let item_data: GetItemData = service
        .view_item(&shop_id, &shops_item_id, languages.as_slice(), &currency)
        .await?
        .into();
    let response = serde_json::to_string(&item_data).map_err(|err| {
        error!(error = %err, payload = ?item_data, "Failed serializing GetItemData.");
        ApiError::internal_server_error(INTERNAL_SERVER_ERROR)
    })?;

    let content_language = item_data.title.language;

    Ok(ApiGatewayProxyResponseBuilder::json(200)
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
    use aws_lambda_events::apigw::ApiGatewayProxyRequest;
    use aws_lambda_events::encodings::Body::Text;
    use aws_lambda_events::query_map::QueryMap;
    use common::event_id::EventId;
    use common::language::domain::Language;
    use common::localized::Localized;
    use common::shop_id::ShopId;
    use common::shops_item_id::ShopsItemId;
    use http::header::{ETAG, LAST_MODIFIED};
    use item_core::item::domain::LocalizedItemView;
    use item_core::item::hash::ItemHash;
    use item_core::item_state::domain::ItemState;
    use item_read::service::MockQueryItemService;
    use lambda_runtime::LambdaEvent;
    use serde_json::Value;
    use std::collections::HashMap;
    use time::OffsetDateTime;
    use time::macros::datetime;
    use url::Url;

    #[tokio::test]
    async fn should_400_when_currency_query_param_is_invalid() {
        let mut service = MockQueryItemService::default();
        service.expect_view_item().never();
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
    async fn should_400_when_path_param_shop_id_is_missing() {
        let mut service = MockQueryItemService::default();
        service.expect_view_item().never();
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
        let mut service = MockQueryItemService::default();
        service.expect_view_item().never();
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
