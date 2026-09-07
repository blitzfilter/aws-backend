use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventAppendError,
    ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, ProductListingWriteEffects,
    stamp_product_listing_event,
};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::change_outcome::ChangeOutcome;
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct WithdrawProductListingResult {
    pub product_listing_id: ProductListingId,
    pub outcome: ChangeOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum WithdrawProductListingError {
    #[error("authenticated actor required to withdraw product listing")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("listing source not found")]
    ListingSourceNotFound,
    #[error("partner product listing authorization is temporarily unavailable")]
    PartnerAuthorizationTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("partner product listing authorization failed internally")]
    PartnerAuthorizationInternal {
        #[source]
        source: BoxError,
    },
    #[error("product listing not found")]
    NotFound,
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event storage failed")]
    EventAppenderFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin withdraw product listing transaction")]
    BeginTransactionFailed,
    #[error("failed to commit withdraw product listing transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait WithdrawProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError>;
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError>;
}

pub struct WithdrawProductListingHandler<U, R, E, A> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
}
impl<U, R, E, A> WithdrawProductListingHandler<U, R, E, A> {
    pub fn new(unit_of_work: U, products: R, events: E, authorizer: A) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
        }
    }
}

enum WithdrawTarget {
    Id(ProductListingId),
    Key(ProductListingKey),
}

impl<U, R, E, A> WithdrawProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    async fn withdraw(
        &self,
        context: &OperationContext,
        target: WithdrawTarget,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<WithdrawProductListingError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| WithdrawProductListingError::BeginTransactionFailed)?;
        let loaded = match target {
            WithdrawTarget::Id(id) => {
                let loaded = self
                    .products
                    .in_transaction(&mut tx)
                    .find_by_id(id)
                    .await?
                    .ok_or(WithdrawProductListingError::NotFound)?;
                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(&mut tx)
                        .authorize(actor_id, loaded.value.listing_source_id())
                        .await?;
                }
                loaded
            }
            WithdrawTarget::Key(key) => {
                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(&mut tx)
                        .authorize(actor_id, key.listing_source_id)
                        .await?;
                }
                self.products
                    .in_transaction(&mut tx)
                    .find_by_key(&key)
                    .await?
                    .ok_or(WithdrawProductListingError::NotFound)?
            }
        };
        let expected_version = loaded.version;
        let mut product = loaded.value;
        let outcome = product
            .withdraw()
            .map_err(|_| WithdrawProductListingError::PersistenceFailed)?;
        let event = product.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(product.id(), time::OffsetDateTime::now_utc(), payload)
        });
        let current_event_id = event.as_ref().map(|event| event.event_id);
        if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            product = self
                .products
                .in_transaction(&mut tx)
                .update(&product, expected_version, event.event_id, effects)
                .await?
                .value;
            self.events.in_transaction(&mut tx).append(&event).await?;
        }
        tx.commit()
            .await
            .map_err(|_| WithdrawProductListingError::CommitTransactionFailed)?;
        tracing::info!(event = "product_listing.withdrawn", actor_type = context.principal.kind(), actor_id = %context.principal.label(), product_listing_id = %product.id(), event_id = ?current_event_id, outcome = "success");
        Ok(WithdrawProductListingResult {
            product_listing_id: product.id(),
            outcome,
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A> WithdrawProductListingUseCase for WithdrawProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    #[tracing::instrument(name = "withdraw_product_listing", skip_all, fields(product_listing_id = %product_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError> {
        self.withdraw(context, WithdrawTarget::Id(product_listing_id))
            .await
    }

    #[tracing::instrument(name = "withdraw_product_listing_by_key", skip_all, fields(listing_source_id = %product_key.listing_source_id, source_listing_id = %product_key.source_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError> {
        self.withdraw(context, WithdrawTarget::Key(product_key))
            .await
    }
}

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}
impl From<OperationAuthorizationError> for WithdrawProductListingError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}
impl From<PartnerProductListingAuthorizationError> for WithdrawProductListingError {
    fn from(error: PartnerProductListingAuthorizationError) -> Self {
        match error {
            PartnerProductListingAuthorizationError::ListingSourceNotFound => {
                Self::ListingSourceNotFound
            }
            PartnerProductListingAuthorizationError::Forbidden => Self::Forbidden,
            PartnerProductListingAuthorizationError::TemporarilyUnavailable { source } => {
                Self::PartnerAuthorizationTemporarilyUnavailable { source }
            }
            PartnerProductListingAuthorizationError::Internal { source } => {
                Self::PartnerAuthorizationInternal { source }
            }
        }
    }
}
impl From<ProductListingRepositoryError> for WithdrawProductListingError {
    fn from(error: ProductListingRepositoryError) -> Self {
        match error {
            ProductListingRepositoryError::ProductListingLookupByIdFailed
            | ProductListingRepositoryError::ProductListingLookupByKeyFailed { .. }
            | ProductListingRepositoryError::ProductListingInsertFailed
            | ProductListingRepositoryError::ProductListingUpdateFailed
            | ProductListingRepositoryError::ConcurrencyConflict
            | ProductListingRepositoryError::SourceListingAlreadyExists
            | ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists
            | ProductListingRepositoryError::InvalidProductListingSlugPersisted
            | ProductListingRepositoryError::InvalidSourceListingIdPersisted
            | ProductListingRepositoryError::IncompleteTitlePersisted
            | ProductListingRepositoryError::InvalidTitleLanguagePersisted
            | ProductListingRepositoryError::IncompleteDescriptionPersisted
            | ProductListingRepositoryError::InvalidDescriptionLanguagePersisted
            | ProductListingRepositoryError::IncompletePricePersisted
            | ProductListingRepositoryError::NegativePriceAmountPersisted
            | ProductListingRepositoryError::InvalidPriceCurrencyPersisted
            | ProductListingRepositoryError::InvalidListingAvailabilityPersisted
            | ProductListingRepositoryError::InvalidListingLifecyclePersisted
            | ProductListingRepositoryError::InvalidProductListingUrlPersisted
            | ProductListingRepositoryError::InvalidProductListingImagesPersisted
            | ProductListingRepositoryError::InvalidProductListingImageUrlPersisted
            | ProductListingRepositoryError::InvalidAggregateStatePersisted => {
                Self::PersistenceFailed
            }
        }
    }
}
impl From<ProductListingEventAppendError> for WithdrawProductListingError {
    fn from(error: ProductListingEventAppendError) -> Self {
        Self::EventAppenderFailed {
            source: box_error(error),
        }
    }
}
