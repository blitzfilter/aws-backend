use application::{
    error::BoxError,
    operation_context::{OperationContext, Principal},
    transaction::Transaction,
};
use user_core::role::UserRole;
use user_service::ports::{UserAdminReadError, UserAdminReader, UserAdminReaderFactory};

#[derive(Debug, thiserror::Error)]
pub(crate) enum AdminAuthorizationError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary admin authorization failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid admin authorization read model")]
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
        Principal::Anonymous => Err(AdminAuthorizationError::AuthenticatedActorRequired),
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => {
            match reader.in_transaction(tx).find_admin_actor(user_id).await? {
                Some(actor) if actor.role == UserRole::Admin => Ok(()),
                _ => Err(AdminAuthorizationError::Forbidden),
            }
        }
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
