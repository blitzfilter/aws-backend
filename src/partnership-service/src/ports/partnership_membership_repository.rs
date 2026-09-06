use application::error::BoxError;
use partnership_core::partnership_id::PartnershipId;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
pub enum PartnershipGrantError {
    #[error("temporary partnership grant failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership grant failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartnershipMembershipAddOutcome {
    Added,
    AlreadyMember,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartnershipMembershipRemoveOutcome {
    Removed,
    AlreadyAbsent,
}

#[async_trait::async_trait]
pub trait PartnershipMembershipRepository: Send {
    async fn add_member(
        &mut self,
        user_id: UserId,
        partnership_id: PartnershipId,
    ) -> Result<PartnershipMembershipAddOutcome, PartnershipGrantError>;

    async fn remove_member(
        &mut self,
        user_id: UserId,
        partnership_id: PartnershipId,
    ) -> Result<PartnershipMembershipRemoveOutcome, PartnershipGrantError>;
}

pub trait PartnershipMembershipRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl PartnershipMembershipRepository + 'tx;
}
