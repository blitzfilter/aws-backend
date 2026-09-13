use crate::ports::PersonalizedProductListingDetailsReadModel;
use application::{
    error::BoxError,
    pagination::{Cursor, CursoredResult},
};
use auction_core::AuctionId;
use localization::Language;
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuctionCatalogueCursor {
    pub auction_id: AuctionId,
    pub catalogue_position: Option<u32>,
    pub product_listing_id: product_listing_core::product_listing_id::ProductListingId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionCatalogueReadRequest {
    pub auction_id: AuctionId,
    pub language: Language,
    pub user_id: Option<UserId>,
    pub cursor: Cursor<AuctionCatalogueCursor>,
}

pub type AuctionCataloguePage =
    CursoredResult<PersonalizedProductListingDetailsReadModel, AuctionCatalogueCursor>;

#[derive(Debug, thiserror::Error)]
pub enum AuctionCatalogueReadError {
    #[error("Auction catalogue query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("Auction catalogue read model is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AuctionCatalogueReader: Send {
    async fn list(
        &mut self,
        request: &AuctionCatalogueReadRequest,
    ) -> Result<AuctionCataloguePage, AuctionCatalogueReadError>;
}

pub trait AuctionCatalogueReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl AuctionCatalogueReader + 'tx;
}
