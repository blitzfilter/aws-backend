use crate::{
    ports::{
        AuctionCatalogueCursor, AuctionCatalogueReadError, AuctionCatalogueReadRequest,
        AuctionCatalogueReader, AuctionCatalogueReaderFactory,
    },
    use_cases::{
        PersonalizedProductListingDetailsView, present_product_details, redact_hidden_product,
    },
};
use application::{
    error::{BoxError, box_error},
    operation_context::{OperationContext, Principal},
    pagination::{Cursor, CursoredResult},
    transaction::{Transaction, UnitOfWork},
};
use auction_core::AuctionId;
use auction_service::ports::{
    AuctionSummary, PublicAuctionDetailsReadError, PublicAuctionDetailsReader,
};

use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use localization::Language;
use money::Currency;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing::ListingSaleObservation,
};
use std::collections::HashMap;
use time::OffsetDateTime;

const MAX_CATALOGUE_SIZE: u64 = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct GetAuctionCatalogueRequest {
    pub auction_id: AuctionId,
    pub language: Language,
    pub currency: Currency,
    pub cursor: Option<Cursor<AuctionCatalogueCursor>>,
}

#[derive(Debug, thiserror::Error)]
pub enum GetAuctionCatalogueError {
    #[error("Auction not found")]
    AuctionNotFound,
    #[error("Auction catalogue cursor belongs to another Auction")]
    CursorScopeMismatch,
    #[error("Auction catalogue unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("Auction catalogue invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("no persisted FX snapshot is available for catalogue pricing")]
    PricingFxSnapshotMissing,
    #[error("catalogue price presentation failed")]
    PricingPresentationFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait GetAuctionCatalogueUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAuctionCatalogueRequest,
    ) -> Result<
        CursoredResult<PersonalizedProductListingDetailsView, AuctionCatalogueCursor>,
        GetAuctionCatalogueError,
    >;
}

pub struct GetAuctionCatalogueHandler<U, C, F, A> {
    unit_of_work: U,
    catalogue: C,
    fx_rates: F,
    auctions: A,
}

impl<U, C, F, A> GetAuctionCatalogueHandler<U, C, F, A> {
    pub fn new(unit_of_work: U, catalogue: C, fx_rates: F, auctions: A) -> Self {
        Self {
            unit_of_work,
            catalogue,
            fx_rates,
            auctions,
        }
    }
}

#[async_trait::async_trait]
impl<U, C, F, A> GetAuctionCatalogueUseCase for GetAuctionCatalogueHandler<U, C, F, A>
where
    U: UnitOfWork,
    C: AuctionCatalogueReaderFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    A: PublicAuctionDetailsReader,
{
    #[tracing::instrument(name = "get_auction_catalogue", skip_all, fields(auction_id = %request.auction_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAuctionCatalogueRequest,
    ) -> Result<
        CursoredResult<PersonalizedProductListingDetailsView, AuctionCatalogueCursor>,
        GetAuctionCatalogueError,
    > {
        let auction = self
            .auctions
            .find_by_id(request.auction_id)
            .await?
            .ok_or(GetAuctionCatalogueError::AuctionNotFound)?;
        let cursor = validate_cursor(request.auction_id, request.cursor)?;

        let mut transaction = self.unit_of_work.begin().await.map_err(|source| {
            GetAuctionCatalogueError::TemporarilyUnavailable {
                source: box_error(source),
            }
        })?;
        let page = self
            .catalogue
            .in_transaction(&mut transaction)
            .list(&AuctionCatalogueReadRequest {
                auction_id: request.auction_id,
                language: request.language,
                user_id: personalization_user_id(&context.principal),
                cursor,
            })
            .await?;
        let items = present_catalogue_items(
            page.items,
            &self.fx_rates,
            &mut transaction,
            request.currency,
            AuctionSummary {
                auction_id: auction.auction_id,
                name: auction.name,
                format: auction.format,
                reported_status: auction.reported_status,
                schedule: auction.schedule,
            },
        )
        .await?;
        transaction.commit().await.map_err(|source| {
            GetAuctionCatalogueError::TemporarilyUnavailable {
                source: box_error(source),
            }
        })?;

        Ok(CursoredResult {
            items,
            cursor: page.cursor,
            total: page.total,
        })
    }
}

async fn present_catalogue_items<Tx, F>(
    factual_items: Vec<crate::ports::PersonalizedProductListingDetailsReadModel>,
    fx_rates: &F,
    transaction: &mut Tx,
    currency: Currency,
    auction_summary: AuctionSummary,
) -> Result<Vec<PersonalizedProductListingDetailsView>, GetAuctionCatalogueError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    if factual_items.is_empty() {
        return Ok(Vec::new());
    }
    let sale_snapshot_ids = factual_items
        .iter()
        .filter_map(|item| applicable_sale_observation(&item.item).map(|sale| sale.fx_rate_id()))
        .collect::<Vec<_>>();
    let requires_current_snapshot = factual_items
        .iter()
        .any(|item| applicable_sale_observation(&item.item).is_none());
    let mut repository = fx_rates.in_transaction(transaction);
    let current_snapshot = if requires_current_snapshot {
        repository
            .find_latest_at_or_before(OffsetDateTime::now_utc())
            .await?
            .ok_or(GetAuctionCatalogueError::PricingFxSnapshotMissing)
            .map(Some)?
    } else {
        None
    };
    let sale_snapshots = repository.find_by_ids(&sale_snapshot_ids).await?;
    let sale_snapshots = sale_snapshots
        .into_iter()
        .map(|snapshot| (snapshot.id(), snapshot))
        .collect::<HashMap<_, _>>();

    factual_items
        .into_iter()
        .map(|factual| {
            let snapshot = applicable_sale_observation(&factual.item)
                .map(|sale| {
                    sale_snapshots
                        .get(&sale.fx_rate_id())
                        .ok_or(GetAuctionCatalogueError::PricingFxSnapshotMissing)
                })
                .transpose()?
                .or(current_snapshot.as_ref())
                .ok_or(GetAuctionCatalogueError::PricingFxSnapshotMissing)?;
            let mut view =
                present_product_details(factual, snapshot, currency).map_err(|source| {
                    GetAuctionCatalogueError::PricingPresentationFailed {
                        source: box_error(source),
                    }
                })?;
            view.item.auction_summary = Some(auction_summary.clone());
            if view
                .user_state
                .as_ref()
                .is_some_and(|state| state.search_filter.hidden)
            {
                redact_hidden_product(&mut view.item).map_err(|error| {
                    GetAuctionCatalogueError::PricingPresentationFailed {
                        source: box_error(error),
                    }
                })?;
            }
            Ok(view)
        })
        .collect()
}

fn validate_cursor(
    auction_id: AuctionId,
    cursor: Option<Cursor<AuctionCatalogueCursor>>,
) -> Result<Cursor<AuctionCatalogueCursor>, GetAuctionCatalogueError> {
    let mut cursor = cursor.unwrap_or_default();
    cursor.size = cursor.size.clamp(1, MAX_CATALOGUE_SIZE);
    if cursor
        .search_after
        .is_some_and(|search_after| search_after.auction_id != auction_id)
    {
        return Err(GetAuctionCatalogueError::CursorScopeMismatch);
    }
    Ok(cursor)
}

fn applicable_sale_observation(
    item: &crate::ports::ProductListingDetailsReadModel,
) -> Option<ListingSaleObservation> {
    if item.availability == Some(ListingAvailability::SoldOut) {
        item.sale_observation
    } else {
        None
    }
}

fn personalization_user_id(principal: &Principal) -> Option<user_core::user_id::UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

impl From<AuctionCatalogueReadError> for GetAuctionCatalogueError {
    fn from(value: AuctionCatalogueReadError) -> Self {
        match value {
            AuctionCatalogueReadError::QueryFailed { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AuctionCatalogueReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
        }
    }
}

impl From<PublicAuctionDetailsReadError> for GetAuctionCatalogueError {
    fn from(value: PublicAuctionDetailsReadError) -> Self {
        match value {
            PublicAuctionDetailsReadError::QueryFailed { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PublicAuctionDetailsReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
        }
    }
}

impl From<FxRateSnapshotRepositoryError> for GetAuctionCatalogueError {
    fn from(error: FxRateSnapshotRepositoryError) -> Self {
        match error {
            FxRateSnapshotRepositoryError::ReadFailed { source }
            | FxRateSnapshotRepositoryError::InsertFailed { source } => {
                Self::TemporarilyUnavailable { source }
            }
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
                Self::InvalidReadModel { source }
            }
            FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => Self::PricingFxSnapshotMissing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_core::product_listing_id::ProductListingId;

    #[test]
    fn should_reject_a_catalogue_cursor_for_another_auction() {
        let requested_auction = AuctionId::new();
        let cursor = Cursor {
            size: 21,
            search_after: Some(AuctionCatalogueCursor {
                auction_id: AuctionId::new(),
                catalogue_position: Some(1),
                product_listing_id: ProductListingId::new(),
            }),
        };

        assert!(matches!(
            validate_cursor(requested_auction, Some(cursor)),
            Err(GetAuctionCatalogueError::CursorScopeMismatch)
        ));
    }

    #[test]
    fn should_bound_a_catalogue_cursor_for_its_auction() {
        let auction_id = AuctionId::new();
        let result = validate_cursor(
            auction_id,
            Some(Cursor {
                size: 0,
                search_after: Some(AuctionCatalogueCursor {
                    auction_id,
                    catalogue_position: None,
                    product_listing_id: ProductListingId::new(),
                }),
            }),
        );

        assert!(matches!(result, Ok(cursor) if cursor.size == 1));
    }
}
