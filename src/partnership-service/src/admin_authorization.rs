use application::{
    error::BoxError,
    operation_context::{OperationContext, Principal},
    transaction::Transaction,
};
use user_core::role::UserRole;
use user_service::ports::{UserAdminReadError, UserAdminReader, UserAdminReaderFactory};

#[derive(Debug, thiserror::Error)]
pub(crate) enum AdminAuthorizationError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary admin authorization failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid admin authorization data")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal admin authorization failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}
pub(crate) async fn authorize_admin<Tx: Transaction, R: UserAdminReaderFactory<Tx>>(
    context: &OperationContext,
    tx: &mut Tx,
    reader: &R,
) -> Result<(), AdminAuthorizationError> {
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => {
            match reader.in_transaction(tx).find_admin_actor(user_id).await? {
                Some(actor) if actor.role == UserRole::Admin => Ok(()),
                _ => Err(AdminAuthorizationError::Forbidden),
            }
        }
        Principal::Anonymous => Err(AdminAuthorizationError::Forbidden),
    }
}
impl From<UserAdminReadError> for AdminAuthorizationError {
    fn from(value: UserAdminReadError) -> Self {
        match value {
            UserAdminReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAdminReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            UserAdminReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        error::static_error,
        operation_context::{CorrelationId, OperationContext, Principal, RequestId},
    };
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::{role::UserRole, user_id::UserId};
    use user_service::ports::{
        UserAdminActorView, UserAdminReadError, UserAdminReader, UserAdminReaderFactory,
    };

    #[derive(Default)]
    struct State {
        actor: Option<UserAdminActorView>,
        error: Option<UserAdminReadError>,
        bindings: Vec<usize>,
        reads: usize,
    }

    struct FakeTransaction {
        id: usize,
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), application::transaction::TransactionError> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct FakeAdminReaderFactory {
        state: Arc<Mutex<State>>,
    }

    struct FakeAdminReader {
        state: Arc<Mutex<State>>,
    }

    impl UserAdminReaderFactory<FakeTransaction> for FakeAdminReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut FakeTransaction,
        ) -> impl UserAdminReader + 'tx {
            lock(&self.state).bindings.push(tx.id);
            FakeAdminReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeAdminReader {
        async fn find_admin_actor(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            let mut state = lock(&self.state);
            state.reads += 1;
            if let Some(error) = state.error.take() {
                return Err(error);
            }
            Ok(state.actor.clone())
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn lock(state: &Arc<Mutex<State>>) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[tokio::test]
    async fn should_apply_admin_authorization_principal_matrix() {
        let user_id = UserId::new();
        let cases = [
            (Principal::Anonymous, None, false, 0),
            (Principal::User(user_id), Some(UserRole::Admin), true, 1),
            (Principal::User(user_id), Some(UserRole::User), false, 1),
            (Principal::User(user_id), None, false, 1),
            (
                Principal::DelegatedUser {
                    user_id,
                    capabilities: Default::default(),
                },
                Some(UserRole::Admin),
                true,
                1,
            ),
            (
                Principal::DelegatedUser {
                    user_id,
                    capabilities: Default::default(),
                },
                Some(UserRole::User),
                false,
                1,
            ),
            (
                Principal::DelegatedUser {
                    user_id,
                    capabilities: Default::default(),
                },
                None,
                false,
                1,
            ),
            (
                Principal::Service("partnership-service".to_owned()),
                None,
                true,
                0,
            ),
            (Principal::System, None, true, 0),
        ];

        for (principal, role, allowed, expected_reads) in cases {
            let state = Arc::new(Mutex::new(State {
                actor: role.map(|role| UserAdminActorView { user_id, role }),
                error: None,
                bindings: Vec::new(),
                reads: 0,
            }));
            let factory = FakeAdminReaderFactory {
                state: Arc::clone(&state),
            };
            let mut tx = FakeTransaction { id: 17 };

            let result = authorize_admin(&context(principal), &mut tx, &factory).await;

            assert_eq!(allowed, result.is_ok());
            let state = lock(&state);
            assert_eq!(expected_reads, state.reads);
            assert_eq!(expected_reads, state.bindings.len());
            assert!(state.bindings.iter().all(|id| *id == 17));
        }
    }

    #[tokio::test]
    async fn should_translate_each_admin_reader_error() {
        let cases = [
            UserAdminReadError::TemporarilyUnavailable {
                source: static_error("temporary"),
            },
            UserAdminReadError::InvalidReadModel {
                source: static_error("invalid"),
            },
            UserAdminReadError::Internal {
                source: static_error("internal"),
            },
        ];

        for (index, error) in cases.into_iter().enumerate() {
            let state = Arc::new(Mutex::new(State {
                actor: None,
                error: Some(error),
                bindings: Vec::new(),
                reads: 0,
            }));
            let factory = FakeAdminReaderFactory {
                state: Arc::clone(&state),
            };
            let mut tx = FakeTransaction { id: index + 1 };

            let result =
                authorize_admin(&context(Principal::User(UserId::new())), &mut tx, &factory).await;

            match index {
                0 => assert!(matches!(
                    result,
                    Err(AdminAuthorizationError::TemporarilyUnavailable { .. })
                )),
                1 => assert!(matches!(
                    result,
                    Err(AdminAuthorizationError::InvalidReadModel { .. })
                )),
                2 => assert!(matches!(
                    result,
                    Err(AdminAuthorizationError::Internal { .. })
                )),
                _ => unreachable!(),
            }
            let state = lock(&state);
            assert_eq!(1, state.reads);
            assert_eq!(vec![index + 1], state.bindings);
        }
    }
}
