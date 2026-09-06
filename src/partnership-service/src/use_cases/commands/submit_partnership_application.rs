use crate::ports::*;
use application::{
    error::BoxError,
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use partnership_core::{
    partnership_application::{
        NewPartnershipApplication, PartnershipApplication, PartnershipProposal,
    },
    partnership_application_id::PartnershipApplicationId,
};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct SubmitPartnershipApplicationCommand {
    pub applicant_user_id: UserId,
    pub proposal: PartnershipProposal,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SubmitPartnershipApplicationResult {
    pub application: PartnershipApplication,
}
#[derive(Debug, thiserror::Error)]
pub enum SubmitPartnershipApplicationError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary partnership application failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted partnership application state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership application failure")]
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
pub trait SubmitPartnershipApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: SubmitPartnershipApplicationCommand,
    ) -> Result<SubmitPartnershipApplicationResult, SubmitPartnershipApplicationError>;
}
pub struct SubmitPartnershipApplicationHandler<U, R> {
    unit_of_work: U,
    applications: R,
}
impl<U, R> SubmitPartnershipApplicationHandler<U, R> {
    pub fn new(unit_of_work: U, applications: R) -> Self {
        Self {
            unit_of_work,
            applications,
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: PartnershipApplicationRepositoryFactory<U::Tx>>
    SubmitPartnershipApplicationUseCase for SubmitPartnershipApplicationHandler<U, R>
{
    #[tracing::instrument(name="submit_partnership_application", skip_all, fields(principal_type=context.principal.kind(), request_id=%context.request_id, correlation_id=%context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: SubmitPartnershipApplicationCommand,
    ) -> Result<SubmitPartnershipApplicationResult, SubmitPartnershipApplicationError> {
        authorize(context, command.applicant_user_id)?;
        let application = PartnershipApplication::submit(NewPartnershipApplication {
            id: PartnershipApplicationId::new(),
            applicant_user_id: command.applicant_user_id,
            proposal: command.proposal,
        });
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SubmitPartnershipApplicationError::BeginTransactionFailed)?;
        let application = self
            .applications
            .in_transaction(&mut tx)
            .insert(&application)
            .await?
            .value;
        tx.commit()
            .await
            .map_err(|_| SubmitPartnershipApplicationError::CommitTransactionFailed)?;
        tracing::info!(event="partnership_application.submitted", partnership_application_id=%application.id(), actor_type=context.principal.kind(), outcome="success");
        Ok(SubmitPartnershipApplicationResult { application })
    }
}
fn authorize(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), SubmitPartnershipApplicationError> {
    match context.principal {
        Principal::Anonymous => Err(SubmitPartnershipApplicationError::AuthenticatedActorRequired),
        Principal::User(actor) | Principal::DelegatedUser { user_id: actor, .. }
            if actor == user_id =>
        {
            Ok(())
        }
        Principal::Service(_) | Principal::System => Ok(()),
        _ => Err(SubmitPartnershipApplicationError::Forbidden),
    }
}
impl From<PartnershipApplicationRepositoryError> for SubmitPartnershipApplicationError {
    fn from(value: PartnershipApplicationRepositoryError) -> Self {
        match value {
            PartnershipApplicationRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipApplicationRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnershipApplicationRepositoryError::ConcurrencyConflict
            | PartnershipApplicationRepositoryError::Internal { source: _ } => Self::Internal {
                source: application::error::static_error(
                    "unexpected partnership application insert failure",
                ),
            },
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
    use partnership_core::partnership_application::{
        NewPartnershipApplication, PartnershipApplication, PartnershipProposal,
    };
    use partnership_core::partnership_application_id::PartnershipApplicationId;
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
        insert_transaction_ids: Vec<usize>,
        insert_calls: usize,
        inserted_applicants: Vec<UserId>,
        inserted_proposals: Vec<PartnershipProposal>,
        begin_fails: bool,
        commit_fails: bool,
        insert_error: Option<PartnershipApplicationRepositoryError>,
        insert_result: Option<PartnershipApplication>,
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
            _user_id: UserId,
            _id: PartnershipApplicationId,
        ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
        {
            Err(unexpected_repository_call())
        }

        async fn insert(
            &mut self,
            application: &PartnershipApplication,
        ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>
        {
            let mut state = lock(&self.state);
            state.insert_calls += 1;
            state.insert_transaction_ids.push(self.id);
            state
                .inserted_applicants
                .push(application.applicant_user_id());
            state
                .inserted_proposals
                .push(application.proposal().clone());
            if let Some(error) = state.insert_error.take() {
                return Err(error);
            }
            let persisted = match state.insert_result.clone() {
                Some(application) => application,
                None => application.clone(),
            };
            Ok(Versioned::new(
                persisted,
                PartnershipApplicationStorageVersion::INITIAL,
            ))
        }

        async fn update(
            &mut self,
            _application: &PartnershipApplication,
            _expected: PartnershipApplicationStorageVersion,
        ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>
        {
            Err(unexpected_repository_call())
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
            request_id: RequestId::new("submit-request"),
            correlation_id: CorrelationId::new("submit-correlation"),
        }
    }

    fn proposal() -> PartnershipProposal {
        PartnershipProposal::ExistingListingSource {
            listing_source_id: ListingSourceId::new(),
        }
    }

    fn command(
        applicant_user_id: UserId,
        proposal: PartnershipProposal,
    ) -> SubmitPartnershipApplicationCommand {
        SubmitPartnershipApplicationCommand {
            applicant_user_id,
            proposal,
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

    fn handler(
        state: Arc<Mutex<State>>,
    ) -> SubmitPartnershipApplicationHandler<FakeUnitOfWork, FakeRepositoryFactory> {
        SubmitPartnershipApplicationHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(&state),
            },
            FakeRepositoryFactory { state },
        )
    }

    #[tokio::test]
    async fn should_submit_for_own_user_and_forward_actor_and_proposal() {
        let applicant_user_id = UserId::new();
        let proposal = proposal();
        let persisted = submitted_application(
            PartnershipApplicationId::new(),
            applicant_user_id,
            proposal.clone(),
        );
        let state = Arc::new(Mutex::new(State {
            insert_result: Some(persisted.clone()),
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(applicant_user_id)),
                command(applicant_user_id, proposal.clone()),
            )
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => panic!("submission failed: {error}"),
        };

        assert_eq!(persisted, result.application);
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.insert_calls);
        assert_eq!(1, state.commits);
        assert_eq!(vec![applicant_user_id], state.inserted_applicants);
        assert_eq!(vec![proposal], state.inserted_proposals);
        assert_eq!(vec![1], state.factory_transaction_ids);
        assert_eq!(vec![1], state.insert_transaction_ids);
    }

    #[tokio::test]
    async fn should_submit_for_delegated_own_user() {
        let applicant_user_id = UserId::new();
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::DelegatedUser {
                    user_id: applicant_user_id,
                    capabilities: BTreeSet::new(),
                }),
                command(applicant_user_id, proposal()),
            )
            .await;

        assert!(result.is_ok());
        let state = lock(&state);
        assert_eq!(1, state.insert_calls);
        assert_eq!(1, state.commits);
        assert_eq!(vec![applicant_user_id], state.inserted_applicants);
    }

    #[tokio::test]
    async fn should_allow_service_and_system_submitters() {
        let applicant_user_id = UserId::new();
        let state = Arc::new(Mutex::new(State::default()));
        let handler = handler(Arc::clone(&state));

        for principal in [
            Principal::Service("partnership-worker".to_owned()),
            Principal::System,
        ] {
            let result = handler
                .execute(&context(principal), command(applicant_user_id, proposal()))
                .await;
            assert!(result.is_ok());
        }

        let state = lock(&state);
        assert_eq!(2, state.begins);
        assert_eq!(2, state.insert_calls);
        assert_eq!(2, state.commits);
        assert_eq!(
            vec![applicant_user_id, applicant_user_id],
            state.inserted_applicants
        );
    }

    #[tokio::test]
    async fn should_reject_anonymous_before_beginning_submission_transaction() {
        let applicant_user_id = UserId::new();
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::Anonymous),
                command(applicant_user_id, proposal()),
            )
            .await;

        assert!(matches!(
            result,
            Err(SubmitPartnershipApplicationError::AuthenticatedActorRequired)
        ));
        let state = lock(&state);
        assert_eq!(0, state.begin_attempts);
        assert_eq!(0, state.insert_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_reject_another_user_before_beginning_submission_transaction() {
        let actor_user_id = UserId::new();
        let applicant_user_id = UserId::new();
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(actor_user_id)),
                command(applicant_user_id, proposal()),
            )
            .await;

        assert!(matches!(
            result,
            Err(SubmitPartnershipApplicationError::Forbidden)
        ));
        let state = lock(&state);
        assert_eq!(0, state.begin_attempts);
        assert_eq!(0, state.insert_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_report_begin_failure_without_inserting_or_committing() {
        let state = Arc::new(Mutex::new(State {
            begin_fails: true,
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::System),
                command(UserId::new(), proposal()),
            )
            .await;

        assert!(matches!(
            result,
            Err(SubmitPartnershipApplicationError::BeginTransactionFailed)
        ));
        let state = lock(&state);
        assert_eq!(1, state.begin_attempts);
        assert_eq!(0, state.insert_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_report_commit_failure_after_insert() {
        let state = Arc::new(Mutex::new(State {
            commit_fails: true,
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::System),
                command(UserId::new(), proposal()),
            )
            .await;

        assert!(matches!(
            result,
            Err(SubmitPartnershipApplicationError::CommitTransactionFailed)
        ));
        let state = lock(&state);
        assert_eq!(1, state.insert_calls);
        assert_eq!(1, state.commit_attempts);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_translate_insert_failure_without_committing() {
        let state = Arc::new(Mutex::new(State {
            insert_error: Some(
                PartnershipApplicationRepositoryError::TemporarilyUnavailable {
                    source: static_error("insert unavailable"),
                },
            ),
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::System),
                command(UserId::new(), proposal()),
            )
            .await;

        assert!(matches!(
            result,
            Err(SubmitPartnershipApplicationError::TemporarilyUnavailable { .. })
        ));
        let state = lock(&state);
        assert_eq!(1, state.insert_calls);
        assert_eq!(0, state.commit_attempts);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_reject_mismatched_delegated_user_before_beginning_submission_transaction() {
        let actor_user_id = UserId::new();
        let applicant_user_id = UserId::new();
        assert_ne!(actor_user_id, applicant_user_id);
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::DelegatedUser {
                    user_id: actor_user_id,
                    capabilities: BTreeSet::new(),
                }),
                command(applicant_user_id, proposal()),
            )
            .await;

        assert!(matches!(
            result,
            Err(SubmitPartnershipApplicationError::Forbidden)
        ));
        let state = lock(&state);
        assert_eq!(0, state.begin_attempts);
        assert_eq!(0, state.insert_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_translate_all_insert_failures_without_committing() {
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

        for (index, error) in errors.into_iter().enumerate() {
            let state = Arc::new(Mutex::new(State {
                insert_error: Some(error),
                ..State::default()
            }));
            let result = handler(Arc::clone(&state))
                .execute(
                    &context(Principal::System),
                    command(UserId::new(), proposal()),
                )
                .await;

            match index {
                0 | 3 => assert!(matches!(
                    result,
                    Err(SubmitPartnershipApplicationError::Internal { .. })
                )),
                1 => assert!(matches!(
                    result,
                    Err(SubmitPartnershipApplicationError::TemporarilyUnavailable { .. })
                )),
                _ => assert!(matches!(
                    result,
                    Err(SubmitPartnershipApplicationError::InvalidPersistedState { .. })
                )),
            }
            let state = lock(&state);
            assert_eq!(1, state.insert_calls);
            assert_eq!(0, state.commit_attempts);
            assert_eq!(0, state.commits);
        }
    }
}
