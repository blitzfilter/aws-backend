use super::types::PartyData;
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::PartiesState;
use crate::wire::parse_path_object_id;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use party_core::party_id::PartyId;
use party_service::use_cases::queries::get_party::GetPartyRequest;

pub async fn get_party(
    State(state): State<PartiesState>,
    headers: HeaderMap,
    Path(raw_party_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let party_id = match parse_party_id(&raw_party_id) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };

    match state
        .get_party
        .execute(&context, GetPartyRequest::ById(party_id))
        .await
    {
        Ok(result) => no_store(Json(PartyData::from(result)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_party_id(raw: &str) -> Result<PartyId, ApiError> {
    parse_path_object_id(raw, "partyId", "Party")
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
    fn should_parse_party_id_from_typed_path_value() {
        let expected = PartyId::new();
        let party_id = parse_party_id(&expected.to_string())
            .unwrap_or_else(|error| panic!("failed to parse party ID: {error}"));

        assert_eq!(expected, party_id);
    }

    #[test]
    fn should_report_invalid_party_object_id_as_path_problem() {
        let error = match parse_party_id("not-an-object-id") {
            Ok(_) => panic!("invalid party ID was accepted"),
            Err(error) => error,
        };

        assert_eq!(crate::error::INVALID_OBJECT_ID, error.code());
        let response = error.into_response();
        assert_eq!(StatusCode::BAD_REQUEST, response.status());
    }
}
