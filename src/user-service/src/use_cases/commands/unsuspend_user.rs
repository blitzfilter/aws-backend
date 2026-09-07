use crate::ports::{
    UserAdminReadError, UserAdminReaderFactory, UserRepository, UserRepositoryError,
    UserRepositoryFactory,
};
use crate::use_cases::authorization::{
    RequireAdminActorError, require_admin_actor, require_admin_actor_credential,
};
use application::error::BoxError;
use application::operation_context::{CredentialCapability, OperationContext};
use application::transaction::{Transaction, UnitOfWork};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsuspendUserCommand {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsuspendUserResult {
    pub user_id: UserId,
    pub suspended: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum UnsuspendUserError {
    #[error("authenticated actor required to unsuspend user")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    UserNotFound,
    #[error("concurrent user update")]
    ConcurrencyConflict,
    #[error("user email already exists")]
    EmailConflict {
        #[source]
        source: BoxError,
    },
    #[error("user stripe customer already exists")]
    StripeCustomerConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary user persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted user state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal user persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin unsuspend user transaction")]
    BeginTransactionFailed,
    #[error("failed to commit unsuspend user transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UnsuspendUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UnsuspendUserCommand,
    ) -> Result<UnsuspendUserResult, UnsuspendUserError>;
}

pub struct UnsuspendUserHandler<U, R, A> {
    unit_of_work: U,
    users: R,
    admin_reader: A,
}

impl<U, R, A> UnsuspendUserHandler<U, R, A> {
    pub fn new(unit_of_work: U, users: R, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            users,
            admin_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> UnsuspendUserUseCase for UnsuspendUserHandler<U, R, A>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "unsuspend_user",
        skip_all,
        fields(
            user_id = %command.user_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UnsuspendUserCommand,
    ) -> Result<UnsuspendUserResult, UnsuspendUserError> {
        require_admin_actor_credential(context, CredentialCapability::UsersWrite)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UnsuspendUserError::BeginTransactionFailed)?;
        {
            let mut admin_reader =
                UserAdminReaderFactory::in_transaction(&self.admin_reader, &mut tx);
            require_admin_actor(context, &mut admin_reader).await?;
        }
        let domain_primitives::versioned::Versioned {
            value: mut user,
            version,
        } = self
            .users
            .in_transaction(&mut tx)
            .find_by_id(command.user_id)
            .await?
            .ok_or(UnsuspendUserError::UserNotFound)?;

        let outcome = user.unsuspend();
        if outcome.changed() {
            user = self
                .users
                .in_transaction(&mut tx)
                .update(&user, version)
                .await?
                .value;
        }

        tx.commit()
            .await
            .map_err(|_| UnsuspendUserError::CommitTransactionFailed)?;

        tracing::info!(
            event = "user.unsuspended",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %user.id(),
            changed = outcome.changed(),
            suspended = user.is_suspended(),
            outcome = "success",
        );

        Ok(UnsuspendUserResult {
            user_id: user.id(),
            suspended: user.is_suspended(),
        })
    }
}

impl From<RequireAdminActorError> for UnsuspendUserError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for UnsuspendUserError {
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

impl From<UserRepositoryError> for UnsuspendUserError {
    fn from(error: UserRepositoryError) -> Self {
        match error {
            UserRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            UserRepositoryError::EmailConflict { source } => Self::EmailConflict { source },
            UserRepositoryError::StripeCustomerConflict { source } => {
                Self::StripeCustomerConflict { source }
            }
            UserRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            UserRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        UnsuspendUserCommand, UnsuspendUserError, UnsuspendUserHandler, UnsuspendUserUseCase,
    };
    use crate::ports::{
        UserAdminActorView, UserAdminReadError, UserAdminReader, UserAdminReaderFactory,
        UserInsertOutcome, UserRepository, UserRepositoryError, UserRepositoryFactory,
        UserStorageVersion, VersionedUser,
    };
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use application::transaction::{Transaction, TransactionError, UnitOfWork};
    use domain_primitives::versioned::Versioned;
    use serde_email::Email;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::role::UserRole;
    use user_core::stripe_customer_id::StripeCustomerId;
    use user_core::tier::UserTier;
    use user_core::user::{NewUser, User, UserAccount, UserPreferences, UserProfile};
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct TxState {
        begins: usize,
        commits: usize,
    }

    #[derive(Clone, Default)]
    struct FakeUnitOfWork(Arc<Mutex<TxState>>);

    struct FakeTx(Arc<Mutex<TxState>>);

    #[derive(Default)]
    struct UserState {
        user: Option<VersionedUser>,
        updates: usize,
    }

    #[derive(Clone, Default)]
    struct FakeUserRepositoryFactory(Arc<Mutex<UserState>>);

    struct FakeUserRepository(Arc<Mutex<UserState>>);

    #[derive(Clone, Default)]
    struct FakeAdminReaderFactory(Arc<Mutex<Option<UserRole>>>);

    struct FakeAdminReader(Arc<Mutex<Option<UserRole>>>);

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

    fn user(user_id: UserId, role: UserRole, suspended: bool) -> VersionedUser {
        let mut user = User::create(NewUser {
            id: user_id,
            email: Email::try_from("user@example.com")
                .unwrap_or_else(|error| panic!("invalid test email: {error}")),
            profile: UserProfile::default(),
            preferences: UserPreferences::default(),
            account: UserAccount {
                tier: UserTier::Pro,
                role,
                stripe_customer_id: None,
            },
        })
        .unwrap_or_else(|error| panic!("invalid test user: {error}"));
        if suspended {
            let _ = user.suspend();
        }
        Versioned::new(user, UserStorageVersion::INITIAL)
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.0).commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(FakeTx(Arc::clone(&self.0)))
        }
    }

    #[async_trait::async_trait]
    impl UserRepository for FakeUserRepository {
        async fn find_by_id(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<VersionedUser>, UserRepositoryError> {
            Ok(lock(&self.0).user.clone())
        }

        async fn find_by_email(
            &mut self,
            _email: &Email,
        ) -> Result<Option<VersionedUser>, UserRepositoryError> {
            Ok(None)
        }

        async fn find_by_stripe_customer_id(
            &mut self,
            _stripe_customer_id: &StripeCustomerId,
        ) -> Result<Option<VersionedUser>, UserRepositoryError> {
            Ok(None)
        }

        async fn insert(&mut self, user: &User) -> Result<VersionedUser, UserRepositoryError> {
            Ok(Versioned::new(user.clone(), UserStorageVersion::INITIAL))
        }

        async fn insert_if_absent(
            &mut self,
            user: &User,
        ) -> Result<UserInsertOutcome, UserRepositoryError> {
            Ok(UserInsertOutcome::Created(Versioned::new(
                user.clone(),
                UserStorageVersion::INITIAL,
            )))
        }

        async fn update(
            &mut self,
            user: &User,
            _expected_version: UserStorageVersion,
        ) -> Result<VersionedUser, UserRepositoryError> {
            let mut state = lock(&self.0);
            state.updates += 1;
            let updated = Versioned::new(user.clone(), UserStorageVersion::INITIAL);
            state.user = Some(updated.clone());
            Ok(updated)
        }

        async fn delete_by_id(&mut self, _user_id: UserId) -> Result<bool, UserRepositoryError> {
            Ok(false)
        }
    }

    impl UserRepositoryFactory<FakeTx> for FakeUserRepositoryFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserRepository + 'tx {
            FakeUserRepository(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeAdminReader {
        async fn find_admin_actor(
            &mut self,
            user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            Ok((*lock(&self.0)).map(|role| UserAdminActorView { user_id, role }))
        }
    }

    impl UserAdminReaderFactory<FakeTx> for FakeAdminReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserAdminReader + 'tx {
            FakeAdminReader(Arc::clone(&self.0))
        }
    }

    #[tokio::test]
    async fn should_unsuspend_user_and_skip_repeat_persistence() {
        let user_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let users = FakeUserRepositoryFactory::default();
        lock(&users.0).user = Some(user(user_id, UserRole::User, true));
        let handler = UnsuspendUserHandler::new(
            unit_of_work.clone(),
            users.clone(),
            FakeAdminReaderFactory::default(),
        );

        let first = handler
            .execute(
                &context(Principal::System),
                UnsuspendUserCommand { user_id },
            )
            .await;
        assert!(matches!(first, Ok(result) if result.user_id == user_id && !result.suspended));
        assert_eq!(1, lock(&users.0).updates);

        let second = handler
            .execute(
                &context(Principal::System),
                UnsuspendUserCommand { user_id },
            )
            .await;
        assert!(matches!(second, Ok(result) if !result.suspended));
        assert_eq!(1, lock(&users.0).updates);
        assert_eq!(2, lock(&unit_of_work.0).commits);
    }

    #[tokio::test]
    async fn should_preserve_role_and_tier_when_unsuspending() {
        let user_id = UserId::new();
        let users = FakeUserRepositoryFactory::default();
        lock(&users.0).user = Some(user(user_id, UserRole::Admin, true));
        let handler = UnsuspendUserHandler::new(
            FakeUnitOfWork::default(),
            users.clone(),
            FakeAdminReaderFactory::default(),
        );

        let result = handler
            .execute(
                &context(Principal::System),
                UnsuspendUserCommand { user_id },
            )
            .await;

        assert!(matches!(result, Ok(result) if !result.suspended));
        let state = lock(&users.0);
        let user = state.user.as_ref().map(|user| &user.value);
        assert!(
            matches!(user, Some(user) if user.account().role == UserRole::Admin && user.account().tier == UserTier::Pro)
        );
    }

    #[tokio::test]
    async fn should_reject_non_admin_anonymous_and_missing_targets() {
        let user_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let users = FakeUserRepositoryFactory::default();
        lock(&users.0).user = Some(user(user_id, UserRole::User, true));
        let admins = FakeAdminReaderFactory::default();
        *lock(&admins.0) = Some(UserRole::User);
        let handler = UnsuspendUserHandler::new(unit_of_work.clone(), users, admins);

        let non_admin = handler
            .execute(
                &context(Principal::User(UserId::new())),
                UnsuspendUserCommand { user_id },
            )
            .await;
        assert!(matches!(non_admin, Err(UnsuspendUserError::Forbidden)));

        let anonymous = handler
            .execute(
                &context(Principal::Anonymous),
                UnsuspendUserCommand { user_id },
            )
            .await;
        assert!(matches!(
            anonymous,
            Err(UnsuspendUserError::AuthenticatedActorRequired)
        ));

        let missing = UnsuspendUserHandler::new(
            FakeUnitOfWork::default(),
            FakeUserRepositoryFactory::default(),
            FakeAdminReaderFactory::default(),
        )
        .execute(
            &context(Principal::System),
            UnsuspendUserCommand {
                user_id: UserId::new(),
            },
        )
        .await;
        assert!(matches!(missing, Err(UnsuspendUserError::UserNotFound)));
        assert_eq!(1, lock(&unit_of_work.0).begins);
        assert_eq!(0, lock(&unit_of_work.0).commits);
    }

    #[tokio::test]
    async fn should_reject_delegated_actor_without_users_write_capability() {
        let unit_of_work = FakeUnitOfWork::default();
        let handler = UnsuspendUserHandler::new(
            unit_of_work.clone(),
            FakeUserRepositoryFactory::default(),
            FakeAdminReaderFactory::default(),
        );

        let result = handler
            .execute(
                &context(Principal::DelegatedUser {
                    user_id: UserId::new(),
                    capabilities: BTreeSet::new(),
                }),
                UnsuspendUserCommand {
                    user_id: UserId::new(),
                },
            )
            .await;

        assert!(matches!(result, Err(UnsuspendUserError::Forbidden)));
        assert_eq!(0, lock(&unit_of_work.0).begins);
    }
}
