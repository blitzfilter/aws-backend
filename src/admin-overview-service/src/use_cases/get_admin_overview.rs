use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::{AdminOverviewReadError, AdminOverviewReader, AdminOverviewReaderFactory},
};
use application::{
    error::BoxError,
    operation_context::OperationContext,
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use user_service::ports::UserAdminReaderFactory;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminOverview {
    pub users: AdminOverviewUsers,
    pub partnership_applications: AdminOverviewPartnershipApplications,
    pub parties_total: u64,
    pub listing_sources: AdminOverviewListingSources,
    pub partnerships_total: u64,
    pub product_listings: AdminOverviewProductListings,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminOverviewUsers {
    pub total: u64,
    pub by_tier: AdminOverviewUserTierCounts,
    pub by_role: AdminOverviewUserRoleCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminOverviewUserTierCounts {
    pub free: u64,
    pub pro: u64,
    pub ultimate: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminOverviewUserRoleCounts {
    pub user: u64,
    pub admin: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminOverviewPartnershipApplications {
    pub total: u64,
    pub by_state: AdminOverviewPartnershipApplicationStateCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminOverviewPartnershipApplicationStateCounts {
    pub submitted: u64,
    pub in_review: u64,
    pub approved: u64,
    pub rejected: u64,
    pub withdrawn: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminOverviewListingSources {
    pub total: u64,
    pub without_ingestion_method: u64,
    pub method_assignments: AdminOverviewListingSourceMethodAssignmentCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminOverviewListingSourceMethodAssignmentCounts {
    pub web_crawl: u64,
    pub shopify: u64,
    pub woocommerce: u64,
    pub partner_api: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminOverviewProductListings {
    pub total: u64,
    pub by_lifecycle: AdminOverviewProductListingLifecycleCounts,
    pub active_availability: AdminOverviewActiveListingAvailabilityCounts,
    pub active_without_availability: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminOverviewProductListingLifecycleCounts {
    pub active: u64,
    pub withdrawn: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminOverviewActiveListingAvailabilityCounts {
    pub available: u64,
    pub in_stock: u64,
    pub limited_availability: u64,
    pub back_order: u64,
    pub made_to_order: u64,
    pub pre_order: u64,
    pub pre_sale: u64,
    pub unavailable: u64,
    pub reserved: u64,
    pub out_of_stock: u64,
    pub sold_out: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum GetAdminOverviewError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary admin authorization failure")]
    AdminAuthorizationTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid admin authorization read model")]
    AdminAuthorizationInvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal admin authorization failure")]
    AdminAuthorizationInternal {
        #[source]
        source: BoxError,
    },
    #[error("temporary admin overview reader failure")]
    ReaderTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid admin overview reader model")]
    ReaderInvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal admin overview reader failure")]
    ReaderInternal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin admin overview transaction")]
    BeginTransaction {
        #[source]
        source: TransactionError,
    },
    #[error("failed to commit admin overview transaction")]
    CommitTransaction {
        #[source]
        source: TransactionError,
    },
}

#[async_trait::async_trait]
pub trait GetAdminOverviewUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
    ) -> Result<AdminOverview, GetAdminOverviewError>;
}

pub struct GetAdminOverviewHandler<U, R, A> {
    unit_of_work: U,
    reader: R,
    admins: A,
}

impl<U, R, A> GetAdminOverviewHandler<U, R, A> {
    pub fn new(unit_of_work: U, reader: R, admins: A) -> Self {
        Self {
            unit_of_work,
            reader,
            admins,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> GetAdminOverviewUseCase for GetAdminOverviewHandler<U, R, A>
where
    U: UnitOfWork,
    R: AdminOverviewReaderFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "get_admin_overview",
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
    ) -> Result<AdminOverview, GetAdminOverviewError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|source| GetAdminOverviewError::BeginTransaction { source })?;
        authorize_admin(context, &mut tx, &self.admins).await?;
        let overview = self.reader.in_transaction(&mut tx).read_overview().await?;

        tx.commit()
            .await
            .map_err(|source| GetAdminOverviewError::CommitTransaction { source })?;

        Ok(overview)
    }
}

impl From<AdminAuthorizationError> for GetAdminOverviewError {
    fn from(value: AdminAuthorizationError) -> Self {
        match value {
            AdminAuthorizationError::AuthenticatedActorRequired => Self::AuthenticatedActorRequired,
            AdminAuthorizationError::Forbidden => Self::Forbidden,
            AdminAuthorizationError::TemporarilyUnavailable { source } => {
                Self::AdminAuthorizationTemporarilyUnavailable { source }
            }
            AdminAuthorizationError::InvalidReadModel { source } => {
                Self::AdminAuthorizationInvalidReadModel { source }
            }
            AdminAuthorizationError::Internal { source } => {
                Self::AdminAuthorizationInternal { source }
            }
        }
    }
}

impl From<AdminOverviewReadError> for GetAdminOverviewError {
    fn from(value: AdminOverviewReadError) -> Self {
        match value {
            AdminOverviewReadError::TemporarilyUnavailable { source } => {
                Self::ReaderTemporarilyUnavailable { source }
            }
            AdminOverviewReadError::InvalidReadModel { source } => {
                Self::ReaderInvalidReadModel { source }
            }
            AdminOverviewReadError::Internal { source } => Self::ReaderInternal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        error::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
    };
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::{role::UserRole, user_id::UserId};
    use user_service::ports::{
        UserAdminActorView, UserAdminReadError, UserAdminReader, UserAdminReaderFactory,
    };

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        admin_bindings: usize,
        admin_reads: usize,
        overview_bindings: usize,
        overview_reads: usize,
    }

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: Arc<Mutex<State>>,
        begin_fails: bool,
        commit_fails: bool,
    }

    struct FakeTransaction {
        state: Arc<Mutex<State>>,
        commit_fails: bool,
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            if self.commit_fails {
                return Err(TransactionError::CommitFailed);
            }
            lock(&self.state).commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            if self.begin_fails {
                return Err(TransactionError::BeginFailed);
            }
            lock(&self.state).begins += 1;
            Ok(FakeTransaction {
                state: Arc::clone(&self.state),
                commit_fails: self.commit_fails,
            })
        }
    }

    #[derive(Clone, Copy)]
    enum ReaderFailure {
        TemporarilyUnavailable,
    }

    #[derive(Clone)]
    struct FakeOverviewReaderFactory {
        state: Arc<Mutex<State>>,
        overview: AdminOverview,
        failure: Option<ReaderFailure>,
    }

    struct FakeOverviewReader {
        state: Arc<Mutex<State>>,
        overview: AdminOverview,
        failure: Option<ReaderFailure>,
    }

    impl AdminOverviewReaderFactory<FakeTransaction> for FakeOverviewReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl AdminOverviewReader + 'tx {
            lock(&self.state).overview_bindings += 1;
            FakeOverviewReader {
                state: Arc::clone(&self.state),
                overview: self.overview.clone(),
                failure: self.failure,
            }
        }
    }

    #[async_trait::async_trait]
    impl AdminOverviewReader for FakeOverviewReader {
        async fn read_overview(&mut self) -> Result<AdminOverview, AdminOverviewReadError> {
            lock(&self.state).overview_reads += 1;
            match self.failure {
                Some(ReaderFailure::TemporarilyUnavailable) => {
                    Err(AdminOverviewReadError::TemporarilyUnavailable {
                        source: static_error("reader unavailable"),
                    })
                }
                None => Ok(self.overview.clone()),
            }
        }
    }

    #[derive(Clone)]
    struct FakeAdminReaderFactory {
        state: Arc<Mutex<State>>,
        actor: Option<UserAdminActorView>,
    }

    struct FakeAdminReader {
        state: Arc<Mutex<State>>,
        actor: Option<UserAdminActorView>,
    }

    impl UserAdminReaderFactory<FakeTransaction> for FakeAdminReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl UserAdminReader + 'tx {
            lock(&self.state).admin_bindings += 1;
            FakeAdminReader {
                state: Arc::clone(&self.state),
                actor: self.actor.clone(),
            }
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeAdminReader {
        async fn find_admin_actor(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            lock(&self.state).admin_reads += 1;
            Ok(self.actor.clone())
        }
    }

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        match value.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn admin_actor() -> UserAdminActorView {
        UserAdminActorView {
            user_id: UserId::new(),
            role: UserRole::Admin,
        }
    }

    fn handler(
        state: Arc<Mutex<State>>,
        overview: AdminOverview,
        actor: Option<UserAdminActorView>,
        reader_failure: Option<ReaderFailure>,
    ) -> GetAdminOverviewHandler<FakeUnitOfWork, FakeOverviewReaderFactory, FakeAdminReaderFactory>
    {
        GetAdminOverviewHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(&state),
                begin_fails: false,
                commit_fails: false,
            },
            FakeOverviewReaderFactory {
                state: Arc::clone(&state),
                overview,
                failure: reader_failure,
            },
            FakeAdminReaderFactory { state, actor },
        )
    }

    #[tokio::test]
    async fn should_authorize_admin_and_commit_overview_read() {
        let state = Arc::new(Mutex::new(State::default()));
        let expected = AdminOverview {
            users: AdminOverviewUsers {
                total: 3,
                ..Default::default()
            },
            ..Default::default()
        };

        let result = handler(
            Arc::clone(&state),
            expected.clone(),
            Some(admin_actor()),
            None,
        )
        .execute(&context(Principal::User(UserId::new())))
        .await;

        let overview = match result {
            Ok(overview) => overview,
            Err(error) => panic!("overview read failed: {error}"),
        };
        assert_eq!(expected, overview);
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.admin_bindings);
        assert_eq!(1, state.admin_reads);
        assert_eq!(1, state.overview_bindings);
        assert_eq!(1, state.overview_reads);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_reject_non_admin_without_read_or_commit() {
        let state = Arc::new(Mutex::new(State::default()));
        let result = handler(Arc::clone(&state), AdminOverview::default(), None, None)
            .execute(&context(Principal::User(UserId::new())))
            .await;

        assert!(matches!(result, Err(GetAdminOverviewError::Forbidden)));
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.admin_reads);
        assert_eq!(0, state.overview_reads);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_reject_anonymous_actor_as_unauthenticated() {
        let state = Arc::new(Mutex::new(State::default()));
        let result = handler(Arc::clone(&state), AdminOverview::default(), None, None)
            .execute(&context(Principal::Anonymous))
            .await;

        assert!(matches!(
            result,
            Err(GetAdminOverviewError::AuthenticatedActorRequired)
        ));
        assert_eq!(0, lock(&state).overview_reads);
    }

    #[tokio::test]
    async fn should_return_empty_overview_for_system_actor() {
        let state = Arc::new(Mutex::new(State::default()));
        let result = handler(Arc::clone(&state), AdminOverview::default(), None, None)
            .execute(&context(Principal::System))
            .await;

        let overview = match result {
            Ok(overview) => overview,
            Err(error) => panic!("overview read failed: {error}"),
        };
        assert_eq!(AdminOverview::default(), overview);
        let state = lock(&state);
        assert_eq!(0, state.admin_reads);
        assert_eq!(1, state.overview_reads);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_preserve_reader_failure_without_committing() {
        let state = Arc::new(Mutex::new(State::default()));
        let result = handler(
            Arc::clone(&state),
            AdminOverview::default(),
            None,
            Some(ReaderFailure::TemporarilyUnavailable),
        )
        .execute(&context(Principal::System))
        .await;

        assert!(matches!(
            result,
            Err(GetAdminOverviewError::ReaderTemporarilyUnavailable { .. })
        ));
        assert_eq!(0, lock(&state).commits);
    }
}
