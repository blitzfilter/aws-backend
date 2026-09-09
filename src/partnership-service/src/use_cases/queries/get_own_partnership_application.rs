use crate::ports::*;
use application::{
    error::BoxError,
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use partnership_core::partnership_application_id::PartnershipApplicationId;
#[derive(Debug, Clone, PartialEq)]
pub struct GetOwnPartnershipApplicationRequest {
    pub application_id: PartnershipApplicationId,
}
pub type GetOwnPartnershipApplicationResult = PartnershipApplicationView;
#[derive(Debug, thiserror::Error)]
pub enum GetOwnPartnershipApplicationError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("partnership application not found")]
    NotFound,
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
pub trait GetOwnPartnershipApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetOwnPartnershipApplicationRequest,
    ) -> Result<GetOwnPartnershipApplicationResult, GetOwnPartnershipApplicationError>;
}
pub struct GetOwnPartnershipApplicationHandler<U, R> {
    unit_of_work: U,
    applications: R,
}
impl<U, R> GetOwnPartnershipApplicationHandler<U, R> {
    pub fn new(unit_of_work: U, applications: R) -> Self {
        Self {
            unit_of_work,
            applications,
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: PartnershipApplicationRepositoryFactory<U::Tx>>
    GetOwnPartnershipApplicationUseCase for GetOwnPartnershipApplicationHandler<U, R>
{
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetOwnPartnershipApplicationRequest,
    ) -> Result<GetOwnPartnershipApplicationResult, GetOwnPartnershipApplicationError> {
        let user = match context.principal {
            Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => id,
            Principal::Anonymous => {
                return Err(GetOwnPartnershipApplicationError::AuthenticatedActorRequired);
            }
            Principal::Service(_) | Principal::System => {
                return Err(GetOwnPartnershipApplicationError::Forbidden);
            }
        };
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetOwnPartnershipApplicationError::BeginTransactionFailed)?;
        let app = self
            .applications
            .in_transaction(&mut tx)
            .find_by_user_and_id(user, request.application_id)
            .await?
            .ok_or(GetOwnPartnershipApplicationError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| GetOwnPartnershipApplicationError::CommitTransactionFailed)?;
        Ok(PartnershipApplicationView {
            id: app.value.id(),
            applicant_user_id: app.value.applicant_user_id(),
            state: app.value.state(),
            proposal: app.value.proposal().clone(),
            approval_result: app.value.approval_result(),
        })
    }
}
impl From<PartnershipApplicationRepositoryError> for GetOwnPartnershipApplicationError {
    fn from(v: PartnershipApplicationRepositoryError) -> Self {
        match v {
            PartnershipApplicationRepositoryError::ListingSourceNotFound => {
                Self::InvalidPersistedState {
                    source: application::error::static_error("missing existing listing source"),
                }
            }
            PartnershipApplicationRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipApplicationRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnershipApplicationRepositoryError::ConcurrencyConflict => Self::Internal {
                source: application::error::static_error("unexpected concurrency"),
            },
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
    use partnership_core::partnership_application::{
        NewPartnershipApplication, PartnershipApplication, PartnershipApplicationApprovalResult,
        PartnershipProposal,
    };
    use partnership_core::partnership_application_id::PartnershipApplicationId;
    use partnership_core::partnership_id::PartnershipId;
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
        find_calls: usize,
        find_user_id: Option<UserId>,
        find_application_id: Option<PartnershipApplicationId>,
        application: Option<PartnershipApplication>,
        application_version: PartnershipApplicationStorageVersion,
        find_error: Option<PartnershipApplicationRepositoryError>,
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
            request_id: RequestId::new("get-own-request"),
            correlation_id: CorrelationId::new("get-own-correlation"),
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
                    PartnershipId::new(),
                    ListingSourceId::new(),
                ))
                .is_ok()
        );
        application
    }

    fn handler(
        state: Arc<Mutex<State>>,
    ) -> GetOwnPartnershipApplicationHandler<FakeUnitOfWork, FakeRepositoryFactory> {
        GetOwnPartnershipApplicationHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(&state),
            },
            FakeRepositoryFactory { state },
        )
    }

    #[tokio::test]
    async fn should_get_own_application_for_user_in_one_transaction_and_map_all_fields() {
        let user_id = UserId::new();
        let application =
            approved_application(PartnershipApplicationId::new(), user_id, proposal());
        let application_id = application.id();
        let state = Arc::new(Mutex::new(State {
            application: Some(application.clone()),
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(user_id)),
                GetOwnPartnershipApplicationRequest { application_id },
            )
            .await;
        let view = match result {
            Ok(view) => view,
            Err(error) => panic!("application read failed: {error}"),
        };

        assert_eq!(application.id(), view.id);
        assert_eq!(application.applicant_user_id(), view.applicant_user_id);
        assert_eq!(application.state(), view.state);
        assert_eq!(application.proposal(), &view.proposal);
        assert_eq!(application.approval_result(), view.approval_result);
        let state = lock(&state);
        assert_eq!(1, state.begin_attempts);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.find_calls);
        assert_eq!(1, state.commits);
        assert_eq!(Some(user_id), state.find_user_id);
        assert_eq!(Some(application_id), state.find_application_id);
        assert_eq!(vec![1], state.factory_transaction_ids);
        assert_eq!(vec![1], state.find_transaction_ids);
    }

    #[tokio::test]
    async fn should_get_own_application_for_delegated_user() {
        let user_id = UserId::new();
        let application =
            submitted_application(PartnershipApplicationId::new(), user_id, proposal());
        let application_id = application.id();
        let state = Arc::new(Mutex::new(State {
            application: Some(application),
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::DelegatedUser {
                    user_id,
                    capabilities: BTreeSet::new(),
                }),
                GetOwnPartnershipApplicationRequest { application_id },
            )
            .await;

        assert!(result.is_ok());
        let state = lock(&state);
        assert_eq!(Some(user_id), state.find_user_id);
        assert_eq!(1, state.find_calls);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_reject_anonymous_before_beginning_read_transaction() {
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::Anonymous),
                GetOwnPartnershipApplicationRequest {
                    application_id: PartnershipApplicationId::new(),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(GetOwnPartnershipApplicationError::AuthenticatedActorRequired)
        ));
        let state = lock(&state);
        assert_eq!(0, state.begin_attempts);
        assert_eq!(0, state.find_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_reject_service_and_system_before_beginning_read_transaction() {
        for principal in [
            Principal::Service("partnership-worker".to_owned()),
            Principal::System,
        ] {
            let state = Arc::new(Mutex::new(State::default()));
            let result = handler(Arc::clone(&state))
                .execute(
                    &context(principal),
                    GetOwnPartnershipApplicationRequest {
                        application_id: PartnershipApplicationId::new(),
                    },
                )
                .await;

            assert!(matches!(
                result,
                Err(GetOwnPartnershipApplicationError::Forbidden)
            ));
            let state = lock(&state);
            assert_eq!(0, state.begin_attempts);
            assert_eq!(0, state.find_calls);
            assert_eq!(0, state.commit_attempts);
        }
    }

    #[tokio::test]
    async fn should_scope_read_to_authenticated_actor() {
        let actor_user_id = UserId::new();
        let applicant_user_id = UserId::new();
        assert_ne!(actor_user_id, applicant_user_id);
        let application = submitted_application(
            PartnershipApplicationId::new(),
            applicant_user_id,
            proposal(),
        );
        let application_id = application.id();

        for principal in [
            Principal::User(actor_user_id),
            Principal::DelegatedUser {
                user_id: actor_user_id,
                capabilities: BTreeSet::new(),
            },
        ] {
            let state = Arc::new(Mutex::new(State {
                application: Some(application.clone()),
                ..State::default()
            }));
            let result = handler(Arc::clone(&state))
                .execute(
                    &context(principal),
                    GetOwnPartnershipApplicationRequest { application_id },
                )
                .await;

            assert!(matches!(
                result,
                Err(GetOwnPartnershipApplicationError::NotFound)
            ));
            let state = lock(&state);
            assert_eq!(1, state.begin_attempts);
            assert_eq!(1, state.find_calls);
            assert_eq!(Some(actor_user_id), state.find_user_id);
            assert_eq!(0, state.commit_attempts);
        }
    }

    #[tokio::test]
    async fn should_return_not_found_without_committing() {
        let user_id = UserId::new();
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(user_id)),
                GetOwnPartnershipApplicationRequest {
                    application_id: PartnershipApplicationId::new(),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(GetOwnPartnershipApplicationError::NotFound)
        ));
        let state = lock(&state);
        assert_eq!(1, state.find_calls);
        assert_eq!(0, state.commits);
        assert_eq!(vec![1], state.factory_transaction_ids);
    }

    #[tokio::test]
    async fn should_translate_all_read_failures_without_committing() {
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
                    GetOwnPartnershipApplicationRequest {
                        application_id: PartnershipApplicationId::new(),
                    },
                )
                .await;

            match index {
                0 => assert!(matches!(
                    result,
                    Err(GetOwnPartnershipApplicationError::TemporarilyUnavailable { .. })
                )),
                1 => assert!(matches!(
                    result,
                    Err(GetOwnPartnershipApplicationError::InvalidPersistedState { .. })
                )),
                _ => assert!(matches!(
                    result,
                    Err(GetOwnPartnershipApplicationError::Internal { .. })
                )),
            }
            let state = lock(&state);
            assert_eq!(1, state.find_calls);
            assert_eq!(0, state.commit_attempts);
        }
    }

    #[tokio::test]
    async fn should_report_begin_failure_without_reading_or_committing() {
        let state = Arc::new(Mutex::new(State {
            begin_fails: true,
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(UserId::new())),
                GetOwnPartnershipApplicationRequest {
                    application_id: PartnershipApplicationId::new(),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(GetOwnPartnershipApplicationError::BeginTransactionFailed)
        ));
        let state = lock(&state);
        assert_eq!(1, state.begin_attempts);
        assert_eq!(0, state.begins);
        assert_eq!(0, state.find_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_report_commit_failure_after_reading() {
        let user_id = UserId::new();
        let application =
            submitted_application(PartnershipApplicationId::new(), user_id, proposal());
        let state = Arc::new(Mutex::new(State {
            application: Some(application.clone()),
            commit_fails: true,
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(user_id)),
                GetOwnPartnershipApplicationRequest {
                    application_id: application.id(),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(GetOwnPartnershipApplicationError::CommitTransactionFailed)
        ));
        let state = lock(&state);
        assert_eq!(1, state.find_calls);
        assert_eq!(1, state.commit_attempts);
        assert_eq!(0, state.commits);
    }
}
