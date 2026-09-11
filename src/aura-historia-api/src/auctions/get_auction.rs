use crate::{
    auctions::types::AuctionAdminData, auth::protected_context, error::ApiError,
    state::AuctionsState, wire::parse_path_object_id,
};
use auction_core::AuctionId;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};

pub async fn get_auction(
    State(state): State<AuctionsState>,
    headers: HeaderMap,
    Path(raw_auction_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let auction_id = match parse_auction_id(&raw_auction_id) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };

    match state.get.execute(&context, auction_id).await {
        Ok(result) => no_store(axum::Json(AuctionAdminData::from(result)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_auction_id(raw: &str) -> Result<AuctionId, ApiError> {
    parse_path_object_id(raw, "auctionId", "Auction")
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn should_reject_invalid_auction_object_id_as_a_path_problem() {
        let error = parse_auction_id("not-an-auction").err();
        assert!(matches!(
            error.as_ref().map(|value| value.code()),
            Some(crate::error::INVALID_OBJECT_ID)
        ));
        assert_eq!(
            StatusCode::BAD_REQUEST,
            error
                .unwrap_or_else(|| panic!("missing error"))
                .into_response()
                .status()
        );
    }
}
