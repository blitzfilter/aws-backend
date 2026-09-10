use crate::ports::{
    UserAdminReadError, UserAdminReaderFactory, UserCognitoIdentityRegistry,
    UserCognitoIdentityRegistryError, UserCognitoIdentityRegistryFactory,
    UserSessionRevocationError, UserSessionRevoker,
};
use crate::use_cases::authorization::{
    RequireAdminActorError, require_admin_actor, require_admin_actor_credential,
};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokeUserSessionsCommand {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokeUserSessionsResult {
    pub user_id: UserId,
}

#[derive(Debug, thiserror::Error)]
pub enum RevokeUserSessionsError {
    #[error("authenticated actor required to revoke user sessions")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    UserNotFound,
    #[error("identity-provider session revocation is temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted user state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("identity-provider session revocation failed internally")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin revoke user sessions transaction")]
    BeginTransactionFailed,
    #[error("failed to commit revoke user sessions transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait RevokeUserSessionsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: RevokeUserSessionsCommand,
    ) -> Result<RevokeUserSessionsResult, RevokeUserSessionsError>;
}

pub struct RevokeUserSessionsHandler<U, A, I, S> {
    unit_of_work: U,
    admin_reader: A,
    identities: I,
    session_revoker: S,
}

impl<U, A, I, S> RevokeUserSessionsHandler<U, A, I, S> {
    pub fn new(unit_of_work: U, admin_reader: A, identities: I, session_revoker: S) -> Self {
        Self {
            unit_of_work,
            admin_reader,
            identities,
            session_revoker,
        }
    }
}

#[async_trait::async_trait]
impl<U, A, I, S> RevokeUserSessionsUseCase for RevokeUserSessionsHandler<U, A, I, S>
where
    U: UnitOfWork,
    A: UserAdminReaderFactory<U::Tx>,
    I: UserCognitoIdentityRegistryFactory<U::Tx>,
    S: UserSessionRevoker,
{
    #[tracing::instrument(
        name = "revoke_user_sessions",
        skip_all,
        fields(
            target_user_id = %command.user_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            outcome = tracing::field::Empty,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: RevokeUserSessionsCommand,
    ) -> Result<RevokeUserSessionsResult, RevokeUserSessionsError> {
        let actor_id = context.principal.actor_id();
        if let Some(actor_id) = actor_id.as_deref() {
            tracing::Span::current().record("actor_id", actor_id);
        }

        let result = async {
            require_admin_actor_credential(context, CredentialCapability::UsersWrite)?;

            let mut tx = self
                .unit_of_work
                .begin()
                .await
                .map_err(|_| RevokeUserSessionsError::BeginTransactionFailed)?;
            {
                let mut admin_reader = self.admin_reader.in_transaction(&mut tx);
                require_admin_actor(context, &mut admin_reader).await?;
            }
            let identity = self
                .identities
                .in_transaction(&mut tx)
                .find_by_user_id(command.user_id)
                .await?
                .ok_or(RevokeUserSessionsError::UserNotFound)?;
            tx.commit()
                .await
                .map_err(|_| RevokeUserSessionsError::CommitTransactionFailed)?;

            self.session_revoker.revoke_sessions(&identity).await?;
            Ok(RevokeUserSessionsResult {
                user_id: command.user_id,
            })
        }
        .await;

        match result {
            Ok(result) => {
                tracing::Span::current().record("outcome", "success");
                tracing::info!(
                    event = "user.sessions_revoked",
                    action = "revoke_user_sessions",
                    actor_type = context.principal.kind(),
                    actor_id = actor_id.as_deref().unwrap_or(""),
                    target_type = "user_sessions",
                    target_user_id = %result.user_id,
                    request_id = %context.request_id,
                    correlation_id = %context.correlation_id,
                    outcome = "success",
                );
                Ok(result)
            }
            Err(error) => {
                tracing::Span::current().record("outcome", "failure");
                tracing::warn!(
                    event = "user.sessions_revoked",
                    action = "revoke_user_sessions",
                    actor_type = context.principal.kind(),
                    actor_id = actor_id.as_deref().unwrap_or(""),
                    target_type = "user_sessions",
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

impl From<OperationAuthorizationError> for RevokeUserSessionsError {
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

impl From<RequireAdminActorError> for RevokeUserSessionsError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for RevokeUserSessionsError {
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

impl From<UserCognitoIdentityRegistryError> for RevokeUserSessionsError {
    fn from(error: UserCognitoIdentityRegistryError) -> Self {
        match error {
            UserCognitoIdentityRegistryError::Conflict { source }
            | UserCognitoIdentityRegistryError::Internal { source } => Self::Internal { source },
            UserCognitoIdentityRegistryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserCognitoIdentityRegistryError::InvalidPersistedIdentity { source } => {
                Self::InvalidPersistedState { source }
            }
        }
    }
}

impl From<UserSessionRevocationError> for RevokeUserSessionsError {
    fn from(error: UserSessionRevocationError) -> Self {
        match error {
            UserSessionRevocationError::UserNotFound => Self::UserNotFound,
            UserSessionRevocationError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserSessionRevocationError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        CognitoIdentity, CognitoIssuer, CognitoSubject, UserAdminActorView, UserAdminReader,
    };
    use application::error::box_error;
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::transaction::TransactionError;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::role::UserRole;

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        revoked_identities: Vec<(String, String)>,
        operations: Vec<&'static str>,
    }
    #[derive(Clone, Default)]
    struct Fakes(Arc<Mutex<State>>);
    struct FakeTx(Fakes);
    #[derive(Clone, Copy)]
    struct Admins(Option<UserRole>);
    #[derive(Clone, Copy)]
    struct Identities(bool);
    #[derive(Clone)]
    struct Revoker {
        fakes: Fakes,
        error: Option<RevokerError>,
    }
    #[derive(Clone, Copy)]
    enum RevokerError {
        Temporary,
        Internal,
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
    fn cognito_identity() -> CognitoIdentity {
        CognitoIdentity {
            issuer: CognitoIssuer::try_from("https://issuer.example")
                .unwrap_or_else(|error| panic!("invalid test issuer: {error}")),
            subject: CognitoSubject::try_from("provider|opaque-subject")
                .unwrap_or_else(|error| panic!("invalid test subject: {error}")),
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.0.0);
            state.commits += 1;
            state.operations.push("commit");
            Ok(())
        }
    }
    #[async_trait::async_trait]
    impl UnitOfWork for Fakes {
        type Tx = FakeTx;
        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = lock(&self.0);
            state.begins += 1;
            state.operations.push("begin");
            Ok(FakeTx(self.clone()))
        }
    }
    struct AdminReader(Option<UserRole>);
    #[async_trait::async_trait]
    impl UserAdminReader for AdminReader {
        async fn find_admin_actor(
            &mut self,
            user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            Ok(self.0.map(|role| UserAdminActorView { user_id, role }))
        }
    }
    impl UserAdminReaderFactory<FakeTx> for Admins {
        fn in_transaction<'tx>(&'tx self, _: &'tx mut FakeTx) -> impl UserAdminReader + 'tx {
            AdminReader(self.0)
        }
    }
    struct IdentityRegistry(bool);
    #[async_trait::async_trait]
    impl UserCognitoIdentityRegistry for IdentityRegistry {
        async fn lock_and_find_user_id(
            &mut self,
            _: &CognitoIdentity,
        ) -> Result<Option<UserId>, UserCognitoIdentityRegistryError> {
            Ok(None)
        }

        async fn find_by_user_id(
            &mut self,
            _: UserId,
        ) -> Result<Option<CognitoIdentity>, UserCognitoIdentityRegistryError> {
            Ok(self.0.then(cognito_identity))
        }

        async fn bind(
            &mut self,
            _: &CognitoIdentity,
            _: UserId,
        ) -> Result<(), UserCognitoIdentityRegistryError> {
            Ok(())
        }
    }
    impl UserCognitoIdentityRegistryFactory<FakeTx> for Identities {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut FakeTx,
        ) -> impl UserCognitoIdentityRegistry + 'tx {
            IdentityRegistry(self.0)
        }
    }
    #[async_trait::async_trait]
    impl UserSessionRevoker for Revoker {
        async fn revoke_sessions(
            &self,
            identity: &CognitoIdentity,
        ) -> Result<(), UserSessionRevocationError> {
            let mut state = lock(&self.fakes.0);
            state.revoked_identities.push((
                identity.issuer.as_str().to_owned(),
                identity.subject.as_str().to_owned(),
            ));
            state.operations.push("revoke");
            drop(state);
            match self.error {
                None => Ok(()),
                Some(RevokerError::Temporary) => {
                    Err(UserSessionRevocationError::TemporarilyUnavailable {
                        source: box_error(std::io::Error::other("unavailable")),
                    })
                }
                Some(RevokerError::Internal) => Err(UserSessionRevocationError::Internal {
                    source: box_error(std::io::Error::other("invalid response")),
                }),
            }
        }
    }
    fn handler(
        fakes: &Fakes,
        role: Option<UserRole>,
        user_exists: bool,
        error: Option<RevokerError>,
    ) -> impl RevokeUserSessionsUseCase {
        RevokeUserSessionsHandler::new(
            fakes.clone(),
            Admins(role),
            Identities(user_exists),
            Revoker {
                fakes: fakes.clone(),
                error,
            },
        )
    }

    #[tokio::test]
    async fn should_authorize_target_and_revoke_after_commit() {
        let fakes = Fakes::default();
        let user_id = UserId::new();
        let result = handler(&fakes, Some(UserRole::Admin), true, None)
            .execute(
                &context(Principal::User(UserId::new())),
                RevokeUserSessionsCommand { user_id },
            )
            .await;
        assert!(matches!(result, Ok(result) if result.user_id == user_id));
        let state = lock(&fakes.0);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.commits);
        assert_eq!(
            vec![(
                "https://issuer.example".to_owned(),
                "provider|opaque-subject".to_owned(),
            )],
            state.revoked_identities
        );
        assert_eq!(vec!["begin", "commit", "revoke"], state.operations);
    }

    #[tokio::test]
    async fn should_not_revoke_missing_or_non_admin_target() {
        let fakes = Fakes::default();
        let missing = handler(&fakes, Some(UserRole::Admin), false, None)
            .execute(
                &context(Principal::User(UserId::new())),
                RevokeUserSessionsCommand {
                    user_id: UserId::new(),
                },
            )
            .await;
        assert!(matches!(
            missing,
            Err(RevokeUserSessionsError::UserNotFound)
        ));
        let non_admin = handler(&fakes, Some(UserRole::User), true, None)
            .execute(
                &context(Principal::User(UserId::new())),
                RevokeUserSessionsCommand {
                    user_id: UserId::new(),
                },
            )
            .await;
        assert!(matches!(non_admin, Err(RevokeUserSessionsError::Forbidden)));
        let state = lock(&fakes.0);
        assert!(state.revoked_identities.is_empty());
        assert_eq!(2, state.begins);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_reject_anonymous_or_unscoped_delegated_actor_before_transaction() {
        let fakes = Fakes::default();
        let anonymous = handler(&fakes, Some(UserRole::Admin), true, None)
            .execute(
                &context(Principal::Anonymous),
                RevokeUserSessionsCommand {
                    user_id: UserId::new(),
                },
            )
            .await;
        let delegated = handler(&fakes, Some(UserRole::Admin), true, None)
            .execute(
                &context(Principal::DelegatedUser {
                    user_id: UserId::new(),
                    capabilities: Default::default(),
                }),
                RevokeUserSessionsCommand {
                    user_id: UserId::new(),
                },
            )
            .await;

        assert!(matches!(
            anonymous,
            Err(RevokeUserSessionsError::AuthenticatedActorRequired)
        ));
        assert!(matches!(delegated, Err(RevokeUserSessionsError::Forbidden)));
        assert_eq!(0, lock(&fakes.0).begins);
    }

    #[tokio::test]
    async fn should_map_provider_failures_after_authorization() {
        let fakes = Fakes::default();
        let temporary = handler(
            &fakes,
            Some(UserRole::Admin),
            true,
            Some(RevokerError::Temporary),
        )
        .execute(
            &context(Principal::User(UserId::new())),
            RevokeUserSessionsCommand {
                user_id: UserId::new(),
            },
        )
        .await;
        assert!(matches!(
            temporary,
            Err(RevokeUserSessionsError::TemporarilyUnavailable { .. })
        ));
        let internal = handler(
            &fakes,
            Some(UserRole::Admin),
            true,
            Some(RevokerError::Internal),
        )
        .execute(
            &context(Principal::User(UserId::new())),
            RevokeUserSessionsCommand {
                user_id: UserId::new(),
            },
        )
        .await;
        assert!(matches!(
            internal,
            Err(RevokeUserSessionsError::Internal { .. })
        ));
        assert_eq!(2, lock(&fakes.0).commits);
    }
}
