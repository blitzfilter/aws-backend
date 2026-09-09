use crate::ports::{PartyRepository, PartyRepositoryError, PartyRepositoryFactory};
use crate::use_cases::queries::get_party::PartyDetailsView;
use application::error::{BoxError, static_error};
use application::operation_context::{OperationContext, Principal};
use application::patch_field::PatchField;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::change_outcome::ChangeOutcome;
use party_core::{party::PartyContact, party_id::PartyId, party_name::PartyName};
use serde_email::Email;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdatePartyCommand {
    pub party_id: PartyId,
    pub name: PatchField<PartyName>,
    pub phone: PatchField<String>,
    pub email: PatchField<Email>,
}

impl UpdatePartyCommand {
    pub fn is_empty(&self) -> bool {
        !self.name.is_changed() && !self.phone.is_changed() && !self.email.is_changed()
    }
}

pub type UpdatePartyResult = PartyDetailsView;

#[derive(Debug, thiserror::Error)]
pub enum UpdatePartyError {
    #[error("authenticated actor required to update party")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("party not found")]
    NotFound,
    #[error("concurrent party update")]
    ConcurrencyConflict,
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
    #[error("failed to begin update party transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update party transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdatePartyUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdatePartyCommand,
    ) -> Result<UpdatePartyResult, UpdatePartyError>;
}

pub struct UpdatePartyHandler<U, R, A> {
    unit_of_work: U,
    parties: R,
    check_user_admin: A,
}

impl<U, R, A> UpdatePartyHandler<U, R, A> {
    pub fn new(unit_of_work: U, parties: R, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            parties,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> UpdatePartyUseCase for UpdatePartyHandler<U, R, A>
where
    U: UnitOfWork,
    R: PartyRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "update_party",
        skip_all,
        fields(
            party_id = %command.party_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdatePartyCommand,
    ) -> Result<UpdatePartyResult, UpdatePartyError> {
        ensure_admin_or_internal(context, &self.check_user_admin).await?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdatePartyError::BeginTransactionFailed)?;
        let stored = self
            .parties
            .in_transaction(&mut tx)
            .find_by_id(command.party_id)
            .await?
            .ok_or(UpdatePartyError::NotFound)?;
        let mut party = stored.party;
        let outcome = apply_update(&mut party, command);

        let result = if outcome.changed() {
            self.parties
                .in_transaction(&mut tx)
                .update(&party, stored.version)
                .await?
                .into()
        } else {
            PartyDetailsView::from(crate::ports::StoredParty {
                party,
                version: stored.version,
                created: stored.created,
                updated: stored.updated,
            })
        };

        tx.commit()
            .await
            .map_err(|_| UpdatePartyError::CommitTransactionFailed)?;

        tracing::info!(
            event = "party.updated",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            party_id = %result.party_id,
            party_slug_id = %result.party_slug_id,
            changed = outcome.changed(),
            outcome = "success",
        );

        Ok(result)
    }
}

fn apply_update(
    party: &mut party_core::party::Party,
    command: UpdatePartyCommand,
) -> ChangeOutcome {
    let mut outcome = ChangeOutcome::Unchanged;
    if let PatchField::Set(name) = command.name {
        outcome = outcome.combine(party.rename(name));
    }

    if command.phone.is_changed() || command.email.is_changed() {
        let contact = PartyContact {
            phone: patch_option(party.contact().phone.clone(), command.phone),
            email: patch_option(party.contact().email.clone(), command.email),
        };
        outcome = outcome.combine(party.replace_contact(contact));
    }

    outcome
}

fn patch_option<T>(current: Option<T>, patch: PatchField<T>) -> Option<T> {
    match patch {
        PatchField::Unchanged => current,
        PatchField::Set(value) => Some(value),
        PatchField::Clear => None,
    }
}

async fn ensure_admin_or_internal<A>(
    context: &OperationContext,
    check_user_admin: &A,
) -> Result<(), UpdatePartyError>
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
        Principal::Anonymous => Err(UpdatePartyError::AuthenticatedActorRequired),
    }
}

fn map_admin_error(error: CheckUserAdminError) -> UpdatePartyError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            UpdatePartyError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => UpdatePartyError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            UpdatePartyError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => UpdatePartyError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            UpdatePartyError::TemporarilyUnavailable {
                source: static_error("check user admin transaction failed"),
            }
        }
    }
}

impl From<PartyRepositoryError> for UpdatePartyError {
    fn from(error: PartyRepositoryError) -> Self {
        match error {
            PartyRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            PartyRepositoryError::SlugConflict { source } => Self::SlugConflict { source },
            PartyRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartyRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartyRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{PartyStorageVersion, StoredParty};
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::transaction::TransactionError;
    use party_core::party::{NewParty, Party};
    use serde_email::Email;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use user_core::user_id::UserId;
    use user_service::use_cases::queries::check_user_admin::{
        CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminResult,
    };

    #[derive(Default)]
    struct State {
        party: Option<StoredParty>,
        updates: usize,
        commits: usize,
        reject_update: bool,
    }

    #[derive(Clone)]
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

    #[derive(Clone)]
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
            let state = self.0.lock().map_err(|_| PartyRepositoryError::Internal {
                source: static_error("fake Party state poisoned"),
            })?;
            Ok(state.party.clone())
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

        async fn insert(&mut self, _party: &Party) -> Result<StoredParty, PartyRepositoryError> {
            Err(PartyRepositoryError::Internal {
                source: static_error("insert not expected"),
            })
        }

        async fn update(
            &mut self,
            party: &Party,
            expected_version: PartyStorageVersion,
        ) -> Result<StoredParty, PartyRepositoryError> {
            let mut state = self.0.lock().map_err(|_| PartyRepositoryError::Internal {
                source: static_error("fake Party state poisoned"),
            })?;
            if state.reject_update {
                return Err(PartyRepositoryError::ConcurrencyConflict);
            }
            let previous = state.party.clone().ok_or(PartyRepositoryError::Internal {
                source: static_error("fake Party was not seeded"),
            })?;
            let stored = StoredParty {
                party: party.clone(),
                version: expected_version.next(),
                created: previous.created,
                updated: previous.updated,
            };
            state.updates += 1;
            state.party = Some(stored.clone());
            Ok(stored)
        }
    }

    #[derive(Clone, Copy)]
    struct FakeAdmin {
        allowed: bool,
    }

    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for FakeAdmin {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            if self.allowed {
                Ok(CheckUserAdminResult)
            } else {
                Err(CheckUserAdminError::Forbidden)
            }
        }
    }

    fn handler(
        state: Arc<Mutex<State>>,
        allowed: bool,
    ) -> UpdatePartyHandler<FakeUnitOfWork, FakePartyRepositoryFactory, FakeAdmin> {
        UpdatePartyHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakePartyRepositoryFactory(state),
            FakeAdmin { allowed },
        )
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn party_name(value: &str) -> PartyName {
        PartyName::try_from(value)
            .unwrap_or_else(|error| panic!("invalid test Party name: {error}"))
    }

    fn party_email(value: &str) -> Email {
        Email::try_from(value).unwrap_or_else(|error| panic!("invalid test Party email: {error}"))
    }

    fn seeded_state() -> Arc<Mutex<State>> {
        let party = Party::create(NewParty {
            id: PartyId::new(),
            name: party_name("Original Party"),
            contact: PartyContact {
                phone: Some("+49 30 111111".to_owned()),
                email: Some(party_email("original@example.com")),
            },
        });
        Arc::new(Mutex::new(State {
            party: Some(StoredParty {
                party,
                version: PartyStorageVersion::INITIAL,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            }),
            ..Default::default()
        }))
    }

    fn seeded_party_id(state: &Arc<Mutex<State>>) -> PartyId {
        match state.lock() {
            Ok(state) => match state.party.as_ref() {
                Some(stored) => stored.party.id(),
                None => panic!("fake Party was not seeded"),
            },
            Err(error) => panic!("fake Party state poisoned: {error}"),
        }
    }

    #[tokio::test]
    async fn should_rename_party_without_changing_its_slug() {
        let state = seeded_state();
        let party_id = seeded_party_id(&state);
        let original_slug = match state.lock() {
            Ok(state) => match state.party.as_ref() {
                Some(stored) => stored.party.slug_id().clone(),
                None => panic!("fake Party was not seeded"),
            },
            Err(error) => panic!("fake Party state poisoned: {error}"),
        };

        let result = handler(Arc::clone(&state), true)
            .execute(
                &context(Principal::User(UserId::new())),
                UpdatePartyCommand {
                    party_id,
                    name: PatchField::Set(party_name("Renamed Party")),
                    phone: PatchField::Unchanged,
                    email: PatchField::Unchanged,
                },
            )
            .await;

        let result = match result {
            Ok(result) => result,
            Err(error) => panic!("failed to rename Party: {error}"),
        };
        assert_eq!(original_slug, result.party_slug_id);
        assert_eq!("Renamed Party", result.name.as_ref());
        let updates = match state.lock() {
            Ok(state) => state.updates,
            Err(error) => panic!("fake Party state poisoned: {error}"),
        };
        assert_eq!(1, updates);
    }

    #[tokio::test]
    async fn should_set_and_clear_optional_party_contact_fields() {
        let state = seeded_state();
        let party_id = seeded_party_id(&state);
        let handler = handler(Arc::clone(&state), true);
        let context = context(Principal::User(UserId::new()));

        let set_result = handler
            .execute(
                &context,
                UpdatePartyCommand {
                    party_id,
                    name: PatchField::Unchanged,
                    phone: PatchField::Set("+49 30 222222".to_owned()),
                    email: PatchField::Set(party_email("updated@example.com")),
                },
            )
            .await;
        assert!(set_result.is_ok());

        let clear_result = handler
            .execute(
                &context,
                UpdatePartyCommand {
                    party_id,
                    name: PatchField::Unchanged,
                    phone: PatchField::Clear,
                    email: PatchField::Clear,
                },
            )
            .await;
        let result = match clear_result {
            Ok(result) => result,
            Err(error) => panic!("failed to clear Party contact: {error}"),
        };
        assert!(result.contact.phone.is_none());
        assert!(result.contact.email.is_none());
        let updates = match state.lock() {
            Ok(state) => state.updates,
            Err(error) => panic!("fake Party state poisoned: {error}"),
        };
        assert_eq!(2, updates);
    }

    #[tokio::test]
    async fn should_skip_party_persistence_for_no_op_patch() {
        let state = seeded_state();
        let party_id = seeded_party_id(&state);

        let result = handler(Arc::clone(&state), true)
            .execute(
                &context(Principal::User(UserId::new())),
                UpdatePartyCommand {
                    party_id,
                    ..Default::default()
                },
            )
            .await;

        assert!(result.is_ok());
        let (updates, commits) = match state.lock() {
            Ok(state) => (state.updates, state.commits),
            Err(error) => panic!("fake Party state poisoned: {error}"),
        };
        assert_eq!(0, updates);
        assert_eq!(1, commits);
    }

    #[tokio::test]
    async fn should_map_stale_party_update_to_concurrency_conflict() {
        let state = seeded_state();
        match state.lock() {
            Ok(mut state) => state.reject_update = true,
            Err(error) => panic!("fake Party state poisoned: {error}"),
        }
        let party_id = seeded_party_id(&state);

        let result = handler(Arc::clone(&state), true)
            .execute(
                &context(Principal::User(UserId::new())),
                UpdatePartyCommand {
                    party_id,
                    name: PatchField::Set(party_name("Concurrent Party")),
                    ..Default::default()
                },
            )
            .await;

        assert!(matches!(result, Err(UpdatePartyError::ConcurrencyConflict)));
        let commits = match state.lock() {
            Ok(state) => state.commits,
            Err(error) => panic!("fake Party state poisoned: {error}"),
        };
        assert_eq!(0, commits);
    }

    #[tokio::test]
    async fn should_reject_non_admin_party_update_before_persistence() {
        let state = seeded_state();
        let party_id = seeded_party_id(&state);

        let result = handler(Arc::clone(&state), false)
            .execute(
                &context(Principal::User(UserId::new())),
                UpdatePartyCommand {
                    party_id,
                    name: PatchField::Set(party_name("Rejected Party")),
                    ..Default::default()
                },
            )
            .await;

        assert!(matches!(result, Err(UpdatePartyError::Forbidden)));
        let (updates, commits) = match state.lock() {
            Ok(state) => (state.updates, state.commits),
            Err(error) => panic!("fake Party state poisoned: {error}"),
        };
        assert_eq!(0, commits);
        assert_eq!(0, updates);
    }
}
