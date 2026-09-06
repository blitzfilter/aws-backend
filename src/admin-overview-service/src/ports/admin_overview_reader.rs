use application::error::BoxError;

use crate::use_cases::get_admin_overview::AdminOverview;

#[derive(Debug, thiserror::Error)]
pub enum AdminOverviewReadError {
    #[error("temporary admin overview read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid admin overview read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal admin overview read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AdminOverviewReader: Send {
    async fn read_overview(&mut self) -> Result<AdminOverview, AdminOverviewReadError>;
}

pub trait AdminOverviewReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl AdminOverviewReader + 'tx;
}
