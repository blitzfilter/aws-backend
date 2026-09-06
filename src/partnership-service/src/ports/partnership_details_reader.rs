use application::error::BoxError;
use partnership_core::partnership_id::PartnershipId;

use crate::use_cases::queries::get_admin_partnership::AdminPartnershipDetailsView;

#[derive(Debug, thiserror::Error)]
pub enum PartnershipDetailsReadError {
    #[error("temporary partnership details read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid partnership details read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership details read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PartnershipDetailsReader: Send {
    async fn find_by_id(
        &mut self,
        partnership_id: PartnershipId,
    ) -> Result<Option<AdminPartnershipDetailsView>, PartnershipDetailsReadError>;
}

pub trait PartnershipDetailsReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartnershipDetailsReader + 'tx;
}
