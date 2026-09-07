#![allow(dead_code)]

use application::error::BoxError;
use domain_primitives::event_id::EventId;
use domain_primitives::versioned::Versioned;
use product_listing_core::{
    product_listing::ProductListing,
    product_listing_event::ProductListingEventPayload,
    product_listing_id::{ProductListingId, ProductListingKey},
};

domain_primitives::version_newtype!(ProductListingStorageVersion, no_serde);

pub type VersionedProductListing = Versioned<ProductListing, ProductListingStorageVersion>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProductListingWriteEffects {
    pub advance_embedding_source: bool,
}

impl From<&ProductListingEventPayload> for ProductListingWriteEffects {
    fn from(payload: &ProductListingEventPayload) -> Self {
        match payload {
            ProductListingEventPayload::Discovered(_) => Self {
                advance_embedding_source: true,
            },
            ProductListingEventPayload::Changed(changes) => Self {
                advance_embedding_source: changes.image_count().is_some(),
            },
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingRepositoryError {
    #[error("product storage version did not match expected version")]
    ConcurrencyConflict,
    #[error("product already exists for source listing identity")]
    SourceListingAlreadyExists,
    #[error("product slug already exists")]
    ProductListingTitleSlugAlreadyExists,
    #[error("product lookup by id failed")]
    ProductListingLookupByIdFailed,
    #[error("product lookup by source listing identity failed")]
    ProductListingLookupByKeyFailed {
        #[source]
        source: BoxError,
    },
    #[error("product insert failed")]
    ProductListingInsertFailed,
    #[error("product update failed")]
    ProductListingUpdateFailed,
    #[error("persisted product slug is invalid")]
    InvalidProductListingSlugPersisted,
    #[error("persisted source listing ID is invalid")]
    InvalidSourceListingIdPersisted,
    #[error("persisted title is incomplete")]
    IncompleteTitlePersisted,
    #[error("persisted title language is invalid")]
    InvalidTitleLanguagePersisted,
    #[error("persisted description is incomplete")]
    IncompleteDescriptionPersisted,
    #[error("persisted description language is invalid")]
    InvalidDescriptionLanguagePersisted,
    #[error("persisted price is incomplete")]
    IncompletePricePersisted,
    #[error("persisted price amount is negative")]
    NegativePriceAmountPersisted,
    #[error("persisted price currency is invalid")]
    InvalidPriceCurrencyPersisted,
    #[error("persisted listing availability is invalid")]
    InvalidListingAvailabilityPersisted,
    #[error("persisted listing lifecycle is invalid")]
    InvalidListingLifecyclePersisted,
    #[error("persisted product URL is invalid")]
    InvalidProductListingUrlPersisted,
    #[error("persisted product images value is invalid")]
    InvalidProductListingImagesPersisted,
    #[error("persisted product image URL is invalid")]
    InvalidProductListingImageUrlPersisted,

    #[error("persisted aggregate state is invalid")]
    InvalidAggregateStatePersisted,
}

#[async_trait::async_trait]
pub trait ProductListingRepository: Send {
    async fn find_by_id(
        &mut self,
        id: ProductListingId,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError>;

    async fn find_by_key(
        &mut self,
        key: &ProductListingKey,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError>;

    async fn insert(
        &mut self,
        product: &ProductListing,
        current_event_id: EventId,
    ) -> Result<VersionedProductListing, ProductListingRepositoryError>;

    async fn update(
        &mut self,
        product: &ProductListing,
        expected_version: ProductListingStorageVersion,
        current_event_id: EventId,
        effects: ProductListingWriteEffects,
    ) -> Result<VersionedProductListing, ProductListingRepositoryError>;
}

pub trait ProductListingRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingRepository + 'tx;
}
