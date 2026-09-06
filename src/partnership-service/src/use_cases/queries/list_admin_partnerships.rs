use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::{PartnershipSearchReader, PartnershipSearchReaderFactory},
};
use application::{
    error::BoxError,
    operation_context::OperationContext,
    pagination::{Cursor, CursoredResult},
    transaction::{Transaction, UnitOfWork},
};
use listing_source_core::ListingSourceId;
use partnership_core::partnership_id::PartnershipId;
use party_core::{party_id::PartyId, party_name::PartyName, party_slug_id::PartySlugId};
use time::OffsetDateTime;
use user_core::user_id::UserId;
use user_service::ports::UserAdminReaderFactory;

const MAX_CURSOR_SIZE: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartnershipSearchCursor {
    pub position: OffsetDateTime,
    pub partnership_id: PartnershipId,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListAdminPartnershipsRequest {
    pub party_id: Option<PartyId>,
    pub member_user_id: Option<UserId>,
    pub listing_source_id: Option<ListingSourceId>,
    pub cursor: Option<Cursor<PartnershipSearchCursor>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartnershipPartySummary {
    pub party_id: PartyId,
    pub party_slug_id: PartySlugId,
    pub name: PartyName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminPartnershipSummary {
    pub partnership_id: PartnershipId,
    pub party: PartnershipPartySummary,
    pub member_count: u64,
    pub listing_source_grant_count: u64,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

pub type ListAdminPartnershipsResult =
    CursoredResult<AdminPartnershipSummary, PartnershipSearchCursor>;

#[derive(Debug, thiserror::Error)]
pub enum ListAdminPartnershipsError {
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
pub trait ListAdminPartnershipsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAdminPartnershipsRequest,
    ) -> Result<ListAdminPartnershipsResult, ListAdminPartnershipsError>;
}

pub struct ListAdminPartnershipsHandler<U, R, A> {
    unit_of_work: U,
    reader: R,
    admins: A,
}

impl<U, R, A> ListAdminPartnershipsHandler<U, R, A> {
    pub fn new(unit_of_work: U, reader: R, admins: A) -> Self {
        Self {
            unit_of_work,
            reader,
            admins,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> ListAdminPartnershipsUseCase for ListAdminPartnershipsHandler<U, R, A>
where
    U: UnitOfWork,
    R: PartnershipSearchReaderFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "list_admin_partnerships",
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
        request: ListAdminPartnershipsRequest,
    ) -> Result<ListAdminPartnershipsResult, ListAdminPartnershipsError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListAdminPartnershipsError::BeginTransactionFailed)?;
        authorize_admin(context, &mut tx, &self.admins).await?;

        let request = clamp_request_cursor(request);
        let mut result = self.reader.in_transaction(&mut tx).search(&request).await?;
        result.cursor.size = result.cursor.size.clamp(1, MAX_CURSOR_SIZE);
        tracing::Span::current().record("result_count", result.items.len());

        tx.commit()
            .await
            .map_err(|_| ListAdminPartnershipsError::CommitTransactionFailed)?;
        Ok(result)
    }
}

fn clamp_request_cursor(mut request: ListAdminPartnershipsRequest) -> ListAdminPartnershipsRequest {
    if let Some(cursor) = request.cursor.as_mut() {
        cursor.size = cursor.size.clamp(1, MAX_CURSOR_SIZE);
    }
    request
}

impl From<AdminAuthorizationError> for ListAdminPartnershipsError {
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

impl From<crate::ports::PartnershipSearchReadError> for ListAdminPartnershipsError {
    fn from(value: crate::ports::PartnershipSearchReadError) -> Self {
        match value {
            crate::ports::PartnershipSearchReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            crate::ports::PartnershipSearchReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            crate::ports::PartnershipSearchReadError::Internal { source } => {
                Self::Internal { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        PartnershipSearchReadError, PartnershipSearchReader, PartnershipSearchReaderFactory,
    };
    use application::{
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use std::sync::{Arc, Mutex};
    use user_core::role::UserRole;
    use user_service::ports::{
        UserAdminActorView, UserAdminReadError, UserAdminReader, UserAdminReaderFactory,
    };

    #[derive(Clone, Copy)]
    enum ReaderFailure {
        TemporarilyUnavailable,
        InvalidReadModel,
        Internal,
    }

    #[derive(Default)]
    struct State {
        begins: usize,
        admin_bindings: usize,
        admin_reads: usize,
        reader_bindings: usize,
        searches: usize,
        commits: usize,
        request: Option<ListAdminPartnershipsRequest>,
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
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.commits += 1;
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
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.begins += 1;
            drop(state);
            Ok(FakeTransaction {
                state: Arc::clone(&self.state),
                commit_fails: self.commit_fails,
            })
        }
    }

    #[derive(Clone)]
    struct FakeReaderFactory {
        state: Arc<Mutex<State>>,
        failure: Option<ReaderFailure>,
        returned_cursor_size: u64,
    }

    struct FakeReader {
        state: Arc<Mutex<State>>,
        failure: Option<ReaderFailure>,
        returned_cursor_size: u64,
    }

    impl PartnershipSearchReaderFactory<FakeTransaction> for FakeReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PartnershipSearchReader + 'tx {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.reader_bindings += 1;
            drop(state);
            FakeReader {
                state: Arc::clone(&self.state),
                failure: self.failure,
                returned_cursor_size: self.returned_cursor_size,
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnershipSearchReader for FakeReader {
        async fn search(
            &mut self,
            request: &ListAdminPartnershipsRequest,
        ) -> Result<ListAdminPartnershipsResult, PartnershipSearchReadError> {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.searches += 1;
            state.request = Some(request.clone());
            drop(state);

            match self.failure {
                Some(ReaderFailure::TemporarilyUnavailable) => {
                    Err(PartnershipSearchReadError::TemporarilyUnavailable {
                        source: application::error::static_error("reader unavailable"),
                    })
                }
                Some(ReaderFailure::InvalidReadModel) => {
                    Err(PartnershipSearchReadError::InvalidReadModel {
                        source: application::error::static_error("reader model invalid"),
                    })
                }
                Some(ReaderFailure::Internal) => Err(PartnershipSearchReadError::Internal {
                    source: application::error::static_error("reader failed"),
                }),
                None => Ok(CursoredResult {
                    items: Vec::new(),
                    cursor: Cursor {
                        size: self.returned_cursor_size,
                        search_after: None,
                    },
                    total: None,
                }),
            }
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
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.admin_bindings += 1;
            drop(state);
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
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.admin_reads += 1;
            drop(state);
            Ok(self.actor.clone())
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn request(size: u64) -> ListAdminPartnershipsRequest {
        ListAdminPartnershipsRequest {
            party_id: Some(PartyId::new()),
            member_user_id: Some(UserId::new()),
            listing_source_id: Some(ListingSourceId::new()),
            cursor: Some(Cursor {
                size,
                search_after: Some(PartnershipSearchCursor {
                    position: time::macros::datetime!(2026-09-04 12:00 UTC),
                    partnership_id: PartnershipId::new(),
                }),
            }),
        }
    }

    fn handler(
        state: Arc<Mutex<State>>,
        actor: Option<UserAdminActorView>,
        failure: Option<ReaderFailure>,
        begin_fails: bool,
        commit_fails: bool,
        returned_cursor_size: u64,
    ) -> ListAdminPartnershipsHandler<FakeUnitOfWork, FakeReaderFactory, FakeAdminFactory> {
        ListAdminPartnershipsHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(&state),
                begin_fails,
                commit_fails,
            },
            FakeReaderFactory {
                state: Arc::clone(&state),
                failure,
                returned_cursor_size,
            },
            FakeAdminFactory { state, actor },
        )
    }

    fn admin_actor(user_id: UserId) -> UserAdminActorView {
        UserAdminActorView {
            user_id,
            role: UserRole::Admin,
        }
    }

    #[tokio::test]
    async fn should_authorize_admin_forward_request_and_commit() {
        let state = Arc::new(Mutex::new(State::default()));
        let admin_id = UserId::new();
        let expected_request = request(2);
        let result = handler(
            Arc::clone(&state),
            Some(admin_actor(admin_id)),
            None,
            false,
            false,
            2,
        )
        .execute(
            &context(Principal::User(admin_id)),
            expected_request.clone(),
        )
        .await;

        assert!(result.is_ok());
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(1, state.begins);
        assert_eq!(1, state.admin_bindings);
        assert_eq!(1, state.admin_reads);
        assert_eq!(1, state.reader_bindings);
        assert_eq!(1, state.searches);
        assert_eq!(1, state.commits);
        assert_eq!(Some(expected_request), state.request);
    }

    #[tokio::test]
    async fn should_clamp_request_and_result_cursor_sizes() {
        let state = Arc::new(Mutex::new(State::default()));
        let admin_id = UserId::new();
        let input_request = request(0);
        let mut expected_request = input_request.clone();
        if let Some(cursor) = expected_request.cursor.as_mut() {
            cursor.size = 1;
        }
        let result = handler(
            Arc::clone(&state),
            Some(admin_actor(admin_id)),
            None,
            false,
            false,
            101,
        )
        .execute(&context(Principal::User(admin_id)), input_request)
        .await;

        assert!(matches!(result, Ok(result) if result.cursor.size == 100));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(Some(expected_request), state.request);
    }

    #[tokio::test]
    async fn should_map_reader_errors_without_committing() {
        let failures = [
            ReaderFailure::TemporarilyUnavailable,
            ReaderFailure::InvalidReadModel,
            ReaderFailure::Internal,
        ];

        for failure in failures {
            let state = Arc::new(Mutex::new(State::default()));
            let admin_id = UserId::new();
            let result = handler(
                Arc::clone(&state),
                Some(admin_actor(admin_id)),
                Some(failure),
                false,
                false,
                2,
            )
            .execute(&context(Principal::User(admin_id)), request(2))
            .await;

            assert!(matches!(
                (failure, result),
                (
                    ReaderFailure::TemporarilyUnavailable,
                    Err(ListAdminPartnershipsError::TemporarilyUnavailable { .. })
                ) | (
                    ReaderFailure::InvalidReadModel,
                    Err(ListAdminPartnershipsError::InvalidReadModel { .. })
                ) | (
                    ReaderFailure::Internal,
                    Err(ListAdminPartnershipsError::Internal { .. })
                )
            ));
            let state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(1, state.begins);
            assert_eq!(1, state.searches);
            assert_eq!(0, state.commits);
        }
    }

    #[tokio::test]
    async fn should_map_begin_and_commit_failures() {
        let state = Arc::new(Mutex::new(State::default()));
        let admin_id = UserId::new();
        let result = handler(
            Arc::clone(&state),
            Some(admin_actor(admin_id)),
            None,
            true,
            false,
            2,
        )
        .execute(&context(Principal::User(admin_id)), request(2))
        .await;
        assert!(matches!(
            result,
            Err(ListAdminPartnershipsError::BeginTransactionFailed)
        ));

        let state = Arc::new(Mutex::new(State::default()));
        let admin_id = UserId::new();
        let result = handler(
            Arc::clone(&state),
            Some(admin_actor(admin_id)),
            None,
            false,
            true,
            2,
        )
        .execute(&context(Principal::User(admin_id)), request(2))
        .await;
        assert!(matches!(
            result,
            Err(ListAdminPartnershipsError::CommitTransactionFailed)
        ));
    }

    #[tokio::test]
    async fn should_reject_non_admin_without_searching_or_committing() {
        let state = Arc::new(Mutex::new(State::default()));
        let user_id = UserId::new();
        let result = handler(
            Arc::clone(&state),
            Some(UserAdminActorView {
                user_id,
                role: UserRole::User,
            }),
            None,
            false,
            false,
            2,
        )
        .execute(&context(Principal::User(user_id)), request(2))
        .await;

        assert!(matches!(result, Err(ListAdminPartnershipsError::Forbidden)));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(1, state.begins);
        assert_eq!(1, state.admin_bindings);
        assert_eq!(1, state.admin_reads);
        assert_eq!(0, state.reader_bindings);
        assert_eq!(0, state.searches);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_reject_anonymous_without_searching_or_committing() {
        let state = Arc::new(Mutex::new(State::default()));
        let result = handler(Arc::clone(&state), None, None, false, false, 2)
            .execute(&context(Principal::Anonymous), request(2))
            .await;

        assert!(matches!(result, Err(ListAdminPartnershipsError::Forbidden)));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(1, state.begins);
        assert_eq!(0, state.admin_bindings);
        assert_eq!(0, state.reader_bindings);
        assert_eq!(0, state.searches);
        assert_eq!(0, state.commits);
    }
}
