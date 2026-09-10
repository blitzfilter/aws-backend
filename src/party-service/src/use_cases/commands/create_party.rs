use crate::ports::{PartyRepository, PartyRepositoryError, PartyRepositoryFactory};
use application::error::{BoxError, static_error};
use application::operation_context::{OperationContext, Principal};
use application::transaction::{Transaction, UnitOfWork};
use party_core::{
    party::{NewParty, Party, PartyContact},
    party_id::PartyId,
    party_name::PartyName,
};

use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub struct CreatePartyCommand {
    pub name: PartyName,
    pub contact: PartyContact,
}

pub type CreatePartyResult = PartyDetailsView;

#[derive(Debug, thiserror::Error)]
pub enum CreatePartyError {
    #[error("authenticated actor required to create party")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("party slug already exists")]
    SlugConflict {
        #[source]
        source: BoxError,
    },
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
    #[error("internal party persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin create party transaction")]
    BeginTransactionFailed,
    #[error("failed to commit create party transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CreatePartyUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreatePartyCommand,
    ) -> Result<CreatePartyResult, CreatePartyError>;
}

pub struct CreatePartyHandler<U, R, A> {
    unit_of_work: U,
    parties: R,
    check_user_admin: A,
}

impl<U, R, A> CreatePartyHandler<U, R, A> {
    pub fn new(unit_of_work: U, parties: R, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            parties,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> CreatePartyUseCase for CreatePartyHandler<U, R, A>
where
    U: UnitOfWork,
    R: PartyRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "create_party",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreatePartyCommand,
    ) -> Result<CreatePartyResult, CreatePartyError> {
        ensure_admin_or_internal(context, &self.check_user_admin).await?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let party = Party::create(NewParty {
            id: PartyId::new(),
            name: command.name,
            contact: command.contact,
        });
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreatePartyError::BeginTransactionFailed)?;

        if self
            .parties
            .in_transaction(&mut tx)
            .find_by_slug(party.slug_id())
            .await?
            .is_some()
        {
            return Err(CreatePartyError::SlugConflict {
                source: static_error("party slug already exists"),
            });
        }

        let result = self
            .parties
            .in_transaction(&mut tx)
            .insert(&party)
            .await?
            .into();

        tx.commit()
            .await
            .map_err(|_| CreatePartyError::CommitTransactionFailed)?;

        tracing::info!(
            event = "party.created",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            party_id = %party.id(),
            party_slug_id = %party.slug_id(),
            outcome = "success",
        );

        Ok(result)
    }
}

async fn ensure_admin_or_internal<A>(
    context: &OperationContext,
    check_user_admin: &A,
) -> Result<(), CreatePartyError>
where
    A: CheckUserAdminUseCase,
{
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(_) | Principal::DelegatedUser { .. } => check_user_admin
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(map_admin_error),
        Principal::Anonymous => Err(CreatePartyError::AuthenticatedActorRequired),
    }
}

fn map_admin_error(error: CheckUserAdminError) -> CreatePartyError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            CreatePartyError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => CreatePartyError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            CreatePartyError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => CreatePartyError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            CreatePartyError::TemporarilyUnavailable {
                source: static_error("check user admin transaction failed"),
            }
        }
    }
}

impl From<PartyRepositoryError> for CreatePartyError {
    fn from(error: PartyRepositoryError) -> Self {
        match error {
            PartyRepositoryError::SlugConflict { source } => Self::SlugConflict { source },
            PartyRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartyRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartyRepositoryError::ConcurrencyConflict => Self::Internal {
                source: static_error("unexpected create party concurrency conflict"),
            },
            PartyRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

use crate::use_cases::queries::get_party::PartyDetailsView;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{PartyStorageVersion, StoredParty};
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::transaction::TransactionError;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use user_service::use_cases::queries::check_user_admin::CheckUserAdminResult;

    #[derive(Default)]
    struct State {
        inserts: usize,
        commits: usize,
    }

    #[derive(Clone, Default)]
    struct FakeUnitOfWork(Arc<Mutex<State>>);

    struct FakeTransaction(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = self.0.lock().map_err(|_| TransactionError::CommitFailed)?;
            state.commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(FakeTransaction(Arc::clone(&self.0)))
        }
    }

    #[derive(Clone, Default)]
    struct FakePartyRepositoryFactory(Arc<Mutex<State>>);

    struct FakePartyRepository(Arc<Mutex<State>>);

    impl PartyRepositoryFactory<FakeTransaction> for FakePartyRepositoryFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PartyRepository + 'tx {
            FakePartyRepository(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl PartyRepository for FakePartyRepository {
        async fn find_by_id(
            &mut self,
            _id: PartyId,
        ) -> Result<Option<StoredParty>, PartyRepositoryError> {
            Ok(None)
        }

        async fn find_by_slug(
            &mut self,
            _slug_id: &party_core::party_slug_id::PartySlugId,
        ) -> Result<Option<StoredParty>, PartyRepositoryError> {
            Ok(None)
        }

        async fn find_by_id_for_update(
            &mut self,
            _id: PartyId,
        ) -> Result<Option<StoredParty>, PartyRepositoryError> {
            Err(PartyRepositoryError::Internal {
                source: static_error("locked party lookup not expected"),
            })
        }

        async fn find_deletion_blocker(
            &mut self,
            _id: PartyId,
        ) -> Result<Option<crate::ports::PartyDeletionBlocker>, PartyRepositoryError> {
            Err(PartyRepositoryError::Internal {
                source: static_error("party deletion blocker lookup not expected"),
            })
        }

        async fn delete_unused(
            &mut self,
            _id: PartyId,
            _expected_version: PartyStorageVersion,
        ) -> Result<(), PartyRepositoryError> {
            Err(PartyRepositoryError::Internal {
                source: static_error("party deletion not expected"),
            })
        }

        async fn insert(&mut self, party: &Party) -> Result<StoredParty, PartyRepositoryError> {
            let mut state = self.0.lock().map_err(|_| PartyRepositoryError::Internal {
                source: static_error("fake party state poisoned"),
            })?;
            state.inserts += 1;
            Ok(StoredParty {
                party: party.clone(),
                version: PartyStorageVersion::INITIAL,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            })
        }

        async fn update(
            &mut self,
            _party: &Party,
            _expected_version: PartyStorageVersion,
        ) -> Result<StoredParty, PartyRepositoryError> {
            Err(PartyRepositoryError::Internal {
                source: static_error("update not expected"),
            })
        }
    }

    struct InternalAdmin;

    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for InternalAdmin {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            Ok(CheckUserAdminResult)
        }
    }

    #[tokio::test]
    async fn should_create_party_in_service_owned_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = CreatePartyHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakePartyRepositoryFactory(Arc::clone(&state)),
            InternalAdmin,
        );
        let context = OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        };

        let result = handler
            .execute(
                &context,
                CreatePartyCommand {
                    name: PartyName::try_from("Antik und Stil")
                        .unwrap_or_else(|error| panic!("invalid test party name: {error}")),
                    contact: PartyContact::default(),
                },
            )
            .await;

        assert!(matches!(
            result,
            Ok(PartyDetailsView { ref party_slug_id, party_id, .. })
                if party_slug_id.as_ref() == format!("antik-und-stil-{}", party_id.as_uuid())
        ));
        let state = match state.lock() {
            Ok(state) => state,
            Err(error) => panic!("fake state poisoned: {error}"),
        };
        assert_eq!(1, state.inserts);
        assert_eq!(1, state.commits);
    }
}
