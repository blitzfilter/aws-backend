use application::error::BoxError;

use crate::use_cases::queries::list_admin_partnerships::{
    ListAdminPartnershipsRequest, ListAdminPartnershipsResult,
};

#[derive(Debug, thiserror::Error)]
pub enum PartnershipSearchReadError {
    #[error("temporary partnership search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid partnership search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PartnershipSearchReader: Send {
    /// Returns the bounded admin collection in fixed `created DESC, partnership_id DESC` order.
    /// Implementations use the shared default cursor size when no cursor is supplied.
    async fn search(
        &mut self,
        request: &ListAdminPartnershipsRequest,
    ) -> Result<ListAdminPartnershipsResult, PartnershipSearchReadError>;
}

pub trait PartnershipSearchReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartnershipSearchReader + 'tx;
}
