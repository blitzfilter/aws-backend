use crate::{
    ports::{
        AuctionEventAppendError, AuctionEventAppender, AuctionEventAppenderFactory,
        AuctionMetadataField, AuctionMetadataPolicyAudit, AuctionMetadataPolicyRepository,
        AuctionMetadataPolicyRepositoryError, AuctionRepository, AuctionRepositoryError,
        AuctionRepositoryFactory,
    },
    use_cases::queries::get_auction::AuctionAdminDetailsView,
};
use application::{
    error::{BoxError, static_error},
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use auction_core::{
    Auction, AuctionDescription, AuctionFormat, AuctionId, AuctionKey, AuctionName,
    AuctionReportedStatus, AuctionSchedule, NewAuction, ReportedCatalogueLotCount, SourceAuctionId,
};
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use std::collections::BTreeSet;
use time::OffsetDateTime;
use url::Url;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub struct CreateAuctionCommand {
    pub listing_source_id: ListingSourceId,
    pub source_auction_id: SourceAuctionId,
    pub name: Option<Localized<Language, AuctionName>>,
    pub description: Option<Localized<Language, AuctionDescription>>,
    pub catalogue_url: Option<Url>,
    pub format: Option<AuctionFormat>,
    pub schedule: AuctionSchedule,
    pub reported_status: Option<AuctionReportedStatus>,
    pub reported_lot_count: Option<ReportedCatalogueLotCount>,
}

pub type CreateAuctionResult = AuctionAdminDetailsView;

#[derive(Debug, thiserror::Error)]
pub enum CreateAuctionError {
    #[error("authenticated actor required to create auction")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("listing source not found")]
    ListingSourceNotFound,
    #[error("source auction key already exists")]
    SourceAuctionAlreadyExists,
    #[error("temporary auction persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted auction state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("auction event persistence failed")]
    EventPersistenceFailed {
        #[source]
        source: BoxError,
    },
    #[error("auction policy persistence failed")]
    PolicyPersistenceFailed {
        #[source]
        source: BoxError,
    },
    #[error("invalid auction audit actor")]
    InvalidAuditActor {
        #[source]
        source: super::super::queries::get_auction::AuctionAuditActorLabelError,
    },
    #[error("internal auction failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin create auction transaction")]
    BeginTransactionFailed,
    #[error("failed to commit create auction transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CreateAuctionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateAuctionCommand,
    ) -> Result<CreateAuctionResult, CreateAuctionError>;
}

pub struct CreateAuctionHandler<U, R, E, P, A> {
    unit_of_work: U,
    auctions: R,
    events: E,
    policies: P,
    check_user_admin: A,
}

impl<U, R, E, P, A> CreateAuctionHandler<U, R, E, P, A> {
    pub fn new(unit_of_work: U, auctions: R, events: E, policies: P, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            auctions,
            events,
            policies,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, E, P, A> CreateAuctionUseCase for CreateAuctionHandler<U, R, E, P, A>
where
    U: UnitOfWork,
    R: AuctionRepositoryFactory<U::Tx>,
    E: AuctionEventAppenderFactory<U::Tx>,
    P: crate::ports::AuctionMetadataPolicyRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(name = "create_auction", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateAuctionCommand,
    ) -> Result<CreateAuctionResult, CreateAuctionError> {
        super::super::queries::get_auction::ensure_admin(
            context,
            &self.check_user_admin,
            map_admin_error,
            CreateAuctionError::AuthenticatedActorRequired,
        )
        .await?;

        let protected_fields = initial_protected_fields(&command);
        let audit_actor_label = (!protected_fields.is_empty())
            .then(|| super::super::queries::get_auction::auction_audit_actor_label(context))
            .transpose()
            .map_err(|source| CreateAuctionError::InvalidAuditActor { source })?;
        let mut auction = Auction::create(NewAuction {
            id: AuctionId::new(),
            key: AuctionKey::new(command.listing_source_id, command.source_auction_id),
            name: command.name,
            description: command.description,
            catalogue_url: command.catalogue_url,
            format: command.format,
            schedule: command.schedule,
            reported_status: command.reported_status,
            reported_lot_count: command.reported_lot_count,
        })
        .map_err(|error| CreateAuctionError::Internal {
            source: Box::new(error),
        })?;
        let event_payload =
            auction
                .take_pending_event_payload()
                .ok_or_else(|| CreateAuctionError::Internal {
                    source: static_error("new auction has no discovery event"),
                })?;
        let event = crate::ports::stamp_auction_event(
            auction.id(),
            OffsetDateTime::now_utc(),
            event_payload,
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreateAuctionError::BeginTransactionFailed)?;
        let stored = self
            .auctions
            .in_transaction(&mut tx)
            .insert(&auction)
            .await?;
        self.events.in_transaction(&mut tx).append(&event).await?;
        if !protected_fields.is_empty() {
            self.policies
                .in_transaction(&mut tx)
                .protect(&AuctionMetadataPolicyAudit {
                    audit_id: EventId::new(),
                    auction_id: auction.id(),
                    actor_label: audit_actor_label.clone().ok_or_else(|| {
                        CreateAuctionError::Internal {
                            source: static_error("missing auction audit actor label"),
                        }
                    })?,
                    recorded_at: OffsetDateTime::now_utc(),
                    fields: protected_fields.clone(),
                })
                .await?;
        }
        tx.commit()
            .await
            .map_err(|_| CreateAuctionError::CommitTransactionFailed)?;

        tracing::info!(event = "auction.created", auction_id = %auction.id(), actor_type = context.principal.kind(), outcome = "success");
        Ok(AuctionAdminDetailsView::from_details(
            crate::ports::AuctionDetails {
                stored,
                protected_fields,
            },
        ))
    }
}

fn initial_protected_fields(command: &CreateAuctionCommand) -> BTreeSet<AuctionMetadataField> {
    let mut fields = BTreeSet::new();
    if command.name.is_some() {
        fields.insert(AuctionMetadataField::Name);
    }
    if command.description.is_some() {
        fields.insert(AuctionMetadataField::Description);
    }
    if command.catalogue_url.is_some() {
        fields.insert(AuctionMetadataField::CatalogueUrl);
    }
    if command.format.is_some() {
        fields.insert(AuctionMetadataField::Format);
    }
    if command.reported_status.is_some() {
        fields.insert(AuctionMetadataField::ReportedStatus);
    }
    if command.reported_lot_count.is_some() {
        fields.insert(AuctionMetadataField::ReportedLotCount);
    }
    if command.schedule.bidding_opens().is_some() {
        fields.insert(AuctionMetadataField::BiddingOpens);
    }
    if command.schedule.live_starts().is_some() {
        fields.insert(AuctionMetadataField::LiveStarts);
    }
    if command.schedule.lots_begin_closing().is_some() {
        fields.insert(AuctionMetadataField::LotsBeginClosing);
    }
    if command.schedule.scheduled_end().is_some() {
        fields.insert(AuctionMetadataField::ScheduledEnd);
    }
    fields
}

fn map_admin_error(error: CheckUserAdminError) -> CreateAuctionError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            CreateAuctionError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => CreateAuctionError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            CreateAuctionError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => CreateAuctionError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            CreateAuctionError::TemporarilyUnavailable {
                source: static_error("check user admin transaction failed"),
            }
        }
    }
}

impl From<AuctionRepositoryError> for CreateAuctionError {
    fn from(error: AuctionRepositoryError) -> Self {
        match error {
            AuctionRepositoryError::SourceAuctionAlreadyExists { .. } => {
                Self::SourceAuctionAlreadyExists
            }
            AuctionRepositoryError::ListingSourceNotFound { .. } => Self::ListingSourceNotFound,
            AuctionRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AuctionRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            AuctionRepositoryError::ConcurrencyConflict => Self::Internal {
                source: static_error("unexpected create auction concurrency conflict"),
            },
            AuctionRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<AuctionEventAppendError> for CreateAuctionError {
    fn from(error: AuctionEventAppendError) -> Self {
        Self::EventPersistenceFailed {
            source: Box::new(error),
        }
    }
}

impl From<AuctionMetadataPolicyRepositoryError> for CreateAuctionError {
    fn from(error: AuctionMetadataPolicyRepositoryError) -> Self {
        match error {
            AuctionMetadataPolicyRepositoryError::PersistenceFailed { source } => {
                Self::PolicyPersistenceFailed { source }
            }
            AuctionMetadataPolicyRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        AuctionEvent, AuctionMetadataPolicyRepositoryFactory, AuctionStorageVersion, StoredAuction,
    };
    use application::{
        error::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::user_id::UserId;
    use user_service::use_cases::queries::check_user_admin::{
        CheckUserAdminRequest, CheckUserAdminResult,
    };

    type SharedState = Arc<Mutex<State>>;

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        admin_checks: usize,
        inserts: usize,
        event_appends: usize,
        policy_protects: usize,
        fail_events: bool,
        fail_policies: bool,
    }

    struct FakeUnitOfWork(SharedState);
    struct FakeTransaction(SharedState);
    struct FakeAuctions(SharedState);
    struct FakeAuctionRepository(SharedState);
    struct FakeEvents(SharedState);
    struct FakeEventAppender(SharedState);
    struct FakePolicies(SharedState);
    struct FakePolicyRepository(SharedState);
    struct FakeCheckUserAdmin {
        state: SharedState,
        allowed: bool,
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.0).commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(FakeTransaction(Arc::clone(&self.0)))
        }
    }

    impl AuctionRepositoryFactory<FakeTransaction> for FakeAuctions {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl AuctionRepository + 'tx {
            FakeAuctionRepository(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl AuctionRepository for FakeAuctionRepository {
        async fn find_by_id(
            &mut self,
            _id: AuctionId,
        ) -> Result<Option<StoredAuction>, AuctionRepositoryError> {
            Err(repository_error())
        }

        async fn find_by_key(
            &mut self,
            _key: &AuctionKey,
        ) -> Result<Option<StoredAuction>, AuctionRepositoryError> {
            Err(repository_error())
        }

        async fn insert(
            &mut self,
            auction: &Auction,
        ) -> Result<StoredAuction, AuctionRepositoryError> {
            lock(&self.0).inserts += 1;
            Ok(stored(auction.clone()))
        }

        async fn update(
            &mut self,
            _auction: &Auction,
            _expected_version: AuctionStorageVersion,
        ) -> Result<StoredAuction, AuctionRepositoryError> {
            Err(repository_error())
        }
    }

    impl AuctionEventAppenderFactory<FakeTransaction> for FakeEvents {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl AuctionEventAppender + 'tx {
            FakeEventAppender(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl AuctionEventAppender for FakeEventAppender {
        async fn append(&mut self, _event: &AuctionEvent) -> Result<(), AuctionEventAppendError> {
            let mut state = lock(&self.0);
            state.event_appends += 1;
            if state.fail_events {
                return Err(AuctionEventAppendError::AuctionEventAppendFailed {
                    source: static_error("event append failed"),
                });
            }
            Ok(())
        }
    }

    impl AuctionMetadataPolicyRepositoryFactory<FakeTransaction> for FakePolicies {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl AuctionMetadataPolicyRepository + 'tx {
            FakePolicyRepository(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl AuctionMetadataPolicyRepository for FakePolicyRepository {
        async fn find_protected_fields(
            &mut self,
            _auction_id: AuctionId,
        ) -> Result<BTreeSet<AuctionMetadataField>, AuctionMetadataPolicyRepositoryError> {
            Ok(BTreeSet::new())
        }

        async fn protect(
            &mut self,
            _audit: &AuctionMetadataPolicyAudit,
        ) -> Result<(), AuctionMetadataPolicyRepositoryError> {
            let mut state = lock(&self.0);
            state.policy_protects += 1;
            if state.fail_policies {
                return Err(AuctionMetadataPolicyRepositoryError::PersistenceFailed {
                    source: static_error("policy persistence failed"),
                });
            }
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for FakeCheckUserAdmin {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            lock(&self.state).admin_checks += 1;
            if self.allowed {
                Ok(CheckUserAdminResult)
            } else {
                Err(CheckUserAdminError::Forbidden)
            }
        }
    }

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn name(value: &str) -> Localized<Language, AuctionName> {
        Localized::new(
            Language::En,
            AuctionName::try_from(value)
                .unwrap_or_else(|error| panic!("valid test auction name: {error}")),
        )
    }

    fn command() -> CreateAuctionCommand {
        CreateAuctionCommand {
            listing_source_id: ListingSourceId::new(),
            source_auction_id: SourceAuctionId::try_from("catalogue-42")
                .unwrap_or_else(|error| panic!("valid test source auction ID: {error}")),
            name: Some(name("Spring sale")),
            description: None,
            catalogue_url: None,
            format: None,
            schedule: AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
        }
    }

    fn stored(auction: Auction) -> StoredAuction {
        StoredAuction {
            auction,
            version: AuctionStorageVersion::INITIAL,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn repository_error() -> AuctionRepositoryError {
        AuctionRepositoryError::Internal {
            source: static_error("unexpected fake repository call"),
        }
    }

    fn handler(
        state: SharedState,
        allowed: bool,
    ) -> CreateAuctionHandler<
        FakeUnitOfWork,
        FakeAuctions,
        FakeEvents,
        FakePolicies,
        FakeCheckUserAdmin,
    > {
        CreateAuctionHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeAuctions(Arc::clone(&state)),
            FakeEvents(Arc::clone(&state)),
            FakePolicies(Arc::clone(&state)),
            FakeCheckUserAdmin { state, allowed },
        )
    }

    #[tokio::test]
    async fn should_authorize_before_beginning_create_transaction() {
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(Arc::clone(&state), false)
            .execute(&context(), command())
            .await;

        assert!(matches!(result, Err(CreateAuctionError::Forbidden)));
        let state = lock(&state);
        assert_eq!(1, state.admin_checks);
        assert_eq!(0, state.begins);
        assert_eq!(0, state.inserts);
    }

    #[tokio::test]
    async fn should_create_auction_append_event_protect_metadata_and_commit() {
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(Arc::clone(&state), true)
            .execute(&context(), command())
            .await;

        assert!(result.is_ok());
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.inserts);
        assert_eq!(1, state.event_appends);
        assert_eq!(1, state.policy_protects);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_leave_create_transaction_uncommitted_when_event_or_policy_persistence_fails() {
        {
            let event_state = Arc::new(Mutex::new(State {
                fail_events: true,
                ..Default::default()
            }));
            let event_result = handler(Arc::clone(&event_state), true)
                .execute(&context(), command())
                .await;

            assert!(matches!(
                event_result,
                Err(CreateAuctionError::EventPersistenceFailed { .. })
            ));
            let event_state = lock(&event_state);
            assert_eq!(1, event_state.inserts);
            assert_eq!(1, event_state.event_appends);
            assert_eq!(0, event_state.policy_protects);
            assert_eq!(0, event_state.commits);
        }

        let policy_state = Arc::new(Mutex::new(State {
            fail_policies: true,
            ..Default::default()
        }));
        let policy_result = handler(Arc::clone(&policy_state), true)
            .execute(&context(), command())
            .await;

        assert!(matches!(
            policy_result,
            Err(CreateAuctionError::PolicyPersistenceFailed { .. })
        ));
        let policy_state = lock(&policy_state);
        assert_eq!(1, policy_state.inserts);
        assert_eq!(1, policy_state.event_appends);
        assert_eq!(1, policy_state.policy_protects);
        assert_eq!(0, policy_state.commits);
    }
}
