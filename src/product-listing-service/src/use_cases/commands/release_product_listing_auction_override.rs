use crate::ports::{
    ProductListingAuctionOverrideError, ProductListingAuctionOverrideRepository,
    ProductListingAuctionOverrideRepositoryFactory, ProductListingAuctionPolicyVersion,
    ProductListingRepository, ProductListingRepositoryError, ProductListingRepositoryFactory,
    ProductListingStorageVersion,
};
use application::{
    error::{BoxError, box_error},
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;
use time::OffsetDateTime;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseProductListingAuctionOverrideCommand {
    pub product_listing_id: ProductListingId,
    pub expected_version: ProductListingStorageVersion,
    pub expected_auction_policy_version: ProductListingAuctionPolicyVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseProductListingAuctionOverrideResult {
    pub product_listing_id: ProductListingId,
    pub version: ProductListingStorageVersion,
    pub auction_policy_version: ProductListingAuctionPolicyVersion,
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseProductListingAuctionOverrideError {
    #[error("authenticated administrator required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("product listing not found")]
    NotFound,
    #[error("expected listing or auction policy state did not match")]
    ConcurrencyConflict,
    #[error("safe raw-observation floor cannot be established")]
    UnsafeRelease,
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
    #[error("internal release failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin release transaction")]
    BeginTransactionFailed,
    #[error("failed to commit release transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ReleaseProductListingAuctionOverrideUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ReleaseProductListingAuctionOverrideCommand,
    ) -> Result<ReleaseProductListingAuctionOverrideResult, ReleaseProductListingAuctionOverrideError>;
}

pub struct ReleaseProductListingAuctionOverrideHandler<U, R, O, C> {
    unit_of_work: U,
    products: R,
    overrides: O,
    check_user_admin: C,
}
impl<U, R, O, C> ReleaseProductListingAuctionOverrideHandler<U, R, O, C> {
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
impl<U, R, O, C> ReleaseProductListingAuctionOverrideUseCase
    for ReleaseProductListingAuctionOverrideHandler<U, R, O, C>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    O: ProductListingAuctionOverrideRepositoryFactory<U::Tx>,
    C: CheckUserAdminUseCase,
{
    #[tracing::instrument(name = "release_product_listing_auction_override", skip_all, fields(product_listing_id = %command.product_listing_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: ReleaseProductListingAuctionOverrideCommand,
    ) -> Result<ReleaseProductListingAuctionOverrideResult, ReleaseProductListingAuctionOverrideError>
    {
        self.check_user_admin
            .execute(context, CheckUserAdminRequest)
            .await
            .map_err(map_admin_error)?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ReleaseProductListingAuctionOverrideError::BeginTransactionFailed)?;
        let listing = self
            .products
            .in_transaction(&mut tx)
            .find_by_id(command.product_listing_id)
            .await?
            .ok_or(ReleaseProductListingAuctionOverrideError::NotFound)?;
        if listing.version != command.expected_version {
            return Err(ReleaseProductListingAuctionOverrideError::ConcurrencyConflict);
        }
        let override_state = self
            .overrides
            .in_transaction(&mut tx)
            .release(
                command.product_listing_id,
                command.expected_auction_policy_version,
                EventId::new(),
                context.principal.label().to_owned(),
                OffsetDateTime::now_utc(),
            )
            .await?;
        tx.commit()
            .await
            .map_err(|_| ReleaseProductListingAuctionOverrideError::CommitTransactionFailed)?;
        tracing::info!(event = "product_listing.auction_override_released", product_listing_id = %command.product_listing_id, outcome = "success");
        Ok(ReleaseProductListingAuctionOverrideResult {
            product_listing_id: command.product_listing_id,
            version: listing.version,
            auction_policy_version: override_state.version,
        })
    }
}
fn map_admin_error(error: CheckUserAdminError) -> ReleaseProductListingAuctionOverrideError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            ReleaseProductListingAuctionOverrideError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => ReleaseProductListingAuctionOverrideError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            ReleaseProductListingAuctionOverrideError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source } => {
            ReleaseProductListingAuctionOverrideError::InvalidPersistedState { source }
        }
        CheckUserAdminError::Internal { source } => {
            ReleaseProductListingAuctionOverrideError::Internal { source }
        }
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            ReleaseProductListingAuctionOverrideError::TemporarilyUnavailable {
                source: box_error(error),
            }
        }
    }
}
impl From<ProductListingRepositoryError> for ReleaseProductListingAuctionOverrideError {
    fn from(error: ProductListingRepositoryError) -> Self {
        match error {
            ProductListingRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
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
impl From<ProductListingAuctionOverrideError> for ReleaseProductListingAuctionOverrideError {
    fn from(error: ProductListingAuctionOverrideError) -> Self {
        match error {
            ProductListingAuctionOverrideError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ProductListingAuctionOverrideError::UnsafeRelease => Self::UnsafeRelease,
            ProductListingAuctionOverrideError::Persistence { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ProductListingAuctionOverrideError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
        }
    }
}
