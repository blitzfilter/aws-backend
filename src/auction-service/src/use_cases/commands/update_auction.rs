use crate::{
    ports::{
        AuctionEventAppendError, AuctionEventAppender, AuctionEventAppenderFactory,
        AuctionMetadataField, AuctionMetadataPolicyAudit, AuctionMetadataPolicyRepository,
        AuctionMetadataPolicyRepositoryError, AuctionRepository, AuctionRepositoryError,
        AuctionRepositoryFactory, AuctionStorageVersion,
    },
    use_cases::queries::get_auction::AuctionAdminDetailsView,
};
use application::{
    error::{BoxError, static_error},
    operation_context::OperationContext,
    patch_field::PatchField,
    transaction::{Transaction, UnitOfWork},
};
use auction_core::{
    AuctionDescription, AuctionFormat, AuctionId, AuctionName, AuctionReportedStatus,
    AuctionSchedule, AuctionTime, ReplaceAuctionScheduleError, ReportedCatalogueLotCount,
};
use domain_primitives::{change_outcome::ChangeOutcome, event_id::EventId};
use localization::{Language, Localized};
use std::collections::BTreeSet;
use time::OffsetDateTime;
use url::Url;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AuctionSchedulePatch {
    pub bidding_opens: PatchField<AuctionTime>,
    pub live_starts: PatchField<AuctionTime>,
    pub lots_begin_closing: PatchField<AuctionTime>,
    pub scheduled_end: PatchField<AuctionTime>,
}

impl AuctionSchedulePatch {
    fn is_changed(&self) -> bool {
        self.bidding_opens.is_changed()
            || self.live_starts.is_changed()
            || self.lots_begin_closing.is_changed()
            || self.scheduled_end.is_changed()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateAuctionCommand {
    pub auction_id: AuctionId,
    pub expected_version: AuctionStorageVersion,
    pub name: PatchField<Localized<Language, AuctionName>>,
    pub description: PatchField<Localized<Language, AuctionDescription>>,
    pub catalogue_url: PatchField<Url>,
    pub format: PatchField<AuctionFormat>,
    pub schedule: AuctionSchedulePatch,
    pub reported_status: PatchField<AuctionReportedStatus>,
    pub reported_lot_count: PatchField<ReportedCatalogueLotCount>,
}

impl UpdateAuctionCommand {
    fn protected_fields(&self) -> BTreeSet<AuctionMetadataField> {
        let mut fields = BTreeSet::new();
        if self.name.is_changed() {
            fields.insert(AuctionMetadataField::Name);
        }
        if self.description.is_changed() {
            fields.insert(AuctionMetadataField::Description);
        }
        if self.catalogue_url.is_changed() {
            fields.insert(AuctionMetadataField::CatalogueUrl);
        }
        if self.format.is_changed() {
            fields.insert(AuctionMetadataField::Format);
        }
        if self.reported_status.is_changed() {
            fields.insert(AuctionMetadataField::ReportedStatus);
        }
        if self.reported_lot_count.is_changed() {
            fields.insert(AuctionMetadataField::ReportedLotCount);
        }
        if self.schedule.bidding_opens.is_changed() {
            fields.insert(AuctionMetadataField::BiddingOpens);
        }
        if self.schedule.live_starts.is_changed() {
            fields.insert(AuctionMetadataField::LiveStarts);
        }
        if self.schedule.lots_begin_closing.is_changed() {
            fields.insert(AuctionMetadataField::LotsBeginClosing);
        }
        if self.schedule.scheduled_end.is_changed() {
            fields.insert(AuctionMetadataField::ScheduledEnd);
        }
        fields
    }
}

pub type UpdateAuctionResult = AuctionAdminDetailsView;

#[derive(Debug, thiserror::Error)]
pub enum UpdateAuctionError {
    #[error("authenticated actor required to update auction")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("auction not found")]
    NotFound,
    #[error("concurrent auction update")]
    ConcurrencyConflict,
    #[error("invalid auction schedule")]
    InvalidSchedule {
        #[source]
        source: ReplaceAuctionScheduleError,
    },
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
    #[error("failed to begin update auction transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update auction transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateAuctionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateAuctionCommand,
    ) -> Result<UpdateAuctionResult, UpdateAuctionError>;
}

pub struct UpdateAuctionHandler<U, R, E, P, A> {
    unit_of_work: U,
    auctions: R,
    events: E,
    policies: P,
    check_user_admin: A,
}

impl<U, R, E, P, A> UpdateAuctionHandler<U, R, E, P, A> {
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
impl<U, R, E, P, A> UpdateAuctionUseCase for UpdateAuctionHandler<U, R, E, P, A>
where
    U: UnitOfWork,
    R: AuctionRepositoryFactory<U::Tx>,
    E: AuctionEventAppenderFactory<U::Tx>,
    P: crate::ports::AuctionMetadataPolicyRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(name = "update_auction", skip_all, fields(auction_id = %command.auction_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateAuctionCommand,
    ) -> Result<UpdateAuctionResult, UpdateAuctionError> {
        super::super::queries::get_auction::ensure_admin(
            context,
            &self.check_user_admin,
            map_admin_error,
            UpdateAuctionError::AuthenticatedActorRequired,
        )
        .await?;
        let protected_fields = command.protected_fields();
        if protected_fields.is_empty() {
            return self.read_noop(context, command).await;
        }
        let audit_actor_label =
            super::super::queries::get_auction::auction_audit_actor_label(context)
                .map_err(|source| UpdateAuctionError::InvalidAuditActor { source })?;

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateAuctionError::BeginTransactionFailed)?;
        let stored = self
            .auctions
            .in_transaction(&mut tx)
            .find_by_id(command.auction_id)
            .await?
            .ok_or(UpdateAuctionError::NotFound)?;
        if stored.version != command.expected_version {
            return Err(UpdateAuctionError::ConcurrencyConflict);
        }
        let existing_protected_fields = self
            .policies
            .in_transaction(&mut tx)
            .find_protected_fields(command.auction_id)
            .await?;
        let mut auction = stored.auction;
        let changed = apply_update(&mut auction, &command)?;
        let event = auction.take_pending_event_payload().map(|payload| {
            crate::ports::stamp_auction_event(auction.id(), OffsetDateTime::now_utc(), payload)
        });
        // A policy touch must fence concurrent writers even if aggregate facts are equal.
        let persisted = self
            .auctions
            .in_transaction(&mut tx)
            .update(&auction, stored.version)
            .await?;
        if let Some(event) = event.as_ref() {
            self.events.in_transaction(&mut tx).append(event).await?;
        }
        self.policies
            .in_transaction(&mut tx)
            .protect(&AuctionMetadataPolicyAudit {
                audit_id: EventId::new(),
                auction_id: auction.id(),
                actor_label: audit_actor_label,
                recorded_at: OffsetDateTime::now_utc(),
                fields: protected_fields.clone(),
            })
            .await?;
        tx.commit()
            .await
            .map_err(|_| UpdateAuctionError::CommitTransactionFailed)?;

        tracing::info!(event = "auction.updated", auction_id = %auction.id(), actor_type = context.principal.kind(), changed = changed.changed(), policy_changed = true, outcome = "success");
        let mut next_protected_fields = existing_protected_fields;
        next_protected_fields.extend(protected_fields);
        Ok(AuctionAdminDetailsView::from_details(
            crate::ports::AuctionDetails {
                stored: persisted,
                protected_fields: next_protected_fields,
            },
        ))
    }
}

impl<U, R, E, P, A> UpdateAuctionHandler<U, R, E, P, A>
where
    U: UnitOfWork,
    R: AuctionRepositoryFactory<U::Tx>,
    E: AuctionEventAppenderFactory<U::Tx>,
    P: crate::ports::AuctionMetadataPolicyRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    async fn read_noop(
        &self,
        _context: &OperationContext,
        command: UpdateAuctionCommand,
    ) -> Result<UpdateAuctionResult, UpdateAuctionError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateAuctionError::BeginTransactionFailed)?;
        let stored = self
            .auctions
            .in_transaction(&mut tx)
            .find_by_id(command.auction_id)
            .await?
            .ok_or(UpdateAuctionError::NotFound)?;
        if stored.version != command.expected_version {
            return Err(UpdateAuctionError::ConcurrencyConflict);
        }
        let protected_fields = self
            .policies
            .in_transaction(&mut tx)
            .find_protected_fields(command.auction_id)
            .await?;
        tx.commit()
            .await
            .map_err(|_| UpdateAuctionError::CommitTransactionFailed)?;
        Ok(AuctionAdminDetailsView::from_details(
            crate::ports::AuctionDetails {
                stored,
                protected_fields,
            },
        ))
    }
}

fn apply_update(
    auction: &mut auction_core::Auction,
    command: &UpdateAuctionCommand,
) -> Result<ChangeOutcome, UpdateAuctionError> {
    let mut outcome = ChangeOutcome::Unchanged;
    outcome = outcome.combine(match &command.name {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Clear => auction.clear_name(),
        PatchField::Set(value) => auction.rename(value.clone()),
    });
    outcome = outcome.combine(match &command.description {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Clear => auction.clear_description(),
        PatchField::Set(value) => auction.replace_description(value.clone()),
    });
    outcome = outcome.combine(match &command.catalogue_url {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Clear => auction.clear_catalogue_url(),
        PatchField::Set(value) => auction.replace_catalogue_url(value.clone()),
    });
    outcome = outcome.combine(match &command.format {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Clear => auction.clear_format(),
        PatchField::Set(value) => auction.set_format(*value),
    });
    outcome = outcome.combine(match &command.reported_status {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Clear => auction.clear_reported_status(),
        PatchField::Set(value) => auction.set_reported_status(*value),
    });
    outcome = outcome.combine(match &command.reported_lot_count {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Clear => auction.clear_reported_lot_count(),
        PatchField::Set(value) => auction.set_reported_lot_count(*value),
    });

    if command.schedule.is_changed() {
        let current = auction.schedule();
        let schedule = AuctionSchedule::new(
            patch_option(
                current.bidding_opens().cloned(),
                &command.schedule.bidding_opens,
            ),
            patch_option(
                current.live_starts().cloned(),
                &command.schedule.live_starts,
            ),
            patch_option(
                current.lots_begin_closing().cloned(),
                &command.schedule.lots_begin_closing,
            ),
            patch_option(
                current.scheduled_end().cloned(),
                &command.schedule.scheduled_end,
            ),
        )
        .map_err(|source| UpdateAuctionError::InvalidSchedule {
            source: ReplaceAuctionScheduleError::InvalidSchedule(source),
        })?;
        outcome = outcome.combine(
            auction
                .replace_schedule(schedule)
                .map_err(|source| UpdateAuctionError::InvalidSchedule { source })?,
        );
    }
    Ok(outcome)
}

fn patch_option<T: Clone>(current: Option<T>, patch: &PatchField<T>) -> Option<T> {
    match patch {
        PatchField::Unchanged => current,
        PatchField::Set(value) => Some(value.clone()),
        PatchField::Clear => None,
    }
}

fn map_admin_error(error: CheckUserAdminError) -> UpdateAuctionError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            UpdateAuctionError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => UpdateAuctionError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            UpdateAuctionError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => UpdateAuctionError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            UpdateAuctionError::TemporarilyUnavailable {
                source: static_error("check user admin transaction failed"),
            }
        }
    }
}

impl From<AuctionRepositoryError> for UpdateAuctionError {
    fn from(error: AuctionRepositoryError) -> Self {
        match error {
            AuctionRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            AuctionRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AuctionRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            AuctionRepositoryError::SourceAuctionAlreadyExists { source }
            | AuctionRepositoryError::ListingSourceNotFound { source }
            | AuctionRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<AuctionEventAppendError> for UpdateAuctionError {
    fn from(error: AuctionEventAppendError) -> Self {
        Self::EventPersistenceFailed {
            source: Box::new(error),
        }
    }
}

impl From<AuctionMetadataPolicyRepositoryError> for UpdateAuctionError {
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
    use crate::ports::{AuctionEvent, AuctionMetadataPolicyRepositoryFactory, StoredAuction};
    use application::{
        error::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use auction_core::{AuctionKey, RehydratedAuctionState, SourceAuctionId};
    use listing_source_core::ListingSourceId;
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
        finds: usize,
        updates: usize,
        event_appends: usize,
        policy_reads: usize,
        policy_protects: usize,
        stored: Option<StoredAuction>,
        protected_fields: BTreeSet<AuctionMetadataField>,
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
    struct FakeCheckUserAdmin;

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
            let mut state = lock(&self.0);
            state.finds += 1;
            Ok(state.stored.clone())
        }

        async fn find_by_key(
            &mut self,
            _key: &AuctionKey,
        ) -> Result<Option<StoredAuction>, AuctionRepositoryError> {
            Err(repository_error())
        }

        async fn insert(
            &mut self,
            _auction: &auction_core::Auction,
        ) -> Result<StoredAuction, AuctionRepositoryError> {
            Err(repository_error())
        }

        async fn update(
            &mut self,
            auction: &auction_core::Auction,
            _expected_version: AuctionStorageVersion,
        ) -> Result<StoredAuction, AuctionRepositoryError> {
            let mut state = lock(&self.0);
            state.updates += 1;
            let version = state
                .stored
                .as_ref()
                .map(|stored| stored.version)
                .unwrap_or(AuctionStorageVersion::INITIAL);
            Ok(StoredAuction {
                auction: auction.clone(),
                version,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            })
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
            lock(&self.0).event_appends += 1;
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
            let mut state = lock(&self.0);
            state.policy_reads += 1;
            Ok(state.protected_fields.clone())
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
            Ok(CheckUserAdminResult)
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

    fn stored_auction() -> StoredAuction {
        let auction = auction_core::Auction::rehydrate(RehydratedAuctionState {
            id: AuctionId::new(),
            key: AuctionKey::new(
                ListingSourceId::new(),
                SourceAuctionId::try_from("catalogue-42")
                    .unwrap_or_else(|error| panic!("valid test source auction ID: {error}")),
            ),
            name: Some(name("Spring sale")),
            description: None,
            catalogue_url: None,
            format: None,
            schedule: AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
        })
        .unwrap_or_else(|error| panic!("valid rehydrated test auction: {error}"));
        StoredAuction {
            auction,
            version: AuctionStorageVersion::INITIAL,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn command(
        auction_id: AuctionId,
        expected_version: AuctionStorageVersion,
    ) -> UpdateAuctionCommand {
        UpdateAuctionCommand {
            auction_id,
            expected_version,
            name: PatchField::Unchanged,
            description: PatchField::Unchanged,
            catalogue_url: PatchField::Unchanged,
            format: PatchField::Unchanged,
            schedule: AuctionSchedulePatch::default(),
            reported_status: PatchField::Unchanged,
            reported_lot_count: PatchField::Unchanged,
        }
    }

    fn repository_error() -> AuctionRepositoryError {
        AuctionRepositoryError::Internal {
            source: static_error("unexpected fake repository call"),
        }
    }

    fn handler(
        state: SharedState,
    ) -> UpdateAuctionHandler<
        FakeUnitOfWork,
        FakeAuctions,
        FakeEvents,
        FakePolicies,
        FakeCheckUserAdmin,
    > {
        UpdateAuctionHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeAuctions(Arc::clone(&state)),
            FakeEvents(Arc::clone(&state)),
            FakePolicies(Arc::clone(&state)),
            FakeCheckUserAdmin,
        )
    }

    #[tokio::test]
    async fn should_not_commit_when_expected_version_does_not_match() {
        let stored = stored_auction();
        let auction_id = stored.auction.id();
        let state = Arc::new(Mutex::new(State {
            stored: Some(stored),
            ..Default::default()
        }));
        let expected_version = AuctionStorageVersion::try_from(2_i64)
            .unwrap_or_else(|error| panic!("valid test version: {error}"));
        let mut request = command(auction_id, expected_version);
        request.name = PatchField::Set(name("Summer sale"));

        let result = handler(Arc::clone(&state))
            .execute(&context(), request)
            .await;

        assert!(matches!(
            result,
            Err(UpdateAuctionError::ConcurrencyConflict)
        ));
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.finds);
        assert_eq!(0, state.policy_reads);
        assert_eq!(0, state.updates);
        assert_eq!(0, state.event_appends);
        assert_eq!(0, state.policy_protects);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_commit_read_when_update_has_no_explicit_fields() {
        let stored = stored_auction();
        let request = command(stored.auction.id(), stored.version);
        let state = Arc::new(Mutex::new(State {
            stored: Some(stored),
            ..Default::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(&context(), request)
            .await;

        assert!(result.is_ok());
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.finds);
        assert_eq!(1, state.policy_reads);
        assert_eq!(0, state.updates);
        assert_eq!(0, state.event_appends);
        assert_eq!(0, state.policy_protects);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_persist_equal_explicit_field_policy_touch_without_event() {
        let stored = stored_auction();
        let auction_id = stored.auction.id();
        let version = stored.version;
        let state = Arc::new(Mutex::new(State {
            stored: Some(stored),
            ..Default::default()
        }));
        let mut request = command(auction_id, version);
        request.name = PatchField::Set(name("Spring sale"));

        let result = handler(Arc::clone(&state))
            .execute(&context(), request)
            .await;

        let view = match result {
            Ok(view) => view,
            Err(error) => panic!("equal field update failed: {error}"),
        };
        assert!(view.protected_fields.contains(&AuctionMetadataField::Name));
        let state = lock(&state);
        assert_eq!(1, state.updates);
        assert_eq!(0, state.event_appends);
        assert_eq!(1, state.policy_protects);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_leave_update_transaction_uncommitted_when_policy_persistence_fails() {
        let stored = stored_auction();
        let auction_id = stored.auction.id();
        let version = stored.version;
        let state = Arc::new(Mutex::new(State {
            stored: Some(stored),
            fail_policies: true,
            ..Default::default()
        }));
        let mut request = command(auction_id, version);
        request.name = PatchField::Set(name("Spring sale"));

        let result = handler(Arc::clone(&state))
            .execute(&context(), request)
            .await;

        assert!(matches!(
            result,
            Err(UpdateAuctionError::PolicyPersistenceFailed { .. })
        ));
        let state = lock(&state);
        assert_eq!(1, state.updates);
        assert_eq!(0, state.event_appends);
        assert_eq!(1, state.policy_protects);
        assert_eq!(0, state.commits);
    }
}
