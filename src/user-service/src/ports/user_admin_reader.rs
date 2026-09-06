#![allow(dead_code)]

use application::error::BoxError;
use user_core::role::UserRole;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct UserAdminActorView {
    pub user_id: UserId,
    pub role: UserRole,
}

#[derive(Debug, thiserror::Error)]
pub enum UserAdminReadError {
    #[error("temporary user admin read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user admin read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user admin read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UserAdminReader: Send {
    /// Finds a persisted administrator that is active (not suspended).
    async fn find_admin_actor(
        &mut self,
        user_id: UserId,
    ) -> Result<Option<UserAdminActorView>, UserAdminReadError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserAdminRemovalDecision {
    TargetNotFound,
    TargetNotAdmin,
    #[default]
    Allowed,
    LastAdmin,
}

#[async_trait::async_trait]
pub trait UserAdminMutationGuard: Send {
    async fn check_removal(
        &mut self,
        user_id: UserId,
    ) -> Result<UserAdminRemovalDecision, UserAdminReadError>;
}

pub trait UserAdminReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl UserAdminReader + 'tx;
}

pub trait UserAdminMutationGuardFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl UserAdminMutationGuard + 'tx;
}
