use crate::{
    auctions::types::{AuctionAdminData, UpdateAuctionData},
    auth::protected_context,
    error::{ApiError, BAD_BODY_VALUE},
    state::AuctionsState,
    wire::parse_path_object_id,
};
use auction_core::AuctionId;
use auction_service::use_cases::commands::update_auction::UpdateAuctionCommand;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};

pub async fn update_auction(
    State(state): State<AuctionsState>,
    headers: HeaderMap,
    Path(raw_auction_id): Path<String>,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let auction_id = match parse_auction_id(&raw_auction_id) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let command = match parse_body(&body, auction_id) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };

    match state.update.execute(&context, command).await {
        Ok(result) => no_store(axum::Json(AuctionAdminData::from(result)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_auction_id(raw: &str) -> Result<AuctionId, ApiError> {
    parse_path_object_id(raw, "auctionId", "Auction")
}

fn parse_body(body: &str, auction_id: AuctionId) -> Result<UpdateAuctionCommand, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }
    let data = serde_json::from_str::<UpdateAuctionData>(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))?;
    data.into_command(auction_id)
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
