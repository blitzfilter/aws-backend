use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::{
    api::{
        api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder,
        collection::{CollectionData, PaginationData},
        error::ApiError,
        error_code::{BAD_PARAMETER, INTERNAL_SERVER_ERROR},
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
use search_filter_core::{search_filter::SearchFilter, text_query::TextQuery};
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
        .into();
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
