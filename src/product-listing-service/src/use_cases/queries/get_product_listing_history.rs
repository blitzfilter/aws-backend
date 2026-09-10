use crate::ports::{
    ProductListingHistoryReadError, ProductListingHistoryReader, ProductListingHistoryReaderFactory,
};
use application::{
    error::BoxError,
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::Price;
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    product_listing::{ListingSaleObservation, ProductListingAuction, ProductListingPricing},
    product_listing_event::ProductListingEventType,
    product_listing_id::ProductListingId,
    product_listing_price::ProductListingPrice,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingHistoryLookup {
    ById(ProductListingId),
    ByTitleSlug(ProductListingSlugId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetProductListingHistoryRequest {
    pub lookup: ProductListingHistoryLookup,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingHistoryEntry {
    pub product_listing_id: ProductListingId,
    pub event_id: EventId,
    pub occurred_at: OffsetDateTime,
    pub kind: ProductListingHistoryEntryKind,
}

impl ProductListingHistoryEntry {
    pub const fn event_type(&self) -> ProductListingEventType {
        self.kind.event_type()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingHistoryEntryKind {
    Discovered(Box<ProductListingDiscoveryHistory>),
    Changed(ProductListingHistoryChanges),
}

impl ProductListingHistoryEntryKind {
    pub const fn event_type(&self) -> ProductListingEventType {
        match self {
            Self::Discovered(_) => ProductListingEventType::Discovered,
            Self::Changed(_) => ProductListingEventType::Changed,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingDiscoveryHistory {
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductListingPricing,
    pub availability: Option<ListingAvailability>,
    pub url: Url,
    pub image_count: u64,
    pub auction: ProductListingAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingHistoryChanges(Vec<ProductListingHistoryChange>);

impl ProductListingHistoryChanges {
    pub fn as_slice(&self) -> &[ProductListingHistoryChange] {
        &self.0
    }

    pub fn into_inner(self) -> Vec<ProductListingHistoryChange> {
        self.0
    }
}

#[derive(Debug, thiserror::Error)]
#[error("ProductListing history changes must not be empty")]
pub struct EmptyProductListingHistoryChangesError;

impl TryFrom<Vec<ProductListingHistoryChange>> for ProductListingHistoryChanges {
    type Error = EmptyProductListingHistoryChangesError;

    fn try_from(changes: Vec<ProductListingHistoryChange>) -> Result<Self, Self::Error> {
        if changes.is_empty() {
            return Err(EmptyProductListingHistoryChangesError);
        }

        Ok(Self(changes))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingHistoryChange {
    MainPriceChanged {
        previous: Option<ProductListingPrice>,
        current: Option<ProductListingPrice>,
    },
    MinimumEstimateChanged {
        previous: Option<Price>,
        current: Option<Price>,
    },
    MaximumEstimateChanged {
        previous: Option<Price>,
        current: Option<Price>,
    },
    AvailabilityChanged {
        previous: Option<ListingAvailability>,
        current: Option<ListingAvailability>,
    },
    UrlChanged {
        previous: Url,
        current: Url,
    },
    ImagesChanged {
        previous_count: u64,
        current_count: u64,
    },
    AuctionChanged {
        previous: ProductListingAuction,
        current: ProductListingAuction,
    },
    Withdrawn {
        previous_availability: Option<ListingAvailability>,
    },
    Restored,
    SaleObserved {
        observation: ListingSaleObservation,
    },
    SaleObservationRetracted {
        observation: ListingSaleObservation,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum GetProductListingHistoryError {
    #[error("product listing not found")]
    NotFound,
    #[error("product listing history query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product listing history read model is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin product listing history transaction")]
    BeginTransactionFailed,
    #[error("failed to commit product listing history transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GetProductListingHistoryUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductListingHistoryRequest,
    ) -> Result<Vec<ProductListingHistoryEntry>, GetProductListingHistoryError>;
}

pub struct GetProductListingHistoryHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> GetProductListingHistoryHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> GetProductListingHistoryUseCase for GetProductListingHistoryHandler<U, R>
where
    U: UnitOfWork,
    R: ProductListingHistoryReaderFactory<U::Tx>,
{
    #[tracing::instrument(name = "get_product_listing_history", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductListingHistoryRequest,
    ) -> Result<Vec<ProductListingHistoryEntry>, GetProductListingHistoryError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetProductListingHistoryError::BeginTransactionFailed)?;
        let history = self
            .reader
            .in_transaction(&mut tx)
            .find_history(&request.lookup)
            .await?
            .ok_or(GetProductListingHistoryError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| GetProductListingHistoryError::CommitTransactionFailed)?;
        Ok(history)
    }
}

impl From<ProductListingHistoryReadError> for GetProductListingHistoryError {
    fn from(error: ProductListingHistoryReadError) -> Self {
        match error {
            ProductListingHistoryReadError::ProductListingHistoryQueryFailed { source } => {
                Self::QueryFailed { source }
            }
            ProductListingHistoryReadError::ProductListingHistoryReadModelInvalid { source } => {
                Self::InvalidReadModel { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_an_empty_history_change_set() {
        let result = ProductListingHistoryChanges::try_from(Vec::new());

        assert!(matches!(
            result,
            Err(EmptyProductListingHistoryChangesError)
        ));
    }
}
