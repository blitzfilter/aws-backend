use crate::ports::{
    CognitoIdentity, UserCognitoIdentityRegistry, UserCognitoIdentityRegistryError,
    UserCognitoIdentityRegistryFactory, UserRepository, UserRepositoryError, UserRepositoryFactory,
};
use application::error::{BoxError, static_error};
use application::operation_context::{OperationContext, Principal};
use application::transaction::{Transaction, UnitOfWork};
use serde_email::Email;
use user_core::user::{NewUser, User, UserAccount, UserPreferences, UserProfile};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct RegisterCognitoUserCommand {
    pub identity: CognitoIdentity,
    pub email: Email,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegisterCognitoUserResult {
    pub user_id: UserId,
    pub email: Email,
}

#[derive(Debug, thiserror::Error)]
pub enum RegisterCognitoUserError {
    #[error("system actor required to register Cognito user")]
    Forbidden,
    #[error("Cognito identity already exists with a different email")]
    IdentityConflict,
    #[error("user email already exists")]
    EmailConflict {
        #[source]
        source: BoxError,
    },
    #[error("Cognito identity persistence conflict")]
    IdentityPersistenceConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary user registration persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted user registration state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal user registration persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin Cognito user registration transaction")]
    BeginTransactionFailed,
    #[error("failed to commit Cognito user registration transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait RegisterCognitoUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: RegisterCognitoUserCommand,
    ) -> Result<RegisterCognitoUserResult, RegisterCognitoUserError>;
}

pub struct RegisterCognitoUserHandler<U, R, I> {
    unit_of_work: U,
    users: R,
    identities: I,
}

impl<U, R, I> RegisterCognitoUserHandler<U, R, I> {
    pub fn new(unit_of_work: U, users: R, identities: I) -> Self {
        Self {
            unit_of_work,
            users,
            identities,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, I> RegisterCognitoUserUseCase for RegisterCognitoUserHandler<U, R, I>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
    I: UserCognitoIdentityRegistryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "register_cognito_user",
        skip_all,
        fields(
            user_id = tracing::field::Empty,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: RegisterCognitoUserCommand,
    ) -> Result<RegisterCognitoUserResult, RegisterCognitoUserError> {
        if !matches!(context.principal, Principal::System) {
            return Err(RegisterCognitoUserError::Forbidden);
        }

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| RegisterCognitoUserError::BeginTransactionFailed)?;
        let existing_user_id = self
            .identities
            .in_transaction(&mut tx)
            .lock_and_find_user_id(&command.identity)
            .await?;

        let user = if let Some(user_id) = existing_user_id {
            let user = self
                .users
                .in_transaction(&mut tx)
                .find_by_id(user_id)
                .await?
                .ok_or_else(|| RegisterCognitoUserError::InvalidPersistedState {
                    source: static_error("Cognito identity refers to a missing user"),
                })?
                .value;
            if user.email() != &command.email {
                return Err(RegisterCognitoUserError::IdentityConflict);
            }
            user
        } else {
            let user = create_user(command.email.clone());
            let user = self
                .users
                .in_transaction(&mut tx)
                .insert(&user)
                .await?
                .value;
            self.identities
                .in_transaction(&mut tx)
                .bind(&command.identity, user.id())
                .await?;
            user
        };

        tx.commit()
            .await
            .map_err(|_| RegisterCognitoUserError::CommitTransactionFailed)?;
        tracing::Span::current().record("user_id", tracing::field::display(user.id()));
        tracing::info!(
            event = "user.cognito_identity_registered",
            actor_type = context.principal.kind(),
            user_id = %user.id(),
            outcome = "success",
        );

        Ok(RegisterCognitoUserResult {
            user_id: user.id(),
            email: user.email().clone(),
        })
    }
}

fn create_user(email: Email) -> User {
    match User::create(NewUser {
        id: UserId::new(),
        email,
        profile: UserProfile::default(),
        preferences: UserPreferences::default(),
        account: UserAccount::default(),
    }) {
        Ok(user) => user,
        Err(error) => match error {},
    }
}

impl From<UserRepositoryError> for RegisterCognitoUserError {
    fn from(error: UserRepositoryError) -> Self {
        match error {
            UserRepositoryError::ConcurrencyConflict => Self::Internal {
                source: static_error("unexpected user concurrency conflict during registration"),
            },
            UserRepositoryError::EmailConflict { source } => Self::EmailConflict { source },
            UserRepositoryError::StripeCustomerConflict { source }
            | UserRepositoryError::Internal { source } => Self::Internal { source },
            UserRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
        }
    }
}

impl From<UserCognitoIdentityRegistryError> for RegisterCognitoUserError {
    fn from(error: UserCognitoIdentityRegistryError) -> Self {
        match error {
            UserCognitoIdentityRegistryError::Conflict { source } => {
                Self::IdentityPersistenceConflict { source }
            }
            UserCognitoIdentityRegistryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserCognitoIdentityRegistryError::InvalidPersistedIdentity { source } => {
                Self::InvalidPersistedState { source }
            }
            UserCognitoIdentityRegistryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        CognitoIssuer, CognitoSubject, UserInsertOutcome, UserStorageVersion, VersionedUser,
    };
    use application::error::box_error;
    use application::operation_context::{CorrelationId, RequestId};
    use application::transaction::TransactionError;
    use domain_primitives::versioned::Versioned;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::stripe_customer_id::StripeCustomerId;

    const TX_ID: usize = 17;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Operation {
        name: &'static str,
        tx_id: usize,
    }

    #[derive(Default)]
    struct State {
        operations: Vec<Operation>,
        inserted_user_ids: Vec<UserId>,
        bindings: Vec<(CognitoIdentity, UserId)>,
    }

    #[derive(Clone, Default)]
    struct SharedState(Arc<Mutex<State>>);

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
        fail_begin: bool,
        fail_commit: bool,
    }

    struct FakeTx {
        state: SharedState,
        fail_commit: bool,
        id: usize,
    }

    #[derive(Clone)]
    struct FakeUsers {
        state: SharedState,
        existing: Option<User>,
        insert_failure: Option<UserFailure>,
    }

    struct FakeUserRepository {
        state: SharedState,
        existing: Option<User>,
        insert_failure: Option<UserFailure>,
        tx_id: usize,
    }

    #[derive(Clone)]
    struct FakeIdentities {
        state: SharedState,
        existing_user_id: Option<UserId>,
        bind_conflict: bool,
    }

    struct FakeIdentityRegistry {
        state: SharedState,
        existing_user_id: Option<UserId>,
        bind_conflict: bool,
        tx_id: usize,
    }

    #[derive(Clone, Copy)]
    enum UserFailure {
        EmailConflict,
    }

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        match state.0.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn record(state: &SharedState, name: &'static str, tx_id: usize) {
        lock(state).operations.push(Operation { name, tx_id });
    }

    fn identity() -> CognitoIdentity {
        CognitoIdentity {
            issuer: CognitoIssuer::try_from("https://issuer.example/pool-a")
                .unwrap_or_else(|error| panic!("invalid test issuer: {error}")),
            subject: CognitoSubject::try_from("provider|opaque-subject")
                .unwrap_or_else(|error| panic!("invalid test subject: {error}")),
        }
    }

    fn email(value: &str) -> Email {
        Email::try_from(value).unwrap_or_else(|error| panic!("invalid test email: {error}"))
    }

    fn user(user_id: UserId, email: &str) -> User {
        create_user_with_id(user_id, self::email(email))
    }

    fn create_user_with_id(user_id: UserId, email: Email) -> User {
        User::create(NewUser {
            id: user_id,
            email,
            profile: UserProfile::default(),
            preferences: UserPreferences::default(),
            account: UserAccount::default(),
        })
        .unwrap_or_else(|error| match error {})
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request-test"),
            correlation_id: CorrelationId::new("correlation-test"),
        }
    }

    fn command(email_value: &str) -> RegisterCognitoUserCommand {
        RegisterCognitoUserCommand {
            identity: identity(),
            email: email(email_value),
        }
    }

    fn handler(
        state: &SharedState,
        existing: Option<User>,
        existing_user_id: Option<UserId>,
        insert_failure: Option<UserFailure>,
        bind_conflict: bool,
    ) -> RegisterCognitoUserHandler<FakeUnitOfWork, FakeUsers, FakeIdentities> {
        RegisterCognitoUserHandler::new(
            FakeUnitOfWork {
                state: state.clone(),
                fail_begin: false,
                fail_commit: false,
            },
            FakeUsers {
                state: state.clone(),
                existing,
                insert_failure,
            },
            FakeIdentities {
                state: state.clone(),
                existing_user_id,
                bind_conflict,
            },
        )
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            record(&self.state, "commit", self.id);
            if self.fail_commit {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            if self.fail_begin {
                return Err(TransactionError::BeginFailed);
            }
            record(&self.state, "begin", TX_ID);
            Ok(FakeTx {
                state: self.state.clone(),
                fail_commit: self.fail_commit,
                id: TX_ID,
            })
        }
    }

    impl UserRepositoryFactory<FakeTx> for FakeUsers {
        fn in_transaction<'tx>(&'tx self, tx: &'tx mut FakeTx) -> impl UserRepository + 'tx {
            FakeUserRepository {
                state: self.state.clone(),
                existing: self.existing.clone(),
                insert_failure: self.insert_failure,
                tx_id: tx.id,
            }
        }
    }

    #[async_trait::async_trait]
    impl UserRepository for FakeUserRepository {
        async fn find_by_id(
            &mut self,
            _: UserId,
        ) -> Result<Option<VersionedUser>, UserRepositoryError> {
            record(&self.state, "find_user", self.tx_id);
            Ok(self
                .existing
                .clone()
                .map(|user| Versioned::new(user, UserStorageVersion::INITIAL)))
        }

        async fn find_by_email(
            &mut self,
            _: &Email,
        ) -> Result<Option<VersionedUser>, UserRepositoryError> {
            Ok(None)
        }

        async fn find_by_stripe_customer_id(
            &mut self,
            _: &StripeCustomerId,
        ) -> Result<Option<VersionedUser>, UserRepositoryError> {
            Ok(None)
        }

        async fn insert(&mut self, user: &User) -> Result<VersionedUser, UserRepositoryError> {
            record(&self.state, "insert_user", self.tx_id);
            if matches!(self.insert_failure, Some(UserFailure::EmailConflict)) {
                return Err(UserRepositoryError::EmailConflict {
                    source: box_error(std::io::Error::other("email conflict")),
                });
            }
            lock(&self.state).inserted_user_ids.push(user.id());
            Ok(Versioned::new(user.clone(), UserStorageVersion::INITIAL))
        }

        async fn insert_if_absent(
            &mut self,
            user: &User,
        ) -> Result<UserInsertOutcome, UserRepositoryError> {
            self.insert(user).await.map(UserInsertOutcome::Created)
        }

        async fn update(
            &mut self,
            user: &User,
            _: UserStorageVersion,
        ) -> Result<VersionedUser, UserRepositoryError> {
            Ok(Versioned::new(user.clone(), UserStorageVersion::INITIAL))
        }

        async fn delete_by_id(&mut self, _: UserId) -> Result<bool, UserRepositoryError> {
            Ok(false)
        }
    }

    impl UserCognitoIdentityRegistryFactory<FakeTx> for FakeIdentities {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut FakeTx,
        ) -> impl UserCognitoIdentityRegistry + 'tx {
            FakeIdentityRegistry {
                state: self.state.clone(),
                existing_user_id: self.existing_user_id,
                bind_conflict: self.bind_conflict,
                tx_id: tx.id,
            }
        }
    }

    #[async_trait::async_trait]
    impl UserCognitoIdentityRegistry for FakeIdentityRegistry {
        async fn lock_and_find_user_id(
            &mut self,
            _: &CognitoIdentity,
        ) -> Result<Option<UserId>, UserCognitoIdentityRegistryError> {
            record(&self.state, "lock_identity", self.tx_id);
            Ok(self.existing_user_id)
        }

        async fn find_by_user_id(
            &mut self,
            _: UserId,
        ) -> Result<Option<CognitoIdentity>, UserCognitoIdentityRegistryError> {
            Ok(None)
        }

        async fn bind(
            &mut self,
            identity: &CognitoIdentity,
            user_id: UserId,
        ) -> Result<(), UserCognitoIdentityRegistryError> {
            record(&self.state, "bind_identity", self.tx_id);
            if self.bind_conflict {
                return Err(UserCognitoIdentityRegistryError::Conflict {
                    source: box_error(std::io::Error::other("identity conflict")),
                });
            }
            lock(&self.state).bindings.push((identity.clone(), user_id));
            Ok(())
        }
    }

    #[tokio::test]
    async fn should_create_independent_uuid_v7_user_and_bind_identity_in_one_transaction() {
        let state = SharedState::default();
        let result = handler(&state, None, None, None, false)
            .execute(&context(Principal::System), command("ada@example.com"))
            .await
            .unwrap_or_else(|error| panic!("registration failed: {error}"));
        let state = lock(&state);

        assert_eq!(7, result.user_id.as_uuid().get_version_num());
        assert_eq!(email("ada@example.com"), result.email);
        assert_eq!(vec![result.user_id], state.inserted_user_ids);
        assert_eq!(vec![(identity(), result.user_id)], state.bindings);
        assert_eq!(
            vec![
                Operation {
                    name: "begin",
                    tx_id: TX_ID,
                },
                Operation {
                    name: "lock_identity",
                    tx_id: TX_ID,
                },
                Operation {
                    name: "insert_user",
                    tx_id: TX_ID,
                },
                Operation {
                    name: "bind_identity",
                    tx_id: TX_ID,
                },
                Operation {
                    name: "commit",
                    tx_id: TX_ID,
                },
            ],
            state.operations
        );
    }

    #[tokio::test]
    async fn should_replay_existing_identity_without_new_user_or_binding() {
        let state = SharedState::default();
        let user_id = UserId::new();
        let result = handler(
            &state,
            Some(user(user_id, "ada@example.com")),
            Some(user_id),
            None,
            false,
        )
        .execute(&context(Principal::System), command("ada@example.com"))
        .await;
        let state = lock(&state);

        assert!(matches!(result, Ok(result) if result.user_id == user_id));
        assert!(state.inserted_user_ids.is_empty());
        assert!(state.bindings.is_empty());
        assert_eq!(
            vec!["begin", "lock_identity", "find_user", "commit"],
            state
                .operations
                .iter()
                .map(|operation| operation.name)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn should_reject_replay_with_different_email_without_commit() {
        let state = SharedState::default();
        let user_id = UserId::new();
        let result = handler(
            &state,
            Some(user(user_id, "ada@example.com")),
            Some(user_id),
            None,
            false,
        )
        .execute(&context(Principal::System), command("grace@example.com"))
        .await;
        let state = lock(&state);

        assert!(matches!(
            result,
            Err(RegisterCognitoUserError::IdentityConflict)
        ));
        assert!(
            !state
                .operations
                .iter()
                .any(|operation| operation.name == "commit")
        );
        assert!(state.inserted_user_ids.is_empty());
        assert!(state.bindings.is_empty());
    }

    #[tokio::test]
    async fn should_reject_non_system_actor_before_beginning_transaction() {
        let state = SharedState::default();
        let result = handler(&state, None, None, None, false)
            .execute(
                &context(Principal::User(UserId::new())),
                command("ada@example.com"),
            )
            .await;

        assert!(matches!(result, Err(RegisterCognitoUserError::Forbidden)));
        assert!(lock(&state).operations.is_empty());
    }

    #[tokio::test]
    async fn should_not_bind_or_commit_when_email_conflicts() {
        let state = SharedState::default();
        let result = handler(&state, None, None, Some(UserFailure::EmailConflict), false)
            .execute(&context(Principal::System), command("ada@example.com"))
            .await;
        let state = lock(&state);

        assert!(matches!(
            result,
            Err(RegisterCognitoUserError::EmailConflict { .. })
        ));
        assert!(state.bindings.is_empty());
        assert!(
            !state
                .operations
                .iter()
                .any(|operation| operation.name == "commit")
        );
    }

    #[tokio::test]
    async fn should_not_commit_when_identity_binding_conflicts() {
        let state = SharedState::default();
        let result = handler(&state, None, None, None, true)
            .execute(&context(Principal::System), command("ada@example.com"))
            .await;

        assert!(matches!(
            result,
            Err(RegisterCognitoUserError::IdentityPersistenceConflict { .. })
        ));
        assert!(
            !lock(&state)
                .operations
                .iter()
                .any(|operation| operation.name == "commit")
        );
    }

    #[tokio::test]
    async fn should_map_transaction_lifecycle_failures() {
        let state = SharedState::default();
        let begin_failure = RegisterCognitoUserHandler::new(
            FakeUnitOfWork {
                state: state.clone(),
                fail_begin: true,
                fail_commit: false,
            },
            FakeUsers {
                state: state.clone(),
                existing: None,
                insert_failure: None,
            },
            FakeIdentities {
                state: state.clone(),
                existing_user_id: None,
                bind_conflict: false,
            },
        )
        .execute(&context(Principal::System), command("ada@example.com"))
        .await;
        assert!(matches!(
            begin_failure,
            Err(RegisterCognitoUserError::BeginTransactionFailed)
        ));

        let commit_failure = RegisterCognitoUserHandler::new(
            FakeUnitOfWork {
                state: state.clone(),
                fail_begin: false,
                fail_commit: true,
            },
            FakeUsers {
                state: state.clone(),
                existing: None,
                insert_failure: None,
            },
            FakeIdentities {
                state,
                existing_user_id: None,
                bind_conflict: false,
            },
        )
        .execute(&context(Principal::System), command("grace@example.com"))
        .await;
        assert!(matches!(
            commit_failure,
            Err(RegisterCognitoUserError::CommitTransactionFailed)
        ));
    }

    #[tokio::test]
    async fn should_report_missing_user_for_existing_identity_as_invalid_persisted_state() {
        let state = SharedState::default();
        let result = handler(&state, None, Some(UserId::new()), None, false)
            .execute(&context(Principal::System), command("ada@example.com"))
            .await;

        assert!(matches!(
            result,
            Err(RegisterCognitoUserError::InvalidPersistedState { .. })
        ));
        assert!(
            !lock(&state)
                .operations
                .iter()
                .any(|operation| operation.name == "commit")
        );
    }
}
