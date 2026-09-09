use crate::ports::ListingSourceSummary;
use application::error::BoxError;
use domain_primitives::event_id::EventId;
use localization::Language;
use product_listing_core::product_listing_price::ProductListingPrice;
use product_listing_core::{
    content_policy::ContentPolicyDecision, listing_availability::ListingAvailability,
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
    product_listing_image::ProductListingImage, product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId, title::Title,
};
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingWatchlistNotificationSource {
    pub event_id: EventId,
    pub event_time: OffsetDateTime,
    pub product_listing_id: ProductListingId,
    pub lifecycle: ListingLifecycle,
    pub product_listing_title_slug_id: ProductListingSlugId,
    pub source: ListingSourceSummary,
    pub source_listing_id: SourceListingId,
    pub title: Option<HashMap<Language, Title>>,
    pub image: Option<ProductListingImage>,
    pub content_policy: Option<ContentPolicyDecision>,
    pub url: Url,
    pub view_url: Url,
    pub changes: Vec<ProductListingWatchlistNotificationChange>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingWatchlistNotificationChange {
    PriceChanged {
        old_price: Option<ProductListingPrice>,
        new_price: Option<ProductListingPrice>,
    },
    AvailabilityChanged {
        old_availability: Option<ListingAvailability>,
        new_availability: Option<ListingAvailability>,
    },
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingWatchlistNotificationSourceReadOutcome {
    Found(ProductListingWatchlistNotificationSource),
    MissingSource,
    IgnoredEvent,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingWatchlistNotificationSourceReadError {
    #[error("watchlist notification source query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("watchlist notification source persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingWatchlistNotificationSourceReader: Send {
    async fn find_source(
        &mut self,
        event_id: EventId,
        product_listing_id: ProductListingId,
    ) -> Result<
        ProductListingWatchlistNotificationSourceReadOutcome,
        ProductListingWatchlistNotificationSourceReadError,
    >;
}

pub trait ProductListingWatchlistNotificationSourceReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingWatchlistNotificationSourceReader + 'tx;
}
