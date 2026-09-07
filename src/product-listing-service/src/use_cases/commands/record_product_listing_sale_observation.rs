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

use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use product_listing_core::product_listing::{
    ListingSaleObservation, RecordListingSaleObservationError,
};
use product_listing_core::product_listing_id::ProductListingId;
use time::OffsetDateTime;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordProductListingSaleObservationCommand {
    pub product_listing_id: ProductListingId,
    pub observed_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordProductListingSaleObservationResult {
    pub product_listing_id: ProductListingId,
    pub outcome: ChangeOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum RecordProductListingSaleObservationError {
    #[error("authenticated actor required to record a product listing sale observation")]
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
    #[error("no persisted FX snapshot is available at the observation time")]
    FxSnapshotMissing,
    #[error("FX snapshot lookup is temporarily unavailable")]
    FxSnapshotUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("persisted FX snapshot is invalid")]
    FxSnapshotInvalid {
        #[source]
        source: BoxError,
    },
    #[error("a different sale observation already exists")]
    ConflictingExistingObservation,
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event append failed")]
    EventAppenderFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin sale observation transaction")]
    BeginTransactionFailed,
    #[error("failed to commit sale observation transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait RecordProductListingSaleObservationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: RecordProductListingSaleObservationCommand,
    ) -> Result<RecordProductListingSaleObservationResult, RecordProductListingSaleObservationError>;
}

pub struct RecordProductListingSaleObservationHandler<U, R, E, A, F> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    fx_rates: F,
}

impl<U, R, E, A, F> RecordProductListingSaleObservationHandler<U, R, E, A, F> {
    pub fn new(unit_of_work: U, products: R, events: E, authorizer: A, fx_rates: F) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
            fx_rates,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, E, A, F> RecordProductListingSaleObservationUseCase
    for RecordProductListingSaleObservationHandler<U, R, E, A, F>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(name = "record_product_listing_sale_observation", skip_all, fields(product_listing_id = %command.product_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: RecordProductListingSaleObservationCommand,
    ) -> Result<RecordProductListingSaleObservationResult, RecordProductListingSaleObservationError>
    {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<RecordProductListingSaleObservationError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let observed_at = command.observed_at;
        let recorded_at = OffsetDateTime::now_utc();
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| RecordProductListingSaleObservationError::BeginTransactionFailed)?;
        let loaded = self
            .products
            .in_transaction(&mut tx)
            .find_by_id(command.product_listing_id)
            .await?
            .ok_or(RecordProductListingSaleObservationError::NotFound)?;
        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(&mut tx)
                .authorize(actor_id, loaded.value.listing_source_id())
                .await?;
        }
        let snapshot = self
            .fx_rates
            .in_transaction(&mut tx)
            .find_latest_at_or_before(observed_at)
            .await?
            .ok_or(RecordProductListingSaleObservationError::FxSnapshotMissing)?;
        let observation = ListingSaleObservation::new(observed_at, snapshot.id());
        let expected_version = loaded.version;
        let mut listing = loaded.value;
        let outcome = listing.record_sale_observation(observation)?;
        let event = listing
            .take_pending_event_payload()
            .map(|payload| stamp_product_listing_event(listing.id(), recorded_at, payload));
        let current_event_id = event.as_ref().map(|event| event.event_id);
        if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            listing = self
                .products
                .in_transaction(&mut tx)
                .update(&listing, expected_version, event.event_id, effects)
                .await?
                .value;
            self.events.in_transaction(&mut tx).append(&event).await?;
        }
        tx.commit()
            .await
            .map_err(|_| RecordProductListingSaleObservationError::CommitTransactionFailed)?;
        tracing::info!(event = "product_listing.sale_observed", actor_type = context.principal.kind(), actor_id = %context.principal.label(), product_listing_id = %listing.id(), event_id = ?current_event_id, outcome = "success");
        Ok(RecordProductListingSaleObservationResult {
            product_listing_id: listing.id(),
            outcome,
        })
    }
}

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

impl From<RecordListingSaleObservationError> for RecordProductListingSaleObservationError {
    fn from(error: RecordListingSaleObservationError) -> Self {
        match error {
            RecordListingSaleObservationError::ConflictingExistingObservation => {
                Self::ConflictingExistingObservation
            }
            RecordListingSaleObservationError::InitialDiscoverySaleObservation
            | RecordListingSaleObservationError::ConflictingPendingObservation
            | RecordListingSaleObservationError::ImageCountOverflow => Self::PersistenceFailed,
        }
    }
}

impl From<OperationAuthorizationError> for RecordProductListingSaleObservationError {
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

impl From<PartnerProductListingAuthorizationError> for RecordProductListingSaleObservationError {
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

impl From<ProductListingRepositoryError> for RecordProductListingSaleObservationError {
    fn from(_: ProductListingRepositoryError) -> Self {
        Self::PersistenceFailed
    }
}

impl From<ProductListingEventAppendError> for RecordProductListingSaleObservationError {
    fn from(error: ProductListingEventAppendError) -> Self {
        Self::EventAppenderFailed {
            source: box_error(error),
        }
    }
}

impl From<FxRateSnapshotRepositoryError> for RecordProductListingSaleObservationError {
    fn from(error: FxRateSnapshotRepositoryError) -> Self {
        match error {
            FxRateSnapshotRepositoryError::ReadFailed { source }
            | FxRateSnapshotRepositoryError::InsertFailed { source } => {
                Self::FxSnapshotUnavailable { source }
            }
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
                Self::FxSnapshotInvalid { source }
            }
            FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => Self::FxSnapshotUnavailable {
                source: box_error(error),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{ProductListingStorageVersion, VersionedProductListing};
    use application::{
        operation_context::{CorrelationId, RequestId},
        transaction::TransactionError,
    };
    use domain_primitives::{event_id::EventId, versioned::Versioned};
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateId, FxRateQuote, FxRateSnapshot, FxRateSource,
        NewFxRateSnapshot,
    };
    use indexmap::IndexSet;
    use listing_source_core::ListingSourceId;
    use money::Currency;
    use product_listing_core::{
        listing_lifecycle::ListingLifecycle,
        product_listing::{
            ProductListing, ProductListingAuction, ProductListingPricing,
            RehydratedProductListingState,
        },
        product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
    };
    use std::sync::{Arc, Mutex, MutexGuard};
    use strum::IntoEnumIterator;
    use url::Url;

    #[derive(Default)]
    struct State {
        listing: Option<VersionedProductListing>,
        snapshot: Option<FxRateSnapshot>,
        commits: usize,
        updates: usize,
        appends: usize,
        recorded_events: Vec<crate::ports::product_listing_event_appender::ProductListingEvent>,
        persisted_event_ids: Vec<EventId>,
        fx_lookups: usize,
    }

    type SharedState = Arc<Mutex<State>>;

    #[derive(Clone)]
    struct UnitOfWorkFake(SharedState);
    struct TxFake(SharedState);
    #[derive(Clone)]
    struct ProductsFake(SharedState);
    struct ProductRepositoryFake(SharedState);
    #[derive(Clone)]
    struct EventsFake(SharedState);
    struct EventAppenderFake(SharedState);
    #[derive(Clone, Copy)]
    struct AuthorizerFake;
    struct AuthorizerRepositoryFake;
    #[derive(Clone)]
    struct FxRatesFake(SharedState);
    struct FxRateRepositoryFake(SharedState);

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for UnitOfWorkFake {
        type Tx = TxFake;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(TxFake(Arc::clone(&self.0)))
        }
    }

    #[async_trait::async_trait]
    impl Transaction for TxFake {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.0).commits += 1;
            Ok(())
        }
    }

    impl ProductListingRepositoryFactory<TxFake> for ProductsFake {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TxFake,
        ) -> impl ProductListingRepository + 'tx {
            ProductRepositoryFake(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRepository for ProductRepositoryFake {
        async fn find_by_id(
            &mut self,
            _id: ProductListingId,
        ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
            Ok(lock(&self.0).listing.clone())
        }

        async fn find_by_key(
            &mut self,
            _key: &product_listing_core::product_listing_id::ProductListingKey,
        ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
            Ok(None)
        }

        async fn insert(
            &mut self,
            _product: &ProductListing,
            _current_event_id: EventId,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            Err(ProductListingRepositoryError::ProductListingInsertFailed)
        }

        async fn update(
            &mut self,
            product: &ProductListing,
            expected_version: ProductListingStorageVersion,
            current_event_id: EventId,
            _: ProductListingWriteEffects,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            let persisted = Versioned::new(product.clone(), expected_version.next());
            let mut state = lock(&self.0);
            state.updates += 1;
            state.persisted_event_ids.push(current_event_id);
            state.listing = Some(persisted.clone());
            Ok(persisted)
        }
    }

    impl ProductListingEventAppenderFactory<TxFake> for EventsFake {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TxFake,
        ) -> impl ProductListingEventAppender + 'tx {
            EventAppenderFake(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ProductListingEventAppender for EventAppenderFake {
        async fn append(
            &mut self,
            event: &crate::ports::product_listing_event_appender::ProductListingEvent,
        ) -> Result<(), ProductListingEventAppendError> {
            let mut state = lock(&self.0);
            state.appends += 1;
            state.recorded_events.push(event.clone());
            Ok(())
        }
    }

    impl PartnerProductListingAuthorizerFactory<TxFake> for AuthorizerFake {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TxFake,
        ) -> impl PartnerProductListingAuthorizer + 'tx {
            AuthorizerRepositoryFake
        }
    }

    #[async_trait::async_trait]
    impl PartnerProductListingAuthorizer for AuthorizerRepositoryFake {
        async fn authorize(
            &mut self,
            _actor_id: UserId,
            _listing_source_id: ListingSourceId,
        ) -> Result<(), PartnerProductListingAuthorizationError> {
            Ok(())
        }
    }

    impl FxRateSnapshotRepositoryFactory<TxFake> for FxRatesFake {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TxFake,
        ) -> impl FxRateSnapshotRepository + 'tx {
            FxRateRepositoryFake(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for FxRateRepositoryFake {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock(&self.0);
            state.fx_lookups += 1;
            Ok(state.snapshot.clone())
        }

        async fn find_by_id(
            &mut self,
            _id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_by_ids(
            &mut self,
            _ids: &[FxRateId],
        ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Vec::new())
        }

        async fn insert(
            &mut self,
            _snapshot: &NewFxRateSnapshot,
            _source_event_id: &str,
        ) -> Result<fxrate_service::ports::FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>
        {
            Ok(fxrate_service::ports::FxRateSnapshotInsertOutcome::Duplicate)
        }
    }

    fn handler(
        state: &SharedState,
    ) -> RecordProductListingSaleObservationHandler<
        UnitOfWorkFake,
        ProductsFake,
        EventsFake,
        AuthorizerFake,
        FxRatesFake,
    > {
        RecordProductListingSaleObservationHandler::new(
            UnitOfWorkFake(Arc::clone(state)),
            ProductsFake(Arc::clone(state)),
            EventsFake(Arc::clone(state)),
            AuthorizerFake,
            FxRatesFake(Arc::clone(state)),
        )
    }

    fn listing(
        sale_observation: Option<ListingSaleObservation>,
    ) -> Result<ProductListing, Box<dyn std::error::Error>> {
        Ok(ProductListing::rehydrate(RehydratedProductListingState {
            id: ProductListingId::new(),
            title_slug_id: ProductListingSlugId::raw("listing-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("listing")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: None,
            description: None,
            pricing: ProductListingPricing::default(),
            sale_observation,
            availability: None,
            lifecycle: ListingLifecycle::Active,
            url: Url::parse("https://shop.example/listing")?,
            images: IndexSet::new(),
            auction: ProductListingAuction::default(),
        })?)
    }

    fn snapshot() -> Result<FxRateSnapshot, fxrate_core::FxRateSnapshotError> {
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
        )
        .and_then(|snapshot| Ok(snapshot.into_persisted(FxRateGeneration::try_from(1)?)))
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    #[tokio::test]
    async fn should_reject_anonymous_before_starting_a_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let result = handler(&state)
            .execute(
                &context(Principal::Anonymous),
                RecordProductListingSaleObservationCommand {
                    product_listing_id: ProductListingId::new(),
                    observed_at: OffsetDateTime::UNIX_EPOCH,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(RecordProductListingSaleObservationError::AuthenticatedActorRequired)
        ));
        assert_eq!(0, lock(&state).commits);
    }

    #[tokio::test]
    async fn should_accept_delegated_product_listings_write_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let listing = listing(None)?;
        let product_listing_id = listing.id();
        lock(&state).listing = Some(Versioned::new(
            listing,
            ProductListingStorageVersion::INITIAL,
        ));
        lock(&state).snapshot = Some(snapshot()?);
        let context = context(Principal::DelegatedUser {
            user_id: UserId::new(),
            capabilities: [CredentialCapability::ProductListingsWrite]
                .into_iter()
                .collect(),
        });

        let result = handler(&state)
            .execute(
                &context,
                RecordProductListingSaleObservationCommand {
                    product_listing_id,
                    observed_at: OffsetDateTime::UNIX_EPOCH,
                },
            )
            .await;

        assert!(result.is_ok());
        assert_eq!(1, lock(&state).commits);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_delegated_unsupported_scope_before_starting_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let context = context(Principal::DelegatedUser {
            user_id: UserId::new(),
            capabilities: [CredentialCapability::UsersRead].into_iter().collect(),
        });

        let result = handler(&state)
            .execute(
                &context,
                RecordProductListingSaleObservationCommand {
                    product_listing_id: ProductListingId::new(),
                    observed_at: OffsetDateTime::UNIX_EPOCH,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(RecordProductListingSaleObservationError::Forbidden)
        ));
        assert_eq!(0, lock(&state).commits);
    }

    #[tokio::test]
    async fn should_commit_idempotent_observation_without_persistence()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let snapshot = snapshot()?;
        let observation = ListingSaleObservation::new(OffsetDateTime::UNIX_EPOCH, snapshot.id());
        let listing = listing(Some(observation))?;
        let product_listing_id = listing.id();
        lock(&state).listing = Some(Versioned::new(
            listing,
            ProductListingStorageVersion::INITIAL,
        ));
        lock(&state).snapshot = Some(snapshot);

        let result = handler(&state)
            .execute(
                &context(Principal::System),
                RecordProductListingSaleObservationCommand {
                    product_listing_id,
                    observed_at: OffsetDateTime::UNIX_EPOCH,
                },
            )
            .await?;

        assert_eq!(product_listing_id, result.product_listing_id);
        assert_eq!(ChangeOutcome::Unchanged, result.outcome);
        let state = lock(&state);
        assert_eq!(1, state.fx_lookups);
        assert_eq!(1, state.commits);
        assert_eq!(0, state.updates);
        assert_eq!(0, state.appends);
        Ok(())
    }

    #[tokio::test]
    async fn should_keep_sale_fact_time_in_payload_and_record_event_at_processing_time()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let listing = listing(None)?;
        let product_listing_id = listing.id();
        lock(&state).listing = Some(Versioned::new(
            listing,
            ProductListingStorageVersion::INITIAL,
        ));
        lock(&state).snapshot = Some(snapshot()?);
        let observed_at = OffsetDateTime::UNIX_EPOCH;

        handler(&state)
            .execute(
                &context(Principal::System),
                RecordProductListingSaleObservationCommand {
                    product_listing_id,
                    observed_at,
                },
            )
            .await?;

        let state = lock(&state);
        assert_eq!(1, state.recorded_events.len());
        assert_eq!(1, state.persisted_event_ids.len());
        let event = &state.recorded_events[0];
        assert_eq!(event.event_id, state.persisted_event_ids[0]);
        assert!(event.timestamp > observed_at);
        assert!(matches!(
            &event.payload,
            product_listing_core::product_listing_event::ProductListingEventPayload::Changed(change)
                if matches!(
                    change.sale_observation(),
                    Some(change)
                        if matches!(
                            (change.previous(), change.current()),
                            (None, Some(observation)) if observation.observed_at() == observed_at
                        )
                )
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_conflicting_observation_without_persistence()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let existing_snapshot = snapshot()?;
        let requested_snapshot = snapshot()?;
        let listing = listing(Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            existing_snapshot.id(),
        )))?;
        let product_listing_id = listing.id();
        lock(&state).listing = Some(Versioned::new(
            listing,
            ProductListingStorageVersion::INITIAL,
        ));
        lock(&state).snapshot = Some(requested_snapshot);

        let result = handler(&state)
            .execute(
                &context(Principal::System),
                RecordProductListingSaleObservationCommand {
                    product_listing_id,
                    observed_at: OffsetDateTime::UNIX_EPOCH,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(RecordProductListingSaleObservationError::ConflictingExistingObservation)
        ));
        let state = lock(&state);
        assert_eq!(0, state.commits);
        assert_eq!(0, state.updates);
        assert_eq!(0, state.appends);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_or_persist_when_snapshot_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let listing = listing(None)?;
        let product_listing_id = listing.id();
        lock(&state).listing = Some(Versioned::new(
            listing,
            ProductListingStorageVersion::INITIAL,
        ));

        let result = handler(&state)
            .execute(
                &context(Principal::System),
                RecordProductListingSaleObservationCommand {
                    product_listing_id,
                    observed_at: OffsetDateTime::UNIX_EPOCH,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(RecordProductListingSaleObservationError::FxSnapshotMissing)
        ));
        let state = lock(&state);
        assert_eq!(1, state.fx_lookups);
        assert_eq!(0, state.commits);
        assert_eq!(0, state.updates);
        assert_eq!(0, state.appends);
        Ok(())
    }
}
