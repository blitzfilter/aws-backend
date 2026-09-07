use application::error::BoxError;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};

#[derive(Debug, thiserror::Error)]
pub enum ProductListingLifecycleGuardError {
    #[error("product listing lifecycle lock failed")]
    LockFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted product listing lifecycle is invalid")]
    InvalidListingLifecyclePersisted,
}

/// Locks the authoritative ProductListing row until its surrounding transaction ends.
#[async_trait::async_trait]
pub trait ProductListingLifecycleGuard: Send {
    /// Returns the locked lifecycle, or `None` when the ProductListing does not exist.
    async fn lock_and_find_lifecycle(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ListingLifecycle>, ProductListingLifecycleGuardError>;
}

pub trait ProductListingLifecycleGuardFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingLifecycleGuard + 'tx;
}
