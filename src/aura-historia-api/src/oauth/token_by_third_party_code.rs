use super::no_store;
use super::token::TokenResponseData;
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE};
use crate::state::OAuthState;
use axum::{
    Json,
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use oauth_core::third_party_exchange_code::ThirdPartyExchangeCode;

pub async fn token_by_third_party_code(
    State(state): State<OAuthState>,
    Path(raw): Path<String>,
) -> Response {
    let code = match ThirdPartyExchangeCode::try_from(raw.as_str()) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_path_field("thirdPartyCode")
                .with_detail("Path parameter 'thirdPartyCode' is invalid.")
                .into_response();
        }
    };
    match state.token_by_third_party_code.execute(&code).await {
        Ok(result) => no_store(Json(TokenResponseData::from(result)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}
