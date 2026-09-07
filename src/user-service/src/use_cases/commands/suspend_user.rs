use crate::ports::{
    UserAdminMutationGuard, UserAdminMutationGuardFactory, UserAdminReadError,
    UserAdminReaderFactory, UserAdminRemovalDecision, UserRepository, UserRepositoryError,
    UserRepositoryFactory,
};
use crate::use_cases::authorization::{
    RequireAdminActorError, require_admin_actor, require_admin_actor_credential,
};
use application::error::BoxError;
use application::operation_context::{CredentialCapability, OperationContext};
use application::transaction::{Transaction, UnitOfWork};
use user_core::role::UserRole;
use user_core::user_id::UserId;

const MAX_ADMINISTRATIVE_REASON_BYTES: usize = 1_000;
const SECRET_MARKERS: [&str; 8] = [
    "authorization:",
    "bearer ",
    "password",
    "secret",
    "api key",
    "access token",
    "token",
    "credential",
];

#[derive(Debug, Clone, PartialEq)]
pub struct SuspendUserCommand {
    pub user_id: UserId,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuspendUserResult {
    pub user_id: UserId,
    pub suspended: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SuspendUserError {
    #[error(
        "administrative reason must be nonempty and at most {MAX_ADMINISTRATIVE_REASON_BYTES} bytes"
    )]
    InvalidReason,
    #[error("authenticated actor required to suspend user")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    UserNotFound,
    #[error("cannot suspend the last active administrator")]
    LastAdminProtected,
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
    #[error("failed to begin suspend user transaction")]
    BeginTransactionFailed,
    #[error("failed to commit suspend user transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait SuspendUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: SuspendUserCommand,
    ) -> Result<SuspendUserResult, SuspendUserError>;
}

pub struct SuspendUserHandler<U, R, A> {
    unit_of_work: U,
    users: R,
    admin_reader: A,
}

impl<U, R, A> SuspendUserHandler<U, R, A> {
    pub fn new(unit_of_work: U, users: R, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            users,
            admin_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> SuspendUserUseCase for SuspendUserHandler<U, R, A>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx> + UserAdminMutationGuardFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "suspend_user",
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
        command: SuspendUserCommand,
    ) -> Result<SuspendUserResult, SuspendUserError> {
        let reason = validate_reason(&command.reason)?;
        require_admin_actor_credential(context, CredentialCapability::UsersWrite)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SuspendUserError::BeginTransactionFailed)?;
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
            .ok_or(SuspendUserError::UserNotFound)?;

        let role = user.account().role;
        let outcome = user.suspend();
        if outcome.changed() {
            match UserAdminMutationGuardFactory::in_transaction(&self.admin_reader, &mut tx)
                .check_removal(command.user_id)
                .await?
            {
                UserAdminRemovalDecision::TargetNotFound => {
                    return Err(SuspendUserError::UserNotFound);
                }
                UserAdminRemovalDecision::TargetNotAdmin if role == UserRole::Admin => {
                    return Err(SuspendUserError::ConcurrencyConflict);
                }
                UserAdminRemovalDecision::LastAdmin if role == UserRole::Admin => {
                    return Err(SuspendUserError::LastAdminProtected);
                }
                UserAdminRemovalDecision::TargetNotAdmin
                | UserAdminRemovalDecision::Allowed
                | UserAdminRemovalDecision::LastAdmin => {}
            }
            user = self
                .users
                .in_transaction(&mut tx)
                .update(&user, version)
                .await?
                .value;
        }

        tx.commit()
            .await
            .map_err(|_| SuspendUserError::CommitTransactionFailed)?;

        tracing::info!(
            event = "user.suspended",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %user.id(),
            reason = %reason,
            changed = outcome.changed(),
            suspended = user.is_suspended(),
            outcome = "success",
        );

        Ok(SuspendUserResult {
            user_id: user.id(),
            suspended: user.is_suspended(),
        })
    }
}

fn validate_reason(reason: &str) -> Result<&str, SuspendUserError> {
    let reason = reason.trim();
    if reason.is_empty() || reason.len() > MAX_ADMINISTRATIVE_REASON_BYTES {
        return Err(SuspendUserError::InvalidReason);
    }

    let normalized_reason = reason.to_ascii_lowercase();
    if SECRET_MARKERS
        .iter()
        .any(|marker| normalized_reason.contains(marker))
    {
        return Err(SuspendUserError::InvalidReason);
    }

    Ok(reason)
}

impl From<RequireAdminActorError> for SuspendUserError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for SuspendUserError {
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

impl From<UserRepositoryError> for SuspendUserError {
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
        SuspendUserCommand, SuspendUserError, SuspendUserHandler, SuspendUserUseCase,
        validate_reason,
    };
    use crate::ports::{
        UserAdminActorView, UserAdminMutationGuard, UserAdminMutationGuardFactory,
        UserAdminReadError, UserAdminReader, UserAdminReaderFactory, UserAdminRemovalDecision,
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

    #[derive(Default)]
    struct AdminState {
        actor_role: Option<UserRole>,
        removal_decision: UserAdminRemovalDecision,
    }

    #[derive(Clone, Default)]
    struct FakeAdminReaderFactory(Arc<Mutex<AdminState>>);

    struct FakeAdminReader(Arc<Mutex<AdminState>>);

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

    fn email(value: &str) -> Email {
        match Email::try_from(value) {
            Ok(email) => email,
            Err(error) => panic!("invalid test email: {error}"),
        }
    }

    fn user(user_id: UserId, role: UserRole) -> VersionedUser {
        let user = User::create(NewUser {
            id: user_id,
            email: email("user@example.com"),
            profile: UserProfile::default(),
            preferences: UserPreferences::default(),
            account: UserAccount {
                tier: UserTier::Pro,
                role,
                stripe_customer_id: None,
            },
        })
        .unwrap_or_else(|error| panic!("invalid test user: {error}"));
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
            Ok(lock(&self.0)
                .actor_role
                .map(|role| UserAdminActorView { user_id, role }))
        }
    }

    #[async_trait::async_trait]
    impl UserAdminMutationGuard for FakeAdminReader {
        async fn check_removal(
            &mut self,
            _user_id: UserId,
        ) -> Result<UserAdminRemovalDecision, UserAdminReadError> {
            Ok(lock(&self.0).removal_decision)
        }
    }

    impl UserAdminReaderFactory<FakeTx> for FakeAdminReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserAdminReader + 'tx {
            FakeAdminReader(Arc::clone(&self.0))
        }
    }

    impl UserAdminMutationGuardFactory<FakeTx> for FakeAdminReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl UserAdminMutationGuard + 'tx {
            FakeAdminReader(Arc::clone(&self.0))
        }
    }

    #[tokio::test]
    async fn should_suspend_user_and_skip_repeat_persistence() {
        let user_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let users = FakeUserRepositoryFactory::default();
        lock(&users.0).user = Some(user(user_id, UserRole::User));
        let handler = SuspendUserHandler::new(
            unit_of_work.clone(),
            users.clone(),
            FakeAdminReaderFactory::default(),
        );

        let first = handler
            .execute(
                &context(Principal::System),
                SuspendUserCommand {
                    user_id,
                    reason: "  repeated policy violation  ".to_owned(),
                },
            )
            .await;
        assert!(matches!(first, Ok(result) if result.user_id == user_id && result.suspended));
        assert_eq!(1, lock(&users.0).updates);

        let second = handler
            .execute(
                &context(Principal::System),
                SuspendUserCommand {
                    user_id,
                    reason: "repeat request".to_owned(),
                },
            )
            .await;
        assert!(matches!(second, Ok(result) if result.suspended));
        assert_eq!(1, lock(&users.0).updates);
        assert_eq!(2, lock(&unit_of_work.0).commits);
    }

    #[tokio::test]
    async fn should_reject_non_admin_and_anonymous_actors() {
        let user_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let users = FakeUserRepositoryFactory::default();
        lock(&users.0).user = Some(user(user_id, UserRole::User));
        let admins = FakeAdminReaderFactory::default();
        lock(&admins.0).actor_role = Some(UserRole::User);
        let handler = SuspendUserHandler::new(unit_of_work.clone(), users, admins);

        let non_admin = handler
            .execute(
                &context(Principal::User(UserId::new())),
                SuspendUserCommand {
                    user_id,
                    reason: "policy violation".to_owned(),
                },
            )
            .await;
        assert!(matches!(non_admin, Err(SuspendUserError::Forbidden)));

        let anonymous = handler
            .execute(
                &context(Principal::Anonymous),
                SuspendUserCommand {
                    user_id,
                    reason: "policy violation".to_owned(),
                },
            )
            .await;
        assert!(matches!(
            anonymous,
            Err(SuspendUserError::AuthenticatedActorRequired)
        ));

        let missing_credential = handler
            .execute(
                &context(Principal::DelegatedUser {
                    user_id: UserId::new(),
                    capabilities: BTreeSet::new(),
                }),
                SuspendUserCommand {
                    user_id,
                    reason: "policy violation".to_owned(),
                },
            )
            .await;
        assert!(matches!(
            missing_credential,
            Err(SuspendUserError::Forbidden)
        ));
        assert_eq!(1, lock(&unit_of_work.0).begins);
        assert_eq!(0, lock(&unit_of_work.0).commits);
    }

    #[tokio::test]
    async fn should_reject_missing_user_and_last_active_admin() {
        let missing_id = UserId::new();
        let missing = SuspendUserHandler::new(
            FakeUnitOfWork::default(),
            FakeUserRepositoryFactory::default(),
            FakeAdminReaderFactory::default(),
        )
        .execute(
            &context(Principal::System),
            SuspendUserCommand {
                user_id: missing_id,
                reason: "policy violation".to_owned(),
            },
        )
        .await;
        assert!(matches!(missing, Err(SuspendUserError::UserNotFound)));

        let user_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let users = FakeUserRepositoryFactory::default();
        lock(&users.0).user = Some(user(user_id, UserRole::Admin));
        let admins = FakeAdminReaderFactory::default();
        lock(&admins.0).removal_decision = UserAdminRemovalDecision::LastAdmin;
        let last_admin = SuspendUserHandler::new(unit_of_work.clone(), users.clone(), admins)
            .execute(
                &context(Principal::System),
                SuspendUserCommand {
                    user_id,
                    reason: "policy violation".to_owned(),
                },
            )
            .await;
        assert!(matches!(
            last_admin,
            Err(SuspendUserError::LastAdminProtected)
        ));
        assert_eq!(0, lock(&users.0).updates);
        assert_eq!(0, lock(&unit_of_work.0).commits);
    }

    #[test]
    fn should_reject_common_secret_markers_in_reason_case_insensitively() {
        for reason in [
            "Authorization: Bearer value",
            "BEARER value",
            "Password value",
            "Secret value",
            "API Key value",
            "Access Token value",
            "OAuth token value",
            "Credential value",
        ] {
            assert!(matches!(
                validate_reason(reason),
                Err(SuspendUserError::InvalidReason)
            ));
        }
    }

    #[tokio::test]
    async fn should_reject_invalid_reason_before_transaction() {
        let unit_of_work = FakeUnitOfWork::default();
        let handler = SuspendUserHandler::new(
            unit_of_work.clone(),
            FakeUserRepositoryFactory::default(),
            FakeAdminReaderFactory::default(),
        );

        let whitespace = handler
            .execute(
                &context(Principal::System),
                SuspendUserCommand {
                    user_id: UserId::new(),
                    reason: " \n\t ".to_owned(),
                },
            )
            .await;
        assert!(matches!(whitespace, Err(SuspendUserError::InvalidReason)));

        let too_long = handler
            .execute(
                &context(Principal::System),
                SuspendUserCommand {
                    user_id: UserId::new(),
                    reason: "a".repeat(1_001),
                },
            )
            .await;
        assert!(matches!(too_long, Err(SuspendUserError::InvalidReason)));

        let secret_marker = handler
            .execute(
                &context(Principal::System),
                SuspendUserCommand {
                    user_id: UserId::new(),
                    reason: "Included bearer credential for review".to_owned(),
                },
            )
            .await;
        assert!(matches!(
            secret_marker,
            Err(SuspendUserError::InvalidReason)
        ));
        assert_eq!(0, lock(&unit_of_work.0).begins);
    }
}
