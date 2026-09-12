use crate::ports::{
    ProductListingAuctionOverrideRepository, ProductListingAuctionOverrideRepositoryFactory,
    ProductListingAuctionPolicyVersion, ProductListingRepository, ProductListingRepositoryError,
    ProductListingRepositoryFactory, ProductListingStorageVersion,
};
use application::{
    error::{BoxError, box_error},
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use listing_source_core::ListingSourceId;
use product_listing_core::{
    product_listing::ProductListingAuction, product_listing_id::ProductListingId,
};
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingAuctionContextAdminView {
    pub product_listing_id: ProductListingId,
    pub listing_source_id: ListingSourceId,
    pub auction: Option<ProductListingAuction>,
    pub version: ProductListingStorageVersion,
    pub auction_policy_version: ProductListingAuctionPolicyVersion,
    pub auction_context_override_active: bool,
}
#[derive(Debug, thiserror::Error)]
pub enum GetProductListingAuctionContextError {
    #[error("authenticated administrator required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("product listing not found")]
    NotFound,
    #[error("temporary persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin read transaction")]
    BeginTransactionFailed,
    #[error("failed to commit read transaction")]
    CommitTransactionFailed,
}
#[async_trait::async_trait]
pub trait GetProductListingAuctionContextUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
    ) -> Result<ProductListingAuctionContextAdminView, GetProductListingAuctionContextError>;
}
pub struct GetProductListingAuctionContextHandler<U, R, O, C> {
    unit_of_work: U,
    products: R,
    overrides: O,
    check_user_admin: C,
}
impl<U, R, O, C> GetProductListingAuctionContextHandler<U, R, O, C> {
    pub fn new(unit_of_work: U, products: R, overrides: O, check_user_admin: C) -> Self {
        Self {
            unit_of_work,
            products,
            overrides,
            check_user_admin,
        }
    }
}
#[async_trait::async_trait]
impl<U, R, O, C> GetProductListingAuctionContextUseCase
    for GetProductListingAuctionContextHandler<U, R, O, C>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    O: ProductListingAuctionOverrideRepositoryFactory<U::Tx>,
    C: CheckUserAdminUseCase,
{
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
    ) -> Result<ProductListingAuctionContextAdminView, GetProductListingAuctionContextError> {
        self.check_user_admin
            .execute(context, CheckUserAdminRequest)
            .await
            .map_err(map_admin_error)?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetProductListingAuctionContextError::BeginTransactionFailed)?;
        let listing = self
            .products
            .in_transaction(&mut tx)
            .find_by_id(product_listing_id)
            .await?
            .ok_or(GetProductListingAuctionContextError::NotFound)?;
        let policy = self
            .overrides
            .in_transaction(&mut tx)
            .find(product_listing_id)
            .await
            .map_err(|error| GetProductListingAuctionContextError::Internal {
                source: box_error(error),
            })?;
        tx.commit()
            .await
            .map_err(|_| GetProductListingAuctionContextError::CommitTransactionFailed)?;
        Ok(ProductListingAuctionContextAdminView {
            product_listing_id,
            listing_source_id: listing.value.listing_source_id(),
            auction: listing.value.auction().cloned(),
            version: listing.version,
            auction_policy_version: policy
                .map_or(ProductListingAuctionPolicyVersion::default(), |state| {
                    state.version
                }),
            auction_context_override_active: policy.is_some_and(|state| state.active),
        })
    }
}
fn map_admin_error(error: CheckUserAdminError) -> GetProductListingAuctionContextError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            GetProductListingAuctionContextError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => GetProductListingAuctionContextError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            GetProductListingAuctionContextError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source } => {
            GetProductListingAuctionContextError::InvalidPersistedState { source }
        }
        CheckUserAdminError::Internal { source } => {
            GetProductListingAuctionContextError::Internal { source }
        }
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            GetProductListingAuctionContextError::TemporarilyUnavailable {
                source: box_error(error),
            }
        }
    }
}
impl From<ProductListingRepositoryError> for GetProductListingAuctionContextError {
    fn from(error: ProductListingRepositoryError) -> Self {
        match error {
            ProductListingRepositoryError::InvalidAggregateStatePersisted => {
                Self::InvalidPersistedState {
                    source: box_error(error),
                }
            }
            error => Self::Internal {
                source: box_error(error),
            },
        }
    }
}
