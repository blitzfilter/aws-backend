use super::types::PartyData;
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use crate::state::PartiesState;
use crate::wire::parse_path_object_id;
use application::patch_field::PatchField;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use party_core::party_id::PartyId;
use party_core::party_name::PartyName;
use party_service::use_cases::commands::update_party::UpdatePartyCommand;
use serde::Deserialize;
use serde_email::Email;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdatePartyData {
    #[serde(default)]
    name: PatchValue<String>,
    #[serde(default)]
    phone: PatchValue<String>,
    #[serde(default)]
    email: PatchValue<Email>,
}

struct UpdatePartyInput {
    party_id: PartyId,
    data: UpdatePartyData,
}

impl TryFrom<UpdatePartyInput> for UpdatePartyCommand {
    type Error = ApiError;

    fn try_from(input: UpdatePartyInput) -> Result<Self, Self::Error> {
        let UpdatePartyInput { party_id, data } = input;
        let name = map_name_patch(non_nullable_patch(data.name, "name")?)?;

        Ok(Self {
            party_id,
            name,
            phone: clearable(data.phone),
            email: clearable(data.email),
        })
    }
}

pub async fn update_party(
    State(state): State<PartiesState>,
    headers: HeaderMap,
    Path(raw_party_id): Path<String>,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let party_id = match parse_party_id(&raw_party_id) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let data = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let command = match UpdatePartyCommand::try_from(UpdatePartyInput { party_id, data }) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };

    match state.update_party.execute(&context, command).await {
        Ok(result) => no_store(Json(PartyData::from(result)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_party_id(raw: &str) -> Result<PartyId, ApiError> {
    parse_path_object_id(raw, "partyId", "Party")
}

fn parse_body(body: &str) -> Result<UpdatePartyData, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }
    serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}

fn map_name_patch(value: PatchField<String>) -> Result<PatchField<PartyName>, ApiError> {
    match value {
        PatchField::Unchanged => Ok(PatchField::Unchanged),
        PatchField::Set(value) => PartyName::try_from(value)
            .map(PatchField::Set)
            .map_err(|_| {
                ApiError::bad_request(BAD_BODY_VALUE)
                    .with_detail("name must be nonblank and at most 255 UTF-8 bytes.")
            }),
        PatchField::Clear => Err(ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("Body field 'name' must not be null.")),
    }
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
    use application::patch_field::PatchField;

    #[test]
    fn should_preserve_omitted_party_patch_fields() -> Result<(), ApiError> {
        let party_id = PartyId::new();
        let command = UpdatePartyCommand::try_from(UpdatePartyInput {
            party_id,
            data: parse_body("{}")?,
        })?;

        assert_eq!(party_id, command.party_id);
        assert!(matches!(command.name, PatchField::Unchanged));
        assert!(matches!(command.phone, PatchField::Unchanged));
        assert!(matches!(command.email, PatchField::Unchanged));
        Ok(())
    }

    #[test]
    fn should_map_party_rename_and_contact_values() -> Result<(), ApiError> {
        let command = UpdatePartyCommand::try_from(UpdatePartyInput {
            party_id: PartyId::new(),
            data: parse_body(
                r#"{
                    "name": "  Renamed Party  ",
                    "phone": "+49 30 123456",
                    "email": "renamed@example.com"
                }"#,
            )?,
        })?;

        assert!(matches!(command.name, PatchField::Set(name) if name.as_ref() == "Renamed Party"));
        assert!(matches!(command.phone, PatchField::Set(phone) if phone == "+49 30 123456"));
        assert!(matches!(
            command.email,
            PatchField::Set(email)
                            if <Email as AsRef<str>>::as_ref(&email) == "renamed@example.com"
        ));
        Ok(())
    }

    #[test]
    fn should_map_null_optional_contacts_to_clear() -> Result<(), ApiError> {
        let command = UpdatePartyCommand::try_from(UpdatePartyInput {
            party_id: PartyId::new(),
            data: parse_body(r#"{"phone":null,"email":null}"#)?,
        })?;

        assert!(matches!(command.name, PatchField::Unchanged));
        assert!(matches!(command.phone, PatchField::Clear));
        assert!(matches!(command.email, PatchField::Clear));
        Ok(())
    }

    #[test]
    fn should_reject_null_or_invalid_party_name() {
        for body in [r#"{"name":null}"#, r#"{"name":"  "}"#] {
            let error = match parse_body(body) {
                Ok(data) => match UpdatePartyCommand::try_from(UpdatePartyInput {
                    party_id: PartyId::new(),
                    data,
                }) {
                    Ok(_) => panic!("invalid Party name was accepted"),
                    Err(error) => error,
                },
                Err(error) => error,
            };
            assert_eq!(BAD_BODY_VALUE, error.code());
        }
    }

    #[test]
    fn should_reject_invalid_party_email() {
        let error = match parse_body(r#"{"email":"not-an-email"}"#) {
            Ok(_) => panic!("invalid Party email was accepted"),
            Err(error) => error,
        };

        assert_eq!(BAD_BODY_VALUE, error.code());
    }

    #[test]
    fn should_reject_empty_party_patch_body() {
        let error = match parse_body("  ") {
            Ok(_) => panic!("empty Party patch body was accepted"),
            Err(error) => error,
        };

        assert_eq!(BAD_BODY_VALUE, error.code());
    }

    #[test]
    fn should_report_invalid_party_object_id_as_path_problem() {
        let error = match parse_party_id("not-an-object-id") {
            Ok(_) => panic!("invalid Party ID was accepted"),
            Err(error) => error,
        };

        assert_eq!(crate::error::INVALID_OBJECT_ID, error.code());
    }
}
