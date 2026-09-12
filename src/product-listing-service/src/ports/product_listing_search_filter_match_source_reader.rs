use crate::ports::ListingSourceSummary;
use domain_primitives::event_id::EventId;
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    listing_lifecycle::ListingLifecycle,
    product_listing::{ListingSaleObservation, ProductListingAuction, ProductListingPricing},
    product_listing_id::ProductListingId,
    product_listing_image::ProductListingImage,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingSearchFilterMatchSourceEventKind {
    Domain,
    Enrichment,
    Ignored,
}

impl ProductListingSearchFilterMatchSourceEventKind {
    pub fn is_percolation_trigger(self) -> bool {
        matches!(self, Self::Domain | Self::Enrichment)
    }
}

/// Exact immutable ProductListing event reference for batched source reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProductListingSearchFilterMatchSourceRef {
    pub product_listing_id: ProductListingId,
    pub event_id: EventId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingSearchFilterMatchSource {
    /// Stable identifier of the source ProductListing event.
    pub event_id: EventId,
    /// Whether this event type is routed to search-filter percolation.
    pub event_kind: ProductListingSearchFilterMatchSourceEventKind,
    /// Immutable occurrence time from `product_listing_events.event_time`.
    pub origin_event_time: OffsetDateTime,
    /// Current ProductListing event identity. This rejects stale CDC triggers.
    pub current_event_id: EventId,
    /// Monotonic authoritative version for external OpenSearch writes.
    pub projection_version: i64,
    pub product_listing_id: ProductListingId,
    pub product_listing_title_slug_id: ProductListingSlugId,
    pub source: ListingSourceSummary,
    pub source_listing_id: SourceListingId,
    pub product_title: Option<Localized<Language, Title>>,
    pub product_description: Option<Localized<Language, Description>>,
    /// Translations take precedence over the original text for the same language.
    pub titles: HashMap<Language, Title>,
    /// Translations take precedence over the original text for the same language.
    pub descriptions: HashMap<Language, Description>,
    /// Native persisted prices.
    pub pricing: ProductListingPricing,
    /// Explicit observed sold evidence, independent from availability and lifecycle.
    pub sale_observation: Option<ListingSaleObservation>,
    pub availability: Option<ListingAvailability>,
    pub lifecycle: ListingLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub image: Option<ProductListingImage>,
    pub images: IndexSet<ProductListingImage>,
    /// Authoritative embedding, when enrichment completed.
    pub embedding: Option<Vec<f32>>,
    pub auction: Option<ProductListingAuction>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingSearchFilterMatchSourceReadError {
    #[error("product search-filter match source query failed")]
    QueryFailed {
        #[source]
        source: application::error::BoxError,
    },
    #[error("product search-filter match source persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: application::error::BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingSearchFilterMatchSourceReader: Send {
    async fn find_source(
        &mut self,
        event_id: EventId,
        product_listing_id: ProductListingId,
    ) -> Result<
        Option<ProductListingSearchFilterMatchSource>,
        ProductListingSearchFilterMatchSourceReadError,
    >;

    /// Reads exact ProductListing event sources in one batch. Missing refs are absent.
    async fn find_sources(
        &mut self,
        refs: &[ProductListingSearchFilterMatchSourceRef],
    ) -> Result<
        HashMap<ProductListingSearchFilterMatchSourceRef, ProductListingSearchFilterMatchSource>,
        ProductListingSearchFilterMatchSourceReadError,
    > {
        let mut sources = HashMap::new();
        for reference in refs {
            if let Some(source) = self
                .find_source(reference.event_id, reference.product_listing_id)
                .await?
            {
                sources.insert(*reference, source);
            }
        }
        Ok(sources)
    }
}

pub trait ProductListingSearchFilterMatchSourceReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingSearchFilterMatchSourceReader + 'tx;
}
