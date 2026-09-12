use application::transaction::{Transaction, TransactionError, UnitOfWork};
use sqlx::{PgConnection, PgPool, Postgres};

mod config;
pub use config::{
    PostgresConnectError, PostgresPoolConfig, PostgresPoolConfigError, PostgresTlsConfig,
};

#[derive(Debug, Clone)]
pub struct SqlxUnitOfWork {
    pool: PgPool,
}

pub struct SqlxTransaction {
    transaction: sqlx::Transaction<'static, Postgres>,
}

impl SqlxUnitOfWork {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl SqlxTransaction {
    pub fn connection(&mut self) -> &mut PgConnection {
        &mut self.transaction
    }
}

#[async_trait::async_trait]
impl UnitOfWork for SqlxUnitOfWork {
    type Tx = SqlxTransaction;

    async fn begin(&self) -> Result<Self::Tx, TransactionError> {
        self.pool
            .begin()
            .await
            .map(|transaction| SqlxTransaction { transaction })
            .map_err(|_| TransactionError::BeginFailed)
    }
}

#[async_trait::async_trait]
impl Transaction for SqlxTransaction {
    async fn commit(self) -> Result<(), TransactionError> {
        self.transaction
            .commit()
            .await
            .map_err(|_| TransactionError::CommitFailed)
    }
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tls_tests;
