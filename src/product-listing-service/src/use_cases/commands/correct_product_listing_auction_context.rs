use crate::ports::{
    ProductListingAuctionOverrideAudit, ProductListingAuctionOverrideError,
    ProductListingAuctionOverrideRepository, ProductListingAuctionOverrideRepositoryFactory,
    ProductListingAuctionPolicyVersion, ProductListingEventAppender,
    ProductListingEventAppenderFactory, ProductListingRepository, ProductListingRepositoryError,
    ProductListingRepositoryFactory, ProductListingStorageVersion, ProductListingWriteEffects,
    stamp_product_listing_event,
};
use application::{
    error::{BoxError, box_error},
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use auction_core::AuctionId;
use auction_service::ports::{AuctionRepository, AuctionRepositoryFactory};
use domain_primitives::event_id::EventId;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing::ProductListingAuction,
    product_listing_id::ProductListingId,
};
use time::OffsetDateTime;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

const MAX_CORRECTION_REASON_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct CorrectProductListingAuctionContextCommand {
    pub product_listing_id: ProductListingId,
    pub expected_version: ProductListingStorageVersion,
    pub expected_auction_policy_version: ProductListingAuctionPolicyVersion,
    /// `None` means the caller expects no resolved membership; listing version disambiguates
    /// absent context from unresolved context.
    pub expected_current_auction_id: Option<AuctionId>,
    pub replacement: Option<ProductListingAuction>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectProductListingAuctionContextResult {
    pub product_listing_id: ProductListingId,
    pub version: ProductListingStorageVersion,
    pub auction_policy_version: ProductListingAuctionPolicyVersion,
}

#[derive(Debug, thiserror::Error)]
pub enum CorrectProductListingAuctionContextError {
    #[error("authenticated administrator required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("product listing not found")]
    NotFound,
    #[error("withdrawn product listing cannot be corrected")]
    ListingWithdrawn,
    #[error("expected listing or auction policy state did not match")]
    ConcurrencyConflict,
    #[error("expected current auction membership did not match")]
    CurrentMembershipConflict,
    #[error("replacement auction belongs to another listing source")]
    AuctionSourceMismatch,
    #[error("replacement auction was not found")]
    AuctionNotFound,
    #[error("correction reason is invalid")]
    InvalidReason,
    #[error("reoffer requires an offering-occurrence model")]
    ReofferRequiresOfferingModel,
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
    #[error("internal correction failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin correction transaction")]
    BeginTransactionFailed,
    #[error("failed to commit correction transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CorrectProductListingAuctionContextUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CorrectProductListingAuctionContextCommand,
    ) -> Result<CorrectProductListingAuctionContextResult, CorrectProductListingAuctionContextError>;
}

pub struct CorrectProductListingAuctionContextHandler<U, R, E, O, A, C> {
    unit_of_work: U,
    products: R,
    events: E,
    overrides: O,
    auctions: A,
    check_user_admin: C,
}

impl<U, R, E, O, A, C> CorrectProductListingAuctionContextHandler<U, R, E, O, A, C> {
    pub fn new(
        unit_of_work: U,
        products: R,
        events: E,
        overrides: O,
        auctions: A,
        check_user_admin: C,
    ) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            overrides,
            auctions,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, E, O, A, C> CorrectProductListingAuctionContextUseCase
    for CorrectProductListingAuctionContextHandler<U, R, E, O, A, C>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    O: ProductListingAuctionOverrideRepositoryFactory<U::Tx>,
    A: AuctionRepositoryFactory<U::Tx>,
    C: CheckUserAdminUseCase,
{
    #[tracing::instrument(name = "correct_product_listing_auction_context", skip_all, fields(product_listing_id = %command.product_listing_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CorrectProductListingAuctionContextCommand,
    ) -> Result<CorrectProductListingAuctionContextResult, CorrectProductListingAuctionContextError>
    {
        self.check_user_admin
            .execute(context, CheckUserAdminRequest)
            .await
            .map_err(map_admin_error)?;
        let reason = validate_reason(command.reason)?;
        let actor_label = context.principal.label().to_owned();
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CorrectProductListingAuctionContextError::BeginTransactionFailed)?;
        let loaded = self
            .products
            .in_transaction(&mut tx)
            .find_by_id(command.product_listing_id)
            .await?
            .ok_or(CorrectProductListingAuctionContextError::NotFound)?;
        if loaded.version != command.expected_version {
            return Err(CorrectProductListingAuctionContextError::ConcurrencyConflict);
        }
        if loaded.value.lifecycle() == ListingLifecycle::Withdrawn {
            return Err(CorrectProductListingAuctionContextError::ListingWithdrawn);
        }
        let current_auction_id = loaded
            .value
            .auction()
            .and_then(ProductListingAuction::membership)
            .map(|value| value.auction_id());
        if current_auction_id != command.expected_current_auction_id {
            return Err(CorrectProductListingAuctionContextError::CurrentMembershipConflict);
        }
        let policy = self
            .overrides
            .in_transaction(&mut tx)
            .find(command.product_listing_id)
            .await?;
        let policy_version = policy
            .map_or_else(ProductListingAuctionPolicyVersion::default, |value| {
                value.version
            });
        if policy_version != command.expected_auction_policy_version {
            return Err(CorrectProductListingAuctionContextError::ConcurrencyConflict);
        }
        if let Some(membership) = command
            .replacement
            .as_ref()
            .and_then(ProductListingAuction::membership)
        {
            let auction = self
                .auctions
                .in_transaction(&mut tx)
                .find_by_id(membership.auction_id())
                .await
                .map_err(|error| CorrectProductListingAuctionContextError::Internal {
                    source: box_error(error),
                })?
                .ok_or(CorrectProductListingAuctionContextError::AuctionNotFound)?;
            if auction.auction.key().listing_source_id() != loaded.value.listing_source_id() {
                return Err(CorrectProductListingAuctionContextError::AuctionSourceMismatch);
            }
        }
        let mut product = loaded.value;
        let changed = product
            .replace_auction(command.replacement.clone())
            .map_err(|error| CorrectProductListingAuctionContextError::Internal {
                source: box_error(error),
            })?;
        let event = product.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload)
        });
        let resulting_version = if let Some(event) = event.as_ref() {
            let effects = ProductListingWriteEffects::from(&event.payload);
            let persisted = self
                .products
                .in_transaction(&mut tx)
                .update(&product, command.expected_version, event.event_id, effects)
                .await?;
            self.events
                .in_transaction(&mut tx)
                .append(event)
                .await
                .map_err(|error| CorrectProductListingAuctionContextError::Internal {
                    source: box_error(error),
                })?;
            persisted.version
        } else {
            command.expected_version
        };
        let override_state = self
            .overrides
            .in_transaction(&mut tx)
            .activate(
                &ProductListingAuctionOverrideAudit {
                    audit_id: EventId::new(),
                    product_listing_id: command.product_listing_id,
                    actor_label,
                    reason,
                    previous_auction_id: current_auction_id,
                    current_auction_id: command
                        .replacement
                        .as_ref()
                        .and_then(ProductListingAuction::membership)
                        .map(|value| value.auction_id()),
                    recorded_at: OffsetDateTime::now_utc(),
                },
                command.expected_auction_policy_version,
            )
            .await?;
        tx.commit()
            .await
            .map_err(|_| CorrectProductListingAuctionContextError::CommitTransactionFailed)?;
        tracing::info!(event = "product_listing.auction_context_corrected", product_listing_id = %command.product_listing_id, changed = changed.changed(), outcome = "success");
        Ok(CorrectProductListingAuctionContextResult {
            product_listing_id: command.product_listing_id,
            version: resulting_version,
            auction_policy_version: override_state.version,
        })
    }
}

fn validate_reason(value: String) -> Result<String, CorrectProductListingAuctionContextError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.contains('\0') || value.len() > MAX_CORRECTION_REASON_BYTES {
        return Err(CorrectProductListingAuctionContextError::InvalidReason);
    }
    Ok(value)
}

fn map_admin_error(error: CheckUserAdminError) -> CorrectProductListingAuctionContextError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            CorrectProductListingAuctionContextError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => CorrectProductListingAuctionContextError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            CorrectProductListingAuctionContextError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source } => {
            CorrectProductListingAuctionContextError::InvalidPersistedState { source }
        }
        CheckUserAdminError::Internal { source } => {
            CorrectProductListingAuctionContextError::Internal { source }
        }
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            CorrectProductListingAuctionContextError::TemporarilyUnavailable {
                source: box_error(error),
            }
        }
    }
}

impl From<ProductListingRepositoryError> for CorrectProductListingAuctionContextError {
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
impl From<ProductListingAuctionOverrideError> for CorrectProductListingAuctionContextError {
    fn from(error: ProductListingAuctionOverrideError) -> Self {
        match error {
            ProductListingAuctionOverrideError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ProductListingAuctionOverrideError::Persistence { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ProductListingAuctionOverrideError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            ProductListingAuctionOverrideError::UnsafeRelease => Self::Internal {
                source: box_error(error),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_trim_and_accept_a_valid_correction_reason() {
        assert!(matches!(
            validate_reason("  reason  ".to_owned()),
            Ok(reason) if reason == "reason"
        ));
    }

    #[test]
    fn should_reject_blank_nul_and_oversized_correction_reasons() {
        for reason in [
            "   ".to_owned(),
            "contains\0nul".to_owned(),
            "a".repeat(MAX_CORRECTION_REASON_BYTES + 1),
        ] {
            assert!(matches!(
                validate_reason(reason),
                Err(CorrectProductListingAuctionContextError::InvalidReason)
            ));
        }
    }
}
