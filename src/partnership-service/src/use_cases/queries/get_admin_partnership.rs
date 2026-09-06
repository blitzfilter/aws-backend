use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::{
        PartnershipDetailsReadError, PartnershipDetailsReader, PartnershipDetailsReaderFactory,
    },
};
use application::{
    error::BoxError,
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use listing_source_core::ListingSourceId;
use partnership_core::partnership_id::PartnershipId;
use time::OffsetDateTime;
use user_core::user_id::UserId;
use user_service::ports::UserAdminReaderFactory;

use super::list_admin_partnerships::PartnershipPartySummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GetAdminPartnershipRequest {
    pub partnership_id: PartnershipId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminPartnershipDetailsView {
    pub partnership_id: PartnershipId,
    pub party: PartnershipPartySummary,
    pub member_user_ids: Vec<UserId>,
    pub listing_source_ids: Vec<ListingSourceId>,
    pub member_count: u64,
    pub listing_source_grant_count: u64,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum GetAdminPartnershipError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("partnership not found")]
    NotFound,
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
pub trait GetAdminPartnershipUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAdminPartnershipRequest,
    ) -> Result<AdminPartnershipDetailsView, GetAdminPartnershipError>;
}

pub struct GetAdminPartnershipHandler<U, R, A> {
    unit_of_work: U,
    reader: R,
    admins: A,
}

impl<U, R, A> GetAdminPartnershipHandler<U, R, A> {
    pub fn new(unit_of_work: U, reader: R, admins: A) -> Self {
        Self {
            unit_of_work,
            reader,
            admins,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> GetAdminPartnershipUseCase for GetAdminPartnershipHandler<U, R, A>
where
    U: UnitOfWork,
    R: PartnershipDetailsReaderFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "get_admin_partnership",
        skip_all,
        fields(
            partnership_id = %request.partnership_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAdminPartnershipRequest,
    ) -> Result<AdminPartnershipDetailsView, GetAdminPartnershipError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetAdminPartnershipError::BeginTransactionFailed)?;
        authorize_admin(context, &mut tx, &self.admins).await?;
        let result = self
            .reader
            .in_transaction(&mut tx)
            .find_by_id(request.partnership_id)
            .await?
            .ok_or(GetAdminPartnershipError::NotFound)?;

        tx.commit()
            .await
            .map_err(|_| GetAdminPartnershipError::CommitTransactionFailed)?;
        Ok(result)
    }
}

impl From<AdminAuthorizationError> for GetAdminPartnershipError {
    fn from(value: AdminAuthorizationError) -> Self {
        match value {
            AdminAuthorizationError::Forbidden => Self::Forbidden,
            AdminAuthorizationError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AdminAuthorizationError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            AdminAuthorizationError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<PartnershipDetailsReadError> for GetAdminPartnershipError {
    fn from(value: PartnershipDetailsReadError) -> Self {
        match value {
            PartnershipDetailsReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipDetailsReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            PartnershipDetailsReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        PartnershipDetailsReadError, PartnershipDetailsReader, PartnershipDetailsReaderFactory,
    };
    use application::{
        error::static_error,
        operation_context::{CorrelationId, OperationContext, Principal, RequestId},
        transaction::TransactionError,
    };
    use listing_source_core::ListingSourceId;
    use party_core::{party_id::PartyId, party_name::PartyName, party_slug_id::PartySlugId};
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::role::UserRole;
    use user_service::ports::{
        UserAdminActorView, UserAdminReadError, UserAdminReader, UserAdminReaderFactory,
    };

    #[derive(Default)]
    struct State {
        begins: usize,
        admin_bindings: usize,
        admin_reads: usize,
        reader_bindings: usize,
        reads: usize,
        commits: usize,
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

    type ReaderOutcome = Arc<
        Mutex<Option<Result<Option<AdminPartnershipDetailsView>, PartnershipDetailsReadError>>>,
    >;

    #[derive(Clone)]
    struct FakeReaderFactory {
        state: Arc<Mutex<State>>,
        outcome: ReaderOutcome,
    }

    struct FakeReader {
        state: Arc<Mutex<State>>,
        outcome: ReaderOutcome,
    }

    impl PartnershipDetailsReaderFactory<FakeTransaction> for FakeReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PartnershipDetailsReader + 'tx {
            lock(&self.state).reader_bindings += 1;
            FakeReader {
                state: Arc::clone(&self.state),
                outcome: Arc::clone(&self.outcome),
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnershipDetailsReader for FakeReader {
        async fn find_by_id(
            &mut self,
            _partnership_id: PartnershipId,
        ) -> Result<Option<AdminPartnershipDetailsView>, PartnershipDetailsReadError> {
            lock(&self.state).reads += 1;
            lock(&self.outcome).take().unwrap_or_else(|| {
                Err(PartnershipDetailsReadError::Internal {
                    source: static_error("test outcome was not configured"),
                })
            })
        }
    }

    #[derive(Clone)]
    struct FakeAdminFactory {
        state: Arc<Mutex<State>>,
        actor: Option<UserAdminActorView>,
    }

    struct FakeAdminReader {
        state: Arc<Mutex<State>>,
        actor: Option<UserAdminActorView>,
    }

    impl UserAdminReaderFactory<FakeTransaction> for FakeAdminFactory {
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

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn details() -> AdminPartnershipDetailsView {
        AdminPartnershipDetailsView {
            partnership_id: PartnershipId::new(),
            party: PartnershipPartySummary {
                party_id: PartyId::new(),
                party_slug_id: PartySlugId::raw("safe-party")
                    .unwrap_or_else(|error| panic!("valid party slug: {error}")),
                name: PartyName::try_from("Safe Party")
                    .unwrap_or_else(|error| panic!("valid party name: {error}")),
            },
            member_user_ids: vec![UserId::new()],
            listing_source_ids: vec![ListingSourceId::new()],
            member_count: 1,
            listing_source_grant_count: 1,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    fn handler(
        state: Arc<Mutex<State>>,
        outcome: Result<Option<AdminPartnershipDetailsView>, PartnershipDetailsReadError>,
        actor: Option<UserAdminActorView>,
        begin_fails: bool,
        commit_fails: bool,
    ) -> GetAdminPartnershipHandler<FakeUnitOfWork, FakeReaderFactory, FakeAdminFactory> {
        GetAdminPartnershipHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(&state),
                begin_fails,
                commit_fails,
            },
            FakeReaderFactory {
                state: Arc::clone(&state),
                outcome: Arc::new(Mutex::new(Some(outcome))),
            },
            FakeAdminFactory { state, actor },
        )
    }

    fn admin_actor() -> UserAdminActorView {
        UserAdminActorView {
            user_id: UserId::new(),
            role: UserRole::Admin,
        }
    }

    #[tokio::test]
    async fn should_authorize_and_read_details_in_one_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let expected = details();
        let partnership_id = expected.partnership_id;
        let result = handler(
            Arc::clone(&state),
            Ok(Some(expected.clone())),
            Some(admin_actor()),
            false,
            false,
        )
        .execute(&context(), GetAdminPartnershipRequest { partnership_id })
        .await
        .unwrap_or_else(|error| panic!("get admin partnership failed: {error}"));

        assert_eq!(expected, result);
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.admin_bindings);
        assert_eq!(1, state.admin_reads);
        assert_eq!(1, state.reader_bindings);
        assert_eq!(1, state.reads);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_return_not_found_without_committing_when_reader_has_no_partnership() {
        let state = Arc::new(Mutex::new(State::default()));
        let result = handler(
            Arc::clone(&state),
            Ok(None),
            Some(admin_actor()),
            false,
            false,
        )
        .execute(
            &context(),
            GetAdminPartnershipRequest {
                partnership_id: PartnershipId::new(),
            },
        )
        .await;

        assert!(matches!(result, Err(GetAdminPartnershipError::NotFound)));
        assert_eq!(0, lock(&state).commits);
    }

    #[tokio::test]
    async fn should_forbid_non_admin_before_detail_read() {
        let state = Arc::new(Mutex::new(State::default()));
        let result = handler(
            Arc::clone(&state),
            Ok(Some(details())),
            Some(UserAdminActorView {
                user_id: UserId::new(),
                role: UserRole::User,
            }),
            false,
            false,
        )
        .execute(
            &context(),
            GetAdminPartnershipRequest {
                partnership_id: PartnershipId::new(),
            },
        )
        .await;

        assert!(matches!(result, Err(GetAdminPartnershipError::Forbidden)));
        let state = lock(&state);
        assert_eq!(0, state.reader_bindings);
        assert_eq!(0, state.reads);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_translate_reader_and_transaction_failures() {
        let reader_state = Arc::new(Mutex::new(State::default()));
        let reader_result = handler(
            Arc::clone(&reader_state),
            Err(PartnershipDetailsReadError::TemporarilyUnavailable {
                source: static_error("temporary"),
            }),
            Some(admin_actor()),
            false,
            false,
        )
        .execute(
            &context(),
            GetAdminPartnershipRequest {
                partnership_id: PartnershipId::new(),
            },
        )
        .await;
        assert!(matches!(
            reader_result,
            Err(GetAdminPartnershipError::TemporarilyUnavailable { .. })
        ));

        let begin_state = Arc::new(Mutex::new(State::default()));
        let begin_result = handler(
            Arc::clone(&begin_state),
            Ok(Some(details())),
            Some(admin_actor()),
            true,
            false,
        )
        .execute(
            &context(),
            GetAdminPartnershipRequest {
                partnership_id: PartnershipId::new(),
            },
        )
        .await;
        assert!(matches!(
            begin_result,
            Err(GetAdminPartnershipError::BeginTransactionFailed)
        ));

        let commit_state = Arc::new(Mutex::new(State::default()));
        let commit_result = handler(
            Arc::clone(&commit_state),
            Ok(Some(details())),
            Some(admin_actor()),
            false,
            true,
        )
        .execute(
            &context(),
            GetAdminPartnershipRequest {
                partnership_id: PartnershipId::new(),
            },
        )
        .await;
        assert!(matches!(
            commit_result,
            Err(GetAdminPartnershipError::CommitTransactionFailed)
        ));
    }
}
