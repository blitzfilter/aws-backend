use crate::ports::*;
use application::{
    error::{BoxError, static_error},
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use listing_source_core::*;
use party_core::{
    party::{NewParty, Party},
    party_id::PartyId,
};
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub enum ListingSourceOperator {
    Existing(PartyId),
    New(NewParty),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateListingSourceCommand {
    pub name: ListingSourceName,
    pub operator: ListingSourceOperator,

    pub ingestion_configuration: ListingSourceIngestionConfigurations,
    pub woocommerce_webhook_secret: Option<String>,
    pub presentation: ListingSourcePresentation,
    pub referral_configuration: Option<ReferralConfiguration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateListingSourceResult {
    pub listing_source_id: ListingSourceId,
    pub slug_id: ListingSourceSlugId,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateListingSourceError {
    #[error("authenticated actor required to create listing source")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("operator party not found")]
    OperatorPartyNotFound,
    #[error("ingestion method/configuration mismatch")]
    ListingIngestionConfigurationMismatch,
    #[error("operator party slug conflict")]
    OperatorPartySlugConflict {
        #[source]
        source: BoxError,
    },
    #[error("listing source slug conflict")]
    SlugConflict {
        #[source]
        source: BoxError,
    },
    #[error("listing source Shopify domain conflict")]
    ShopifyDomainConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary listing source persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted listing source state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal listing source failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin create listing source transaction")]
    BeginTransactionFailed,
    #[error("failed to commit create listing source transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CreateListingSourceUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateListingSourceCommand,
    ) -> Result<CreateListingSourceResult, CreateListingSourceError>;
}

pub struct CreateListingSourceHandler<U, S, P, A> {
    unit_of_work: U,
    sources: S,
    parties: P,
    check_user_admin: A,
}

impl<U, S, P, A> CreateListingSourceHandler<U, S, P, A> {
    pub fn new(unit_of_work: U, sources: S, parties: P, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            sources,
            parties,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, P, A> CreateListingSourceUseCase for CreateListingSourceHandler<U, S, P, A>
where
    U: UnitOfWork,
    S: ListingSourceRepositoryFactory<U::Tx>,
    P: PartyRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "create_listing_source",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateListingSourceCommand,
    ) -> Result<CreateListingSourceResult, CreateListingSourceError> {
        ensure_admin(context, &self.check_user_admin).await?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreateListingSourceError::BeginTransactionFailed)?;
        let operator_party_id = match command.operator {
            ListingSourceOperator::Existing(id) => {
                self.parties
                    .in_transaction(&mut tx)
                    .find_by_id(id)
                    .await
                    .map_err(map_party)?
                    .ok_or(CreateListingSourceError::OperatorPartyNotFound)?;
                id
            }
            ListingSourceOperator::New(input) => self
                .parties
                .in_transaction(&mut tx)
                .insert(&Party::create(input))
                .await
                .map_err(map_party)?
                .party
                .id(),
        };
        let ingestion_methods = command
            .ingestion_configuration
            .methods()
            .map_err(|_| CreateListingSourceError::ListingIngestionConfigurationMismatch)?;
        let source = ListingSource::create(NewListingSource {
            id: ListingSourceId::new(),
            name: command.name,
            operator_party_id,
            ingestion_methods,
            presentation: command.presentation,
            referral_configuration: command.referral_configuration,
        });
        command
            .ingestion_configuration
            .validate_for(&source)
            .map_err(|_| CreateListingSourceError::ListingIngestionConfigurationMismatch)?;
        if command.woocommerce_webhook_secret.is_some()
            && !command.ingestion_configuration.has_woocommerce()
        {
            return Err(CreateListingSourceError::ListingIngestionConfigurationMismatch);
        }
        let result = self
            .sources
            .in_transaction(&mut tx)
            .insert(
                &source,
                &command.ingestion_configuration,
                command.woocommerce_webhook_secret.as_deref(),
            )
            .await
            .map_err(CreateListingSourceError::from)?;
        tx.commit()
            .await
            .map_err(|_| CreateListingSourceError::CommitTransactionFailed)?;

        tracing::info!(
            event = "listing_source.created",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            listing_source_id = %result.source.id(),
            listing_source_slug_id = %result.source.slug_id(),
            outcome = "success",
        );
        Ok(CreateListingSourceResult {
            listing_source_id: result.source.id(),
            slug_id: result.source.slug_id().clone(),
        })
    }
}

async fn ensure_admin<A>(
    context: &OperationContext,
    check: &A,
) -> Result<(), CreateListingSourceError>
where
    A: CheckUserAdminUseCase,
{
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::Anonymous => Err(CreateListingSourceError::AuthenticatedActorRequired),
        Principal::User(_) | Principal::DelegatedUser { .. } => check
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(|error| match error {
                CheckUserAdminError::AuthenticatedActorRequired => {
                    CreateListingSourceError::AuthenticatedActorRequired
                }
                CheckUserAdminError::Forbidden => CreateListingSourceError::Forbidden,
                CheckUserAdminError::TemporarilyUnavailable { source } => {
                    CreateListingSourceError::TemporarilyUnavailable { source }
                }
                CheckUserAdminError::InvalidReadModel { source }
                | CheckUserAdminError::Internal { source } => {
                    CreateListingSourceError::Internal { source }
                }
                CheckUserAdminError::BeginTransactionFailed
                | CheckUserAdminError::CommitTransactionFailed => {
                    CreateListingSourceError::TemporarilyUnavailable {
                        source: static_error("check user admin transaction failed"),
                    }
                }
            }),
    }
}

fn map_party(error: party_service::ports::PartyRepositoryError) -> CreateListingSourceError {
    match error {
        party_service::ports::PartyRepositoryError::TemporarilyUnavailable { source } => {
            CreateListingSourceError::TemporarilyUnavailable { source }
        }
        party_service::ports::PartyRepositoryError::SlugConflict { source } => {
            CreateListingSourceError::OperatorPartySlugConflict { source }
        }
        party_service::ports::PartyRepositoryError::InvalidPersistedState { source }
        | party_service::ports::PartyRepositoryError::Internal { source } => {
            CreateListingSourceError::Internal { source }
        }
        party_service::ports::PartyRepositoryError::ConcurrencyConflict => {
            CreateListingSourceError::Internal {
                source: static_error("unexpected party concurrency"),
            }
        }
    }
}

impl From<ListingSourceRepositoryError> for CreateListingSourceError {
    fn from(error: ListingSourceRepositoryError) -> Self {
        match error {
            ListingSourceRepositoryError::ConcurrencyConflict => Self::Internal {
                source: static_error("unexpected create listing source concurrency"),
            },
            ListingSourceRepositoryError::SlugConflict { source } => Self::SlugConflict { source },
            ListingSourceRepositoryError::ShopifyDomainConflict { source } => {
                Self::ShopifyDomainConflict { source }
            }
            ListingSourceRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ListingSourceRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            ListingSourceRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        operation_context::{CorrelationId, RequestId},
        transaction::TransactionError,
    };
    use party_core::{
        party::{Party, PartyContact},
        party_name::PartyName,
    };
    use party_service::ports::StoredParty;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use user_service::use_cases::queries::check_user_admin::CheckUserAdminResult;

    #[derive(Default)]
    struct State {
        party_inserts: usize,
        source_inserts: usize,
        commits: usize,
        party_tx: Option<u64>,
        source_tx: Option<u64>,
        fail_source: bool,
    }
    #[derive(Clone, Default)]
    struct FakeUnitOfWork(Arc<Mutex<State>>);
    struct FakeTransaction {
        id: u64,
        state: Arc<Mutex<State>>,
    }
    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            self.state
                .lock()
                .map_err(|_| TransactionError::CommitFailed)?
                .commits += 1;
            Ok(())
        }
    }
    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;
        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(FakeTransaction {
                id: 7,
                state: Arc::clone(&self.0),
            })
        }
    }
    #[derive(Clone)]
    struct FakeParties;
    struct FakePartyRepository<'a> {
        tx: &'a mut FakeTransaction,
    }
    impl PartyRepositoryFactory<FakeTransaction> for FakeParties {
        fn in_transaction<'a>(&'a self, tx: &'a mut FakeTransaction) -> impl PartyRepository + 'a {
            FakePartyRepository { tx }
        }
    }
    #[async_trait::async_trait]
    impl PartyRepository for FakePartyRepository<'_> {
        async fn find_by_id(
            &mut self,
            id: PartyId,
        ) -> Result<Option<StoredParty>, party_service::ports::PartyRepositoryError> {
            Ok(Some(stored_party(id)))
        }
        async fn find_by_slug(
            &mut self,
            _: &party_core::party_slug_id::PartySlugId,
        ) -> Result<Option<StoredParty>, party_service::ports::PartyRepositoryError> {
            Ok(None)
        }
        async fn insert(
            &mut self,
            party: &Party,
        ) -> Result<StoredParty, party_service::ports::PartyRepositoryError> {
            let mut state = self.tx.state.lock().map_err(|_| party_error())?;
            state.party_inserts += 1;
            state.party_tx = Some(self.tx.id);
            Ok(stored_party(party.id()))
        }
        async fn update(
            &mut self,
            _: &Party,
            _: party_service::ports::PartyStorageVersion,
        ) -> Result<StoredParty, party_service::ports::PartyRepositoryError> {
            Err(party_error())
        }
    }
    #[derive(Clone)]
    struct FakeSources;
    struct FakeSourceRepository<'a> {
        tx: &'a mut FakeTransaction,
    }
    impl ListingSourceRepositoryFactory<FakeTransaction> for FakeSources {
        fn in_transaction<'a>(
            &'a self,
            tx: &'a mut FakeTransaction,
        ) -> impl ListingSourceRepository + 'a {
            FakeSourceRepository { tx }
        }
    }
    #[async_trait::async_trait]
    impl ListingSourceRepository for FakeSourceRepository<'_> {
        async fn find_by_id(
            &mut self,
            _: ListingSourceId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Ok(None)
        }
        async fn find_by_slug(
            &mut self,
            _: &ListingSourceSlugId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Ok(None)
        }
        async fn find_by_id_for_update(
            &mut self,
            _: ListingSourceId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Err(source_error())
        }
        async fn find_deletion_blocker(
            &mut self,
            _: ListingSourceId,
        ) -> Result<Option<crate::ports::ListingSourceDeletionBlocker>, ListingSourceRepositoryError>
        {
            Err(source_error())
        }
        async fn delete_unused(
            &mut self,
            _: ListingSourceId,
            _: ListingSourceStorageVersion,
        ) -> Result<(), ListingSourceRepositoryError> {
            Err(source_error())
        }
        async fn insert(
            &mut self,
            source: &ListingSource,
            config: &ListingSourceIngestionConfigurations,
            _: Option<&str>,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            let mut state = self.tx.state.lock().map_err(|_| source_error())?;
            if state.fail_source {
                return Err(source_error());
            }
            state.source_inserts += 1;
            state.source_tx = Some(self.tx.id);
            Ok(stored_source(source.clone(), config.clone()))
        }
        async fn update(
            &mut self,
            _: &ListingSource,
            _: &ListingSourceIngestionConfigurations,
            _: application::patch_field::PatchField<&str>,
            _: ListingSourceStorageVersion,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            Err(source_error())
        }
    }
    struct Admin(bool);
    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for Admin {
        async fn execute(
            &self,
            _: &OperationContext,
            _: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            if self.0 {
                Ok(CheckUserAdminResult)
            } else {
                Err(CheckUserAdminError::Forbidden)
            }
        }
    }
    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }
    fn command() -> CreateListingSourceCommand {
        CreateListingSourceCommand {
            name: ListingSourceName::try_from("Source")
                .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
            operator: ListingSourceOperator::New(NewParty {
                id: PartyId::new(),
                name: PartyName::try_from("Operator")
                    .unwrap_or_else(|error| panic!("invalid test party name: {error}")),
                contact: PartyContact::default(),
            }),

            ingestion_configuration: ListingSourceIngestionConfigurations(vec![
                ListingIngestionConfiguration::Woocommerce {
                    currency: None,
                    language: None,
                },
            ]),
            woocommerce_webhook_secret: Some("secret".into()),
            presentation: ListingSourcePresentation::default(),
            referral_configuration: None,
        }
    }
    fn stored_party(id: PartyId) -> StoredParty {
        StoredParty {
            party: Party::create(NewParty {
                id,
                name: PartyName::try_from("Operator")
                    .unwrap_or_else(|error| panic!("invalid test party name: {error}")),
                contact: PartyContact::default(),
            }),
            version: party_service::ports::PartyStorageVersion::INITIAL,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }
    fn stored_source(
        source: ListingSource,
        configuration: ListingSourceIngestionConfigurations,
    ) -> StoredListingSource {
        StoredListingSource {
            source,
            configuration,
            version: ListingSourceStorageVersion::INITIAL,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }
    fn party_error() -> party_service::ports::PartyRepositoryError {
        party_service::ports::PartyRepositoryError::Internal {
            source: static_error("fake party failure"),
        }
    }
    fn source_error() -> ListingSourceRepositoryError {
        ListingSourceRepositoryError::Internal {
            source: static_error("fake source failure"),
        }
    }
    #[tokio::test]
    async fn should_atomically_create_nested_party_and_listing_source() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = CreateListingSourceHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeSources,
            FakeParties,
            Admin(true),
        );
        assert!(handler.execute(&context(), command()).await.is_ok());
        let state = state
            .lock()
            .unwrap_or_else(|error| panic!("fake state poisoned: {error}"));
        assert_eq!(1, state.party_inserts);
        assert_eq!(1, state.source_inserts);
        assert_eq!(Some(7), state.party_tx);
        assert_eq!(state.party_tx, state.source_tx);
        assert_eq!(1, state.commits);
    }
    #[tokio::test]
    async fn should_reject_method_configuration_mismatch_without_persistence() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = CreateListingSourceHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeSources,
            FakeParties,
            Admin(true),
        );
        let mut command = command();
        command.ingestion_configuration =
            ListingSourceIngestionConfigurations(vec![ListingIngestionConfiguration::WebCrawl {
                fallback_currency: None,
            }]);
        assert!(matches!(
            handler.execute(&context(), command).await,
            Err(CreateListingSourceError::ListingIngestionConfigurationMismatch)
        ));
        let state = state
            .lock()
            .unwrap_or_else(|error| panic!("fake state poisoned: {error}"));
        assert_eq!(1, state.party_inserts);
        assert_eq!(0, state.source_inserts);
        assert_eq!(0, state.commits);
    }
    #[tokio::test]
    async fn should_roll_back_nested_party_when_listing_source_insert_fails() {
        let state = Arc::new(Mutex::new(State {
            fail_source: true,
            ..Default::default()
        }));
        let handler = CreateListingSourceHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeSources,
            FakeParties,
            Admin(true),
        );
        assert!(matches!(
            handler.execute(&context(), command()).await,
            Err(CreateListingSourceError::Internal { .. })
        ));
        let state = state
            .lock()
            .unwrap_or_else(|error| panic!("fake state poisoned: {error}"));
        assert_eq!(1, state.party_inserts);
        assert_eq!(0, state.commits);
    }
    #[tokio::test]
    async fn should_reject_unauthorized_create_before_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = CreateListingSourceHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeSources,
            FakeParties,
            Admin(false),
        );
        let denied = OperationContext {
            principal: Principal::Anonymous,
            ..context()
        };
        assert!(matches!(
            handler.execute(&denied, command()).await,
            Err(CreateListingSourceError::AuthenticatedActorRequired)
        ));
        let state = state
            .lock()
            .unwrap_or_else(|error| panic!("fake state poisoned: {error}"));
        assert_eq!(0, state.party_inserts);
        assert_eq!(0, state.source_inserts);
        assert_eq!(0, state.commits);
    }
}
