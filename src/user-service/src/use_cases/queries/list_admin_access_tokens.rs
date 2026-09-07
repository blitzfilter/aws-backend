use crate::ports::{
    AdminAccessTokenListReadError, AdminAccessTokenListReader, AdminAccessTokenListReaderFactory,
    UserAccountReadError, UserAccountReader, UserAccountReaderFactory, UserAdminReadError,
    UserAdminReaderFactory,
};
use crate::use_cases::authorization::{
    RequireAdminActorError, require_admin_actor, require_admin_actor_credential,
};
use crate::use_cases::queries::get_access_token::AccessTokenView;
use application::error::BoxError;
use application::operation_context::{CredentialCapability, OperationContext};
use application::pagination::{Cursor, CursoredResult};
use application::transaction::{Transaction, UnitOfWork};
use time::OffsetDateTime;
use user_core::access_token::AccessTokenId;
use user_core::user_id::UserId;

const MAX_CURSOR_SIZE: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessTokenSearchCursor {
    pub position: OffsetDateTime,
    pub access_token_id: AccessTokenId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListAdminAccessTokensRequest {
    pub user_id: UserId,
    pub cursor: Option<Cursor<AccessTokenSearchCursor>>,
}

pub type ListAdminAccessTokensResult = CursoredResult<AccessTokenView, AccessTokenSearchCursor>;

#[derive(Debug, thiserror::Error)]
pub enum ListAdminAccessTokensError {
    #[error("authenticated actor required to list admin access tokens")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    UserNotFound,
    #[error("temporary admin access token list failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted admin access token list state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal admin access token list failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin list admin access tokens transaction")]
    BeginTransactionFailed,
    #[error("failed to commit list admin access tokens transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ListAdminAccessTokensUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAdminAccessTokensRequest,
    ) -> Result<ListAdminAccessTokensResult, ListAdminAccessTokensError>;
}

pub struct ListAdminAccessTokensHandler<U, R, A, V> {
    unit_of_work: U,
    reader: R,
    admin_reader: A,
    user_reader: V,
}

impl<U, R, A, V> ListAdminAccessTokensHandler<U, R, A, V> {
    pub fn new(unit_of_work: U, reader: R, admin_reader: A, user_reader: V) -> Self {
        Self {
            unit_of_work,
            reader,
            admin_reader,
            user_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A, V> ListAdminAccessTokensUseCase for ListAdminAccessTokensHandler<U, R, A, V>
where
    U: UnitOfWork,
    R: AdminAccessTokenListReaderFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
    V: UserAccountReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "list_admin_access_tokens",
        skip_all,
        fields(
            target_user_id = %request.user_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            result_count = tracing::field::Empty,
            outcome = tracing::field::Empty,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAdminAccessTokensRequest,
    ) -> Result<ListAdminAccessTokensResult, ListAdminAccessTokensError> {
        let actor_id = context.principal.actor_id();
        if let Some(actor_id) = actor_id.as_deref() {
            tracing::Span::current().record("actor_id", actor_id);
        }

        let result = async {
            require_admin_actor_credential(context, CredentialCapability::AccessTokensRead)?;

            let mut tx = self
                .unit_of_work
                .begin()
                .await
                .map_err(|_| ListAdminAccessTokensError::BeginTransactionFailed)?;
            {
                let mut admin_reader = self.admin_reader.in_transaction(&mut tx);
                require_admin_actor(context, &mut admin_reader).await?;
            }

            let target_exists = self
                .user_reader
                .in_transaction(&mut tx)
                .find_by_id(request.user_id)
                .await?
                .is_some();
            if !target_exists {
                return Err(ListAdminAccessTokensError::UserNotFound);
            }

            let cursor = clamp_cursor(request.cursor);
            let result = self
                .reader
                .in_transaction(&mut tx)
                .list_for_user(request.user_id, cursor)
                .await?;
            tx.commit()
                .await
                .map_err(|_| ListAdminAccessTokensError::CommitTransactionFailed)?;

            Ok(result.map_item(AccessTokenView::from))
        }
        .await;

        match result {
            Ok(result) => {
                tracing::Span::current().record("result_count", result.items.len());
                tracing::Span::current().record("outcome", "success");
                Ok(result)
            }
            Err(error) => {
                tracing::Span::current().record("outcome", "failure");
                Err(error)
            }
        }
    }
}

fn clamp_cursor(
    cursor: Option<Cursor<AccessTokenSearchCursor>>,
) -> Cursor<AccessTokenSearchCursor> {
    let mut cursor = cursor.unwrap_or_default();
    cursor.size = cursor.size.clamp(1, MAX_CURSOR_SIZE);
    cursor
}

impl From<RequireAdminActorError> for ListAdminAccessTokensError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for ListAdminAccessTokensError {
    fn from(error: UserAdminReadError) -> Self {
        match error {
            UserAdminReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAdminReadError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            UserAdminReadError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<UserAccountReadError> for ListAdminAccessTokensError {
    fn from(error: UserAccountReadError) -> Self {
        match error {
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

impl From<AdminAccessTokenListReadError> for ListAdminAccessTokensError {
    fn from(error: AdminAccessTokenListReadError) -> Self {
        match error {
            AdminAccessTokenListReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AdminAccessTokenListReadError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            AdminAccessTokenListReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        AccessTokenDetails, AdminAccessTokenListReader, UserAccountReader, UserAdminActorView,
        UserAdminReader, UserDetailsView,
    };
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::transaction::TransactionError;
    use localization::Language;
    use money::Currency;
    use serde_email::Email;
    use std::collections::{BTreeSet, HashSet};
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::access_token::{AccessTokenName, AccessTokenOrigin, Scope};
    use user_core::role::UserRole;
    use user_core::tier::UserTier;

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        admin_reads: usize,
        target_reads: usize,
        token_reads: usize,
        admin_role: Option<UserRole>,
        target_exists: bool,
        items: Vec<AccessTokenDetails>,
        cursor: Option<Cursor<AccessTokenSearchCursor>>,
    }

    #[derive(Clone, Default)]
    struct Fakes(Arc<Mutex<State>>);

    struct FakeTx(Fakes);
    struct FakeAdminReader(Fakes);
    struct FakeUserReader(Fakes);
    struct FakeTokenReader(Fakes);

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("req-test"),
            correlation_id: CorrelationId::new("corr-test"),
        }
    }

    fn target_details(user_id: UserId) -> UserDetailsView {
        UserDetailsView {
            user_id,
            email: Email::try_from("target@example.com")
                .unwrap_or_else(|error| panic!("valid test email: {error}")),
            first_name: None,
            last_name: None,
            language: Some(Language::En),
            currency: Some(Currency::Eur),
            measurement_unit: None,
            show_unassessed_or_sensitive_content: false,
            tier: UserTier::Free,
            role: UserRole::User,
            stripe_customer_id: None,
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.0.0).commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for Fakes {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(FakeTx(self.clone()))
        }
    }

    impl UserAdminReaderFactory<FakeTx> for Fakes {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserAdminReader + 'tx {
            FakeAdminReader(self.clone())
        }
    }

    impl UserAccountReaderFactory<FakeTx> for Fakes {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserAccountReader + 'tx {
            FakeUserReader(self.clone())
        }
    }

    impl AdminAccessTokenListReaderFactory<FakeTx> for Fakes {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl AdminAccessTokenListReader + 'tx {
            FakeTokenReader(self.clone())
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeAdminReader {
        async fn find_admin_actor(
            &mut self,
            user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, crate::ports::UserAdminReadError> {
            let mut state = lock(&self.0.0);
            state.admin_reads += 1;
            Ok(state
                .admin_role
                .map(|role| UserAdminActorView { user_id, role }))
        }
    }

    #[async_trait::async_trait]
    impl UserAccountReader for FakeUserReader {
        async fn find_by_id(
            &mut self,
            user_id: UserId,
        ) -> Result<Option<UserDetailsView>, crate::ports::UserAccountReadError> {
            let mut state = lock(&self.0.0);
            state.target_reads += 1;
            Ok(state.target_exists.then(|| target_details(user_id)))
        }
    }

    #[async_trait::async_trait]
    impl AdminAccessTokenListReader for FakeTokenReader {
        async fn list_for_user(
            &mut self,
            _user_id: UserId,
            cursor: Cursor<AccessTokenSearchCursor>,
        ) -> Result<
            CursoredResult<AccessTokenDetails, AccessTokenSearchCursor>,
            crate::ports::AdminAccessTokenListReadError,
        > {
            let mut state = lock(&self.0.0);
            state.token_reads += 1;
            state.cursor = Some(cursor);
            Ok(CursoredResult {
                items: state.items.clone(),
                cursor,
                total: None,
            })
        }
    }

    #[tokio::test]
    async fn should_list_target_tokens_for_admin_in_bounded_transaction() {
        let user_id = UserId::new();
        let fakes = Fakes::default();
        {
            let mut state = lock(&fakes.0);
            state.admin_role = Some(UserRole::Admin);
            state.target_exists = true;
            state.items.push(AccessTokenDetails {
                user_id,
                access_token_id: AccessTokenId::new(),
                name: AccessTokenName::from("admin inspection"),
                scopes: HashSet::from([Scope::UsersRead]),
                origin: AccessTokenOrigin::User,
                expires: None,
            });
        }

        let result = ListAdminAccessTokensHandler::new(
            fakes.clone(),
            fakes.clone(),
            fakes.clone(),
            fakes.clone(),
        )
        .execute(
            &context(Principal::User(UserId::new())),
            ListAdminAccessTokensRequest {
                user_id,
                cursor: Some(Cursor {
                    size: 200,
                    search_after: None,
                }),
            },
        )
        .await;

        assert!(matches!(result, Ok(result) if result.items.len() == 1));
        let state = lock(&fakes.0);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.commits);
        assert_eq!(1, state.admin_reads);
        assert_eq!(1, state.target_reads);
        assert_eq!(1, state.token_reads);
        assert_eq!(Some(100), state.cursor.map(|cursor| cursor.size));
    }

    #[tokio::test]
    async fn should_reject_non_admin_without_listing_target_tokens() {
        let fakes = Fakes::default();
        {
            let mut state = lock(&fakes.0);
            state.admin_role = Some(UserRole::User);
            state.target_exists = true;
        }

        let result = ListAdminAccessTokensHandler::new(
            fakes.clone(),
            fakes.clone(),
            fakes.clone(),
            fakes.clone(),
        )
        .execute(
            &context(Principal::User(UserId::new())),
            ListAdminAccessTokensRequest {
                user_id: UserId::new(),
                cursor: None,
            },
        )
        .await;

        assert!(matches!(result, Err(ListAdminAccessTokensError::Forbidden)));
        let state = lock(&fakes.0);
        assert_eq!(1, state.begins);
        assert_eq!(0, state.commits);
        assert_eq!(0, state.target_reads);
        assert_eq!(0, state.token_reads);
    }

    #[tokio::test]
    async fn should_return_user_not_found_before_listing_tokens() {
        let fakes = Fakes::default();
        {
            let mut state = lock(&fakes.0);
            state.admin_role = Some(UserRole::Admin);
            state.target_exists = false;
        }

        let result = ListAdminAccessTokensHandler::new(
            fakes.clone(),
            fakes.clone(),
            fakes.clone(),
            fakes.clone(),
        )
        .execute(
            &context(Principal::User(UserId::new())),
            ListAdminAccessTokensRequest {
                user_id: UserId::new(),
                cursor: None,
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(ListAdminAccessTokensError::UserNotFound)
        ));
        let state = lock(&fakes.0);
        assert_eq!(0, state.commits);
        assert_eq!(0, state.token_reads);
    }

    #[tokio::test]
    async fn should_require_read_capability_for_delegated_admin() {
        let fakes = Fakes::default();
        let result =
            ListAdminAccessTokensHandler::new(fakes.clone(), fakes.clone(), fakes.clone(), fakes)
                .execute(
                    &context(Principal::DelegatedUser {
                        user_id: UserId::new(),
                        capabilities: BTreeSet::new(),
                    }),
                    ListAdminAccessTokensRequest {
                        user_id: UserId::new(),
                        cursor: None,
                    },
                )
                .await;

        assert!(matches!(result, Err(ListAdminAccessTokensError::Forbidden)));
    }
}
