use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        collection::{CollectionData, PaginationData},
        error::ApiError,
        error_code::{BAD_PARAMETER, INTERNAL_SERVER_ERROR, TEXT_QUERY_TOO_SHORT},
    },
    currency::{data::api::extract_currency_query, domain::Currency},
    language::{data::api::extract_language_header, domain::Language},
    page::{Page, api::extract_page_query},
    sort::api::extract_sort_query,
};
use item_core::sort_item_field::SortItemField;
use item_data::{get_data::GetItemData, sort_item_field_data::SortItemFieldData};
use item_service::query_service::QueryItemService;
use lambda_runtime::LambdaEvent;
use search_filter_core::{
    search_filter::SearchFilter,
    text_query::{TextQuery, TextQueryTooShortError},
};
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
    let language: Language = extract_language_header(&event.payload.headers)?.into();
    let currency: Currency = extract_currency_query(&event.payload.query_string_parameters)?.into();
    let sort = extract_sort_query::<SortItemFieldData>(&event.payload.query_string_parameters)?
        .map(|sort_data| sort_data.map(SortItemField::from));
    let page = extract_page_query(&event.payload.query_string_parameters)?
        .unwrap_or(Page { from: 0, size: 21 });
    let item_query: TextQuery = event
        .payload
        .query_string_parameters
        .first("q")
        .map(str::trim)
        .ok_or(ApiError::bad_request(BAD_PARAMETER).with_query_field("q"))?
        .try_into()
        .map_err(|err: TextQueryTooShortError| {
            ApiError::bad_request(TEXT_QUERY_TOO_SHORT)
                .with_query_field("q")
                .with_message(err.to_string())
        })?;
    let search_filter = SearchFilter {
        item_query,
        shop_name_query: None,
        price_query: None,
        state_query: Default::default(),
        created_query: None,
        updated_query: None,
    };

    let search_result = service
        .search_items(&search_filter, &language, &currency, &sort, &Some(page))
        .await?;

    let items = search_result
        .hits
        .into_iter()
        .map(GetItemData::from)
        .collect::<Vec<_>>();
    let pagination = PaginationData {
        from: page.from as u64,
        size: page.size as u64,
        total: search_result.total,
    };
    let collection = CollectionData { items, pagination };

    let response = serde_json::to_string(&collection).map_err(|err| {
        error!(
            error = %err,
            payload = ?collection,
            type = %std::any::type_name::<CollectionData<GetItemData>>(),
            "Failed serializing collection of items"
        );
        ApiError::internal_server_error(INTERNAL_SERVER_ERROR)
    })?;

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body(response)
        .cors()
        .build())
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
mod tests {
    use crate::handler;
    use common::opensearch::search_result::SearchResult;
    use http::header::ACCEPT_LANGUAGE;
    use item_core::item::LocalizedItemView;
    use item_service::query_service::MockQueryItemService;
    use lambda_runtime::LambdaEvent;
    use test_api::ApiGatewayV2httpRequestProxy;
    use test_api::extract_apigw_response_json_body;

    #[tokio::test]
    #[rstest::rstest]
    #[case(
        Some("de"),
        "cool item title keywords",
        Some("EUR"),
        Some("price"),
        Some("asc"),
        Some("5"),
        Some("20")
    )]
    #[case(
        Some("en"),
        "boop doop",
        Some("USD"),
        Some("created"),
        Some("desc"),
        None,
        None
    )]
    #[case(Some("en"), "boop doop", Some("USD"), None, None, Some("7"), None)]
    #[case(
        Some("en"),
        "boop doop",
        Some("AUD"),
        Some("updated"),
        Some("desc"),
        None,
        Some("10")
    )]
    #[case(None, "boop doop", None, None, None, None, None)]
    async fn should_handle_request(
        #[case] content_language: Option<&str>,
        #[case] q: &str,
        #[case] currency: Option<&str>,
        #[case] sort: Option<&str>,
        #[case] order: Option<&str>,
        #[case] page_from: Option<&str>,
        #[case] page_size: Option<&str>,
    ) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .try_header(ACCEPT_LANGUAGE.as_str(), content_language)
                .query_string_parameter("q", q)
                .try_query_string_parameter("currency", currency)
                .try_query_string_parameter("sort", sort)
                .try_query_string_parameter("order", order)
                .try_query_string_parameter("from", page_from)
                .try_query_string_parameter("size", page_size)
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryItemService::default();
        service
            .expect_search_items()
            .return_once(|_, _, _, _, page| {
                let count = page.map(|page| page.size).unwrap_or(20) as usize;
                let search_result = SearchResult {
                    hits: fake::vec![LocalizedItemView; count],
                    total: 789,
                };
                Box::pin(async move { Ok(search_result) })
            });
        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(None, "boop doop", None, None)]
    async fn should_default_page_sizing_when_none_given(
        #[case] content_language: Option<&str>,
        #[case] q: &str,
        #[case] page_from: Option<&str>,
        #[case] page_size: Option<&str>,
    ) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .try_header(ACCEPT_LANGUAGE.as_str(), content_language)
                .query_string_parameter("q", q)
                .try_query_string_parameter("from", page_from)
                .try_query_string_parameter("size", page_size)
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryItemService::default();
        service
            .expect_search_items()
            .return_once(|_, _, _, _, page| {
                let count = page.map(|page| page.size).unwrap() as usize;
                let search_result = SearchResult {
                    hits: fake::vec![LocalizedItemView; count],
                    total: 789,
                };
                Box::pin(async move { Ok(search_result) })
            });
        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(0, json["pagination"]["from"]);
        assert_eq!(21, json["pagination"]["size"]);
        assert_eq!(789, json["pagination"]["total"]);
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case("a")]
    #[case("ab")]
    #[case("ba")]
    #[case("12")]
    #[case("0")]
    async fn should_400_when_text_query_is_too_short(#[case] q: &str) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .query_string_parameter("q", q)
                .build(),
            context: Default::default(),
        };

        let mut service = MockQueryItemService::default();
        service.expect_search_items().never();
        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(400, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(400, json["status"]);
        assert_eq!("q", json["source"]["field"]);
    }
}
