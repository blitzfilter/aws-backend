use crate::ports::*;
use application::{
    error::BoxError,
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use user_core::user_id::UserId;
#[derive(Debug, Clone, PartialEq)]
pub struct ListOwnPartnershipApplicationsRequest {
    pub user_id: UserId,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ListOwnPartnershipApplicationsResult {
    pub items: Vec<PartnershipApplicationView>,
}
#[derive(Debug, thiserror::Error)]
pub enum ListOwnPartnershipApplicationsError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid read model")]
    InvalidReadModel {
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
pub trait ListOwnPartnershipApplicationsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListOwnPartnershipApplicationsRequest,
    ) -> Result<ListOwnPartnershipApplicationsResult, ListOwnPartnershipApplicationsError>;
}
pub struct ListOwnPartnershipApplicationsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}
impl<U, R> ListOwnPartnershipApplicationsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: PartnershipApplicationReaderFactory<U::Tx>>
    ListOwnPartnershipApplicationsUseCase for ListOwnPartnershipApplicationsHandler<U, R>
{
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListOwnPartnershipApplicationsRequest,
    ) -> Result<ListOwnPartnershipApplicationsResult, ListOwnPartnershipApplicationsError> {
        match context.principal {
            Principal::User(id) | Principal::DelegatedUser { user_id: id, .. }
                if id == request.user_id => {}
            Principal::Anonymous => {
                return Err(ListOwnPartnershipApplicationsError::AuthenticatedActorRequired);
            }
            _ => return Err(ListOwnPartnershipApplicationsError::Forbidden),
        }
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListOwnPartnershipApplicationsError::BeginTransactionFailed)?;
        let items = self
            .reader
            .in_transaction(&mut tx)
            .list_by_user(request.user_id)
            .await?;
        tx.commit()
            .await
            .map_err(|_| ListOwnPartnershipApplicationsError::CommitTransactionFailed)?;
        Ok(ListOwnPartnershipApplicationsResult { items })
    }
}
impl From<PartnershipApplicationReadError> for ListOwnPartnershipApplicationsError {
    fn from(v: PartnershipApplicationReadError) -> Self {
        match v {
            PartnershipApplicationReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipApplicationReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            PartnershipApplicationReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        PartnershipApplicationReadError, PartnershipApplicationReader,
        PartnershipApplicationReaderFactory, PartnershipApplicationView,
    };
    use crate::use_cases::queries::list_admin_partnership_applications::{
        ListAdminPartnershipApplicationsRequest, ListAdminPartnershipApplicationsResult,
    };
    use application::{
        error::static_error,
        operation_context::{CorrelationId, OperationContext, Principal, RequestId},
        transaction::{Transaction, TransactionError, UnitOfWork},
    };
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
        list_transaction_ids: Vec<usize>,
        list_calls: usize,
        list_user_id: Option<UserId>,
        items: Vec<PartnershipApplicationView>,
        list_error: Option<PartnershipApplicationReadError>,
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
    struct FakeReaderFactory {
        state: Arc<Mutex<State>>,
    }

    struct FakeReader {
        id: usize,
        state: Arc<Mutex<State>>,
    }

    impl PartnershipApplicationReaderFactory<FakeTransaction> for FakeReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut FakeTransaction,
        ) -> impl PartnershipApplicationReader + 'tx {
            let id = tx.id;
            lock(&self.state).factory_transaction_ids.push(id);
            FakeReader {
                id,
                state: Arc::clone(&self.state),
            }
        }
    }

    fn unexpected_reader_call() -> PartnershipApplicationReadError {
        PartnershipApplicationReadError::Internal {
            source: static_error("unexpected reader call"),
        }
    }

    #[async_trait::async_trait]
    impl PartnershipApplicationReader for FakeReader {
        async fn list_by_user(
            &mut self,
            user_id: UserId,
        ) -> Result<Vec<PartnershipApplicationView>, PartnershipApplicationReadError> {
            let mut state = lock(&self.state);
            state.list_calls += 1;
            state.list_transaction_ids.push(self.id);
            state.list_user_id = Some(user_id);
            if let Some(error) = state.list_error.take() {
                return Err(error);
            }
            Ok(state.items.clone())
        }

        async fn search_admin(
            &mut self,
            _request: &ListAdminPartnershipApplicationsRequest,
        ) -> Result<ListAdminPartnershipApplicationsResult, PartnershipApplicationReadError>
        {
            Err(unexpected_reader_call())
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
            request_id: RequestId::new("list-own-request"),
            correlation_id: CorrelationId::new("list-own-correlation"),
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

    fn view(application: &PartnershipApplication) -> PartnershipApplicationView {
        PartnershipApplicationView {
            id: application.id(),
            applicant_user_id: application.applicant_user_id(),
            state: application.state(),
            proposal: application.proposal().clone(),
            approval_result: application.approval_result(),
        }
    }

    fn request(user_id: UserId) -> ListOwnPartnershipApplicationsRequest {
        ListOwnPartnershipApplicationsRequest { user_id }
    }

    fn handler(
        state: Arc<Mutex<State>>,
    ) -> ListOwnPartnershipApplicationsHandler<FakeUnitOfWork, FakeReaderFactory> {
        ListOwnPartnershipApplicationsHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(&state),
            },
            FakeReaderFactory { state },
        )
    }

    #[tokio::test]
    async fn should_list_own_applications_for_user_in_one_transaction_and_map_items() {
        let user_id = UserId::new();
        let submitted = submitted_application(PartnershipApplicationId::new(), user_id, proposal());
        let approved = approved_application(PartnershipApplicationId::new(), user_id, proposal());
        let expected_items = vec![view(&submitted), view(&approved)];
        let state = Arc::new(Mutex::new(State {
            items: expected_items.clone(),
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(&context(Principal::User(user_id)), request(user_id))
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => panic!("application list failed: {error}"),
        };

        assert_eq!(expected_items, result.items);
        let state = lock(&state);
        assert_eq!(1, state.begin_attempts);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.list_calls);
        assert_eq!(1, state.commits);
        assert_eq!(Some(user_id), state.list_user_id);
        assert_eq!(vec![1], state.factory_transaction_ids);
        assert_eq!(vec![1], state.list_transaction_ids);
    }

    #[tokio::test]
    async fn should_list_own_applications_for_delegated_user() {
        let user_id = UserId::new();
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::DelegatedUser {
                    user_id,
                    capabilities: BTreeSet::new(),
                }),
                request(user_id),
            )
            .await;

        assert!(result.is_ok());
        let state = lock(&state);
        assert_eq!(1, state.list_calls);
        assert_eq!(Some(user_id), state.list_user_id);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_reject_anonymous_before_beginning_list_transaction() {
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(Arc::clone(&state))
            .execute(&context(Principal::Anonymous), request(UserId::new()))
            .await;

        assert!(matches!(
            result,
            Err(ListOwnPartnershipApplicationsError::AuthenticatedActorRequired)
        ));
        let state = lock(&state);
        assert_eq!(0, state.begin_attempts);
        assert_eq!(0, state.list_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_reject_service_and_system_before_beginning_list_transaction() {
        for principal in [
            Principal::Service("partnership-worker".to_owned()),
            Principal::System,
        ] {
            let state = Arc::new(Mutex::new(State::default()));
            let result = handler(Arc::clone(&state))
                .execute(&context(principal), request(UserId::new()))
                .await;

            assert!(matches!(
                result,
                Err(ListOwnPartnershipApplicationsError::Forbidden)
            ));
            let state = lock(&state);
            assert_eq!(0, state.begin_attempts);
            assert_eq!(0, state.list_calls);
            assert_eq!(0, state.commit_attempts);
        }
    }

    #[tokio::test]
    async fn should_reject_mismatched_user_before_beginning_list_transaction() {
        let actor_user_id = UserId::new();
        let requested_user_id = UserId::new();
        assert_ne!(actor_user_id, requested_user_id);

        for principal in [
            Principal::User(actor_user_id),
            Principal::DelegatedUser {
                user_id: actor_user_id,
                capabilities: BTreeSet::new(),
            },
        ] {
            let state = Arc::new(Mutex::new(State::default()));
            let result = handler(Arc::clone(&state))
                .execute(&context(principal), request(requested_user_id))
                .await;

            assert!(matches!(
                result,
                Err(ListOwnPartnershipApplicationsError::Forbidden)
            ));
            let state = lock(&state);
            assert_eq!(0, state.begin_attempts);
            assert_eq!(0, state.list_calls);
            assert_eq!(0, state.commit_attempts);
        }
    }

    #[tokio::test]
    async fn should_list_empty_result_as_a_committed_no_op() {
        let user_id = UserId::new();
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(Arc::clone(&state))
            .execute(&context(Principal::User(user_id)), request(user_id))
            .await;

        let result = match result {
            Ok(result) => result,
            Err(error) => panic!("empty application list failed: {error}"),
        };
        assert!(result.items.is_empty());
        let state = lock(&state);
        assert_eq!(1, state.list_calls);
        assert_eq!(1, state.commit_attempts);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_translate_all_reader_failures_without_committing() {
        let errors = [
            PartnershipApplicationReadError::TemporarilyUnavailable {
                source: static_error("temporary"),
            },
            PartnershipApplicationReadError::InvalidReadModel {
                source: static_error("invalid"),
            },
            PartnershipApplicationReadError::Internal {
                source: static_error("internal"),
            },
        ];

        for (index, error) in errors.into_iter().enumerate() {
            let user_id = UserId::new();
            let state = Arc::new(Mutex::new(State {
                list_error: Some(error),
                ..State::default()
            }));
            let result = handler(Arc::clone(&state))
                .execute(&context(Principal::User(user_id)), request(user_id))
                .await;

            match index {
                0 => assert!(matches!(
                    result,
                    Err(ListOwnPartnershipApplicationsError::TemporarilyUnavailable { .. })
                )),
                1 => assert!(matches!(
                    result,
                    Err(ListOwnPartnershipApplicationsError::InvalidReadModel { .. })
                )),
                _ => assert!(matches!(
                    result,
                    Err(ListOwnPartnershipApplicationsError::Internal { .. })
                )),
            }
            let state = lock(&state);
            assert_eq!(1, state.list_calls);
            assert_eq!(0, state.commit_attempts);
            assert_eq!(0, state.commits);
        }
    }

    #[tokio::test]
    async fn should_report_begin_failure_without_reading_or_committing() {
        let user_id = UserId::new();
        let state = Arc::new(Mutex::new(State {
            begin_fails: true,
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(&context(Principal::User(user_id)), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListOwnPartnershipApplicationsError::BeginTransactionFailed)
        ));
        let state = lock(&state);
        assert_eq!(1, state.begin_attempts);
        assert_eq!(0, state.begins);
        assert_eq!(0, state.list_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_report_commit_failure_after_reading() {
        let user_id = UserId::new();
        let state = Arc::new(Mutex::new(State {
            commit_fails: true,
            ..State::default()
        }));

        let result = handler(Arc::clone(&state))
            .execute(&context(Principal::User(user_id)), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListOwnPartnershipApplicationsError::CommitTransactionFailed)
        ));
        let state = lock(&state);
        assert_eq!(1, state.list_calls);
        assert_eq!(1, state.commit_attempts);
        assert_eq!(0, state.commits);
    }
}
