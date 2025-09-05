use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{BAD_PARAMETER, INTERNAL_SERVER_ERROR};
use common::currency::data::api::extract_currency_query;
use common::language::data::api::extract_languages_header;
use common::language::domain::Language;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_data::get_data::GetItemData;
use item_service::get_service::GetItemService;
use lambda_runtime::LambdaEvent;
use tracing::error;

#[tracing::instrument(
    skip(event, service),
    fields(
        requestId = %event.context.request_id,
        path = &event.payload.raw_path,
        query = &event.payload.raw_query_string,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl GetItemService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayV2httpResponse::from(err)),
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl GetItemService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let languages = extract_languages_header(&event.payload.headers)?
        .into_iter()
        .map(Language::from)
        .collect::<Vec<_>>();
    let currency = extract_currency_query(&event.payload.query_string_parameters)?.into();
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
    use common::event_id::EventId;
    use common::item_state::domain::ItemState;
    use common::language::data::LanguageData;
    use common::language::domain::Language;
    use common::localized::Localized;
    use common::shop_id::ShopId;
    use common::shops_item_id::ShopsItemId;
    use http::header::{ACCEPT_LANGUAGE, CONTENT_LANGUAGE, ETAG, LAST_MODIFIED};
    use item_core::{hash::ItemHash, item::LocalizedItemView};
    use item_service::get_service::{GetItemError, MockGetItemService};
    use lambda_runtime::LambdaEvent;
    use test_api::{ApiGatewayV2httpRequestProxy, extract_apigw_response_json_body};
    use time::OffsetDateTime;
    use time::macros::datetime;
    use url::Url;

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

        let mut service = MockGetItemService::default();
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
    async fn should_include_event_id_as_header_e_tag() {
        let event_id = EventId::new();
        let mut service = MockGetItemService::default();
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
        let mut service = MockGetItemService::default();
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
    async fn should_400_when_path_param_shop_id_is_missing() {
        let mut service = MockGetItemService::default();
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
        let mut service = MockGetItemService::default();
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

        let mut service = MockGetItemService::default();
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
