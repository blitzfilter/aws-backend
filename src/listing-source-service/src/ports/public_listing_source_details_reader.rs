use application::error::BoxError;
use listing_source_core::ListingSourceSlugId;

use crate::use_cases::queries::public_listing_source::PublicListingSourceSummary;

#[derive(Debug, thiserror::Error)]
pub enum PublicListingSourceDetailsReadError {
    #[error("temporary public listing source detail read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid public listing source detail read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal public listing source detail read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PublicListingSourceDetailsReader: Send {
    async fn find_by_slug(
        &mut self,
        slug_id: &ListingSourceSlugId,
    ) -> Result<Option<PublicListingSourceSummary>, PublicListingSourceDetailsReadError>;
}

pub trait PublicListingSourceDetailsReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl PublicListingSourceDetailsReader + 'tx;
}
