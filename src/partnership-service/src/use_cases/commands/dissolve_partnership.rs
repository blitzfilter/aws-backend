use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::*,
};
use application::{
    error::BoxError,
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use partnership_core::partnership_id::PartnershipId;
use user_service::ports::UserAdminReaderFactory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DissolvePartnershipCommand {
    pub partnership_id: PartnershipId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DissolvePartnershipOutcome {
    Dissolved,
    AlreadyDissolved,
}

impl DissolvePartnershipOutcome {
    pub fn changed(self) -> bool {
        matches!(self, Self::Dissolved)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dissolved => "dissolved",
            Self::AlreadyDissolved => "already_dissolved",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DissolvePartnershipResult {
    pub outcome: DissolvePartnershipOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum DissolvePartnershipError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("partnership not found")]
    PartnershipNotFound,
    #[error("concurrent partnership update")]
    ConcurrencyConflict,
    #[error("temporary partnership dissolution failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted partnership dissolution state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership dissolution failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin transaction")]
    BeginTransactionFailed,
    #[error("failed to commit transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait DissolvePartnershipUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DissolvePartnershipCommand,
    ) -> Result<DissolvePartnershipResult, DissolvePartnershipError>;
}

pub struct DissolvePartnershipHandler<U, P, A> {
    unit_of_work: U,
    partnerships: P,
    admins: A,
}

impl<U, P, A> DissolvePartnershipHandler<U, P, A> {
    pub fn new(unit_of_work: U, partnerships: P, admins: A) -> Self {
        Self {
            unit_of_work,
            partnerships,
            admins,
        }
    }
}

#[async_trait::async_trait]
impl<U, P, A> DissolvePartnershipUseCase for DissolvePartnershipHandler<U, P, A>
where
    U: UnitOfWork,
    P: PartnershipRepositoryFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "dissolve_partnership",
        skip_all,
        fields(
            partnership_id = %command.partnership_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            changed = tracing::field::Empty,
            lifecycle_outcome = tracing::field::Empty,
            outcome = tracing::field::Empty,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: DissolvePartnershipCommand,
    ) -> Result<DissolvePartnershipResult, DissolvePartnershipError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let result = async {
            let mut tx = self
                .unit_of_work
                .begin()
                .await
                .map_err(|_| DissolvePartnershipError::BeginTransactionFailed)?;

            authorize_admin(context, &mut tx, &self.admins).await?;

            let mut partnership = self
                .partnerships
                .in_transaction(&mut tx)
                .find_by_id(command.partnership_id)
                .await?
                .ok_or(DissolvePartnershipError::PartnershipNotFound)?;

            let outcome = if partnership.value.dissolve() {
                self.partnerships
                    .in_transaction(&mut tx)
                    .dissolve(&partnership.value, partnership.version)
                    .await?;
                DissolvePartnershipOutcome::Dissolved
            } else {
                DissolvePartnershipOutcome::AlreadyDissolved
            };

            tx.commit()
                .await
                .map_err(|_| DissolvePartnershipError::CommitTransactionFailed)?;

            Ok(DissolvePartnershipResult { outcome })
        }
        .await;

        let actor_id = context.principal.actor_id();
        match &result {
            Ok(result) => {
                let changed = result.outcome.changed();
                let lifecycle_outcome = result.outcome.as_str();
                tracing::Span::current().record("changed", changed);
                tracing::Span::current().record("lifecycle_outcome", lifecycle_outcome);
                tracing::Span::current().record("outcome", "success");
                tracing::info!(
                    event = "partnership.dissolved",
                    action = "dissolve_partnership",
                    actor_type = context.principal.kind(),
                    actor_id = actor_id.as_deref().unwrap_or(""),
                    target_type = "partnership",
                    partnership_id = %command.partnership_id,
                    changed,
                    lifecycle_outcome,
                    request_id = %context.request_id,
                    correlation_id = %context.correlation_id,
                    outcome = "success",
                );
            }
            Err(error) => {
                tracing::Span::current().record("changed", "unknown");
                tracing::Span::current().record("lifecycle_outcome", "unknown");
                tracing::Span::current().record("outcome", "failure");
                tracing::warn!(
                    event = "partnership.dissolved",
                    action = "dissolve_partnership",
                    actor_type = context.principal.kind(),
                    actor_id = actor_id.as_deref().unwrap_or(""),
                    target_type = "partnership",
                    partnership_id = %command.partnership_id,
                    changed = "unknown",
                    lifecycle_outcome = "unknown",
                    request_id = %context.request_id,
                    correlation_id = %context.correlation_id,
                    error_category = %error,
                    outcome = "failure",
                );
            }
        }

        result
    }
}

impl From<AdminAuthorizationError> for DissolvePartnershipError {
    fn from(value: AdminAuthorizationError) -> Self {
        match value {
            AdminAuthorizationError::Forbidden => Self::Forbidden,
            AdminAuthorizationError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AdminAuthorizationError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            AdminAuthorizationError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<PartnershipRepositoryError> for DissolvePartnershipError {
    fn from(value: PartnershipRepositoryError) -> Self {
        match value {
            PartnershipRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            PartnershipRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnershipRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        error::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use domain_primitives::versioned::Versioned;
    use partnership_core::{
        partnership::{NewPartnership, Partnership},
        partnership_lifecycle::PartnershipLifecycle,
    };
    use party_core::party_id::PartyId;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::{role::UserRole, user_id::UserId};
    use user_service::ports::{
        UserAdminActorView, UserAdminReadError, UserAdminReader, UserAdminReaderFactory,
    };

    #[derive(Default)]
    struct State {
        partnership: Option<Partnership>,
        admin: Option<UserAdminActorView>,
        partnership_error: Option<PartnershipRepositoryError>,
        dissolve_error: Option<PartnershipRepositoryError>,
        begin_fails: bool,
        commit_fails: bool,
        transaction_id: usize,
        partnership_transaction_ids: Vec<usize>,
        admin_transaction_ids: Vec<usize>,
        partnership_reads: usize,
        dissolve_calls: usize,
        admin_reads: usize,
        commits: usize,
    }

    struct FakeUnitOfWork {
        state: Arc<Mutex<State>>,
    }

    struct FakeTransaction {
        id: usize,
        state: Arc<Mutex<State>>,
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.state);
            if state.commit_fails {
                return Err(TransactionError::CommitFailed);
            }
            state.commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = lock(&self.state);
            if state.begin_fails {
                return Err(TransactionError::BeginFailed);
            }
            state.transaction_id += 1;
            let id = state.transaction_id;
            drop(state);
            Ok(FakeTransaction {
                id,
                state: Arc::clone(&self.state),
            })
        }
    }

    #[derive(Clone)]
    struct FakeFactories {
        state: Arc<Mutex<State>>,
    }

    struct FakePartnershipRepository {
        state: Arc<Mutex<State>>,
    }

    struct FakeAdminReader {
        state: Arc<Mutex<State>>,
    }

    impl PartnershipRepositoryFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut FakeTransaction,
        ) -> impl PartnershipRepository + 'tx {
            lock(&self.state).partnership_transaction_ids.push(tx.id);
            FakePartnershipRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    impl UserAdminReaderFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut FakeTransaction,
        ) -> impl UserAdminReader + 'tx {
            lock(&self.state).admin_transaction_ids.push(tx.id);
            FakeAdminReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnershipRepository for FakePartnershipRepository {
        async fn find_by_id(
            &mut self,
            partnership_id: PartnershipId,
        ) -> Result<Option<VersionedPartnership>, PartnershipRepositoryError> {
            let mut state = lock(&self.state);
            state.partnership_reads += 1;
            if let Some(error) = state.partnership_error.take() {
                return Err(error);
            }
            Ok(state
                .partnership
                .clone()
                .filter(|partnership| partnership.id() == partnership_id)
                .map(|partnership| Versioned::new(partnership, PartnershipStorageVersion::INITIAL)))
        }

        async fn find_or_create_for_party(
            &mut self,
            _party_id: PartyId,
            _new_partnership_id: PartnershipId,
        ) -> Result<VersionedPartnership, PartnershipRepositoryError> {
            Err(PartnershipRepositoryError::Internal {
                source: static_error("unexpected partnership creation"),
            })
        }

        async fn dissolve(
            &mut self,
            partnership: &Partnership,
            _expected_version: PartnershipStorageVersion,
        ) -> Result<VersionedPartnership, PartnershipRepositoryError> {
            let mut state = lock(&self.state);
            state.dissolve_calls += 1;
            if let Some(error) = state.dissolve_error.take() {
                return Err(error);
            }
            state.partnership = Some(partnership.clone());
            Ok(Versioned::new(
                partnership.clone(),
                PartnershipStorageVersion::INITIAL,
            ))
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeAdminReader {
        async fn find_admin_actor(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            let mut state = lock(&self.state);
            state.admin_reads += 1;
            Ok(state.admin.clone())
        }
    }

    fn lock(state: &Arc<Mutex<State>>) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context(actor_id: UserId) -> OperationContext {
        OperationContext {
            principal: Principal::User(actor_id),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn partnership(partnership_id: PartnershipId) -> Partnership {
        Partnership::create(NewPartnership {
            id: partnership_id,
            party_id: PartyId::new(),
        })
    }

    fn state_with_admin(partnership: Option<Partnership>, actor_id: UserId) -> State {
        State {
            partnership,
            admin: Some(UserAdminActorView {
                user_id: actor_id,
                role: UserRole::Admin,
            }),
            ..Default::default()
        }
    }

    fn handler(
        state: Arc<Mutex<State>>,
    ) -> DissolvePartnershipHandler<FakeUnitOfWork, FakeFactories, FakeFactories> {
        DissolvePartnershipHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(&state),
            },
            FakeFactories {
                state: Arc::clone(&state),
            },
            FakeFactories { state },
        )
    }

    #[tokio::test]
    async fn should_dissolve_active_partnership_in_one_transaction() {
        let partnership_id = PartnershipId::new();
        let actor_id = UserId::new();
        let state = Arc::new(Mutex::new(state_with_admin(
            Some(partnership(partnership_id)),
            actor_id,
        )));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(actor_id),
                DissolvePartnershipCommand { partnership_id },
            )
            .await;

        assert!(matches!(
            result,
            Ok(DissolvePartnershipResult {
                outcome: DissolvePartnershipOutcome::Dissolved,
            })
        ));
        let state = lock(&state);
        assert_eq!(1, state.admin_reads);
        assert_eq!(1, state.partnership_reads);
        assert_eq!(1, state.dissolve_calls);
        assert_eq!(1, state.commits);
        assert_eq!(vec![1], state.admin_transaction_ids);
        assert_eq!(vec![1, 1], state.partnership_transaction_ids);
        assert_eq!(
            Some(PartnershipLifecycle::Dissolved),
            state.partnership.as_ref().map(Partnership::lifecycle)
        );
    }

    #[tokio::test]
    async fn should_succeed_without_persistence_when_already_dissolved() {
        let partnership_id = PartnershipId::new();
        let actor_id = UserId::new();
        let mut dissolved_partnership = partnership(partnership_id);
        assert!(dissolved_partnership.dissolve());
        let state = Arc::new(Mutex::new(state_with_admin(
            Some(dissolved_partnership),
            actor_id,
        )));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(actor_id),
                DissolvePartnershipCommand { partnership_id },
            )
            .await;

        assert!(matches!(
            result,
            Ok(DissolvePartnershipResult {
                outcome: DissolvePartnershipOutcome::AlreadyDissolved,
            })
        ));
        let state = lock(&state);
        assert_eq!(1, state.partnership_reads);
        assert_eq!(0, state.dissolve_calls);
        assert_eq!(1, state.commits);
        assert_eq!(vec![1], state.partnership_transaction_ids);
    }

    #[tokio::test]
    async fn should_return_not_found_without_commit_when_partnership_is_missing() {
        let partnership_id = PartnershipId::new();
        let actor_id = UserId::new();
        let state = Arc::new(Mutex::new(state_with_admin(None, actor_id)));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(actor_id),
                DissolvePartnershipCommand { partnership_id },
            )
            .await;

        assert!(matches!(
            result,
            Err(DissolvePartnershipError::PartnershipNotFound)
        ));
        let state = lock(&state);
        assert_eq!(1, state.partnership_reads);
        assert_eq!(0, state.dissolve_calls);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_reject_non_admin_before_partnership_lookup() {
        let partnership_id = PartnershipId::new();
        let actor_id = UserId::new();
        let state = Arc::new(Mutex::new(State {
            partnership: Some(partnership(partnership_id)),
            admin: Some(UserAdminActorView {
                user_id: actor_id,
                role: UserRole::User,
            }),
            ..Default::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(actor_id),
                DissolvePartnershipCommand { partnership_id },
            )
            .await;

        assert!(matches!(result, Err(DissolvePartnershipError::Forbidden)));
        let state = lock(&state);
        assert_eq!(1, state.admin_reads);
        assert_eq!(0, state.partnership_reads);
        assert_eq!(0, state.dissolve_calls);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_map_optimistic_concurrency_conflict_without_commit() {
        let partnership_id = PartnershipId::new();
        let actor_id = UserId::new();
        let mut initial_state = state_with_admin(Some(partnership(partnership_id)), actor_id);
        initial_state.dissolve_error = Some(PartnershipRepositoryError::ConcurrencyConflict);
        let state = Arc::new(Mutex::new(initial_state));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(actor_id),
                DissolvePartnershipCommand { partnership_id },
            )
            .await;

        assert!(matches!(
            result,
            Err(DissolvePartnershipError::ConcurrencyConflict)
        ));
        let state = lock(&state);
        assert_eq!(1, state.dissolve_calls);
        assert_eq!(0, state.commits);
    }
}
