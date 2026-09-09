use std::collections::HashSet;

use domain_primitives::change_outcome::ChangeOutcome;
use party_core::party_id::PartyId;
use url::Url;

use crate::{
    InvalidListingSourceSlug, ListingIngestionMethod, ListingSourceId, ListingSourceName,
    ListingSourceNameError, ListingSourceSlugId, ReferralConfiguration, ReferralUrlError,
    outbound_url,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListingSourcePresentation {
    pub url: Option<Url>,
    pub image: Option<Url>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewListingSource {
    pub id: ListingSourceId,
    pub name: ListingSourceName,
    pub operator_party_id: PartyId,
    pub ingestion_methods: HashSet<ListingIngestionMethod>,
    pub presentation: ListingSourcePresentation,
    pub referral_configuration: Option<ReferralConfiguration>,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedListingSourceState {
    pub id: ListingSourceId,
    pub slug_id: String,
    pub name: String,
    pub operator_party_id: PartyId,
    pub ingestion_methods: HashSet<ListingIngestionMethod>,
    pub presentation: ListingSourcePresentation,
    pub referral_configuration: Option<ReferralConfiguration>,
}

#[derive(Debug, thiserror::Error)]
pub enum RehydrateListingSourceError {
    #[error("invalid persisted listing source slug")]
    InvalidSlug(#[source] InvalidListingSourceSlug),
    #[error("invalid persisted listing source name")]
    InvalidName(#[source] ListingSourceNameError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListingSource {
    id: ListingSourceId,
    slug_id: ListingSourceSlugId,
    name: ListingSourceName,
    operator_party_id: PartyId,
    ingestion_methods: HashSet<ListingIngestionMethod>,
    presentation: ListingSourcePresentation,
    referral_configuration: Option<ReferralConfiguration>,
}

impl ListingSource {
    pub fn create(input: NewListingSource) -> Self {
        let slug_id = ListingSourceSlugId::derive(input.name.as_ref(), input.id);
        Self {
            slug_id,
            id: input.id,
            name: input.name,
            operator_party_id: input.operator_party_id,
            ingestion_methods: input.ingestion_methods,
            presentation: input.presentation,
            referral_configuration: input.referral_configuration,
        }
    }

    #[doc(hidden)]
    pub fn rehydrate(
        state: RehydratedListingSourceState,
    ) -> Result<Self, RehydrateListingSourceError> {
        Ok(Self {
            id: state.id,
            slug_id: ListingSourceSlugId::raw(state.slug_id)
                .map_err(RehydrateListingSourceError::InvalidSlug)?,
            name: ListingSourceName::try_from(state.name)
                .map_err(RehydrateListingSourceError::InvalidName)?,
            operator_party_id: state.operator_party_id,
            ingestion_methods: state.ingestion_methods,
            presentation: state.presentation,
            referral_configuration: state.referral_configuration,
        })
    }

    pub fn rename(&mut self, name: ListingSourceName) -> ChangeOutcome {
        replace(&mut self.name, name)
    }

    pub fn replace_ingestion_methods(
        &mut self,
        methods: HashSet<ListingIngestionMethod>,
    ) -> ChangeOutcome {
        replace(&mut self.ingestion_methods, methods)
    }

    pub fn replace_presentation(
        &mut self,
        presentation: ListingSourcePresentation,
    ) -> ChangeOutcome {
        replace(&mut self.presentation, presentation)
    }

    pub fn replace_referral_configuration(
        &mut self,
        config: Option<ReferralConfiguration>,
    ) -> ChangeOutcome {
        replace(&mut self.referral_configuration, config)
    }

    pub fn referral_url(&self, deeplink: &Url) -> Result<Url, ReferralUrlError> {
        outbound_url(self.referral_configuration.as_ref(), deeplink)
    }

    pub fn id(&self) -> ListingSourceId {
        self.id
    }

    pub fn slug_id(&self) -> &ListingSourceSlugId {
        &self.slug_id
    }

    pub fn name(&self) -> &ListingSourceName {
        &self.name
    }

    pub fn operator_party_id(&self) -> PartyId {
        self.operator_party_id
    }

    pub fn ingestion_methods(&self) -> &HashSet<ListingIngestionMethod> {
        &self.ingestion_methods
    }

    pub fn presentation(&self) -> &ListingSourcePresentation {
        &self.presentation
    }

    pub fn referral_configuration(&self) -> Option<&ReferralConfiguration> {
        self.referral_configuration.as_ref()
    }
}

fn replace<T: PartialEq>(current: &mut T, replacement: T) -> ChangeOutcome {
    if *current == replacement {
        ChangeOutcome::Unchanged
    } else {
        *current = replacement;
        ChangeOutcome::Changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing_source_id() -> ListingSourceId {
        ListingSourceId::try_from(uuid::uuid!("01890a5d-ac96-774b-bf1d-d5586c639f75"))
            .unwrap_or_else(|error| panic!("valid test listing source ID: {error}"))
    }

    fn listing_source_name(value: &str) -> ListingSourceName {
        ListingSourceName::try_from(value)
            .unwrap_or_else(|error| panic!("invalid test listing source name: {error}"))
    }

    #[test]
    fn should_always_disambiguate_slug_when_creating_listing_source() {
        let mut source = ListingSource::create(NewListingSource {
            id: listing_source_id(),
            name: listing_source_name("Antik und Stil"),
            operator_party_id: PartyId::new(),
            ingestion_methods: HashSet::new(),
            presentation: ListingSourcePresentation::default(),
            referral_configuration: None,
        });

        assert_eq!(
            "antik-und-stil-01890a5d-ac96-774b-bf1d-d5586c639f75",
            source.slug_id().as_ref()
        );
        assert!(
            source
                .rename(listing_source_name("Neue Identität"))
                .changed()
        );
        assert_eq!(
            "antik-und-stil-01890a5d-ac96-774b-bf1d-d5586c639f75",
            source.slug_id().as_ref()
        );
    }

    #[test]
    fn should_disambiguate_fallback_slug_when_listing_source_name_has_no_slug_characters() {
        let listing_source_id = listing_source_id();
        let source = ListingSource::create(NewListingSource {
            id: listing_source_id,
            name: listing_source_name("\u{10FFFF}"),
            operator_party_id: PartyId::new(),
            ingestion_methods: HashSet::new(),
            presentation: ListingSourcePresentation::default(),
            referral_configuration: None,
        });

        assert_eq!(
            "listing-source-01890a5d-ac96-774b-bf1d-d5586c639f75",
            source.slug_id().as_ref()
        );
    }

    #[test]
    fn should_reject_invalid_persisted_listing_source_name_and_slug() {
        let state = RehydratedListingSourceState {
            id: ListingSourceId::new(),
            slug_id: "historic--source".to_owned(),
            name: "\u{2003}".to_owned(),
            operator_party_id: PartyId::new(),
            ingestion_methods: HashSet::new(),
            presentation: ListingSourcePresentation::default(),
            referral_configuration: None,
        };

        assert!(matches!(
            ListingSource::rehydrate(state),
            Err(RehydrateListingSourceError::InvalidSlug(_))
        ));

        let state = RehydratedListingSourceState {
            id: ListingSourceId::new(),
            slug_id: "historic-source".to_owned(),
            name: "\u{2003}".to_owned(),
            operator_party_id: PartyId::new(),
            ingestion_methods: HashSet::new(),
            presentation: ListingSourcePresentation::default(),
            referral_configuration: None,
        };

        assert!(matches!(
            ListingSource::rehydrate(state),
            Err(RehydrateListingSourceError::InvalidName(_))
        ));
    }
}
