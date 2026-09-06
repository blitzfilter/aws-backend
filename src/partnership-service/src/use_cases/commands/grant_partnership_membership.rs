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
use user_core::user_id::UserId;
use user_service::ports::{
    UserAccountReadError, UserAccountReader, UserAccountReaderFactory, UserAdminReaderFactory,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrantPartnershipMembershipCommand {
    pub partnership_id: PartnershipId,
    pub user_id: UserId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantPartnershipMembershipOutcome {
    Added,
    AlreadyMember,
}

impl GrantPartnershipMembershipOutcome {
    pub fn changed(self) -> bool {
        matches!(self, Self::Added)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::AlreadyMember => "already_member",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrantPartnershipMembershipResult {
    pub outcome: GrantPartnershipMembershipOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum GrantPartnershipMembershipError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("partnership not found")]
    PartnershipNotFound,
    #[error("user not found")]
    UserNotFound,
    #[error("temporary partnership membership failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted partnership membership state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership membership failure")]
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
pub trait GrantPartnershipMembershipUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: GrantPartnershipMembershipCommand,
    ) -> Result<GrantPartnershipMembershipResult, GrantPartnershipMembershipError>;
}

pub struct GrantPartnershipMembershipHandler<U, P, R, M, A> {
    unit_of_work: U,
    partnerships: P,
    users: R,
    memberships: M,
    admins: A,
}

impl<U, P, R, M, A> GrantPartnershipMembershipHandler<U, P, R, M, A> {
    pub fn new(unit_of_work: U, partnerships: P, users: R, memberships: M, admins: A) -> Self {
        Self {
            unit_of_work,
            partnerships,
            users,
            memberships,
            admins,
        }
    }
}

#[async_trait::async_trait]
impl<U, P, R, M, A> GrantPartnershipMembershipUseCase
    for GrantPartnershipMembershipHandler<U, P, R, M, A>
where
    U: UnitOfWork,
    P: PartnershipRepositoryFactory<U::Tx>,
    R: UserAccountReaderFactory<U::Tx>,
    M: PartnershipMembershipRepositoryFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "grant_partnership_membership",
        skip_all,
        fields(
            partnership_id = %command.partnership_id,
            user_id = %command.user_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            changed = tracing::field::Empty,
            membership_outcome = tracing::field::Empty,
            outcome = tracing::field::Empty,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: GrantPartnershipMembershipCommand,
    ) -> Result<GrantPartnershipMembershipResult, GrantPartnershipMembershipError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let result = async {
            let mut tx = self
                .unit_of_work
                .begin()
                .await
                .map_err(|_| GrantPartnershipMembershipError::BeginTransactionFailed)?;

            authorize_admin(context, &mut tx, &self.admins).await?;

            self.partnerships
                .in_transaction(&mut tx)
                .find_by_id(command.partnership_id)
                .await?
                .ok_or(GrantPartnershipMembershipError::PartnershipNotFound)?;

            self.users
                .in_transaction(&mut tx)
                .find_by_id(command.user_id)
                .await?
                .ok_or(GrantPartnershipMembershipError::UserNotFound)?;

            let outcome = self
                .memberships
                .in_transaction(&mut tx)
                .add_member(command.user_id, command.partnership_id)
                .await?;

            tx.commit()
                .await
                .map_err(|_| GrantPartnershipMembershipError::CommitTransactionFailed)?;

            Ok(GrantPartnershipMembershipResult {
                outcome: match outcome {
                    PartnershipMembershipAddOutcome::Added => {
                        GrantPartnershipMembershipOutcome::Added
                    }
                    PartnershipMembershipAddOutcome::AlreadyMember => {
                        GrantPartnershipMembershipOutcome::AlreadyMember
                    }
                },
            })
        }
        .await;

        let actor_id = context.principal.actor_id();
        match &result {
            Ok(result) => {
                let changed = result.outcome.changed();
                let membership_outcome = result.outcome.as_str();
                tracing::Span::current().record("changed", changed);
                tracing::Span::current().record("membership_outcome", membership_outcome);
                tracing::Span::current().record("outcome", "success");
                tracing::info!(
                    event = "partnership.membership.granted",
                    action = "grant_partnership_membership",
                    actor_type = context.principal.kind(),
                    actor_id = actor_id.as_deref().unwrap_or(""),
                    target_type = "partnership_membership",
                    partnership_id = %command.partnership_id,
                    user_id = %command.user_id,
                    changed,
                    membership_outcome,
                    request_id = %context.request_id,
                    correlation_id = %context.correlation_id,
                    outcome = "success",
                );
            }
            Err(error) => {
                tracing::Span::current().record("changed", "unknown");
                tracing::Span::current().record("membership_outcome", "unknown");
                tracing::Span::current().record("outcome", "failure");
                tracing::warn!(
                    event = "partnership.membership.granted",
                    action = "grant_partnership_membership",
                    actor_type = context.principal.kind(),
                    actor_id = actor_id.as_deref().unwrap_or(""),
                    target_type = "partnership_membership",
                    partnership_id = %command.partnership_id,
                    user_id = %command.user_id,
                    changed = "unknown",
                    membership_outcome = "unknown",
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

impl From<AdminAuthorizationError> for GrantPartnershipMembershipError {
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

impl From<PartnershipRepositoryError> for GrantPartnershipMembershipError {
    fn from(value: PartnershipRepositoryError) -> Self {
        match value {
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

impl From<UserAccountReadError> for GrantPartnershipMembershipError {
    fn from(value: UserAccountReadError) -> Self {
        match value {
            UserAccountReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAccountReadError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            UserAccountReadError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<PartnershipGrantError> for GrantPartnershipMembershipError {
    fn from(value: PartnershipGrantError) -> Self {
        match value {
            PartnershipGrantError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipGrantError::Internal { source } => Self::Internal { source },
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
        partnership_id::PartnershipId,
    };
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::{role::UserRole, user_id::UserId};
    use user_service::ports::{
        UserAccountReadError, UserAccountReader, UserAccountReaderFactory, UserAdminActorView,
        UserAdminReadError, UserAdminReader, UserAdminReaderFactory, UserDetailsView,
    };

    #[derive(Default)]
    struct State {
        partnership: Option<Partnership>,
        user: Option<UserDetailsView>,
        admin: Option<UserAdminActorView>,
        partnership_error: Option<PartnershipRepositoryError>,
        user_error: Option<UserAccountReadError>,
        admin_error: Option<UserAdminReadError>,
        membership_error: Option<PartnershipGrantError>,
        membership_outcome: Option<PartnershipMembershipAddOutcome>,
        begin_fails: bool,
        commit_fails: bool,
        begins: usize,
        admin_reads: usize,
        partnership_reads: usize,
        user_reads: usize,
        membership_calls: usize,
        commit_attempts: usize,
        commits: usize,
    }

    struct FakeUnitOfWork {
        state: Arc<Mutex<State>>,
    }

    struct FakeTransaction {
        state: Arc<Mutex<State>>,
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.state);
            state.commit_attempts += 1;
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
            state.begins += 1;
            Ok(FakeTransaction {
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

    struct FakeUserAccountReader {
        state: Arc<Mutex<State>>,
    }

    struct FakeMembershipRepository {
        state: Arc<Mutex<State>>,
    }

    struct FakeAdminReader {
        state: Arc<Mutex<State>>,
    }

    impl PartnershipRepositoryFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PartnershipRepository + 'tx {
            FakePartnershipRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    impl UserAccountReaderFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl UserAccountReader + 'tx {
            FakeUserAccountReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    impl PartnershipMembershipRepositoryFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PartnershipMembershipRepository + 'tx {
            FakeMembershipRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    impl UserAdminReaderFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl UserAdminReader + 'tx {
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
            let result = {
                let mut state = lock(&self.state);
                if let Some(error) = state.partnership_error.take() {
                    return Err(error);
                }
                state
                    .partnership
                    .clone()
                    .filter(|partnership| partnership.id() == partnership_id)
                    .map(|partnership| {
                        Versioned::new(partnership, PartnershipStorageVersion::INITIAL)
                    })
            };
            lock(&self.state).partnership_reads += 1;
            Ok(result)
        }

        async fn find_or_create_for_party(
            &mut self,
            _party_id: party_core::party_id::PartyId,
            _new_partnership_id: PartnershipId,
        ) -> Result<VersionedPartnership, PartnershipRepositoryError> {
            Err(PartnershipRepositoryError::Internal {
                source: static_error("unexpected partnership creation"),
            })
        }
    }

    #[async_trait::async_trait]
    impl UserAccountReader for FakeUserAccountReader {
        async fn find_by_id(
            &mut self,
            user_id: UserId,
        ) -> Result<Option<UserDetailsView>, UserAccountReadError> {
            let mut state = lock(&self.state);
            state.user_reads += 1;
            if let Some(error) = state.user_error.take() {
                return Err(error);
            }
            Ok(state.user.clone().filter(|user| user.user_id == user_id))
        }
    }

    #[async_trait::async_trait]
    impl PartnershipMembershipRepository for FakeMembershipRepository {
        async fn add_member(
            &mut self,
            _user_id: UserId,
            _partnership_id: PartnershipId,
        ) -> Result<PartnershipMembershipAddOutcome, PartnershipGrantError> {
            let mut state = lock(&self.state);
            state.membership_calls += 1;
            if let Some(error) = state.membership_error.take() {
                return Err(error);
            }
            Ok(state
                .membership_outcome
                .unwrap_or(PartnershipMembershipAddOutcome::Added))
        }

        async fn remove_member(
            &mut self,
            _user_id: UserId,
            _partnership_id: PartnershipId,
        ) -> Result<PartnershipMembershipRemoveOutcome, PartnershipGrantError> {
            Err(PartnershipGrantError::Internal {
                source: static_error("unexpected membership removal"),
            })
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
            if let Some(error) = state.admin_error.take() {
                return Err(error);
            }
            Ok(state.admin.clone())
        }
    }

    fn lock(state: &Arc<Mutex<State>>) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(state) => state,
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

    fn user(user_id: UserId) -> UserDetailsView {
        UserDetailsView {
            user_id,
            email: "member@example.com"
                .parse()
                .unwrap_or_else(|error| panic!("valid test email: {error}")),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
            measurement_unit: None,
            show_unassessed_or_sensitive_content: false,
            tier: user_core::tier::UserTier::Free,
            role: UserRole::User,
            stripe_customer_id: None,
        }
    }

    fn valid_state() -> (State, PartnershipId, UserId) {
        let partnership_id = PartnershipId::new();
        let user_id = UserId::new();
        (
            State {
                partnership: Some(Partnership::create(NewPartnership {
                    id: partnership_id,
                    party_id: party_core::party_id::PartyId::new(),
                })),
                user: Some(user(user_id)),
                admin: Some(UserAdminActorView {
                    user_id: UserId::new(),
                    role: UserRole::Admin,
                }),
                membership_outcome: Some(PartnershipMembershipAddOutcome::Added),
                ..State::default()
            },
            partnership_id,
            user_id,
        )
    }

    type Handler = GrantPartnershipMembershipHandler<
        FakeUnitOfWork,
        FakeFactories,
        FakeFactories,
        FakeFactories,
        FakeFactories,
    >;

    fn handler(state: Arc<Mutex<State>>) -> Handler {
        let factories = FakeFactories {
            state: Arc::clone(&state),
        };
        GrantPartnershipMembershipHandler::new(
            FakeUnitOfWork { state },
            factories.clone(),
            factories.clone(),
            factories.clone(),
            factories,
        )
    }

    #[tokio::test]
    async fn should_add_membership_and_commit_one_transaction() {
        let (state, partnership_id, user_id) = valid_state();
        let state = Arc::new(Mutex::new(state));
        let result = handler(Arc::clone(&state))
            .execute(
                &context(),
                GrantPartnershipMembershipCommand {
                    partnership_id,
                    user_id,
                },
            )
            .await;

        assert!(matches!(
            result,
            Ok(GrantPartnershipMembershipResult {
                outcome: GrantPartnershipMembershipOutcome::Added
            })
        ));
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.admin_reads);
        assert_eq!(1, state.partnership_reads);
        assert_eq!(1, state.user_reads);
        assert_eq!(1, state.membership_calls);
        assert_eq!(1, state.commit_attempts);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_commit_existing_membership_as_successful_no_op() {
        let (mut state, partnership_id, user_id) = valid_state();
        state.membership_outcome = Some(PartnershipMembershipAddOutcome::AlreadyMember);
        let state = Arc::new(Mutex::new(state));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(),
                GrantPartnershipMembershipCommand {
                    partnership_id,
                    user_id,
                },
            )
            .await;

        assert!(matches!(
            result,
            Ok(GrantPartnershipMembershipResult {
                outcome: GrantPartnershipMembershipOutcome::AlreadyMember
            })
        ));
        assert_eq!(1, lock(&state).commits);
    }

    #[tokio::test]
    async fn should_reject_missing_partnership_without_membership_write() {
        let (mut state, partnership_id, user_id) = valid_state();
        state.partnership = None;
        let state = Arc::new(Mutex::new(state));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(),
                GrantPartnershipMembershipCommand {
                    partnership_id,
                    user_id,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(GrantPartnershipMembershipError::PartnershipNotFound)
        ));
        let state = lock(&state);
        assert_eq!(0, state.user_reads);
        assert_eq!(0, state.membership_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_reject_missing_user_without_membership_write() {
        let (mut state, partnership_id, user_id) = valid_state();
        state.user = None;
        let state = Arc::new(Mutex::new(state));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(),
                GrantPartnershipMembershipCommand {
                    partnership_id,
                    user_id,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(GrantPartnershipMembershipError::UserNotFound)
        ));
        let state = lock(&state);
        assert_eq!(0, state.membership_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_reject_non_admin_before_target_reads() {
        let (mut state, partnership_id, user_id) = valid_state();
        state.admin = Some(UserAdminActorView {
            user_id: UserId::new(),
            role: UserRole::User,
        });
        let state = Arc::new(Mutex::new(state));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(),
                GrantPartnershipMembershipCommand {
                    partnership_id,
                    user_id,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(GrantPartnershipMembershipError::Forbidden)
        ));
        let state = lock(&state);
        assert_eq!(0, state.partnership_reads);
        assert_eq!(0, state.user_reads);
        assert_eq!(0, state.membership_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_leave_transaction_uncommitted_when_membership_persistence_fails() {
        let (mut state, partnership_id, user_id) = valid_state();
        state.membership_error = Some(PartnershipGrantError::Internal {
            source: static_error("membership insert failed"),
        });
        let state = Arc::new(Mutex::new(state));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(),
                GrantPartnershipMembershipCommand {
                    partnership_id,
                    user_id,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(GrantPartnershipMembershipError::Internal { .. })
        ));
        let state = lock(&state);
        assert_eq!(1, state.membership_calls);
        assert_eq!(0, state.commit_attempts);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_report_begin_and_commit_failures() {
        let (mut begin_state, partnership_id, user_id) = valid_state();
        begin_state.begin_fails = true;
        let begin_state = Arc::new(Mutex::new(begin_state));
        let begin_result = handler(Arc::clone(&begin_state))
            .execute(
                &context(),
                GrantPartnershipMembershipCommand {
                    partnership_id,
                    user_id,
                },
            )
            .await;
        assert!(matches!(
            begin_result,
            Err(GrantPartnershipMembershipError::BeginTransactionFailed)
        ));

        let (mut commit_state, partnership_id, user_id) = valid_state();
        commit_state.commit_fails = true;
        let commit_state = Arc::new(Mutex::new(commit_state));
        let commit_result = handler(Arc::clone(&commit_state))
            .execute(
                &context(),
                GrantPartnershipMembershipCommand {
                    partnership_id,
                    user_id,
                },
            )
            .await;
        assert!(matches!(
            commit_result,
            Err(GrantPartnershipMembershipError::CommitTransactionFailed)
        ));
        assert_eq!(1, lock(&commit_state).commit_attempts);
    }
}
