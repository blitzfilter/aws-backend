use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::*,
};
use application::{
    error::BoxError,
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use partnership_core::partnership_application_id::PartnershipApplicationId;
use user_service::ports::UserAdminReaderFactory;
#[derive(Debug, Clone, PartialEq)]
pub struct GetPartnershipApplicationRequest {
    pub application_id: PartnershipApplicationId,
}
pub type GetPartnershipApplicationResult = PartnershipApplicationView;
#[derive(Debug, thiserror::Error)]
pub enum GetPartnershipApplicationError {
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
pub trait GetPartnershipApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPartnershipApplicationRequest,
    ) -> Result<GetPartnershipApplicationResult, GetPartnershipApplicationError>;
}
pub struct GetPartnershipApplicationHandler<U, A, R> {
    unit_of_work: U,
    applications: A,
    admins: R,
}
impl<U, A, R> GetPartnershipApplicationHandler<U, A, R> {
    pub fn new(unit_of_work: U, applications: A, admins: R) -> Self {
        Self {
            unit_of_work,
            applications,
            admins,
        }
    }
}
#[async_trait::async_trait]
impl<
    U: UnitOfWork,
    A: PartnershipApplicationRepositoryFactory<U::Tx>,
    R: UserAdminReaderFactory<U::Tx>,
> GetPartnershipApplicationUseCase for GetPartnershipApplicationHandler<U, A, R>
{
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPartnershipApplicationRequest,
    ) -> Result<GetPartnershipApplicationResult, GetPartnershipApplicationError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetPartnershipApplicationError::BeginTransactionFailed)?;
        authorize_admin(context, &mut tx, &self.admins).await?;
        let app = self
            .applications
            .in_transaction(&mut tx)
            .find_by_id(request.application_id)
            .await?
            .ok_or(GetPartnershipApplicationError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| GetPartnershipApplicationError::CommitTransactionFailed)?;
        Ok(PartnershipApplicationView {
            id: app.value.id(),
            applicant_user_id: app.value.applicant_user_id(),
            state: app.value.state(),
            proposal: app.value.proposal().clone(),
            approval_result: app.value.approval_result(),
        })
    }
}
impl From<AdminAuthorizationError> for GetPartnershipApplicationError {
    fn from(v: AdminAuthorizationError) -> Self {
        match v {
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
impl From<PartnershipApplicationRepositoryError> for GetPartnershipApplicationError {
    fn from(v: PartnershipApplicationRepositoryError) -> Self {
        match v {
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
        transaction::{TransactionError, UnitOfWork},
    };
    use domain_primitives::versioned::Versioned;
    use listing_source_core::ListingSourceId;
    use partnership_core::partnership_application::{
        NewPartnershipApplication, PartnershipApplication, PartnershipProposal,
    };
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::{role::UserRole, user_id::UserId};
    use user_service::ports::{
        UserAdminActorView, UserAdminReadError, UserAdminReader, UserAdminReaderFactory,
    };

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Binding {
        Admin,
        Application,
    }

    #[derive(Default)]
    struct State {
        application: Option<PartnershipApplication>,
        application_error: Option<PartnershipApplicationRepositoryError>,
        admin: Option<UserAdminActorView>,
        admin_error: Option<UserAdminReadError>,
        begin_fails: bool,
        commit_fails: bool,
        next_transaction_id: usize,
        begins: usize,
        commit_attempts: usize,
        commits: usize,
        application_bindings: usize,
        application_reads: usize,
        admin_bindings: usize,
        admin_reads: usize,
        bindings: Vec<(Binding, usize)>,
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
            state.begins += 1;
            if state.begin_fails {
                return Err(TransactionError::BeginFailed);
            }
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
    struct FakeFactories {
        state: Arc<Mutex<State>>,
    }

    struct FakeApplicationRepository {
        state: Arc<Mutex<State>>,
    }

    struct FakeAdminReader {
        state: Arc<Mutex<State>>,
    }

    impl PartnershipApplicationRepositoryFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut FakeTransaction,
        ) -> impl PartnershipApplicationRepository + 'tx {
            let mut state = lock(&self.state);
            state.application_bindings += 1;
            state.bindings.push((Binding::Application, tx.id));
            drop(state);
            FakeApplicationRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    impl UserAdminReaderFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut FakeTransaction,
        ) -> impl UserAdminReader + 'tx {
            let mut state = lock(&self.state);
            state.admin_bindings += 1;
            state.bindings.push((Binding::Admin, tx.id));
            drop(state);
            FakeAdminReader {
                state: Arc::clone(&self.state),
            }
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

    #[async_trait::async_trait]
    impl PartnershipApplicationRepository for FakeApplicationRepository {
        async fn find_by_id(
            &mut self,
            id: PartnershipApplicationId,
        ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
        {
            let mut state = lock(&self.state);
            state.application_reads += 1;
            if let Some(error) = state.application_error.take() {
                return Err(error);
            }
            Ok(state
                .application
                .clone()
                .filter(|application| application.id() == id)
                .map(|application| Versioned::new(application, application_version(1))))
        }

        async fn find_by_id_for_update(
            &mut self,
            id: PartnershipApplicationId,
        ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
        {
            self.find_by_id(id).await
        }

        async fn find_by_user_and_id(
            &mut self,
            user_id: UserId,
            id: PartnershipApplicationId,
        ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
        {
            Ok(self
                .find_by_id(id)
                .await?
                .filter(|application| application.value.applicant_user_id() == user_id))
        }

        async fn insert(
            &mut self,
            _application: &PartnershipApplication,
        ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>
        {
            Err(PartnershipApplicationRepositoryError::Internal {
                source: static_error("unexpected application insert"),
            })
        }

        async fn update(
            &mut self,
            _application: &PartnershipApplication,
            _expected: PartnershipApplicationStorageVersion,
        ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>
        {
            Err(PartnershipApplicationRepositoryError::Internal {
                source: static_error("unexpected application update"),
            })
        }
    }

    fn application_version(value: i64) -> PartnershipApplicationStorageVersion {
        match PartnershipApplicationStorageVersion::try_from(value) {
            Ok(version) => version,
            Err(error) => panic!("valid test storage version: {error}"),
        }
    }

    fn application() -> PartnershipApplication {
        PartnershipApplication::submit(NewPartnershipApplication {
            id: PartnershipApplicationId::new(),
            applicant_user_id: UserId::new(),
            proposal: PartnershipProposal::ExistingListingSource {
                listing_source_id: ListingSourceId::new(),
            },
        })
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn lock(state: &Arc<Mutex<State>>) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn state_with(
        application: Option<PartnershipApplication>,
        admin: Option<UserAdminActorView>,
    ) -> State {
        State {
            application,
            admin,
            ..Default::default()
        }
    }

    fn handler(
        state: Arc<Mutex<State>>,
    ) -> GetPartnershipApplicationHandler<FakeUnitOfWork, FakeFactories, FakeFactories> {
        let factories = FakeFactories {
            state: Arc::clone(&state),
        };
        GetPartnershipApplicationHandler::new(
            FakeUnitOfWork { state },
            factories.clone(),
            factories,
        )
    }

    fn assert_same_transaction(state: &State, expected: &[Binding]) {
        let bindings = state
            .bindings
            .iter()
            .map(|(binding, _)| *binding)
            .collect::<Vec<_>>();
        assert_eq!(expected, bindings.as_slice());
        if let Some((_, transaction_id)) = state.bindings.first() {
            assert!(state.bindings.iter().all(|(_, id)| id == transaction_id));
        }
    }

    #[tokio::test]
    async fn should_authorize_and_read_application_in_one_transaction() {
        let application = application();
        let application_id = application.id();
        let admin_id = UserId::new();
        let state = Arc::new(Mutex::new(state_with(
            Some(application.clone()),
            Some(UserAdminActorView {
                user_id: admin_id,
                role: UserRole::Admin,
            }),
        )));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(admin_id)),
                GetPartnershipApplicationRequest { application_id },
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
        assert_eq!(1, state.begins);
        assert_eq!(1, state.commits);
        assert_same_transaction(&state, &[Binding::Admin, Binding::Application]);
    }

    #[tokio::test]
    async fn should_reject_non_admin_before_loading_application() {
        let application = application();
        let application_id = application.id();
        let user_id = UserId::new();
        let state = Arc::new(Mutex::new(state_with(
            Some(application),
            Some(UserAdminActorView {
                user_id,
                role: UserRole::User,
            }),
        )));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(user_id)),
                GetPartnershipApplicationRequest { application_id },
            )
            .await;

        assert!(matches!(
            result,
            Err(GetPartnershipApplicationError::Forbidden)
        ));
        let state = lock(&state);
        assert_eq!(1, state.admin_reads);
        assert_eq!(0, state.application_bindings);
        assert_eq!(0, state.application_reads);
        assert_eq!(0, state.commits);
        assert_eq!([(Binding::Admin, 1)], state.bindings.as_slice());
    }

    #[tokio::test]
    async fn should_map_authorization_failures_without_loading_application() {
        let errors = [
            UserAdminReadError::TemporarilyUnavailable {
                source: static_error("temporary"),
            },
            UserAdminReadError::InvalidReadModel {
                source: static_error("invalid"),
            },
            UserAdminReadError::Internal {
                source: static_error("internal"),
            },
        ];

        for (index, error) in errors.into_iter().enumerate() {
            let admin_id = UserId::new();
            let mut initial = state_with(
                Some(application()),
                Some(UserAdminActorView {
                    user_id: admin_id,
                    role: UserRole::Admin,
                }),
            );
            initial.admin_error = Some(error);
            let state = Arc::new(Mutex::new(initial));

            let result = handler(Arc::clone(&state))
                .execute(
                    &context(Principal::User(admin_id)),
                    GetPartnershipApplicationRequest {
                        application_id: PartnershipApplicationId::new(),
                    },
                )
                .await;

            match index {
                0 => assert!(matches!(
                    result,
                    Err(GetPartnershipApplicationError::TemporarilyUnavailable { .. })
                )),
                1 => assert!(matches!(
                    result,
                    Err(GetPartnershipApplicationError::InvalidPersistedState { .. })
                )),
                _ => assert!(matches!(
                    result,
                    Err(GetPartnershipApplicationError::Internal { .. })
                )),
            }
            let state = lock(&state);
            assert_eq!(0, state.application_reads);
            assert_eq!(0, state.commits);
            assert_eq!([(Binding::Admin, 1)], state.bindings.as_slice());
        }
    }

    #[tokio::test]
    async fn should_return_not_found_without_committing() {
        let admin_id = UserId::new();
        let state = Arc::new(Mutex::new(state_with(
            None,
            Some(UserAdminActorView {
                user_id: admin_id,
                role: UserRole::Admin,
            }),
        )));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::User(admin_id)),
                GetPartnershipApplicationRequest {
                    application_id: PartnershipApplicationId::new(),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(GetPartnershipApplicationError::NotFound)
        ));
        let state = lock(&state);
        assert_eq!(1, state.application_reads);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_map_application_repository_failures_without_committing() {
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
            let admin_id = UserId::new();
            let mut initial = state_with(
                Some(application()),
                Some(UserAdminActorView {
                    user_id: admin_id,
                    role: UserRole::Admin,
                }),
            );
            initial.application_error = Some(error);
            let state = Arc::new(Mutex::new(initial));

            let result = handler(Arc::clone(&state))
                .execute(
                    &context(Principal::User(admin_id)),
                    GetPartnershipApplicationRequest {
                        application_id: PartnershipApplicationId::new(),
                    },
                )
                .await;

            match index {
                0 => assert!(matches!(
                    result,
                    Err(GetPartnershipApplicationError::TemporarilyUnavailable { .. })
                )),
                1 => assert!(matches!(
                    result,
                    Err(GetPartnershipApplicationError::InvalidPersistedState { .. })
                )),
                _ => assert!(matches!(
                    result,
                    Err(GetPartnershipApplicationError::Internal { .. })
                )),
            }
            let state = lock(&state);
            assert_eq!(1, state.application_reads);
            assert_eq!(0, state.commits);
        }
    }

    #[tokio::test]
    async fn should_report_begin_and_commit_failures() {
        let begin_state = Arc::new(Mutex::new(state_with(None, None)));
        lock(&begin_state).begin_fails = true;
        let begin_result = handler(Arc::clone(&begin_state))
            .execute(
                &context(Principal::System),
                GetPartnershipApplicationRequest {
                    application_id: PartnershipApplicationId::new(),
                },
            )
            .await;
        assert!(matches!(
            begin_result,
            Err(GetPartnershipApplicationError::BeginTransactionFailed)
        ));
        {
            let begin_state = lock(&begin_state);
            assert_eq!(1, begin_state.begins);
            assert_eq!(0, begin_state.admin_bindings);
            assert_eq!(0, begin_state.application_reads);
            assert_eq!(0, begin_state.commits);
        }

        let application = application();
        let admin_id = UserId::new();
        let commit_state = Arc::new(Mutex::new(state_with(
            Some(application.clone()),
            Some(UserAdminActorView {
                user_id: admin_id,
                role: UserRole::Admin,
            }),
        )));
        lock(&commit_state).commit_fails = true;
        let commit_result = handler(Arc::clone(&commit_state))
            .execute(
                &context(Principal::User(admin_id)),
                GetPartnershipApplicationRequest {
                    application_id: application.id(),
                },
            )
            .await;
        assert!(matches!(
            commit_result,
            Err(GetPartnershipApplicationError::CommitTransactionFailed)
        ));
        let commit_state = lock(&commit_state);
        assert_eq!(1, commit_state.commit_attempts);
        assert_eq!(0, commit_state.commits);
    }

    #[tokio::test]
    async fn should_reject_anonymous_before_loading_application() {
        let application = application();
        let application_id = application.id();
        let state = Arc::new(Mutex::new(state_with(Some(application), None)));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(Principal::Anonymous),
                GetPartnershipApplicationRequest { application_id },
            )
            .await;

        assert!(matches!(
            result,
            Err(GetPartnershipApplicationError::Forbidden)
        ));
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(0, state.admin_reads);
        assert_eq!(0, state.application_reads);
        assert_eq!(0, state.commits);
        assert!(state.bindings.is_empty());
    }
}
