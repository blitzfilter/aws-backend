use crate::ports::{
    AccessTokenRepository, AccessTokenRepositoryError, AccessTokenRepositoryFactory,
    UserAdminReadError, UserAdminReaderFactory,
};
use crate::use_cases::authorization::{
    RequireAdminActorError, require_admin_actor, require_admin_actor_credential,
};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use user_core::access_token::AccessTokenId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteAccessTokenCommand {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteAccessTokenResult {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteAccessTokenError {
    #[error("authenticated actor required to delete access token")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("access token already exists")]
    Conflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary access token store failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted access token state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal access token persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin delete access token transaction")]
    BeginTransactionFailed,
    #[error("failed to commit delete access token transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait DeleteAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteAccessTokenCommand,
    ) -> Result<DeleteAccessTokenResult, DeleteAccessTokenError>;
}

pub struct DeleteAccessTokenHandler<U, R, A> {
    unit_of_work: U,
    repository: R,
    admin_reader: A,
    admin_only: bool,
}

impl<U, R, A> DeleteAccessTokenHandler<U, R, A> {
    pub fn new(unit_of_work: U, repository: R, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            repository,
            admin_reader,
            admin_only: false,
        }
    }

    pub fn new_admin_only(unit_of_work: U, repository: R, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            repository,
            admin_reader,
            admin_only: true,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> DeleteAccessTokenUseCase for DeleteAccessTokenHandler<U, R, A>
where
    U: UnitOfWork,
    R: AccessTokenRepositoryFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "delete_access_token",
        skip_all,
        fields(
            target_user_id = %command.user_id,
            access_token_id = %command.access_token_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            deletion_outcome = tracing::field::Empty,
            outcome = tracing::field::Empty,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteAccessTokenCommand,
    ) -> Result<DeleteAccessTokenResult, DeleteAccessTokenError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let result = async {
            if self.admin_only {
                require_admin_actor_credential(context, CredentialCapability::AccessTokensWrite)?;
            } else {
                authorize_access_token_write(context, command.user_id)?;
            }

            let mut tx = self
                .unit_of_work
                .begin()
                .await
                .map_err(|_| DeleteAccessTokenError::BeginTransactionFailed)?;
            if self.admin_only {
                let mut admin_reader = self.admin_reader.in_transaction(&mut tx);
                require_admin_actor(context, &mut admin_reader).await?;
            }
            let deleted = self
                .repository
                .in_transaction(&mut tx)
                .delete_by_id(command.user_id, command.access_token_id)
                .await?;
            tx.commit()
                .await
                .map_err(|_| DeleteAccessTokenError::CommitTransactionFailed)?;

            Ok((
                DeleteAccessTokenResult {
                    user_id: command.user_id,
                    access_token_id: command.access_token_id,
                },
                deleted,
            ))
        }
        .await;

        let actor_id = context.principal.actor_id();
        match result {
            Ok((result, deleted)) => {
                let deletion_outcome = if deleted { "deleted" } else { "already_absent" };
                tracing::Span::current().record("deletion_outcome", deletion_outcome);
                tracing::Span::current().record("outcome", "success");
                tracing::info!(
                    event = "access_token.deleted",
                    action = "delete_access_token",
                    actor_type = context.principal.kind(),
                    actor_id = actor_id.as_deref().unwrap_or(""),
                    target_type = "user_access_token",
                    target_user_id = %command.user_id,
                    access_token_id = %command.access_token_id,
                    deletion_outcome,
                    request_id = %context.request_id,
                    correlation_id = %context.correlation_id,
                    outcome = "success",
                );
                Ok(result)
            }
            Err(error) => {
                tracing::Span::current().record("deletion_outcome", "unknown");
                tracing::Span::current().record("outcome", "failure");
                tracing::warn!(
                    event = "access_token.deleted",
                    action = "delete_access_token",
                    actor_type = context.principal.kind(),
                    actor_id = actor_id.as_deref().unwrap_or(""),
                    target_type = "user_access_token",
                    target_user_id = %command.user_id,
                    access_token_id = %command.access_token_id,
                    request_id = %context.request_id,
                    correlation_id = %context.correlation_id,
                    error_category = %error,
                    outcome = "failure",
                );
                Err(error)
            }
        }
    }
}

fn authorize_access_token_write(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), DeleteAccessTokenError> {
    context
        .require()
        .credential_capability(CredentialCapability::AccessTokensWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<DeleteAccessTokenError>()
}

impl From<OperationAuthorizationError> for DeleteAccessTokenError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<RequireAdminActorError> for DeleteAccessTokenError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for DeleteAccessTokenError {
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

impl From<AccessTokenRepositoryError> for DeleteAccessTokenError {
    fn from(error: AccessTokenRepositoryError) -> Self {
        match error {
            AccessTokenRepositoryError::ConcurrencyConflict => Self::Internal {
                source: application::error::box_error(std::io::Error::other(
                    "unexpected access token concurrency conflict during deletion",
                )),
            },
            AccessTokenRepositoryError::Conflict { source } => Self::Conflict { source },
            AccessTokenRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AccessTokenRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            AccessTokenRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeleteAccessTokenCommand, DeleteAccessTokenError, DeleteAccessTokenHandler,
        DeleteAccessTokenUseCase,
    };
    use crate::ports::{
        AccessTokenRepository, AccessTokenRepositoryError, AccessTokenRepositoryFactory,
        AccessTokenStorageVersion, UserAdminActorView, UserAdminReader, UserAdminReaderFactory,
        VersionedAccessToken,
    };
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use application::transaction::{Transaction, TransactionError, UnitOfWork};
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::access_token::{AccessToken, AccessTokenId, HashedRawAccessToken};
    use user_core::role::UserRole;
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        delete_calls: usize,
    }

    #[derive(Clone, Default)]
    struct Fakes(Arc<Mutex<State>>);
    struct FakeTx(Fakes);
    struct FakeRepository(Fakes);

    #[derive(Clone, Copy)]
    struct FakeAdminReaderFactory {
        role: Option<UserRole>,
    }

    struct FakeAdminReader {
        role: Option<UserRole>,
    }

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

    #[async_trait::async_trait]
    impl AccessTokenRepository for FakeRepository {
        async fn find_by_id(
            &mut self,
            _user_id: UserId,
            _access_token_id: AccessTokenId,
        ) -> Result<Option<VersionedAccessToken>, AccessTokenRepositoryError> {
            Ok(None)
        }

        async fn find_by_hashed_token(
            &mut self,
            _hashed_token: &HashedRawAccessToken,
        ) -> Result<Option<VersionedAccessToken>, AccessTokenRepositoryError> {
            Ok(None)
        }

        async fn insert(
            &mut self,
            _token: &AccessToken,
        ) -> Result<VersionedAccessToken, AccessTokenRepositoryError> {
            Err(AccessTokenRepositoryError::Internal {
                source: application::error::box_error(std::io::Error::other("not used")),
            })
        }

        async fn update(
            &mut self,
            _token: &AccessToken,
            _expected_version: AccessTokenStorageVersion,
        ) -> Result<VersionedAccessToken, AccessTokenRepositoryError> {
            Err(AccessTokenRepositoryError::Internal {
                source: application::error::box_error(std::io::Error::other("not used")),
            })
        }

        async fn delete_by_id(
            &mut self,
            _user_id: UserId,
            _access_token_id: AccessTokenId,
        ) -> Result<bool, AccessTokenRepositoryError> {
            lock(&self.0.0).delete_calls += 1;
            Ok(true)
        }
    }

    impl AccessTokenRepositoryFactory<FakeTx> for Fakes {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl AccessTokenRepository + 'tx {
            FakeRepository(self.clone())
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeAdminReader {
        async fn find_admin_actor(
            &mut self,
            user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, crate::ports::UserAdminReadError> {
            Ok(self.role.map(|role| UserAdminActorView { user_id, role }))
        }
    }

    impl UserAdminReaderFactory<FakeTx> for FakeAdminReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserAdminReader + 'tx {
            FakeAdminReader { role: self.role }
        }
    }

    #[tokio::test]
    async fn should_delete_access_token_in_committed_transaction() {
        let user_id = UserId::new();
        let fakes = Fakes::default();
        let result = DeleteAccessTokenHandler::new(
            fakes.clone(),
            fakes.clone(),
            FakeAdminReaderFactory { role: None },
        )
        .execute(
            &context(Principal::User(user_id)),
            DeleteAccessTokenCommand {
                user_id,
                access_token_id: AccessTokenId::new(),
            },
        )
        .await;

        assert!(result.is_ok());
        let state = lock(&fakes.0);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.delete_calls);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_reject_anonymous_delete_before_starting_transaction() {
        let fakes = Fakes::default();
        let result = DeleteAccessTokenHandler::new(
            fakes.clone(),
            fakes.clone(),
            FakeAdminReaderFactory { role: None },
        )
        .execute(
            &context(Principal::Anonymous),
            DeleteAccessTokenCommand {
                user_id: UserId::new(),
                access_token_id: AccessTokenId::new(),
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(DeleteAccessTokenError::AuthenticatedActorRequired)
        ));
        assert_eq!(0, lock(&fakes.0).begins);
    }

    #[tokio::test]
    async fn should_allow_admin_to_delete_another_users_access_token() {
        let admin_id = UserId::new();
        let target_id = UserId::new();
        let fakes = Fakes::default();
        let handler = DeleteAccessTokenHandler::new_admin_only(
            fakes.clone(),
            fakes.clone(),
            FakeAdminReaderFactory {
                role: Some(UserRole::Admin),
            },
        );

        let result = handler
            .execute(
                &context(Principal::User(admin_id)),
                DeleteAccessTokenCommand {
                    user_id: target_id,
                    access_token_id: AccessTokenId::new(),
                },
            )
            .await;

        assert!(result.is_ok());
        assert_eq!(1, lock(&fakes.0).delete_calls);
    }

    #[tokio::test]
    async fn should_reject_non_admin_admin_delete_before_deleting() {
        let actor_id = UserId::new();
        let fakes = Fakes::default();
        let handler = DeleteAccessTokenHandler::new_admin_only(
            fakes.clone(),
            fakes.clone(),
            FakeAdminReaderFactory {
                role: Some(UserRole::User),
            },
        );

        let result = handler
            .execute(
                &context(Principal::User(actor_id)),
                DeleteAccessTokenCommand {
                    user_id: UserId::new(),
                    access_token_id: AccessTokenId::new(),
                },
            )
            .await;

        assert!(matches!(result, Err(DeleteAccessTokenError::Forbidden)));
        let state = lock(&fakes.0);
        assert_eq!(0, state.delete_calls);
        assert_eq!(1, state.begins);
        assert_eq!(0, state.commits);
    }
}
