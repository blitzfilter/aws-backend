use domain_primitives::object_id::ObjectIdError;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use listing_source_service::use_cases::queries::public_listing_source::{
    PublicListingSourceOperatorSummary, PublicListingSourceSummary,
};
use party_core::party_name::PartyName;
use url::Url;

#[derive(Debug, sqlx::FromRow)]
pub(super) struct PublicListingSourceRow {
    pub(super) listing_source_id: uuid::Uuid,
    pub(super) listing_source_slug_id: String,
    pub(super) name: String,
    pub(super) operator_name: String,
    pub(super) url: Option<String>,
    pub(super) image: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum PublicListingSourceMappingError {
    #[error("invalid public ListingSource ID persisted")]
    ListingSourceId(#[source] ObjectIdError),
    #[error("invalid public ListingSource slug persisted")]
    ListingSourceSlug(#[source] listing_source_core::InvalidListingSourceSlug),
    #[error("invalid public ListingSource name persisted")]
    ListingSourceName(#[source] listing_source_core::ListingSourceNameError),
    #[error("invalid public operator name persisted")]
    OperatorName(#[source] party_core::party_name::PartyNameError),
    #[error("invalid public presentation URL persisted")]
    Url(#[source] url::ParseError),
    #[error("unsafe public presentation URL persisted")]
    UnsafeUrl,
}

pub(super) fn map_public_listing_source(
    row: PublicListingSourceRow,
) -> Result<PublicListingSourceSummary, PublicListingSourceMappingError> {
    Ok(PublicListingSourceSummary {
        listing_source_id: ListingSourceId::try_from(row.listing_source_id)
            .map_err(PublicListingSourceMappingError::ListingSourceId)?,
        listing_source_slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id)
            .map_err(PublicListingSourceMappingError::ListingSourceSlug)?,
        name: ListingSourceName::try_from(row.name)
            .map_err(PublicListingSourceMappingError::ListingSourceName)?,
        operator: PublicListingSourceOperatorSummary {
            name: PartyName::try_from(row.operator_name)
                .map_err(PublicListingSourceMappingError::OperatorName)?,
        },
        url: map_public_url(row.url)?,
        image: map_public_url(row.image)?,
    })
}

fn map_public_url(value: Option<String>) -> Result<Option<Url>, PublicListingSourceMappingError> {
    value
        .map(|value| {
            let url = Url::parse(&value).map_err(PublicListingSourceMappingError::Url)?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(PublicListingSourceMappingError::UnsafeUrl);
            }
            Ok(url)
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(url: Option<&str>, image: Option<&str>) -> PublicListingSourceRow {
        PublicListingSourceRow {
            listing_source_id: uuid::Uuid::now_v7(),
            listing_source_slug_id: "source".to_owned(),
            name: "Source".to_owned(),
            operator_name: "Operator".to_owned(),
            url: url.map(ToOwned::to_owned),
            image: image.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn should_accept_safe_public_presentation_urls() {
        let summary = map_public_listing_source(row(
            Some("https://example.test/source"),
            Some("http://example.test/image"),
        ));

        assert!(summary.is_ok());
    }

    #[test]
    fn should_reject_unsafe_public_presentation_urls() {
        for value in [
            "ftp://example.test/source",
            "https://user@example.test/source",
            "https://user:password@example.test/source",
        ] {
            assert!(matches!(
                map_public_listing_source(row(Some(value), None)),
                Err(PublicListingSourceMappingError::UnsafeUrl)
            ));
        }
    }

    #[test]
    fn should_reject_wrong_version_public_listing_source_id() {
        let mut invalid = row(None, None);
        invalid.listing_source_id = uuid::Uuid::new_v4();

        assert!(matches!(
            map_public_listing_source(invalid),
            Err(PublicListingSourceMappingError::ListingSourceId(_))
        ));
    }
}
