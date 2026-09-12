use application::error::box_error;
use platform_postgres::{PostgresConnectError, PostgresPoolConfig};
use search_filter_service::ports::{
    PeriodicSearchFilterMatchingRunLease, PeriodicSearchFilterMatchingRunLock,
    PeriodicSearchFilterMatchingRunLockError,
};
use sqlx::{Connection, PgConnection};

const AURA_SCHEDULER_LOCK_NAMESPACE: i32 = 0x4155_5241;
const SEARCH_FILTER_PERIODIC_MATCH_LOCK_ID: i32 = 0x5346_504d;

#[derive(Clone)]
pub struct SqlxPeriodicSearchFilterMatchingRunLock {
    config: PostgresPoolConfig,
}
impl SqlxPeriodicSearchFilterMatchingRunLock {
    pub fn new(config: PostgresPoolConfig) -> Self {
        Self { config }
    }
}

struct SqlxPeriodicSearchFilterMatchingRunLease {
    connection: PgConnection,
}

#[async_trait::async_trait]
impl PeriodicSearchFilterMatchingRunLock for SqlxPeriodicSearchFilterMatchingRunLock {
    async fn try_acquire(
        &self,
    ) -> Result<
        Option<Box<dyn PeriodicSearchFilterMatchingRunLease>>,
        PeriodicSearchFilterMatchingRunLockError,
    > {
        let mut connection = self.config.connect_session().await.map_err(|source| {
            PeriodicSearchFilterMatchingRunLockError::LockFailed {
                source: box_error(source),
            }
        })?;
        let acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1, $2)")
            .bind(AURA_SCHEDULER_LOCK_NAMESPACE)
            .bind(SEARCH_FILTER_PERIODIC_MATCH_LOCK_ID)
            .fetch_one(&mut connection)
            .await
            .map_err(
                |source| PeriodicSearchFilterMatchingRunLockError::LockFailed {
                    source: box_error(PostgresConnectError::from(source)),
                },
            )?;
        Ok(acquired.then(|| {
            Box::new(SqlxPeriodicSearchFilterMatchingRunLease { connection })
                as Box<dyn PeriodicSearchFilterMatchingRunLease>
        }))
    }
}

#[async_trait::async_trait]
impl PeriodicSearchFilterMatchingRunLease for SqlxPeriodicSearchFilterMatchingRunLease {
    async fn release(mut self: Box<Self>) -> Result<(), PeriodicSearchFilterMatchingRunLockError> {
        sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1, $2)")
            .bind(AURA_SCHEDULER_LOCK_NAMESPACE)
            .bind(SEARCH_FILTER_PERIODIC_MATCH_LOCK_ID)
            .fetch_one(&mut self.connection)
            .await
            .map_err(
                |source| PeriodicSearchFilterMatchingRunLockError::ReleaseFailed {
                    source: box_error(PostgresConnectError::from(source)),
                },
            )?;
        self.connection.close().await.map_err(|source| {
            PeriodicSearchFilterMatchingRunLockError::ReleaseFailed {
                source: box_error(PostgresConnectError::from(source)),
            }
        })
    }
}
