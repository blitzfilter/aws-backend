use super::types::{CreateSearchFilterData, SearchFilterData};
use super::util::last_modified;
use crate::auth::protected_context;
use crate::error::{ApiError, SEARCH_FILTER_INTERNAL_ERROR};
use crate::state::SearchFiltersState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use search_filter_service::use_cases::CreateSearchFilterCommand;

pub(super) async fn create_search_filter(
    State(state): State<SearchFiltersState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (context, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let data: CreateSearchFilterData = match super::util::parse_json(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let search = match product_listing_core::product_listing_search::ProductListingSearch::try_from(
        data.search,
    ) {
        Ok(search) => search,
        Err(error) => return error.into_response(),
    };
    match state
        .create_search_filter
        .execute(
            &context,
            CreateSearchFilterCommand {
                user_id,
                name: data.name,
                notifications: data.notifications,
                search,
            },
        )
        .await
    {
        Ok(result) => {
            let search_filter_id = result.filter.search_filter_id;
            let language = result.filter.search.language.as_str();
            let updated = result.filter.updated;
            let mut response = (
                StatusCode::CREATED,
                Json(SearchFilterData::from(result.filter)),
            )
                .into_response();
            let location = format!("/api/v1/me/search-filters/{search_filter_id}");
            match HeaderValue::from_str(&location) {
                Ok(value) => {
                    response.headers_mut().insert(header::LOCATION, value);
                }
                Err(_) => {
                    return ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                        .with_detail("Search filter location failed internally.")
                        .into_response();
                }
            }
            response
                .headers_mut()
                .insert(header::CONTENT_LANGUAGE, HeaderValue::from_static(language));
            last_modified(&mut response, updated);
            response
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}
