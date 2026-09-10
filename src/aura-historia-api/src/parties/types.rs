use party_core::party_id::PartyId;
use party_service::use_cases::queries::{
    get_party::PartyDetailsView,
    search_parties::{PartySummary, SearchPartiesResult},
};
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartyCollectionData {
    pub(crate) items: Vec<PartySummaryData>,
    pub(crate) size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) search_after: Option<PartyId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total: Option<u64>,
}

impl From<SearchPartiesResult> for PartyCollectionData {
    fn from(result: SearchPartiesResult) -> Self {
        Self {
            items: result
                .items
                .into_iter()
                .map(PartySummaryData::from)
                .collect(),
            size: result.cursor.size,
            search_after: result.cursor.search_after,
            total: result.total,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartyData {
    pub(crate) party_id: PartyId,
    pub(crate) party_slug_id: String,
    pub(crate) name: String,
    pub(crate) contact: PartyContactData,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: OffsetDateTime,
}

impl From<PartyDetailsView> for PartyData {
    fn from(value: PartyDetailsView) -> Self {
        Self {
            party_id: value.party_id,
            party_slug_id: value.party_slug_id.to_string(),
            name: value.name.to_string(),
            contact: PartyContactData {
                phone: value.contact.phone,
                email: value.contact.email.map(|email| email.to_string()),
            },
            created: value.created,
            updated: value.updated,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartySummaryData {
    pub(crate) party_id: PartyId,
    pub(crate) party_slug_id: String,
    pub(crate) name: String,
    pub(crate) contact: PartyContactData,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: OffsetDateTime,
}

impl From<PartySummary> for PartySummaryData {
    fn from(value: PartySummary) -> Self {
        Self {
            party_id: value.party_id,
            party_slug_id: value.party_slug_id.to_string(),
            name: value.name.to_string(),
            contact: PartyContactData {
                phone: value.contact.phone,
                email: value.contact.email.map(|email| email.to_string()),
            },
            created: value.created,
            updated: value.updated,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartyContactData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) email: Option<String>,
}
