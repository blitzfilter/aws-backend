use crate::ports::{
    AccessTokenRepository, AccessTokenRepositoryError, AccessTokenRepositoryFactory,
    UserAccountReadError, UserAccountReader, UserAccountReaderFactory, UserAdminReadError,
    UserAdminReaderFactory,
};
use crate::use_cases::authorization::{
    RequireAdminActorError, require_admin_actor, require_admin_actor_credential,
};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteAccessTokensCommand {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteAccessTokensResult {
    pub user_id: UserId,
    pub deleted_count: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteAccessTokensError {
    #[error("authenticated actor required to delete access tokens")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    UserNotFound,
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
    #[error("failed to begin delete access tokens transaction")]
    BeginTransactionFailed,
    #[error("failed to commit delete access tokens transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait DeleteAccessTokensUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteAccessTokensCommand,
    ) -> Result<DeleteAccessTokensResult, DeleteAccessTokensError>;
}

pub struct DeleteAccessTokensHandler<U, R, A, V> {
    unit_of_work: U,
    repository: R,
    admin_reader: A,
    user_reader: V,
}

impl<U, R, A, V> DeleteAccessTokensHandler<U, R, A, V> {
    pub fn new(unit_of_work: U, repository: R, admin_reader: A, user_reader: V) -> Self {
        Self {
            unit_of_work,
            repository,
            admin_reader,
            user_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A, V> DeleteAccessTokensUseCase for DeleteAccessTokensHandler<U, R, A, V>
where
    U: UnitOfWork,
    R: AccessTokenRepositoryFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
    V: UserAccountReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "delete_access_tokens",
        skip_all,
        fields(
            target_user_id = %command.user_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            deleted_count = tracing::field::Empty,
            outcome = tracing::field::Empty,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteAccessTokensCommand,
    ) -> Result<DeleteAccessTokensResult, DeleteAccessTokensError> {
        let actor_id = context.principal.actor_id();
        if let Some(actor_id) = actor_id.as_deref() {
            tracing::Span::current().record("actor_id", actor_id);
        }

        let result = async {
            require_admin_actor_credential(context, CredentialCapability::AccessTokensWrite)?;

            let mut tx = self
                .unit_of_work
                .begin()
                .await
                .map_err(|_| DeleteAccessTokensError::BeginTransactionFailed)?;
            {
                let mut admin_reader = self.admin_reader.in_transaction(&mut tx);
                require_admin_actor(context, &mut admin_reader).await?;
            }

            let target_exists = self
                .user_reader
                .in_transaction(&mut tx)
                .find_by_id(command.user_id)
                .await?
                .is_some();
            if !target_exists {
                return Err(DeleteAccessTokensError::UserNotFound);
            }

            let deleted_count = self
                .repository
                .in_transaction(&mut tx)
                .delete_by_user_id(command.user_id)
                .await?;
            tx.commit()
                .await
                .map_err(|_| DeleteAccessTokensError::CommitTransactionFailed)?;

            Ok(DeleteAccessTokensResult {
                user_id: command.user_id,
                deleted_count,
            })
        }
        .await;

        match result {
            Ok(result) => {
                tracing::Span::current().record("deleted_count", result.deleted_count);
                tracing::Span::current().record("outcome", "success");
                tracing::info!(
                    event = "access_tokens.deleted",
                    action = "delete_access_tokens",
                    actor_type = context.principal.kind(),
                    actor_id = actor_id.as_deref().unwrap_or(""),
                    target_type = "user_access_tokens",
                    target_user_id = %result.user_id,
                    deleted_count = result.deleted_count,
                    request_id = %context.request_id,
                    correlation_id = %context.correlation_id,
                    outcome = "success",
                );
                Ok(result)
            }
            Err(error) => {
                tracing::Span::current().record("outcome", "failure");
                tracing::warn!(
                    event = "access_tokens.deleted",
                    action = "delete_access_tokens",
                    actor_type = context.principal.kind(),
                    actor_id = actor_id.as_deref().unwrap_or(""),
                    target_type = "user_access_tokens",
                    target_user_id = %command.user_id,
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

impl From<OperationAuthorizationError> for DeleteAccessTokensError {
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

impl From<RequireAdminActorError> for DeleteAccessTokensError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for DeleteAccessTokensError {
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

impl From<UserAccountReadError> for DeleteAccessTokensError {
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

impl From<AccessTokenRepositoryError> for DeleteAccessTokensError {
    fn from(error: AccessTokenRepositoryError) -> Self {
        match error {
            AccessTokenRepositoryError::ConcurrencyConflict => Self::Internal {
                source: box_error(std::io::Error::other(
                    "unexpected access token concurrency conflict during bulk deletion",
                )),
            },
            AccessTokenRepositoryError::Conflict { source }
            | AccessTokenRepositoryError::Internal { source } => Self::Internal { source },
            AccessTokenRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AccessTokenRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DeleteAccessTokensCommand, DeleteAccessTokensError, DeleteAccessTokensHandler,
        DeleteAccessTokensResult, DeleteAccessTokensUseCase,
    };
    use crate::ports::{
        AccessTokenRepository, AccessTokenRepositoryError, AccessTokenRepositoryFactory,
        AccessTokenStorageVersion, UserAccountReadError, UserAccountReader,
        UserAccountReaderFactory, UserAdminActorView, UserAdminReadError, UserAdminReader,
        UserAdminReaderFactory, UserDetailsView, VersionedAccessToken,
    };
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use application::transaction::{Transaction, TransactionError, UnitOfWork};
    use localization::Language;
    use money::Currency;
    use serde_email::Email;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::access_token::{AccessToken, AccessTokenId, HashedRawAccessToken};
    use user_core::measurement_unit::MeasurementUnit;
    use user_core::role::UserRole;
    use user_core::stripe_customer_id::StripeCustomerId;
    use user_core::tier::UserTier;
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        delete_calls: usize,
        deleted_count: u64,
        deleted_user_ids: Vec<UserId>,
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

    #[derive(Clone, Copy)]
    struct FakeUserReaderFactory {
        exists: bool,
    }

    struct FakeUserReader {
        exists: bool,
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
            Err(unused_repository_error())
        }

        async fn update(
            &mut self,
            _token: &AccessToken,
            _expected_version: AccessTokenStorageVersion,
        ) -> Result<VersionedAccessToken, AccessTokenRepositoryError> {
            Err(unused_repository_error())
        }

        async fn delete_by_id(
            &mut self,
            _user_id: UserId,
            _access_token_id: AccessTokenId,
        ) -> Result<bool, AccessTokenRepositoryError> {
            Ok(false)
        }

        async fn delete_by_user_id(
            &mut self,
            user_id: UserId,
        ) -> Result<u64, AccessTokenRepositoryError> {
            let mut state = lock(&self.0.0);
            state.delete_calls += 1;
            state.deleted_user_ids.push(user_id);
            Ok(state.deleted_count)
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
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            Ok(self.role.map(|role| UserAdminActorView { user_id, role }))
        }
    }

    impl UserAdminReaderFactory<FakeTx> for FakeAdminReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserAdminReader + 'tx {
            FakeAdminReader { role: self.role }
        }
    }

    #[async_trait::async_trait]
    impl UserAccountReader for FakeUserReader {
        async fn find_by_id(
            &mut self,
            user_id: UserId,
        ) -> Result<Option<UserDetailsView>, UserAccountReadError> {
            Ok(self.exists.then(|| UserDetailsView {
                user_id,
                email: match Email::try_from("target@example.test") {
                    Ok(email) => email,
                    Err(error) => panic!("invalid test email: {error}"),
                },
                first_name: None,
                last_name: None,
                language: Some(Language::En),
                currency: Some(Currency::Eur),
                measurement_unit: Some(MeasurementUnit::Metric),
                show_unassessed_or_sensitive_content: false,
                tier: UserTier::Free,
                role: UserRole::User,
                stripe_customer_id: None::<StripeCustomerId>,
            }))
        }
    }

    impl UserAccountReaderFactory<FakeTx> for FakeUserReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserAccountReader + 'tx {
            FakeUserReader {
                exists: self.exists,
            }
        }
    }

    fn unused_repository_error() -> AccessTokenRepositoryError {
        AccessTokenRepositoryError::Internal {
            source: application::error::box_error(std::io::Error::other("not used")),
        }
    }

    fn handler(
        fakes: &Fakes,
        role: Option<UserRole>,
        exists: bool,
    ) -> impl DeleteAccessTokensUseCase {
        DeleteAccessTokensHandler::new(
            fakes.clone(),
            fakes.clone(),
            FakeAdminReaderFactory { role },
            FakeUserReaderFactory { exists },
        )
    }

    #[tokio::test]
    async fn should_delete_all_tokens_for_existing_user_and_commit() {
        let actor_id = UserId::new();
        let target_id = UserId::new();
        let fakes = Fakes::default();
        lock(&fakes.0).deleted_count = 3;

        let result = handler(&fakes, Some(UserRole::Admin), true)
            .execute(
                &context(Principal::User(actor_id)),
                DeleteAccessTokensCommand { user_id: target_id },
            )
            .await;

        assert!(matches!(
            result,
            Ok(result) if result.user_id == target_id && result.deleted_count == 3
        ));
        let state = lock(&fakes.0);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.delete_calls);
        assert_eq!(1, state.commits);
        assert_eq!(vec![target_id], state.deleted_user_ids);
    }

    #[tokio::test]
    async fn should_commit_when_existing_user_has_no_tokens() {
        let fakes = Fakes::default();
        let result = handler(&fakes, Some(UserRole::Admin), true)
            .execute(
                &context(Principal::User(UserId::new())),
                DeleteAccessTokensCommand {
                    user_id: UserId::new(),
                },
            )
            .await;

        assert!(matches!(
            result,
            Ok(DeleteAccessTokensResult {
                deleted_count: 0,
                ..
            })
        ));
        assert_eq!(1, lock(&fakes.0).commits);
    }

    #[tokio::test]
    async fn should_return_not_found_without_deleting_when_target_user_is_missing() {
        let fakes = Fakes::default();
        let result = handler(&fakes, Some(UserRole::Admin), false)
            .execute(
                &context(Principal::User(UserId::new())),
                DeleteAccessTokensCommand {
                    user_id: UserId::new(),
                },
            )
            .await;

        assert!(matches!(result, Err(DeleteAccessTokensError::UserNotFound)));
        let state = lock(&fakes.0);
        assert_eq!(0, state.delete_calls);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_reject_non_admin_before_deleting() {
        let fakes = Fakes::default();
        let result = handler(&fakes, Some(UserRole::User), true)
            .execute(
                &context(Principal::User(UserId::new())),
                DeleteAccessTokensCommand {
                    user_id: UserId::new(),
                },
            )
            .await;

        assert!(matches!(result, Err(DeleteAccessTokensError::Forbidden)));
        assert_eq!(0, lock(&fakes.0).delete_calls);
    }

    #[tokio::test]
    async fn should_reject_anonymous_before_starting_transaction() {
        let fakes = Fakes::default();
        let result = handler(&fakes, None, true)
            .execute(
                &context(Principal::Anonymous),
                DeleteAccessTokensCommand {
                    user_id: UserId::new(),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(DeleteAccessTokensError::AuthenticatedActorRequired)
        ));
        assert_eq!(0, lock(&fakes.0).begins);
    }
}
