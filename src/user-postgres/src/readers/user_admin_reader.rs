use application::error::box_error;
use platform_postgres::SqlxTransaction;

use user_core::role::UserRole;
use user_core::user_id::UserId;
use user_service::ports::{
    UserAdminActorView, UserAdminMutationGuard, UserAdminMutationGuardFactory, UserAdminReadError,
    UserAdminReader, UserAdminReaderFactory, UserAdminRemovalDecision,
};

const USER_ADMIN_INVARIANT_LOCK_KEY: i64 = 1_671_000_001;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxUserAdminReaderFactory;

struct SqlxUserAdminReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

#[derive(sqlx::FromRow)]
struct UserAdminActorRow {
    user_id: uuid::Uuid,
    role: String,
}

#[derive(sqlx::FromRow)]
struct UserAdminTargetRow {
    role: String,
    suspended: bool,
}

impl SqlxUserAdminReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl UserAdminReaderFactory<SqlxTransaction> for SqlxUserAdminReaderFactory {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut SqlxTransaction) -> impl UserAdminReader + 'tx {
        SqlxUserAdminReader {
            connection: tx.connection(),
        }
    }
}

impl UserAdminMutationGuardFactory<SqlxTransaction> for SqlxUserAdminReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl UserAdminMutationGuard + 'tx {
        SqlxUserAdminReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl UserAdminReader for SqlxUserAdminReader<'_> {
    async fn find_admin_actor(
        &mut self,
        user_id: UserId,
    ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
        let row = sqlx::query_as::<_, UserAdminActorRow>(
            "SELECT user_id, role FROM users WHERE user_id = $1 AND suspended = false",
        )
        .bind(uuid::Uuid::from(user_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(|source| UserAdminReadError::TemporarilyUnavailable {
            source: box_error(source),
        })?;

        row.map(TryInto::try_into).transpose()
    }
}

#[async_trait::async_trait]
impl UserAdminMutationGuard for SqlxUserAdminReader<'_> {
    async fn check_removal(
        &mut self,
        user_id: UserId,
    ) -> Result<UserAdminRemovalDecision, UserAdminReadError> {
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(USER_ADMIN_INVARIANT_LOCK_KEY)
            .execute(&mut *self.connection)
            .await
            .map_err(|source| UserAdminReadError::TemporarilyUnavailable {
                source: box_error(source),
            })?;

        let target = sqlx::query_as::<_, UserAdminTargetRow>(
            "SELECT role, suspended FROM users WHERE user_id = $1 FOR UPDATE",
        )
        .bind(uuid::Uuid::from(user_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(|source| UserAdminReadError::TemporarilyUnavailable {
            source: box_error(source),
        })?;

        let Some(target) = target else {
            return Ok(UserAdminRemovalDecision::TargetNotFound);
        };
        let role = UserRole::from_code(&target.role).ok_or_else(|| {
            UserAdminReadError::InvalidReadModel {
                source: box_error(InvalidAdminRole),
            }
        })?;
        if role != UserRole::Admin {
            return Ok(UserAdminRemovalDecision::TargetNotAdmin);
        }
        if target.suspended {
            return Ok(UserAdminRemovalDecision::TargetNotAdmin);
        }

        let admin_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM users WHERE role = 'ADMIN' AND suspended = false",
        )
        .fetch_one(&mut *self.connection)
        .await
        .map_err(|source| UserAdminReadError::TemporarilyUnavailable {
            source: box_error(source),
        })?;

        if admin_count <= 1 {
            Ok(UserAdminRemovalDecision::LastAdmin)
        } else {
            Ok(UserAdminRemovalDecision::Allowed)
        }
    }
}

impl TryFrom<UserAdminActorRow> for UserAdminActorView {
    type Error = UserAdminReadError;

    fn try_from(row: UserAdminActorRow) -> Result<Self, Self::Error> {
        let role =
            UserRole::from_code(&row.role).ok_or_else(|| UserAdminReadError::InvalidReadModel {
                source: box_error(InvalidAdminRole),
            })?;

        Ok(Self {
            user_id: UserId::from(row.user_id),
            role,
        })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid persisted admin role")]
struct InvalidAdminRole;
