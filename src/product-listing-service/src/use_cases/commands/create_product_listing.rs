use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventAppendError,
    ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, stamp_product_listing_event,
};
use crate::product_listing_title_slug_creation::{
    MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS, ProductListingTitleSlugGenerator,
    RandomProductListingTitleSlugGenerator, TitleSlugCollisionRetry, title_slug_collision_retry,
};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::transaction::{Transaction, UnitOfWork};

use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    NewProductListing, ProductListing, ProductListingAuction, ProductListingPricing,
    RehydrateProductListingError,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::title::Title;
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductListingCommand {
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductListingPricing,
    pub availability: Option<ListingAvailability>,
    pub url: Url,
    pub images: IndexSet<ProductListingImage>,
    pub auction: ProductListingAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductListingResult {
    pub product_listing_id: ProductListingId,
    pub product_listing_title_slug_id: ProductListingSlugId,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateProductListingError {
    #[error("authenticated actor required to create product listing")]
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
    #[error("product listing already exists for source listing identity")]
    SourceListingAlreadyExists,
    #[error("product listing title slug already exists")]
    ProductListingTitleSlugAlreadyExists,
    #[error("product listing title slug generation was exhausted")]
    ProductListingTitleSlugGenerationExhausted,
    #[error("new product listing is invalid")]
    InvalidProductListing,
    #[error("created product listing did not record a domain event")]
    CreatedEventMissing,
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event append failed")]
    EventAppenderFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin create product listing transaction")]
    BeginTransactionFailed,
    #[error("failed to commit create product listing transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CreateProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateProductListingCommand,
    ) -> Result<CreateProductListingResult, CreateProductListingError>;
}

pub struct CreateProductListingHandler<U, R, E, A, G = RandomProductListingTitleSlugGenerator> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    title_slug_generator: G,
}

impl<U, R, E, A> CreateProductListingHandler<U, R, E, A, RandomProductListingTitleSlugGenerator> {
    pub fn new(unit_of_work: U, products: R, events: E, authorizer: A) -> Self {
        Self::with_title_slug_generator(
            unit_of_work,
            products,
            events,
            authorizer,
            RandomProductListingTitleSlugGenerator,
        )
    }
}

impl<U, R, E, A, G> CreateProductListingHandler<U, R, E, A, G> {
    pub(crate) fn with_title_slug_generator(
        unit_of_work: U,
        products: R,
        events: E,
        authorizer: A,
        title_slug_generator: G,
    ) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
            title_slug_generator,
        }
    }
}

impl<U, R, E, A, G> CreateProductListingHandler<U, R, E, A, G>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    G: ProductListingTitleSlugGenerator,
{
    async fn persist_attempt(
        &self,
        context: &OperationContext,
        command: &CreateProductListingCommand,
        product_listing_id: ProductListingId,
        title_slug_id: ProductListingSlugId,
    ) -> Result<CreateProductListingResult, CreateProductListingError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreateProductListingError::BeginTransactionFailed)?;
        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(&mut tx)
                .authorize(actor_id, command.listing_source_id)
                .await?;
        }

        let mut product = ProductListing::create(
            command
                .clone()
                .into_new_product(product_listing_id, title_slug_id),
        )?;
        let event = stamp_product_listing_event(
            product.id(),
            time::OffsetDateTime::now_utc(),
            product
                .take_pending_event_payload()
                .ok_or(CreateProductListingError::CreatedEventMissing)?,
        );
        let event_id = event.event_id;
        let persisted = self
            .products
            .in_transaction(&mut tx)
            .insert(&product, event_id)
            .await?;
        self.events.in_transaction(&mut tx).append(&event).await?;
        tx.commit()
            .await
            .map_err(|_| CreateProductListingError::CommitTransactionFailed)?;

        tracing::info!(event = "product_listing.created", actor_type = context.principal.kind(), actor_id = %context.principal.label(), product_listing_id = %persisted.value.id(), event_id = %event_id, outcome = "success");
        Ok(CreateProductListingResult {
            product_listing_id: persisted.value.id(),
            product_listing_title_slug_id: persisted.value.title_slug_id().clone(),
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A, G> CreateProductListingUseCase for CreateProductListingHandler<U, R, E, A, G>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    G: ProductListingTitleSlugGenerator,
{
    #[tracing::instrument(name = "create_product_listing", skip_all, fields(listing_source_id = %command.listing_source_id, source_listing_id = %command.source_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateProductListingCommand,
    ) -> Result<CreateProductListingResult, CreateProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<CreateProductListingError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let product_listing_id = ProductListingId::new();
        for attempt in 1..=MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS {
            let title_slug_id = self
                .title_slug_generator
                .generate(
                    command
                        .title
                        .as_ref()
                        .map_or("", |title| title.payload.as_ref()),
                )
                .map_err(|_| CreateProductListingError::InvalidProductListing)?;
            match self
                .persist_attempt(context, &command, product_listing_id, title_slug_id)
                .await
            {
                Ok(result) => return Ok(result),
                Err(CreateProductListingError::ProductListingTitleSlugAlreadyExists)
                    if title_slug_collision_retry(attempt, true)
                        == TitleSlugCollisionRetry::Retry =>
                {
                    tracing::warn!(
                        product_listing_id = %product_listing_id,
                        attempt,
                        constraint_name = "product_listings_title_slug_unique",
                        "product listing title slug collision; regenerating"
                    );
                }
                Err(CreateProductListingError::ProductListingTitleSlugAlreadyExists) => {
                    tracing::error!(
                        product_listing_id = %product_listing_id,
                        attempt,
                        constraint_name = "product_listings_title_slug_unique",
                        "product listing title slug generation exhausted"
                    );
                    return Err(
                        CreateProductListingError::ProductListingTitleSlugGenerationExhausted,
                    );
                }
                Err(error) => return Err(error),
            }
        }
        Err(CreateProductListingError::ProductListingTitleSlugGenerationExhausted)
    }
}

impl CreateProductListingCommand {
    fn into_new_product(
        self,
        id: ProductListingId,
        title_slug_id: ProductListingSlugId,
    ) -> NewProductListing {
        NewProductListing {
            id,
            title_slug_id,
            listing_source_id: self.listing_source_id,
            source_listing_id: self.source_listing_id,
            title: self.title,
            description: self.description,
            pricing: self.pricing,
            availability: self.availability,
            url: self.url,
            images: self.images,
            auction: self.auction,
        }
    }
}

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

impl From<RehydrateProductListingError> for CreateProductListingError {
    fn from(_: RehydrateProductListingError) -> Self {
        Self::InvalidProductListing
    }
}

impl From<OperationAuthorizationError> for CreateProductListingError {
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

impl From<PartnerProductListingAuthorizationError> for CreateProductListingError {
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

impl From<ProductListingRepositoryError> for CreateProductListingError {
    fn from(error: ProductListingRepositoryError) -> Self {
        match error {
            ProductListingRepositoryError::SourceListingAlreadyExists => {
                Self::SourceListingAlreadyExists
            }
            ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists => {
                Self::ProductListingTitleSlugAlreadyExists
            }
            _ => Self::PersistenceFailed,
        }
    }
}

impl From<ProductListingEventAppendError> for CreateProductListingError {
    fn from(error: ProductListingEventAppendError) -> Self {
        Self::EventAppenderFailed {
            source: box_error(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ProductListingStorageVersion, ProductListingWriteEffects, VersionedProductListing,
    };
    use application::operation_context::{CorrelationId, RequestId};
    use application::transaction::TransactionError;
    use domain_primitives::{event_id::EventId, versioned::Versioned};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Default)]
    struct State {
        candidates: Vec<ProductListingSlugId>,
        begins: usize,
        commits: usize,
        rollbacks: usize,
        inserts: usize,
        updates: usize,
        events: usize,
        authorizations: usize,
        insert_results: VecDeque<Result<(), ProductListingRepositoryError>>,
    }

    type SharedState = Arc<Mutex<State>>;

    #[derive(Clone)]
    struct UnitOfWorkFake(SharedState);
    struct TxFake(SharedState, bool);
    impl Drop for TxFake {
        fn drop(&mut self) {
            if !self.1 {
                lock(&self.0).rollbacks += 1;
            }
        }
    }
    #[derive(Clone)]
    struct ProductsFake(SharedState);
    struct ProductRepositoryFake(SharedState);
    #[derive(Clone)]
    struct EventsFake(SharedState);
    struct EventAppenderFake(SharedState);
    #[derive(Clone)]
    struct AuthorizerFake(SharedState);
    struct AuthorizerRepositoryFake(SharedState);
    #[derive(Clone)]
    struct GeneratorFake(SharedState);

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(value) => value,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for UnitOfWorkFake {
        type Tx = TxFake;
        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(TxFake(Arc::clone(&self.0), false))
        }
    }
    #[async_trait::async_trait]
    impl Transaction for TxFake {
        async fn commit(mut self) -> Result<(), TransactionError> {
            self.1 = true;
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
            _: ProductListingId,
        ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
            Ok(None)
        }
        async fn find_by_key(
            &mut self,
            _: &product_listing_core::product_listing_id::ProductListingKey,
        ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
            Ok(None)
        }
        async fn insert(
            &mut self,
            product: &ProductListing,
            _: EventId,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            let mut state = lock(&self.0);
            state.inserts += 1;
            match state.insert_results.pop_front().unwrap_or(Ok(())) {
                Ok(()) => Ok(Versioned::new(
                    product.clone(),
                    ProductListingStorageVersion::INITIAL,
                )),
                Err(error) => Err(error),
            }
        }
        async fn update(
            &mut self,
            _: &ProductListing,
            _: ProductListingStorageVersion,
            _: EventId,
            _: ProductListingWriteEffects,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            lock(&self.0).updates += 1;
            Err(ProductListingRepositoryError::ProductListingUpdateFailed)
        }
    }
    impl ProductListingEventAppenderFactory<TxFake> for EventsFake {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TxFake,
        ) -> impl ProductListingEventAppender + 'tx {
            EventAppenderFake(Arc::clone(&self.0))
        }
    }
    #[async_trait::async_trait]
    impl ProductListingEventAppender for EventAppenderFake {
        async fn append(
            &mut self,
            _: &crate::ports::product_listing_event_appender::ProductListingEvent,
        ) -> Result<(), ProductListingEventAppendError> {
            lock(&self.0).events += 1;
            Ok(())
        }
    }
    impl PartnerProductListingAuthorizerFactory<TxFake> for AuthorizerFake {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TxFake,
        ) -> impl PartnerProductListingAuthorizer + 'tx {
            AuthorizerRepositoryFake(Arc::clone(&self.0))
        }
    }
    #[async_trait::async_trait]
    impl PartnerProductListingAuthorizer for AuthorizerRepositoryFake {
        async fn authorize(
            &mut self,
            _: UserId,
            _: ListingSourceId,
        ) -> Result<(), PartnerProductListingAuthorizationError> {
            lock(&self.0).authorizations += 1;
            Ok(())
        }
    }
    impl ProductListingTitleSlugGenerator for GeneratorFake {
        fn generate(
            &self,
            _: &str,
        ) -> Result<
            ProductListingSlugId,
            product_listing_core::product_listing_slug_id::InvalidProductListingSlugId,
        > {
            let candidate = ProductListingSlugId::from_title_and_suffix(
                "listing",
                &format!("{:06x}", lock(&self.0).candidates.len() + 1),
            )?;
            lock(&self.0).candidates.push(candidate.clone());
            Ok(candidate)
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }
    fn command() -> CreateProductListingCommand {
        CreateProductListingCommand {
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("source")
                .unwrap_or_else(|error| panic!("source: {error}")),
            title: None,
            description: None,
            pricing: ProductListingPricing::default(),
            availability: None,
            url: Url::parse("https://example.com/listing")
                .unwrap_or_else(|error| panic!("url: {error}")),
            images: IndexSet::new(),
            auction: ProductListingAuction::default(),
        }
    }
    fn handler(
        state: &SharedState,
    ) -> CreateProductListingHandler<
        UnitOfWorkFake,
        ProductsFake,
        EventsFake,
        AuthorizerFake,
        GeneratorFake,
    > {
        CreateProductListingHandler::with_title_slug_generator(
            UnitOfWorkFake(Arc::clone(state)),
            ProductsFake(Arc::clone(state)),
            EventsFake(Arc::clone(state)),
            AuthorizerFake(Arc::clone(state)),
            GeneratorFake(Arc::clone(state)),
        )
    }

    #[tokio::test]
    async fn should_retry_collision_in_fresh_transaction_then_commit_created_listing() {
        let state = Arc::new(Mutex::new(State {
            insert_results: VecDeque::from([
                Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists),
                Ok(()),
            ]),
            ..Default::default()
        }));
        let result = handler(&state).execute(&context(), command()).await;
        assert!(result.is_ok());
        let state = lock(&state);
        assert_eq!(state.candidates.len(), 2);
        assert_eq!((state.begins, state.commits, state.rollbacks), (2, 1, 1));
        assert_eq!(
            (
                state.inserts,
                state.updates,
                state.events,
                state.authorizations
            ),
            (2, 0, 1, 2)
        );
    }

    #[tokio::test]
    async fn should_exhaust_after_five_collisions_with_only_rolled_back_attempts() {
        let state = Arc::new(Mutex::new(State {
            insert_results: VecDeque::from([
                Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists),
                Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists),
                Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists),
                Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists),
                Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists),
            ]),
            ..Default::default()
        }));
        assert!(matches!(
            handler(&state).execute(&context(), command()).await,
            Err(CreateProductListingError::ProductListingTitleSlugGenerationExhausted)
        ));
        let state = lock(&state);
        assert_eq!(state.candidates.len(), 5);
        assert_eq!((state.begins, state.commits, state.rollbacks), (5, 0, 5));
        assert_eq!(
            (state.inserts, state.events, state.authorizations),
            (5, 0, 5)
        );
    }

    #[tokio::test]
    async fn should_not_retry_unrelated_insert_error() {
        let state = Arc::new(Mutex::new(State {
            insert_results: VecDeque::from([Err(
                ProductListingRepositoryError::ProductListingInsertFailed,
            )]),
            ..Default::default()
        }));
        assert!(matches!(
            handler(&state).execute(&context(), command()).await,
            Err(CreateProductListingError::PersistenceFailed)
        ));
        let state = lock(&state);
        assert_eq!(state.candidates.len(), 1);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!(
            (state.inserts, state.events, state.authorizations),
            (1, 0, 1)
        );
    }
}
