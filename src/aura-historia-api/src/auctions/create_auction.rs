use crate::{
    auctions::types::{AuctionAdminData, CreateAuctionData},
    auth::protected_context,
    error::{AUCTION_INTERNAL_ERROR, ApiError, BAD_BODY_VALUE},
    state::AuctionsState,
};
use auction_service::use_cases::commands::create_auction::CreateAuctionCommand;
use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

pub async fn create_auction(
    State(state): State<AuctionsState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let command = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };

    match state.create.execute(&context, command).await {
        Ok(result) => {
            let auction_id = result.auction_id;
            let location =
                match HeaderValue::from_str(&format!("/api/v1/admin/auctions/{auction_id}")) {
                    Ok(value) => value,
                    Err(_) => {
                        return no_store(
                            ApiError::internal_server_error(AUCTION_INTERNAL_ERROR)
                                .with_detail("Auction location failed internally.")
                                .into_response(),
                        );
                    }
                };
            let mut response = (
                StatusCode::CREATED,
                axum::Json(AuctionAdminData::from(result)),
            )
                .into_response();
            response.headers_mut().insert(header::LOCATION, location);
            no_store(response)
        }
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_body(body: &str) -> Result<CreateAuctionCommand, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }
    let data = serde_json::from_str::<CreateAuctionData>(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))?;
    data.try_into()
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
