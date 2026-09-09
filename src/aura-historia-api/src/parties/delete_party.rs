use crate::{
    auth::protected_context, error::ApiError, state::PartiesState, wire::parse_path_object_id,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use party_core::party_id::PartyId;
use party_service::use_cases::commands::delete_party::DeletePartyCommand;

pub async fn delete_party(
    State(state): State<PartiesState>,
    headers: HeaderMap,
    Path(raw_party_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let party_id: PartyId = match parse_path_object_id(&raw_party_id, "partyId", "Party") {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };

    match state
        .delete_party
        .execute(&context, DeletePartyCommand { party_id })
        .await
    {
        Ok(()) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
