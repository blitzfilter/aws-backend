use crate::ports::*;
use application::{
    error::BoxError,
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use partnership_core::{
    partnership_application::PartnershipApplication,
    partnership_application_id::PartnershipApplicationId,
};
#[derive(Debug, Clone, PartialEq)]
pub struct WithdrawPartnershipApplicationCommand {
    pub application_id: PartnershipApplicationId,
}
#[derive(Debug, Clone, PartialEq)]
pub struct WithdrawPartnershipApplicationResult {
    pub application: PartnershipApplication,
}
#[derive(Debug, thiserror::Error)]
pub enum WithdrawPartnershipApplicationError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("partnership application not found")]
    NotFound,
    #[error("partnership application is not withdrawable")]
    ApplicationNotWithdrawable,
    #[error("concurrent partnership application update")]
    ConcurrencyConflict,
    #[error("temporary failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal failure")]
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
pub trait WithdrawPartnershipApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: WithdrawPartnershipApplicationCommand,
    ) -> Result<WithdrawPartnershipApplicationResult, WithdrawPartnershipApplicationError>;
}
pub struct WithdrawPartnershipApplicationHandler<U, A> {
    unit_of_work: U,
    applications: A,
}
impl<U, A> WithdrawPartnershipApplicationHandler<U, A> {
    pub fn new(unit_of_work: U, applications: A) -> Self {
        Self {
            unit_of_work,
            applications,
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, A: PartnershipApplicationRepositoryFactory<U::Tx>>
    WithdrawPartnershipApplicationUseCase for WithdrawPartnershipApplicationHandler<U, A>
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: WithdrawPartnershipApplicationCommand,
    ) -> Result<WithdrawPartnershipApplicationResult, WithdrawPartnershipApplicationError> {
        let user = match context.principal {
            Principal::User(user) | Principal::DelegatedUser { user_id: user, .. } => user,
            Principal::Anonymous => {
                return Err(WithdrawPartnershipApplicationError::AuthenticatedActorRequired);
            }
            Principal::Service(_) | Principal::System => {
                return Err(WithdrawPartnershipApplicationError::Forbidden);
            }
        };
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| WithdrawPartnershipApplicationError::BeginTransactionFailed)?;
        let mut application = self
            .applications
            .in_transaction(&mut tx)
            .find_by_user_and_id(user, command.application_id)
            .await?
            .ok_or(WithdrawPartnershipApplicationError::NotFound)?;
        application
            .value
            .withdraw()
            .map_err(|_| WithdrawPartnershipApplicationError::ApplicationNotWithdrawable)?;
        let application = self
            .applications
            .in_transaction(&mut tx)
            .update(&application.value, application.version)
            .await?
            .value;
        tx.commit()
            .await
            .map_err(|_| WithdrawPartnershipApplicationError::CommitTransactionFailed)?;
        Ok(WithdrawPartnershipApplicationResult { application })
    }
}
impl From<PartnershipApplicationRepositoryError> for WithdrawPartnershipApplicationError {
    fn from(value: PartnershipApplicationRepositoryError) -> Self {
        match value {
            PartnershipApplicationRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            PartnershipApplicationRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipApplicationRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnershipApplicationRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        PartnershipApplicationRepository, PartnershipApplicationRepositoryError,
        PartnershipApplicationRepositoryFactory, PartnershipApplicationStorageVersion,
        VersionedPartnershipApplication,
    };
    use application::{
        error::static_error,
        operation_context::{CorrelationId, OperationContext, Principal, RequestId},
        transaction::{Transaction, TransactionError, UnitOfWork},
    };
    use domain_primitives::versioned::Versioned;
    use listing_source_core::ListingSourceId;
    use partnership_core::partnership_application_id::PartnershipApplicationId;
    use partnership_core::{
        partnership_application::{
            NewPartnershipApplication, PartnershipApplication,
            PartnershipApplicationApprovalResult, PartnershipProposal,
        },
        partnership_application_state::PartnershipApplicationState,
    };
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct State {
        begin_attempts: usize,
        begins: usize,
        commit_attempts: usize,
        commits: usize,
        next_transaction_id: usize,
        factory_transaction_ids: Vec<usize>,
        find_transaction_ids: Vec<usize>,
        update_transaction_ids: Vec<usize>,
        find_calls: usize,
        update_calls: usize,
        find_user_id: Option<UserId>,
        find_application_id: Option<PartnershipApplicationId>,
        update_expected_version: Option<PartnershipApplicationStorageVersion>,
        update_application: Option<PartnershipApplication>,
        application: Option<PartnershipApplication>,
        application_version: PartnershipApplicationStorageVersion,
        update_result: Option<PartnershipApplication>,
        find_error: Option<PartnershipApplicationRepositoryError>,
        update_error: Option<PartnershipApplicationRepositoryError>,
        begin_fails: bool,
        commit_fails: bool,
    }

    #[derive(Clone)]
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
            state.begin_attempts += 1;
            if state.begin_fails {
                return Err(TransactionError::BeginFailed);
            }
            state.begins += 1;
            state.next_transaction_id += 1;
            let id = state.next_transaction_id;
            drop(state);
            Ok(FakeTransaction {
                id,
                state: Arc::clone(&self.state),
            })
        }
    }

    #[derive(Clone)]
    struct FakeRepositoryFactory {
        state: Arc<Mutex<State>>,
    }

    struct FakeRepository {
        id: usize,
        state: Arc<Mutex<State>>,
    }

    impl PartnershipApplicationRepositoryFactory<FakeTransaction> for FakeRepositoryFactory {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut FakeTransaction,
        ) -> impl PartnershipApplicationRepository + 'tx {
            let id = tx.id;
            lock(&self.state).factory_transaction_ids.push(id);
            FakeRepository {
                id,
                state: Arc::clone(&self.state),
            }
        }
    }

    fn unexpected_repository_call() -> PartnershipApplicationRepositoryError {
        PartnershipApplicationRepositoryError::Internal {
            source: static_error("unexpected repository call"),
        }
    }

    #[async_trait::async_trait]
    impl PartnershipApplicationRepository for FakeRepository {
        async fn find_by_id(
            &mut self,
            _id: PartnershipApplicationId,
        ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
        {
            Err(unexpected_repository_call())
        }

        async fn find_by_id_for_update(
            &mut self,
            _id: PartnershipApplicationId,
        ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
        {
            Err(unexpected_repository_call())
        }

        async fn find_by_user_and_id(
            &mut self,
            user_id: UserId,
            id: PartnershipApplicationId,
        ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
        {
            let mut state = lock(&self.state);
            state.find_calls += 1;
            state.find_transaction_ids.push(self.id);
            state.find_user_id = Some(user_id);
            state.find_application_id = Some(id);
            if let Some(error) = state.find_error.take() {
                return Err(error);
            }
            Ok(state
                .application
                .clone()
                .filter(|application| {
                    application.applicant_user_id() == user_id && application.id() == id
                })
                .map(|application| Versioned::new(application, state.application_version)))
        }

        async fn insert(
            &mut self,
            _application: &PartnershipApplication,
        ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>
        {
            Err(unexpected_repository_call())
        }

        async fn update(
            &mut self,
            application: &PartnershipApplication,
            expected: PartnershipApplicationStorageVersion,
        ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>
        {
            let mut state = lock(&self.state);
            state.update_calls += 1;
            state.update_transaction_ids.push(self.id);
            state.update_expected_version = Some(expected);
            state.update_application = Some(application.clone());
            if let Some(error) = state.update_error.take() {
                return Err(error);
            }
            let persisted = match state.update_result.clone() {
                Some(application) => application,
                None => application.clone(),
            };
            Ok(Versioned::new(persisted, expected.next()))
        }
    }

    fn lock(state: &Arc<Mutex<State>>) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("withdraw-request"),
            correlation_id: CorrelationId::new("withdraw-correlation"),
        }
    }

    fn proposal() -> PartnershipProposal {
        PartnershipProposal::ExistingListingSource {
            listing_source_id: ListingSourceId::new(),
        }
    }

    fn submitted_application(
        id: PartnershipApplicationId,
        applicant_user_id: UserId,
        proposal: PartnershipProposal,
    ) -> PartnershipApplication {
        PartnershipApplication::submit(NewPartnershipApplication {
            id,
            applicant_user_id,
            proposal,
        })
    }

    fn approved_application(
        id: PartnershipApplicationId,
        applicant_user_id: UserId,
        proposal: PartnershipProposal,
    ) -> PartnershipApplication {
        let mut application = submitted_application(id, applicant_user_id, proposal);
        assert!(application.mark_in_review().is_ok());
        assert!(
            application
                .approve(PartnershipApplicationApprovalResult::new(
                    partnership_core::partnership_id::PartnershipId::new(),
                    ListingSourceId::new(),
                ))
                .is_ok()
        );
        application
    }

    fn withdrawn_application(
        id: PartnershipApplicationId,
        applicant_user_id: UserId,
        proposal: PartnershipProposal,
    ) -> PartnershipApplication {
        let mut application = submitted_application(id, applicant_user_id, proposal);
        assert!(application.withdraw().is_ok());
        application
    }

    fn command(application_id: PartnershipApplicationId) -> WithdrawPartnershipApplicationCommand {
        WithdrawPartnershipApplicationCommand { application_id }
    }

    fn handler(
        state: Arc<Mutex<State>>,
    ) -> WithdrawPartnershipApplicationHandler<FakeUnitOfWork, FakeRepositoryFactory> {
        WithdrawPartnershipApplicationHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(&state),
            },
            FakeRepositoryFactory { state },
        )
    }

    #[tokio::test]
    async fn should_withdraw_own_application_in_one_transaction_with_expected_version() {
        let applicant_user_id = UserId::new();
        let application_id = PartnershipApplicationId::new();
        let proposal = proposal();
        let found = submitted_application(application_id, applicant_user_id, proposal.clone());
        let persisted = withdrawn_application(application_id, applicant_user_id, proposal.clone());
        let expected_version = PartnershipApplicationStorageVersion::INITIAL.next().next();
        let state = Arc::new(Mutex::new(State {
            application: Some(found),
            application_version: expected_version,
            update_result: Some(persisted.clone()),
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(applicant_user_id)),
                command(application_id),
            )
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => panic!("withdrawal failed: {error}"),
        };

        assert_eq!(persisted, result.application);
        assert_eq!(application_id, result.application.id());
        assert_eq!(applicant_user_id, result.application.applicant_user_id());
        assert_eq!(
            PartnershipApplicationState::Withdrawn,
            result.application.state()
        );
        assert_eq!(Some(&proposal), Some(result.application.proposal()));
        assert_eq!(None, result.application.approval_result());
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.find_calls);
        assert_eq!(1, state.update_calls);
        assert_eq!(1, state.commits);
        assert_eq!(Some(applicant_user_id), state.find_user_id);
        assert_eq!(Some(application_id), state.find_application_id);
        assert_eq!(Some(expected_version), state.update_expected_version);
        assert_eq!(vec![1, 1], state.factory_transaction_ids);
        assert_eq!(vec![1], state.find_transaction_ids);
        assert_eq!(vec![1], state.update_transaction_ids);
    }

    #[tokio::test]
    async fn should_withdraw_for_delegated_own_user() {
        let applicant_user_id = UserId::new();
        let application_id = PartnershipApplicationId::new();
        let state = Arc::new(Mutex::new(State {
            application: Some(submitted_application(
                application_id,
                applicant_user_id,
                proposal(),
            )),
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::DelegatedUser {
                    user_id: applicant_user_id,
                    capabilities: BTreeSet::new(),
                }),
                command(application_id),
            )
            .await;

        assert!(result.is_ok());
        let state = lock(&state);
        assert_eq!(Some(applicant_user_id), state.find_user_id);
        assert_eq!(1, state.update_calls);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_reject_anonymous_before_beginning_withdrawal_transaction() {
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::Anonymous),
                command(PartnershipApplicationId::new()),
            )
            .await;

        assert!(matches!(
            result,
            Err(WithdrawPartnershipApplicationError::AuthenticatedActorRequired)
        ));
        let state = lock(&state);
        assert_eq!(0, state.begin_attempts);
        assert_eq!(0, state.find_calls);
        assert_eq!(0, state.update_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_scope_withdrawal_to_authenticated_actor() {
        let actor_user_id = UserId::new();
        let applicant_user_id = UserId::new();
        let application_id = PartnershipApplicationId::new();
        assert_ne!(actor_user_id, applicant_user_id);

        for principal in [
            Principal::User(actor_user_id),
            Principal::DelegatedUser {
                user_id: actor_user_id,
                capabilities: BTreeSet::new(),
            },
        ] {
            let state = Arc::new(Mutex::new(State {
                application: Some(submitted_application(
                    application_id,
                    applicant_user_id,
                    proposal(),
                )),
                ..State::default()
            }));
            let result = handler(Arc::clone(&state))
                .execute(&context(principal), command(application_id))
                .await;

            assert!(matches!(
                result,
                Err(WithdrawPartnershipApplicationError::NotFound)
            ));
            let state = lock(&state);
            assert_eq!(1, state.begin_attempts);
            assert_eq!(1, state.find_calls);
            assert_eq!(Some(actor_user_id), state.find_user_id);
            assert_eq!(0, state.update_calls);
            assert_eq!(0, state.commit_attempts);
        }
    }

    #[tokio::test]
    async fn should_reject_service_and_system_before_beginning_withdrawal_transaction() {
        let application_id = PartnershipApplicationId::new();

        for principal in [
            Principal::Service("partnership-worker".to_owned()),
            Principal::System,
        ] {
            let state = Arc::new(Mutex::new(State::default()));
            let result = handler(Arc::clone(&state))
                .execute(&context(principal), command(application_id))
                .await;

            assert!(matches!(
                result,
                Err(WithdrawPartnershipApplicationError::Forbidden)
            ));
            let state = lock(&state);
            assert_eq!(0, state.begin_attempts);
            assert_eq!(0, state.find_calls);
            assert_eq!(0, state.update_calls);
            assert_eq!(0, state.commit_attempts);
        }
    }

    #[tokio::test]
    async fn should_return_not_found_without_updating_or_committing() {
        let applicant_user_id = UserId::new();
        let application_id = PartnershipApplicationId::new();
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(applicant_user_id)),
                command(application_id),
            )
            .await;

        assert!(matches!(
            result,
            Err(WithdrawPartnershipApplicationError::NotFound)
        ));
        let state = lock(&state);
        assert_eq!(1, state.find_calls);
        assert_eq!(0, state.update_calls);
        assert_eq!(0, state.commit_attempts);
        assert_eq!(vec![1], state.factory_transaction_ids);
    }

    #[tokio::test]
    async fn should_reject_non_withdrawable_state_without_updating_or_committing() {
        let applicant_user_id = UserId::new();
        let application_id = PartnershipApplicationId::new();
        let state = Arc::new(Mutex::new(State {
            application: Some(approved_application(
                application_id,
                applicant_user_id,
                proposal(),
            )),
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(applicant_user_id)),
                command(application_id),
            )
            .await;

        assert!(matches!(
            result,
            Err(WithdrawPartnershipApplicationError::ApplicationNotWithdrawable)
        ));
        let state = lock(&state);
        assert_eq!(1, state.find_calls);
        assert_eq!(0, state.update_calls);
        assert_eq!(0, state.commit_attempts);
        assert_eq!(vec![1], state.factory_transaction_ids);
    }

    #[tokio::test]
    async fn should_translate_read_failure_without_updating_or_committing() {
        let state = Arc::new(Mutex::new(State {
            find_error: Some(
                PartnershipApplicationRepositoryError::TemporarilyUnavailable {
                    source: static_error("application read unavailable"),
                },
            ),
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(UserId::new())),
                command(PartnershipApplicationId::new()),
            )
            .await;

        assert!(matches!(
            result,
            Err(WithdrawPartnershipApplicationError::TemporarilyUnavailable { .. })
        ));
        let state = lock(&state);
        assert_eq!(1, state.find_calls);
        assert_eq!(0, state.update_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_translate_update_failure_without_committing_or_later_writes() {
        let applicant_user_id = UserId::new();
        let application_id = PartnershipApplicationId::new();
        let expected_version = PartnershipApplicationStorageVersion::INITIAL.next();
        let state = Arc::new(Mutex::new(State {
            application: Some(submitted_application(
                application_id,
                applicant_user_id,
                proposal(),
            )),
            application_version: expected_version,
            update_error: Some(PartnershipApplicationRepositoryError::ConcurrencyConflict),
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(applicant_user_id)),
                command(application_id),
            )
            .await;

        assert!(matches!(
            result,
            Err(WithdrawPartnershipApplicationError::ConcurrencyConflict)
        ));
        let state = lock(&state);
        assert_eq!(1, state.find_calls);
        assert_eq!(1, state.update_calls);
        assert_eq!(Some(expected_version), state.update_expected_version);
        assert_eq!(0, state.commit_attempts);
        assert_eq!(0, state.commits);
        assert_eq!(vec![1, 1], state.factory_transaction_ids);
    }

    #[tokio::test]
    async fn should_report_begin_and_commit_failures() {
        let begin_state = Arc::new(Mutex::new(State {
            begin_fails: true,
            ..State::default()
        }));
        let begin_result = handler(Arc::clone(&begin_state))
            .execute(
                &context(Principal::User(UserId::new())),
                command(PartnershipApplicationId::new()),
            )
            .await;
        assert!(matches!(
            begin_result,
            Err(WithdrawPartnershipApplicationError::BeginTransactionFailed)
        ));
        {
            let begin_state = lock(&begin_state);
            assert_eq!(1, begin_state.begin_attempts);
            assert_eq!(0, begin_state.find_calls);
            assert_eq!(0, begin_state.update_calls);
            assert_eq!(0, begin_state.commit_attempts);
        }

        let applicant_user_id = UserId::new();
        let application_id = PartnershipApplicationId::new();
        let commit_state = Arc::new(Mutex::new(State {
            application: Some(submitted_application(
                application_id,
                applicant_user_id,
                proposal(),
            )),
            commit_fails: true,
            ..State::default()
        }));
        let commit_result = handler(Arc::clone(&commit_state))
            .execute(
                &context(Principal::User(applicant_user_id)),
                command(application_id),
            )
            .await;
        assert!(matches!(
            commit_result,
            Err(WithdrawPartnershipApplicationError::CommitTransactionFailed)
        ));
        let commit_state = lock(&commit_state);
        assert_eq!(1, commit_state.find_calls);
        assert_eq!(1, commit_state.update_calls);
        assert_eq!(1, commit_state.commit_attempts);
        assert_eq!(0, commit_state.commits);
    }

    #[tokio::test]
    async fn should_reject_already_withdrawn_application_without_updating_or_committing() {
        let applicant_user_id = UserId::new();
        let application_id = PartnershipApplicationId::new();
        let state = Arc::new(Mutex::new(State {
            application: Some(withdrawn_application(
                application_id,
                applicant_user_id,
                proposal(),
            )),
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(applicant_user_id)),
                command(application_id),
            )
            .await;

        assert!(matches!(
            result,
            Err(WithdrawPartnershipApplicationError::ApplicationNotWithdrawable)
        ));
        let state = lock(&state);
        assert_eq!(1, state.find_calls);
        assert_eq!(0, state.update_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_translate_all_read_failures_without_updating_or_committing() {
        let errors = [
            PartnershipApplicationRepositoryError::TemporarilyUnavailable {
                source: static_error("temporary"),
            },
            PartnershipApplicationRepositoryError::InvalidPersistedState {
                source: static_error("invalid"),
            },
            PartnershipApplicationRepositoryError::Internal {
                source: static_error("internal"),
            },
        ];

        for (index, error) in errors.into_iter().enumerate() {
            let state = Arc::new(Mutex::new(State {
                find_error: Some(error),
                ..State::default()
            }));
            let result = handler(Arc::clone(&state))
                .execute(
                    &context(Principal::User(UserId::new())),
                    command(PartnershipApplicationId::new()),
                )
                .await;

            match index {
                0 => assert!(matches!(
                    result,
                    Err(WithdrawPartnershipApplicationError::TemporarilyUnavailable { .. })
                )),
                1 => assert!(matches!(
                    result,
                    Err(WithdrawPartnershipApplicationError::InvalidPersistedState { .. })
                )),
                _ => assert!(matches!(
                    result,
                    Err(WithdrawPartnershipApplicationError::Internal { .. })
                )),
            }
            let state = lock(&state);
            assert_eq!(1, state.find_calls);
            assert_eq!(0, state.update_calls);
            assert_eq!(0, state.commit_attempts);
        }
    }

    #[tokio::test]
    async fn should_translate_all_update_failures_without_committing_or_later_writes() {
        let errors = [
            PartnershipApplicationRepositoryError::ConcurrencyConflict,
            PartnershipApplicationRepositoryError::TemporarilyUnavailable {
                source: static_error("temporary"),
            },
            PartnershipApplicationRepositoryError::InvalidPersistedState {
                source: static_error("invalid"),
            },
            PartnershipApplicationRepositoryError::Internal {
                source: static_error("internal"),
            },
        ];
        let expected_version = PartnershipApplicationStorageVersion::INITIAL.next();

        for (index, error) in errors.into_iter().enumerate() {
            let applicant_user_id = UserId::new();
            let application_id = PartnershipApplicationId::new();
            let state = Arc::new(Mutex::new(State {
                application: Some(submitted_application(
                    application_id,
                    applicant_user_id,
                    proposal(),
                )),
                application_version: expected_version,
                update_error: Some(error),
                ..State::default()
            }));
            let result = handler(Arc::clone(&state))
                .execute(
                    &context(Principal::User(applicant_user_id)),
                    command(application_id),
                )
                .await;

            match index {
                0 => assert!(matches!(
                    result,
                    Err(WithdrawPartnershipApplicationError::ConcurrencyConflict)
                )),
                1 => assert!(matches!(
                    result,
                    Err(WithdrawPartnershipApplicationError::TemporarilyUnavailable { .. })
                )),
                2 => assert!(matches!(
                    result,
                    Err(WithdrawPartnershipApplicationError::InvalidPersistedState { .. })
                )),
                _ => assert!(matches!(
                    result,
                    Err(WithdrawPartnershipApplicationError::Internal { .. })
                )),
            }
            let state = lock(&state);
            assert_eq!(1, state.find_calls);
            assert_eq!(1, state.update_calls);
            assert_eq!(Some(expected_version), state.update_expected_version);
            assert_eq!(0, state.commit_attempts);
            assert_eq!(0, state.commits);
        }
    }
}
