use crate::ports::{
    PartyDeletionBlocker, PartyRepository, PartyRepositoryError, PartyRepositoryFactory,
};
use application::{
    error::{BoxError, static_error},
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use party_core::party_id::PartyId;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletePartyCommand {
    pub party_id: PartyId,
}

#[derive(Debug, thiserror::Error)]
pub enum DeletePartyError {
    #[error("authenticated actor required to delete party")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("party not found")]
    NotFound,
    #[error("party has protected dependencies")]
    DependencyConflict { blocker: PartyDeletionBlocker },
    #[error("concurrent party mutation")]
    ConcurrencyConflict,
    #[error("temporary party persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted party state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal party failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin delete party transaction")]
    BeginTransactionFailed,
    #[error("failed to commit delete party transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait DeletePartyUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeletePartyCommand,
    ) -> Result<(), DeletePartyError>;
}

pub struct DeletePartyHandler<U, R, A> {
    unit_of_work: U,
    parties: R,
    check_user_admin: A,
}

impl<U, R, A> DeletePartyHandler<U, R, A> {
    pub fn new(unit_of_work: U, parties: R, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            parties,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> DeletePartyUseCase for DeletePartyHandler<U, R, A>
where
    U: UnitOfWork,
    R: PartyRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "delete_party",
        skip_all,
        fields(
            action = "delete_party",
            party_id = %command.party_id,
            principal_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            outcome = tracing::field::Empty,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeletePartyCommand,
    ) -> Result<(), DeletePartyError> {
        let result = async {
            ensure_admin_or_internal(context, &self.check_user_admin).await?;

            let mut tx = self
                .unit_of_work
                .begin()
                .await
                .map_err(|_| DeletePartyError::BeginTransactionFailed)?;
            let stored = self
                .parties
                .in_transaction(&mut tx)
                .find_by_id_for_update(command.party_id)
                .await?
                .ok_or(DeletePartyError::NotFound)?;
            if let Some(blocker) = self
                .parties
                .in_transaction(&mut tx)
                .find_deletion_blocker(command.party_id)
                .await?
            {
                tracing::warn!(
                    event = "party.delete_rejected",
                    party_id = %command.party_id,
                    blocker = blocker_name(blocker),
                    outcome = "dependency_conflict",
                    "party deletion rejected by protected dependency"
                );
                return Err(DeletePartyError::DependencyConflict { blocker });
            }
            self.parties
                .in_transaction(&mut tx)
                .delete_unused(command.party_id, stored.version)
                .await?;
            tx.commit()
                .await
                .map_err(|_| DeletePartyError::CommitTransactionFailed)?;
            Ok(())
        }
        .await;

        let outcome = delete_outcome(&result);
        tracing::Span::current().record("outcome", outcome);
        if result.is_ok() {
            tracing::info!(
                event = "party.deleted",
                party_id = %command.party_id,
                actor_type = context.principal.kind(),
                actor_id = %context.principal.label(),
                request_id = %context.request_id,
                correlation_id = %context.correlation_id,
                outcome,
                "party deleted"
            );
        }
        result
    }
}

fn blocker_name(blocker: PartyDeletionBlocker) -> &'static str {
    match blocker {
        PartyDeletionBlocker::ListingSources => "listing_sources",
        PartyDeletionBlocker::Partnership => "partnership",
    }
}

fn delete_outcome(result: &Result<(), DeletePartyError>) -> &'static str {
    match result {
        Ok(()) => "success",
        Err(DeletePartyError::AuthenticatedActorRequired) => "unauthenticated",
        Err(DeletePartyError::Forbidden) => "forbidden",
        Err(DeletePartyError::NotFound) => "not_found",
        Err(DeletePartyError::DependencyConflict { .. }) => "dependency_conflict",
        Err(DeletePartyError::ConcurrencyConflict) => "concurrency_conflict",
        Err(DeletePartyError::TemporarilyUnavailable { .. }) => "persistence_unavailable",
        Err(DeletePartyError::InvalidPersistedState { .. }) => "invalid_persisted_state",
        Err(DeletePartyError::Internal { .. }) => "internal_failure",
        Err(DeletePartyError::BeginTransactionFailed) => "begin_failed",
        Err(DeletePartyError::CommitTransactionFailed) => "commit_failed",
    }
}

async fn ensure_admin_or_internal<A>(
    context: &OperationContext,
    check_user_admin: &A,
) -> Result<(), DeletePartyError>
where
    A: CheckUserAdminUseCase,
{
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::Anonymous => Err(DeletePartyError::AuthenticatedActorRequired),
        Principal::User(_) | Principal::DelegatedUser { .. } => check_user_admin
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(|error| match error {
                CheckUserAdminError::AuthenticatedActorRequired => {
                    DeletePartyError::AuthenticatedActorRequired
                }
                CheckUserAdminError::Forbidden => DeletePartyError::Forbidden,
                CheckUserAdminError::TemporarilyUnavailable { source } => {
                    DeletePartyError::TemporarilyUnavailable { source }
                }
                CheckUserAdminError::InvalidReadModel { source }
                | CheckUserAdminError::Internal { source } => DeletePartyError::Internal { source },
                CheckUserAdminError::BeginTransactionFailed
                | CheckUserAdminError::CommitTransactionFailed => {
                    DeletePartyError::TemporarilyUnavailable {
                        source: static_error("check user admin transaction failed"),
                    }
                }
            }),
    }
}

impl From<PartyRepositoryError> for DeletePartyError {
    fn from(error: PartyRepositoryError) -> Self {
        match error {
            PartyRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            PartyRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartyRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartyRepositoryError::SlugConflict { source }
            | PartyRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}
