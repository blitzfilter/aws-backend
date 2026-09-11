use application::error::BoxError;

use crate::use_cases::queries::search_public_listing_sources::{
    PublicListingSourceSearchPage, SearchPublicListingSourcesRequest,
};

#[derive(Debug, thiserror::Error)]
pub enum PublicListingSourceSearchReadError {
    #[error("temporary public listing source search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid public listing source search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal public listing source search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PublicListingSourceSearchReader: Send {
    async fn search(
        &mut self,
        request: &SearchPublicListingSourcesRequest,
    ) -> Result<PublicListingSourceSearchPage, PublicListingSourceSearchReadError>;
}

pub trait PublicListingSourceSearchReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl PublicListingSourceSearchReader + 'tx;
}
