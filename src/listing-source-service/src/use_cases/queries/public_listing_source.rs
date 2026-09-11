use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use party_core::party_name::PartyName;
use url::Url;

/// Safe ListingSource card shared by public collection and slug reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicListingSourceSummary {
    pub listing_source_id: ListingSourceId,
    pub listing_source_slug_id: ListingSourceSlugId,
    pub name: ListingSourceName,
    pub operator: PublicListingSourceOperatorSummary,
    pub url: Option<Url>,
    pub image: Option<Url>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicListingSourceOperatorSummary {
    pub name: PartyName,
}
