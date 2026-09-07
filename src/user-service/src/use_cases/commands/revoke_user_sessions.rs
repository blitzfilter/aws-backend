use crate::ports::{
    UserAccountReadError, UserAccountReader, UserAccountReaderFactory, UserAdminReadError,
    UserAdminReaderFactory, UserSessionRevocationError, UserSessionRevoker,
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

pub struct RevokeUserSessionsHandler<U, A, V, S> {
    unit_of_work: U,
    admin_reader: A,
    user_reader: V,
    session_revoker: S,
}

impl<U, A, V, S> RevokeUserSessionsHandler<U, A, V, S> {
    pub fn new(unit_of_work: U, admin_reader: A, user_reader: V, session_revoker: S) -> Self {
        Self {
            unit_of_work,
            admin_reader,
            user_reader,
            session_revoker,
        }
    }
}

#[async_trait::async_trait]
impl<U, A, V, S> RevokeUserSessionsUseCase for RevokeUserSessionsHandler<U, A, V, S>
where
    U: UnitOfWork,
    A: UserAdminReaderFactory<U::Tx>,
    V: UserAccountReaderFactory<U::Tx>,
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
            let target_exists = self
                .user_reader
                .in_transaction(&mut tx)
                .find_by_id(command.user_id)
                .await?
                .is_some();
            if !target_exists {
                return Err(RevokeUserSessionsError::UserNotFound);
            }
            tx.commit()
                .await
                .map_err(|_| RevokeUserSessionsError::CommitTransactionFailed)?;

            self.session_revoker
                .revoke_sessions(command.user_id)
                .await?;
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

impl From<UserAccountReadError> for RevokeUserSessionsError {
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
    use crate::ports::{UserAdminActorView, UserAdminReader, UserDetailsView};
    use application::error::box_error;
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::transaction::TransactionError;
    use localization::Language;
    use money::Currency;
    use serde_email::Email;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::{measurement_unit::MeasurementUnit, role::UserRole, tier::UserTier};

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        revocations: usize,
    }
    #[derive(Clone, Default)]
    struct Fakes(Arc<Mutex<State>>);
    struct FakeTx(Fakes);
    #[derive(Clone, Copy)]
    struct Admins(Option<UserRole>);
    #[derive(Clone, Copy)]
    struct Users(bool);
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
    fn user_details(user_id: UserId) -> UserDetailsView {
        UserDetailsView {
            user_id,
            email: Email::try_from("target@example.test")
                .unwrap_or_else(|error| panic!("invalid test email: {error}")),
            first_name: None,
            last_name: None,
            language: Some(Language::En),
            currency: Some(Currency::Eur),
            measurement_unit: Some(MeasurementUnit::Metric),
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
    struct UserReader(bool);
    #[async_trait::async_trait]
    impl UserAccountReader for UserReader {
        async fn find_by_id(
            &mut self,
            user_id: UserId,
        ) -> Result<Option<UserDetailsView>, UserAccountReadError> {
            Ok(self.0.then(|| user_details(user_id)))
        }
    }
    impl UserAccountReaderFactory<FakeTx> for Users {
        fn in_transaction<'tx>(&'tx self, _: &'tx mut FakeTx) -> impl UserAccountReader + 'tx {
            UserReader(self.0)
        }
    }
    #[async_trait::async_trait]
    impl UserSessionRevoker for Revoker {
        async fn revoke_sessions(&self, _: UserId) -> Result<(), UserSessionRevocationError> {
            lock(&self.fakes.0).revocations += 1;
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
            Users(user_exists),
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
        assert_eq!(1, state.revocations);
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
        assert_eq!(0, lock(&fakes.0).revocations);
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
