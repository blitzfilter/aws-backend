use crate::{
    error::{ApiError, BAD_BODY_VALUE},
    wire::parse_body_object_id,
};
use listing_source_core::{
    ListingIngestionMethod, ListingSourceId, ListingSourceName, ListingSourcePresentation,
};
use partnership_core::{
    partnership_application::{
        PartnershipApplication, PartnershipApplicationApprovalResult, PartnershipProposal,
        ProposedListingSource, ProposedParty,
    },
    partnership_application_id::PartnershipApplicationId,
    partnership_application_state::PartnershipApplicationState,
    partnership_id::PartnershipId,
};
use partnership_service::ports::PartnershipApplicationView;
use partnership_service::use_cases::queries::list_admin_partnership_applications::AdminPartnershipApplicationSummary;
use party_core::{party::PartyContact, party_name::PartyName};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use time::OffsetDateTime;
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SubmitPartnershipApplicationData {
    pub(super) proposal: PartnershipProposalData<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub(super) enum PartnershipProposalData<ListingSourceIdentity = ListingSourceId> {
    #[serde(rename = "EXISTING_LISTING_SOURCE")]
    ExistingListingSource {
        listing_source_id: ListingSourceIdentity,
    },
    #[serde(rename = "PROPOSED_LISTING_SOURCE")]
    ProposedListingSource {
        party: ProposedPartyData,
        listing_source: Box<ProposedListingSourceData>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProposedPartyData {
    pub(super) name: String,
    #[serde(default)]
    pub(super) phone: Option<String>,
    #[serde(default)]
    pub(super) email: Option<Email>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProposedListingSourceData {
    pub(super) name: String,
    #[serde(default)]
    pub(super) url: Option<Url>,
    #[serde(default)]
    pub(super) image: Option<Url>,
    #[serde(with = "crate::wire::ingestion_method::set")]
    pub(super) requested_ingestion_methods: std::collections::HashSet<ListingIngestionMethod>,
}

impl TryFrom<PartnershipProposalData<String>> for PartnershipProposal {
    type Error = ApiError;

    fn try_from(value: PartnershipProposalData<String>) -> Result<Self, Self::Error> {
        match value {
            PartnershipProposalData::ExistingListingSource { listing_source_id } => {
                Ok(Self::ExistingListingSource {
                    listing_source_id: parse_body_object_id(
                        &listing_source_id,
                        "proposal.listingSourceId",
                        "ListingSource",
                    )?,
                })
            }
            PartnershipProposalData::ProposedListingSource {
                party,
                listing_source,
            } => Ok(Self::ProposedListingSource {
                party: ProposedParty {
                    name: PartyName::try_from(party.name).map_err(|_| {
                        ApiError::bad_request(BAD_BODY_VALUE).with_detail(
                            "proposal.party.name must be nonblank and at most 255 UTF-8 bytes.",
                        )
                    })?,
                    contact: PartyContact {
                        phone: party.phone,
                        email: party.email,
                    },
                },
                listing_source: ProposedListingSource {
                    name: ListingSourceName::try_from(listing_source.name).map_err(|_| {
                        ApiError::bad_request(BAD_BODY_VALUE).with_detail(
                            "proposal.listingSource.name must be nonblank and at most 255 UTF-8 bytes.",
                        )
                    })?,
                    presentation: ListingSourcePresentation {
                        url: listing_source.url,
                        image: listing_source.image,
                    },
                    requested_ingestion_methods: listing_source.requested_ingestion_methods,
                },
            }),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OwnPartnershipApplicationData {
    id: PartnershipApplicationId,
    #[serde(with = "crate::wire::partnership_application_state")]
    state: PartnershipApplicationState,
    proposal: PartnershipProposalData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AdminPartnershipApplicationData {
    id: PartnershipApplicationId,
    applicant_user_id: UserId,
    #[serde(with = "crate::wire::partnership_application_state")]
    state: PartnershipApplicationState,
    proposal: PartnershipProposalData,
    approved_partnership_id: Option<PartnershipId>,
    approved_listing_source_id: Option<ListingSourceId>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AdminPartnershipApplicationSummaryData {
    id: PartnershipApplicationId,
    applicant_user_id: UserId,
    #[serde(with = "crate::wire::partnership_application_state")]
    state: PartnershipApplicationState,
    proposal: PartnershipProposalData,
    approved_partnership_id: Option<PartnershipId>,
    approved_listing_source_id: Option<ListingSourceId>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

impl From<PartnershipApplication> for OwnPartnershipApplicationData {
    fn from(value: PartnershipApplication) -> Self {
        Self {
            id: value.id(),
            state: value.state(),
            proposal: proposal_data(value.proposal()),
        }
    }
}

impl From<PartnershipApplicationView> for OwnPartnershipApplicationData {
    fn from(value: PartnershipApplicationView) -> Self {
        Self {
            id: value.id,
            state: value.state,
            proposal: proposal_data(&value.proposal),
        }
    }
}

impl From<PartnershipApplication> for AdminPartnershipApplicationData {
    fn from(value: PartnershipApplication) -> Self {
        let (approved_partnership_id, approved_listing_source_id) =
            approval_references(value.approval_result());
        Self {
            id: value.id(),
            applicant_user_id: value.applicant_user_id(),
            state: value.state(),
            proposal: proposal_data(value.proposal()),
            approved_partnership_id,
            approved_listing_source_id,
        }
    }
}

impl From<PartnershipApplicationView> for AdminPartnershipApplicationData {
    fn from(value: PartnershipApplicationView) -> Self {
        let (approved_partnership_id, approved_listing_source_id) =
            approval_references(value.approval_result);
        Self {
            id: value.id,
            applicant_user_id: value.applicant_user_id,
            state: value.state,
            proposal: proposal_data(&value.proposal),
            approved_partnership_id,
            approved_listing_source_id,
        }
    }
}

impl From<AdminPartnershipApplicationSummary> for AdminPartnershipApplicationSummaryData {
    fn from(value: AdminPartnershipApplicationSummary) -> Self {
        Self {
            id: value.id,
            applicant_user_id: value.applicant_user_id,
            state: value.state,
            proposal: proposal_data(&value.proposal),
            approved_partnership_id: value.approved_partnership_id,
            approved_listing_source_id: value.approved_listing_source_id,
            created: value.created,
            updated: value.updated,
        }
    }
}

fn approval_references(
    value: Option<PartnershipApplicationApprovalResult>,
) -> (Option<PartnershipId>, Option<ListingSourceId>) {
    match value {
        Some(result) => (
            Some(result.partnership_id()),
            Some(result.listing_source_id()),
        ),
        None => (None, None),
    }
}

fn proposal_data(value: &PartnershipProposal) -> PartnershipProposalData {
    match value {
        PartnershipProposal::ExistingListingSource { listing_source_id } => {
            PartnershipProposalData::ExistingListingSource {
                listing_source_id: *listing_source_id,
            }
        }
        PartnershipProposal::ProposedListingSource {
            party,
            listing_source,
        } => PartnershipProposalData::ProposedListingSource {
            party: ProposedPartyData {
                name: party.name.to_string(),
                phone: party.contact.phone.clone(),
                email: party.contact.email.clone(),
            },
            listing_source: Box::new(ProposedListingSourceData {
                name: listing_source.name.to_string(),
                url: listing_source.presentation.url.clone(),
                image: listing_source.presentation.image.clone(),
                requested_ingestion_methods: listing_source.requested_ingestion_methods.clone(),
            }),
        },
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum PartnershipApplicationDecisionData {
    Approve,
    Reject,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DecidePartnershipApplicationData {
    pub(super) decision: PartnershipApplicationDecisionData,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_deserialize_proposed_party_and_listing_source_without_shop_fields()
    -> Result<(), serde_json::Error> {
        let request: SubmitPartnershipApplicationData = serde_json::from_value(json!({
            "proposal": {
                "type": "PROPOSED_LISTING_SOURCE",
                "party": { "name": "Ada Antiques", "email": "ada@example.com" },
                "listingSource": {
                    "name": "Ada Antiques",
                    "url": "https://ada.example",
                    "requestedIngestionMethods": ["PARTNER_API"]
                }
            }
        }))?;

        let proposal: PartnershipProposal =
            request.proposal.try_into().map_err(|error: ApiError| {
                serde_json::Error::io(std::io::Error::other(error.to_string()))
            })?;
        assert!(matches!(
            proposal,
            PartnershipProposal::ProposedListingSource { .. }
        ));
        Ok(())
    }

    #[test]
    fn should_serialize_boxed_proposed_listing_source_with_unchanged_json_shape()
    -> Result<(), serde_json::Error> {
        let expected = json!({
            "type": "PROPOSED_LISTING_SOURCE",
            "party": {
                "name": "Ada Antiques",
                "phone": "+1-555-0100",
                "email": "ada@example.com"
            },
            "listingSource": {
                "name": "Ada Antiques",
                "url": "https://ada.example/",
                "image": "https://ada.example/image.png",
                "requestedIngestionMethods": ["PARTNER_API"]
            }
        });
        let proposal: PartnershipProposalData = serde_json::from_value(expected.clone())?;

        assert_eq!(serde_json::to_value(proposal)?, expected);
        Ok(())
    }

    #[test]
    fn should_accept_only_explicit_approve_or_reject_decisions() {
        let approve = serde_json::from_value::<DecidePartnershipApplicationData>(json!({
            "decision": "APPROVE"
        }));
        let reject = serde_json::from_value::<DecidePartnershipApplicationData>(json!({
            "decision": "REJECT"
        }));

        assert!(matches!(
            approve,
            Ok(DecidePartnershipApplicationData {
                decision: PartnershipApplicationDecisionData::Approve
            })
        ));
        assert!(matches!(
            reject,
            Ok(DecidePartnershipApplicationData {
                decision: PartnershipApplicationDecisionData::Reject
            })
        ));

        for value in ["SUBMITTED", "IN_REVIEW", "APPROVED", "REJECTED"] {
            assert!(
                serde_json::from_value::<DecidePartnershipApplicationData>(json!({
                    "decision": value
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn should_reject_legacy_shop_proposal_values() {
        let parsed = serde_json::from_value::<SubmitPartnershipApplicationData>(json!({
            "proposal": { "type": "EXISTING", "shopId": "legacy-shop-id" }
        }));
        assert!(parsed.is_err());
    }
}
