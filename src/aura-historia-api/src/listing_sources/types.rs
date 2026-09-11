use crate::patch_value::{PatchValue, clearable};
use crate::{
    error::{ApiError, BAD_BODY_VALUE},
    wire::parse_body_object_id,
};
use application::patch_field::PatchField;
use listing_source_core::{
    Domain, ListingIngestionMethod, ListingSourceId, ListingSourceName, PartnerizeCamref,
    ReferralConfiguration,
};
use listing_source_service::ports::{
    ListingIngestionConfiguration, ListingSourceDetails, ListingSourceIngestionConfigurations,
};
use listing_source_service::use_cases::commands::{
    create_listing_source::ListingSourceOperator, update_listing_source::RequiredPatch,
};
use listing_source_service::use_cases::queries::public_listing_source::PublicListingSourceSummary;
use listing_source_service::use_cases::queries::search_listing_sources::{
    ListingSourceSearchSummary, SearchListingSourcesResult,
};
use listing_source_service::use_cases::queries::search_public_listing_sources::SearchPublicListingSourcesResult;
use partnership_service::ports::AdministeredListingSource;
use party_core::{
    party::{NewParty, PartyContact},
    party_id::PartyId,
    party_name::PartyName,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use url::Url;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateListingSourceData {
    pub(crate) name: String,
    pub(crate) operator: ListingSourceOperatorData,

    pub(crate) ingestion_configuration: Vec<ListingIngestionConfigurationData>,
    #[serde(default)]
    pub(crate) woocommerce_webhook_secret: Option<String>,
    #[serde(default)]
    pub(crate) url: Option<Url>,
    #[serde(default)]
    pub(crate) image: Option<Url>,
    #[serde(default)]
    pub(crate) referral_configuration: Option<ReferralConfigurationData>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ListingSourceOperatorData {
    Existing {
        #[serde(rename = "partyId")]
        party_id: String,
    },
    New {
        name: String,
        #[serde(default)]
        phone: Option<String>,
        #[serde(default)]
        email: Option<serde_email::Email>,
    },
}

impl TryFrom<ListingSourceOperatorData> for ListingSourceOperator {
    type Error = ApiError;

    fn try_from(value: ListingSourceOperatorData) -> Result<Self, Self::Error> {
        match value {
            ListingSourceOperatorData::Existing { party_id } => Ok(Self::Existing(
                parse_body_object_id(&party_id, "operator.partyId", "Party")?,
            )),
            ListingSourceOperatorData::New { name, phone, email } => Ok(Self::New(NewParty {
                id: PartyId::new(),
                name: PartyName::try_from(name).map_err(|_| {
                    ApiError::bad_request(BAD_BODY_VALUE)
                        .with_detail("operator.name must be nonblank and at most 255 UTF-8 bytes.")
                })?,
                contact: PartyContact { phone, email },
            })),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateListingSourceData {
    #[serde(default)]
    pub(crate) name: PatchValue<String>,

    #[serde(default)]
    pub(crate) ingestion_configuration: PatchValue<Vec<ListingIngestionConfigurationData>>,
    #[serde(default)]
    pub(crate) woocommerce_webhook_secret: PatchValue<String>,
    #[serde(default)]
    pub(crate) url: PatchValue<Url>,
    #[serde(default)]
    pub(crate) image: PatchValue<Url>,
    #[serde(default)]
    pub(crate) referral_configuration: PatchValue<ReferralConfigurationData>,
}

impl UpdateListingSourceData {
    pub(crate) fn into_parts(self) -> Result<UpdateListingSourceDataParts, ApiError> {
        Ok(UpdateListingSourceDataParts {
            name: map_required_patch(self.name, "name", |value| {
                ListingSourceName::try_from(value).map_err(|_| {
                    ApiError::bad_request(BAD_BODY_VALUE)
                        .with_detail("name must be nonblank and at most 255 UTF-8 bytes.")
                })
            })?,

            ingestion_configuration: map_required_patch(
                self.ingestion_configuration,
                "ingestionConfiguration",
                configurations,
            )?,
            woocommerce_webhook_secret: clearable(self.woocommerce_webhook_secret),
            url: clearable(self.url),
            image: clearable(self.image),
            referral_configuration: map_patch_result(
                clearable(self.referral_configuration),
                TryInto::try_into,
            )?,
        })
    }
}

pub(crate) struct UpdateListingSourceDataParts {
    pub(crate) name: RequiredPatch<listing_source_core::ListingSourceName>,

    pub(crate) ingestion_configuration: RequiredPatch<ListingSourceIngestionConfigurations>,
    pub(crate) woocommerce_webhook_secret: PatchField<String>,
    pub(crate) url: PatchField<Url>,
    pub(crate) image: PatchField<Url>,
    pub(crate) referral_configuration: PatchField<ReferralConfiguration>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ListingIngestionConfigurationData {
    #[serde(rename = "WEB_CRAWL")]
    WebCrawl {
        #[serde(default, rename = "fallbackCurrency")]
        fallback_currency: Option<String>,
    },
    #[serde(rename = "SHOPIFY")]
    Shopify {
        domain: String,
        #[serde(default)]
        currency: Option<String>,
        #[serde(default)]
        language: Option<String>,
    },
    #[serde(rename = "WOOCOMMERCE")]
    Woocommerce {
        #[serde(default)]
        currency: Option<String>,
        #[serde(default)]
        language: Option<String>,
    },
    #[serde(rename = "PARTNER_API")]
    PartnerApi,
}

impl TryFrom<ListingIngestionConfigurationData> for ListingIngestionConfiguration {
    type Error = ApiError;

    fn try_from(value: ListingIngestionConfigurationData) -> Result<Self, Self::Error> {
        match value {
            ListingIngestionConfigurationData::WebCrawl { fallback_currency } => {
                Ok(Self::WebCrawl {
                    fallback_currency: parse_currency(fallback_currency)?,
                })
            }
            ListingIngestionConfigurationData::Shopify {
                domain,
                currency,
                language,
            } => Ok(Self::Shopify {
                domain: Domain::try_from(domain).map_err(|_| invalid_body("domain"))?,
                currency: parse_currency(currency)?,
                language: parse_language(language)?,
            }),
            ListingIngestionConfigurationData::Woocommerce { currency, language } => {
                Ok(Self::Woocommerce {
                    currency: parse_currency(currency)?,
                    language: parse_language(language)?,
                })
            }
            ListingIngestionConfigurationData::PartnerApi => Ok(Self::PartnerApi),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub(crate) enum ReferralConfigurationData {
    #[serde(rename = "PARTNERIZE")]
    Partnerize { camref: String },
}

impl From<ReferralConfiguration> for ReferralConfigurationData {
    fn from(value: ReferralConfiguration) -> Self {
        match value {
            ReferralConfiguration::Partnerize { camref } => Self::Partnerize {
                camref: camref.to_string(),
            },
        }
    }
}

impl TryFrom<ReferralConfigurationData> for ReferralConfiguration {
    type Error = ApiError;

    fn try_from(value: ReferralConfigurationData) -> Result<Self, Self::Error> {
        match value {
            ReferralConfigurationData::Partnerize { camref } => Ok(Self::Partnerize {
                camref: PartnerizeCamref::try_from(camref)
                    .map_err(|_| invalid_body("referralConfiguration.camref"))?,
            }),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicListingSourceData {
    listing_source_id: ListingSourceId,
    listing_source_slug_id: String,
    name: String,
    operator: PublicListingSourceOperatorData,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<Url>,
}

impl From<PublicListingSourceSummary> for PublicListingSourceData {
    fn from(value: PublicListingSourceSummary) -> Self {
        Self {
            listing_source_id: value.listing_source_id,
            listing_source_slug_id: value.listing_source_slug_id.to_string(),
            name: value.name.to_string(),
            operator: PublicListingSourceOperatorData {
                name: value.operator.name.to_string(),
            },
            url: value.url,
            image: value.image,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicListingSourceOperatorData {
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublicListingSourceSearchCollectionData {
    items: Vec<PublicListingSourceData>,
    size: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<String>,
}

impl PublicListingSourceSearchCollectionData {
    pub(crate) fn new(
        result: SearchPublicListingSourcesResult,
        search_after: Option<String>,
    ) -> Self {
        Self {
            items: result.items.into_iter().map(Into::into).collect(),
            size: result.page_size,
            search_after,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListingSourceData {
    listing_source_id: ListingSourceId,
    listing_source_slug_id: String,
    name: String,
    operator: OperatorPartyData,
    ingestion_methods: Vec<ListingIngestionMethodData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<Url>,
    #[serde(with = "time::serde::rfc3339")]
    created: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: time::OffsetDateTime,
}

impl From<ListingSourceDetails> for ListingSourceData {
    fn from(value: ListingSourceDetails) -> Self {
        Self {
            listing_source_id: value.listing_source_id,
            listing_source_slug_id: value.slug_id.to_string(),
            name: value.name.to_string(),
            operator: OperatorPartyData {
                party_id: value.operator_party_id,
                party_slug_id: value.operator_slug_id.to_string(),
                name: value.operator_name.to_string(),
            },
            ingestion_methods: methods_data(value.ingestion_methods),
            url: value.url,
            image: value.image,
            created: value.created,
            updated: value.updated,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListingSourceSearchCollectionData {
    pub(crate) items: Vec<ListingSourceSearchSummaryData>,
    pub(crate) size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) search_after: Option<ListingSourceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total: Option<u64>,
}

impl From<SearchListingSourcesResult> for ListingSourceSearchCollectionData {
    fn from(value: SearchListingSourcesResult) -> Self {
        Self {
            items: value
                .items
                .into_iter()
                .map(ListingSourceSearchSummaryData::from)
                .collect(),
            size: value.cursor.size,
            search_after: value.cursor.search_after,
            total: value.total,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListingSourceSearchSummaryData {
    pub(crate) listing_source_id: ListingSourceId,
    pub(crate) listing_source_slug_id: String,
    pub(crate) name: String,
    operator: OperatorPartyData,
    pub(crate) ingestion_methods: Vec<ListingIngestionMethodData>,
    pub(crate) presentation: ListingSourcePresentationData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) referral_configuration: Option<ReferralConfigurationData>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: time::OffsetDateTime,
}

impl From<ListingSourceSearchSummary> for ListingSourceSearchSummaryData {
    fn from(value: ListingSourceSearchSummary) -> Self {
        Self {
            listing_source_id: value.listing_source_id,
            listing_source_slug_id: value.listing_source_slug_id.to_string(),
            name: value.name.to_string(),
            operator: OperatorPartyData {
                party_id: value.operator.party_id,
                party_slug_id: value.operator.party_slug_id.to_string(),
                name: value.operator.name.to_string(),
            },
            ingestion_methods: methods_data(value.ingestion_methods),
            presentation: ListingSourcePresentationData {
                url: value.presentation.url,
                image: value.presentation.image,
            },
            referral_configuration: value.referral_configuration.map(Into::into),
            created: value.created,
            updated: value.updated,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListingSourcePresentationData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) image: Option<Url>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OperatorPartyData {
    party_id: PartyId,
    party_slug_id: String,
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListingSourceReferenceData {
    listing_source_id: ListingSourceId,
    listing_source_slug_id: String,
}

impl From<(ListingSourceId, listing_source_core::ListingSourceSlugId)>
    for ListingSourceReferenceData
{
    fn from(
        (listing_source_id, slug_id): (ListingSourceId, listing_source_core::ListingSourceSlugId),
    ) -> Self {
        Self {
            listing_source_id,
            listing_source_slug_id: slug_id.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdministeredListingSourceData {
    listing_source_id: ListingSourceId,
    listing_source_slug_id: String,
    name: String,
}

impl From<AdministeredListingSource> for AdministeredListingSourceData {
    fn from(value: AdministeredListingSource) -> Self {
        Self {
            listing_source_id: value.listing_source_id,
            listing_source_slug_id: value.slug_id.to_string(),
            name: value.name.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) enum ListingIngestionMethodData {
    #[serde(rename = "WEB_CRAWL")]
    WebCrawl,
    #[serde(rename = "SHOPIFY")]
    Shopify,
    #[serde(rename = "WOOCOMMERCE")]
    Woocommerce,
    #[serde(rename = "PARTNER_API")]
    PartnerApi,
}

impl From<ListingIngestionMethod> for ListingIngestionMethodData {
    fn from(value: ListingIngestionMethod) -> Self {
        match value {
            ListingIngestionMethod::WebCrawl => Self::WebCrawl,
            ListingIngestionMethod::Shopify => Self::Shopify,
            ListingIngestionMethod::Woocommerce => Self::Woocommerce,
            ListingIngestionMethod::PartnerApi => Self::PartnerApi,
        }
    }
}

fn map_required_patch<T, U>(
    value: PatchValue<T>,
    field: &'static str,
    map: impl FnOnce(T) -> Result<U, ApiError>,
) -> Result<RequiredPatch<U>, ApiError> {
    match value {
        PatchValue::Omitted => Ok(RequiredPatch::Unchanged),
        PatchValue::Value(value) => map(value).map(RequiredPatch::Set),
        PatchValue::Null => Err(ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail(format!("Body field '{field}' must not be null."))),
    }
}

fn map_patch_result<T, U>(
    value: PatchField<T>,
    map: impl FnOnce(T) -> Result<U, ApiError>,
) -> Result<PatchField<U>, ApiError> {
    match value {
        PatchField::Unchanged => Ok(PatchField::Unchanged),
        PatchField::Set(value) => map(value).map(PatchField::Set),
        PatchField::Clear => Ok(PatchField::Clear),
    }
}

fn configurations(
    values: Vec<ListingIngestionConfigurationData>,
) -> Result<ListingSourceIngestionConfigurations, ApiError> {
    values
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<_>, _>>()
        .map(ListingSourceIngestionConfigurations)
}

fn methods_data(values: HashSet<ListingIngestionMethod>) -> Vec<ListingIngestionMethodData> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    values.sort_by_key(|value| value.as_str());
    values.into_iter().map(Into::into).collect()
}

fn parse_currency(value: Option<String>) -> Result<Option<money::Currency>, ApiError> {
    value
        .map(|value| money::Currency::from_code(&value).ok_or_else(|| invalid_body("currency")))
        .transpose()
}

fn parse_language(value: Option<String>) -> Result<Option<localization::Language>, ApiError> {
    value
        .map(|value| {
            localization::Language::from_code(&value).ok_or_else(|| invalid_body("language"))
        })
        .transpose()
}

fn invalid_body(field: &str) -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail(format!("Body field '{field}' is invalid."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_decode_canonical_ingestion_method_values() -> Result<(), serde_json::Error> {
        let source: CreateListingSourceData = serde_json::from_str(
            r#"{
                "name":"Source",
                "operator":{"type":"EXISTING","partyId":"pty_01h455vb4pex5vy7enb1p677vn"},
                "ingestionConfiguration":[{"type":"WEB_CRAWL","fallbackCurrency":"ZAR"},{"type":"PARTNER_API"}]
            }"#,
        )?;

        let configuration = source
            .ingestion_configuration
            .into_iter()
            .next()
            .ok_or_else(|| serde_json::Error::io(std::io::Error::other("missing WEB_CRAWL")))?
            .try_into()
            .map_err(|error: ApiError| {
                serde_json::Error::io(std::io::Error::other(error.to_string()))
            })?;
        assert!(matches!(
            configuration,
            ListingIngestionConfiguration::WebCrawl {
                fallback_currency: Some(money::Currency::Zar),
            }
        ));
        Ok(())
    }

    #[test]
    fn should_map_omitted_required_update_fields_as_unchanged() -> Result<(), serde_json::Error> {
        let update: UpdateListingSourceData = serde_json::from_str("{}")?;
        let parts = update
            .into_parts()
            .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))?;

        assert!(matches!(parts.name, RequiredPatch::Unchanged));
        assert!(matches!(
            parts.ingestion_configuration,
            RequiredPatch::Unchanged
        ));
        Ok(())
    }

    #[test]
    fn should_map_set_required_update_fields_without_clear_variant() -> Result<(), serde_json::Error>
    {
        let update: UpdateListingSourceData = serde_json::from_str(
            r#"{
                "name":"Renamed source",
                "ingestionConfiguration":[{"type":"WEB_CRAWL"}]
            }"#,
        )?;
        let parts = update
            .into_parts()
            .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))?;

        assert!(matches!(parts.name, RequiredPatch::Set(_)));
        assert!(matches!(
            parts.ingestion_configuration,
            RequiredPatch::Set(_)
        ));
        Ok(())
    }

    #[test]
    fn should_reject_null_required_update_fields() -> Result<(), serde_json::Error> {
        let name: UpdateListingSourceData = serde_json::from_str(r#"{"name":null}"#)?;
        let configuration: UpdateListingSourceData =
            serde_json::from_str(r#"{"ingestionConfiguration":null}"#)?;

        assert!(matches!(
            name.into_parts(),
            Err(error) if error.code() == BAD_BODY_VALUE
        ));
        assert!(matches!(
            configuration.into_parts(),
            Err(error) if error.code() == BAD_BODY_VALUE
        ));
        Ok(())
    }

    #[test]
    fn should_reject_invalid_listing_source_name_when_mapping_update()
    -> Result<(), serde_json::Error> {
        let update: UpdateListingSourceData = serde_json::from_str(r#"{"name":"\u2003"}"#)?;

        assert!(update.into_parts().is_err());
        Ok(())
    }

    #[test]
    fn should_reject_unsafe_partnerize_camref_when_mapping_transport() {
        let data = ReferralConfigurationData::Partnerize {
            camref: "campaign/ref".to_owned(),
        };

        assert!(ReferralConfiguration::try_from(data).is_err());
    }

    #[test]
    fn should_preserve_valid_partnerize_camref_when_mapping_transport() {
        let data = ReferralConfigurationData::Partnerize {
            camref: "1101l3AbC".to_owned(),
        };

        let configuration = ReferralConfiguration::try_from(data);
        assert!(matches!(
            configuration,
            Ok(ReferralConfiguration::Partnerize { camref }) if camref.as_ref() == "1101l3AbC"
        ));
    }
}
