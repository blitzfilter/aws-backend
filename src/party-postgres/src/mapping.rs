use domain_primitives::object_id::ObjectIdError;
use party_core::{
    party::{Party, PartyContact, RehydratedPartyState},
    party_id::PartyId,
    party_name::PartyName,
    party_slug_id::PartySlugId,
    sort_party_field::SortPartyField,
};
use party_service::ports::{PartyStorageVersion, StoredParty};
use party_service::use_cases::queries::search_parties::PartySummary;
use serde_email::Email;
use time::OffsetDateTime;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct PartyRow {
    party_id: uuid::Uuid,
    party_slug_id: String,
    name: String,
    phone: Option<String>,
    email: Option<String>,
    version: i64,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PartyRowMappingError {
    #[error("invalid party ID persisted")]
    PartyId(#[source] ObjectIdError),
    #[error("invalid party name persisted")]
    Name,
    #[error("invalid party slug persisted")]
    Slug,
    #[error("invalid party email persisted")]
    Email,
    #[error("invalid party version persisted")]
    Version,
}

const PARTY_COLUMNS: &str = r#"
    party_id, party_slug_id, name, phone, email, version, created, updated
"#;

pub(crate) fn party_columns() -> &'static str {
    PARTY_COLUMNS
}

pub(crate) fn sort_party_field_columns(field: SortPartyField) -> &'static [&'static str] {
    match field {
        SortPartyField::Name => &["name"],
        SortPartyField::Email => &["email"],
        SortPartyField::Phone => &["phone"],
        SortPartyField::Created => &["created"],
        SortPartyField::Updated => &["updated"],
    }
}

pub(crate) fn version_to_i64(version: PartyStorageVersion) -> Result<i64, PartyRowMappingError> {
    i64::try_from(version.into_inner()).map_err(|_| PartyRowMappingError::Version)
}

impl TryFrom<PartyRow> for StoredParty {
    type Error = PartyRowMappingError;

    fn try_from(row: PartyRow) -> Result<Self, Self::Error> {
        let version = PartyStorageVersion::try_from(row.version)
            .map_err(|_| PartyRowMappingError::Version)?;
        let party = Party::rehydrate(RehydratedPartyState {
            id: PartyId::try_from(row.party_id).map_err(PartyRowMappingError::PartyId)?,
            slug_id: row.party_slug_id,
            name: PartyName::try_from(row.name).map_err(|_| PartyRowMappingError::Name)?,
            contact: PartyContact {
                phone: row.phone,
                email: row
                    .email
                    .as_deref()
                    .map(Email::try_from)
                    .transpose()
                    .map_err(|_| PartyRowMappingError::Email)?,
            },
        })
        .map_err(|_| PartyRowMappingError::Slug)?;

        Ok(StoredParty {
            party,
            version,
            created: row.created,
            updated: row.updated,
        })
    }
}

impl TryFrom<PartyRow> for PartySummary {
    type Error = PartyRowMappingError;

    fn try_from(row: PartyRow) -> Result<Self, Self::Error> {
        PartyStorageVersion::try_from(row.version).map_err(|_| PartyRowMappingError::Version)?;

        Ok(Self {
            party_id: PartyId::try_from(row.party_id).map_err(PartyRowMappingError::PartyId)?,
            party_slug_id: PartySlugId::raw(row.party_slug_id)
                .map_err(|_| PartyRowMappingError::Slug)?,
            name: PartyName::try_from(row.name).map_err(|_| PartyRowMappingError::Name)?,
            contact: PartyContact {
                phone: row.phone,
                email: row
                    .email
                    .as_deref()
                    .map(Email::try_from)
                    .transpose()
                    .map_err(|_| PartyRowMappingError::Email)?,
            },
            created: row.created,
            updated: row.updated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn row(party_id: uuid::Uuid) -> PartyRow {
        PartyRow {
            party_id,
            party_slug_id: "valid-party".to_owned(),
            name: "Valid party".to_owned(),
            phone: None,
            email: None,
            version: 1,
            created: datetime!(2026-01-01 00:00 UTC),
            updated: datetime!(2026-01-01 00:00 UTC),
        }
    }

    #[test]
    fn should_reject_wrong_version_party_id_for_aggregate_mapping() {
        assert!(matches!(
            StoredParty::try_from(row(uuid::Uuid::new_v4())),
            Err(PartyRowMappingError::PartyId(_))
        ));
    }

    #[test]
    fn should_reject_wrong_version_party_id_for_summary_mapping() {
        assert!(matches!(
            PartySummary::try_from(row(uuid::Uuid::new_v4())),
            Err(PartyRowMappingError::PartyId(_))
        ));
    }
}
