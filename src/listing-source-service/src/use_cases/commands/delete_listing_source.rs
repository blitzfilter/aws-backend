use crate::ports::{
    ListingSourceDeletionBlocker, ListingSourceRepository, ListingSourceRepositoryError,
    ListingSourceRepositoryFactory,
};
use application::{
    error::{BoxError, static_error},
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use listing_source_core::ListingSourceId;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteListingSourceCommand {
    pub listing_source_id: ListingSourceId,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteListingSourceError {
    #[error("authenticated actor required to delete listing source")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("listing source not found")]
    NotFound,
    #[error("listing source has protected dependencies")]
    DependencyConflict {
        blocker: ListingSourceDeletionBlocker,
    },
    #[error("concurrent listing source mutation")]
    ConcurrencyConflict,
    #[error("temporary listing source persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted listing source state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal listing source failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin delete listing source transaction")]
    BeginTransactionFailed,
    #[error("failed to commit delete listing source transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait DeleteListingSourceUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteListingSourceCommand,
    ) -> Result<(), DeleteListingSourceError>;
}

pub struct DeleteListingSourceHandler<U, S, A> {
    unit_of_work: U,
    sources: S,
    check_user_admin: A,
}

impl<U, S, A> DeleteListingSourceHandler<U, S, A> {
    pub fn new(unit_of_work: U, sources: S, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            sources,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, A> DeleteListingSourceUseCase for DeleteListingSourceHandler<U, S, A>
where
    U: UnitOfWork,
    S: ListingSourceRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "delete_listing_source",
        skip_all,
        fields(
            action = "delete_listing_source",
            listing_source_id = %command.listing_source_id,
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
        command: DeleteListingSourceCommand,
    ) -> Result<(), DeleteListingSourceError> {
        let result = async {
            ensure_admin(context, &self.check_user_admin).await?;

            let mut tx = self
                .unit_of_work
                .begin()
                .await
                .map_err(|_| DeleteListingSourceError::BeginTransactionFailed)?;
            let stored = self
                .sources
                .in_transaction(&mut tx)
                .find_by_id_for_update(command.listing_source_id)
                .await?
                .ok_or(DeleteListingSourceError::NotFound)?;
            if let Some(blocker) = self
                .sources
                .in_transaction(&mut tx)
                .find_deletion_blocker(command.listing_source_id)
                .await?
            {
                tracing::warn!(
                    action = "delete_listing_source",
                    listing_source_id = %command.listing_source_id,
                    blocker = blocker_name(blocker),
                    outcome = "dependency_conflict",
                    "listing source deletion rejected by protected dependency"
                );
                return Err(DeleteListingSourceError::DependencyConflict { blocker });
            }
            self.sources
                .in_transaction(&mut tx)
                .delete_unused(command.listing_source_id, stored.version)
                .await?;
            tx.commit()
                .await
                .map_err(|_| DeleteListingSourceError::CommitTransactionFailed)?;
            Ok(())
        }
        .await;

        let outcome = delete_outcome(&result);
        tracing::Span::current().record("outcome", outcome);
        if result.is_ok() {
            tracing::info!(
                action = "delete_listing_source",
                listing_source_id = %command.listing_source_id,
                actor_type = context.principal.kind(),
                actor_id = %context.principal.label(),
                request_id = %context.request_id,
                correlation_id = %context.correlation_id,
                changed = true,
                outcome,
                "listing source deleted"
            );
        }
        result
    }
}

fn delete_outcome(result: &Result<(), DeleteListingSourceError>) -> &'static str {
    match result {
        Ok(()) => "success",
        Err(DeleteListingSourceError::AuthenticatedActorRequired) => "unauthenticated",
        Err(DeleteListingSourceError::Forbidden) => "forbidden",
        Err(DeleteListingSourceError::NotFound) => "not_found",
        Err(DeleteListingSourceError::DependencyConflict { .. }) => "dependency_conflict",
        Err(DeleteListingSourceError::ConcurrencyConflict) => "concurrency_conflict",
        Err(DeleteListingSourceError::BeginTransactionFailed) => "begin_failed",
        Err(DeleteListingSourceError::CommitTransactionFailed) => "commit_failed",
        Err(DeleteListingSourceError::TemporarilyUnavailable { .. }) => "persistence_unavailable",
        Err(DeleteListingSourceError::InvalidPersistedState { .. }) => "invalid_persisted_state",
        Err(DeleteListingSourceError::Internal { .. }) => "internal_failure",
    }
}

fn blocker_name(blocker: ListingSourceDeletionBlocker) -> &'static str {
    match blocker {
        ListingSourceDeletionBlocker::Auctions => "auctions",
        ListingSourceDeletionBlocker::ProductListings => "product_listings",
        ListingSourceDeletionBlocker::RawStreams => "raw_streams",
        ListingSourceDeletionBlocker::ApprovedPartnershipApplication => "approved_application",
        ListingSourceDeletionBlocker::ExistingSourcePartnershipApplication => {
            "existing_source_application"
        }
    }
}

async fn ensure_admin<A>(
    context: &OperationContext,
    check: &A,
) -> Result<(), DeleteListingSourceError>
where
    A: CheckUserAdminUseCase,
{
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::Anonymous => Err(DeleteListingSourceError::AuthenticatedActorRequired),
        Principal::User(_) | Principal::DelegatedUser { .. } => check
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(|error| match error {
                CheckUserAdminError::AuthenticatedActorRequired => {
                    DeleteListingSourceError::AuthenticatedActorRequired
                }
                CheckUserAdminError::Forbidden => DeleteListingSourceError::Forbidden,
                CheckUserAdminError::TemporarilyUnavailable { source } => {
                    DeleteListingSourceError::TemporarilyUnavailable { source }
                }
                CheckUserAdminError::InvalidReadModel { source }
                | CheckUserAdminError::Internal { source } => {
                    DeleteListingSourceError::Internal { source }
                }
                CheckUserAdminError::BeginTransactionFailed
                | CheckUserAdminError::CommitTransactionFailed => {
                    DeleteListingSourceError::TemporarilyUnavailable {
                        source: static_error("check user admin transaction failed"),
                    }
                }
            }),
    }
}

impl From<ListingSourceRepositoryError> for DeleteListingSourceError {
    fn from(error: ListingSourceRepositoryError) -> Self {
        match error {
            ListingSourceRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ListingSourceRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ListingSourceRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            ListingSourceRepositoryError::SlugConflict { source }
            | ListingSourceRepositoryError::ShopifyDomainConflict { source }
            | ListingSourceRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ListingIngestionConfiguration, ListingSourceIngestionConfigurations,
        ListingSourceStorageVersion, StoredListingSource,
    };
    use application::{
        operation_context::{CorrelationId, RequestId},
        transaction::{Transaction, TransactionError},
    };
    use listing_source_core::{
        ListingIngestionMethod, ListingSource, ListingSourceName, ListingSourcePresentation,
        NewListingSource,
    };
    use party_core::party_id::PartyId;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use user_service::use_cases::queries::check_user_admin::CheckUserAdminResult;

    #[derive(Default)]
    struct State {
        begins: usize,
        locks: usize,
        blocker_reads: usize,
        deletes: usize,
        commits: usize,
        fail_begin: bool,
        fail_commit: bool,
    }

    #[derive(Clone, Copy, Default)]
    enum SourceOutcome {
        #[default]
        Found,
        Missing,
        Blocked(ListingSourceDeletionBlocker),
        LockFailure,
        BlockerReadFailure,
        DeleteFailure,
    }

    #[derive(Clone)]
    struct Uow(Arc<Mutex<State>>);
    struct Tx(Arc<Mutex<State>>);
    #[async_trait::async_trait]
    impl Transaction for Tx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = self.0.lock().map_err(|_| TransactionError::CommitFailed)?;
            if state.fail_commit {
                return Err(TransactionError::CommitFailed);
            }
            state.commits += 1;
            Ok(())
        }
    }
    #[async_trait::async_trait]
    impl UnitOfWork for Uow {
        type Tx = Tx;
        async fn begin(&self) -> Result<Tx, TransactionError> {
            let mut state = self.0.lock().map_err(|_| TransactionError::BeginFailed)?;
            if state.fail_begin {
                return Err(TransactionError::BeginFailed);
            }
            state.begins += 1;
            Ok(Tx(Arc::clone(&self.0)))
        }
    }
    #[derive(Clone)]
    struct Sources {
        state: Arc<Mutex<State>>,
        source: StoredListingSource,
        outcome: SourceOutcome,
    }
    struct Repository<'a> {
        state: Arc<Mutex<State>>,
        source: StoredListingSource,
        outcome: SourceOutcome,
        _tx: &'a mut Tx,
    }
    impl ListingSourceRepositoryFactory<Tx> for Sources {
        fn in_transaction<'a>(&'a self, tx: &'a mut Tx) -> impl ListingSourceRepository + 'a {
            Repository {
                state: Arc::clone(&self.state),
                source: self.source.clone(),
                outcome: self.outcome,
                _tx: tx,
            }
        }
    }
    #[async_trait::async_trait]
    impl ListingSourceRepository for Repository<'_> {
        async fn find_by_id(
            &mut self,
            _: ListingSourceId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Ok(Some(self.source.clone()))
        }
        async fn find_by_slug(
            &mut self,
            _: &listing_source_core::ListingSourceSlugId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Ok(None)
        }
        async fn insert(
            &mut self,
            _: &ListingSource,
            _: &ListingSourceIngestionConfigurations,
            _: Option<&str>,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            Err(failure())
        }
        async fn update(
            &mut self,
            _: &ListingSource,
            _: &ListingSourceIngestionConfigurations,
            _: application::patch_field::PatchField<&str>,
            _: ListingSourceStorageVersion,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            Err(failure())
        }
        async fn find_by_id_for_update(
            &mut self,
            _: ListingSourceId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            self.state.lock().map_err(|_| failure())?.locks += 1;
            match self.outcome {
                SourceOutcome::Missing => Ok(None),
                SourceOutcome::LockFailure => Err(failure()),
                _ => Ok(Some(self.source.clone())),
            }
        }
        async fn find_deletion_blocker(
            &mut self,
            _: ListingSourceId,
        ) -> Result<Option<ListingSourceDeletionBlocker>, ListingSourceRepositoryError> {
            self.state.lock().map_err(|_| failure())?.blocker_reads += 1;
            match self.outcome {
                SourceOutcome::Blocked(blocker) => Ok(Some(blocker)),
                SourceOutcome::BlockerReadFailure => Err(failure()),
                _ => Ok(None),
            }
        }
        async fn delete_unused(
            &mut self,
            _: ListingSourceId,
            _: ListingSourceStorageVersion,
        ) -> Result<(), ListingSourceRepositoryError> {
            self.state.lock().map_err(|_| failure())?.deletes += 1;
            match self.outcome {
                SourceOutcome::DeleteFailure => Err(failure()),
                _ => Ok(()),
            }
        }
    }
    struct Admin {
        allowed: bool,
    }
    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for Admin {
        async fn execute(
            &self,
            _: &OperationContext,
            _: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            if self.allowed {
                Ok(CheckUserAdminResult)
            } else {
                Err(CheckUserAdminError::Forbidden)
            }
        }
    }

    fn sources(
        state: Arc<Mutex<State>>,
        source: StoredListingSource,
        outcome: SourceOutcome,
    ) -> Sources {
        Sources {
            state,
            source,
            outcome,
        }
    }
    fn stored() -> StoredListingSource {
        let source = ListingSource::create(NewListingSource {
            id: ListingSourceId::new(),
            name: ListingSourceName::try_from("Source")
                .unwrap_or_else(|error| panic!("invalid test source: {error}")),
            operator_party_id: PartyId::new(),
            ingestion_methods: std::collections::HashSet::from([ListingIngestionMethod::WebCrawl]),
            presentation: ListingSourcePresentation::default(),
            referral_configuration: None,
        });
        StoredListingSource {
            source,
            configuration: ListingSourceIngestionConfigurations(vec![
                ListingIngestionConfiguration::WebCrawl {
                    fallback_currency: None,
                },
            ]),
            version: ListingSourceStorageVersion::INITIAL,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }
    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }
    fn failure() -> ListingSourceRepositoryError {
        ListingSourceRepositoryError::Internal {
            source: static_error("fake failure"),
        }
    }

    #[test]
    fn should_classify_every_terminal_delete_outcome() {
        let cases = [
            (Ok(()), "success"),
            (
                Err(DeleteListingSourceError::AuthenticatedActorRequired),
                "unauthenticated",
            ),
            (Err(DeleteListingSourceError::Forbidden), "forbidden"),
            (Err(DeleteListingSourceError::NotFound), "not_found"),
            (
                Err(DeleteListingSourceError::DependencyConflict {
                    blocker: ListingSourceDeletionBlocker::ProductListings,
                }),
                "dependency_conflict",
            ),
            (
                Err(DeleteListingSourceError::ConcurrencyConflict),
                "concurrency_conflict",
            ),
            (
                Err(DeleteListingSourceError::BeginTransactionFailed),
                "begin_failed",
            ),
            (
                Err(DeleteListingSourceError::CommitTransactionFailed),
                "commit_failed",
            ),
            (
                Err(DeleteListingSourceError::TemporarilyUnavailable {
                    source: static_error("temporary"),
                }),
                "persistence_unavailable",
            ),
            (
                Err(DeleteListingSourceError::InvalidPersistedState {
                    source: static_error("invalid"),
                }),
                "invalid_persisted_state",
            ),
            (
                Err(DeleteListingSourceError::Internal {
                    source: static_error("internal"),
                }),
                "internal_failure",
            ),
        ];

        for (result, expected) in cases {
            assert_eq!(expected, delete_outcome(&result));
        }
    }

    #[tokio::test]
    async fn should_delete_unused_source_in_one_committed_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let source = stored();
        let handler = DeleteListingSourceHandler::new(
            Uow(Arc::clone(&state)),
            sources(Arc::clone(&state), source.clone(), SourceOutcome::Found),
            Admin { allowed: true },
        );
        let result = handler
            .execute(
                &context(Principal::System),
                DeleteListingSourceCommand {
                    listing_source_id: source.source.id(),
                },
            )
            .await;
        assert!(result.is_ok());
        let state = match state.lock() {
            Ok(state) => state,
            Err(_) => panic!("poisoned state"),
        };
        assert_eq!(
            (
                state.begins,
                state.locks,
                state.blocker_reads,
                state.deletes,
                state.commits
            ),
            (1, 1, 1, 1, 1)
        );
    }

    #[tokio::test]
    async fn should_reject_anonymous_actor_before_beginning_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let source = stored();
        let handler = DeleteListingSourceHandler::new(
            Uow(Arc::clone(&state)),
            sources(Arc::clone(&state), source.clone(), SourceOutcome::Found),
            Admin { allowed: true },
        );
        let result = handler
            .execute(
                &context(Principal::Anonymous),
                DeleteListingSourceCommand {
                    listing_source_id: source.source.id(),
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(DeleteListingSourceError::AuthenticatedActorRequired)
        ));
        let state = match state.lock() {
            Ok(state) => state,
            Err(_) => panic!("poisoned state"),
        };
        assert_eq!(state.begins, 0);
    }

    #[tokio::test]
    async fn should_reject_non_admin_before_source_lookup_or_write() {
        let state = Arc::new(Mutex::new(State::default()));
        let source = stored();
        let handler = DeleteListingSourceHandler::new(
            Uow(Arc::clone(&state)),
            sources(Arc::clone(&state), source.clone(), SourceOutcome::Found),
            Admin { allowed: false },
        );

        let result = handler
            .execute(
                &context(Principal::User(user_core::user_id::UserId::new())),
                DeleteListingSourceCommand {
                    listing_source_id: source.source.id(),
                },
            )
            .await;

        assert!(matches!(result, Err(DeleteListingSourceError::Forbidden)));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(
            (
                state.begins,
                state.locks,
                state.blocker_reads,
                state.deletes,
                state.commits
            ),
            (0, 0, 0, 0, 0)
        );
    }

    #[tokio::test]
    async fn should_preserve_source_and_owned_rows_when_a_dependency_blocks_delete() {
        for blocker in [
            ListingSourceDeletionBlocker::Auctions,
            ListingSourceDeletionBlocker::ProductListings,
            ListingSourceDeletionBlocker::RawStreams,
            ListingSourceDeletionBlocker::ApprovedPartnershipApplication,
            ListingSourceDeletionBlocker::ExistingSourcePartnershipApplication,
        ] {
            let state = Arc::new(Mutex::new(State::default()));
            let source = stored();
            let handler = DeleteListingSourceHandler::new(
                Uow(Arc::clone(&state)),
                sources(
                    Arc::clone(&state),
                    source.clone(),
                    SourceOutcome::Blocked(blocker),
                ),
                Admin { allowed: true },
            );

            let result = handler
                .execute(
                    &context(Principal::System),
                    DeleteListingSourceCommand {
                        listing_source_id: source.source.id(),
                    },
                )
                .await;

            assert!(matches!(
                result,
                Err(DeleteListingSourceError::DependencyConflict { blocker: actual }) if actual == blocker
            ));
            let state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(
                (
                    state.begins,
                    state.locks,
                    state.blocker_reads,
                    state.deletes,
                    state.commits
                ),
                (1, 1, 1, 0, 0),
                "blocker {blocker:?}"
            );
        }
    }

    #[tokio::test]
    async fn should_not_cleanup_or_commit_when_source_is_missing_or_repository_work_fails() {
        for (outcome, expected_locks, expected_blocker_reads, expected_deletes) in [
            (SourceOutcome::Missing, 1, 0, 0),
            (SourceOutcome::LockFailure, 1, 0, 0),
            (SourceOutcome::BlockerReadFailure, 1, 1, 0),
            (SourceOutcome::DeleteFailure, 1, 1, 1),
        ] {
            let state = Arc::new(Mutex::new(State::default()));
            let source = stored();
            let handler = DeleteListingSourceHandler::new(
                Uow(Arc::clone(&state)),
                sources(Arc::clone(&state), source.clone(), outcome),
                Admin { allowed: true },
            );

            let result = handler
                .execute(
                    &context(Principal::System),
                    DeleteListingSourceCommand {
                        listing_source_id: source.source.id(),
                    },
                )
                .await;

            if matches!(outcome, SourceOutcome::Missing) {
                assert!(matches!(result, Err(DeleteListingSourceError::NotFound)));
            } else {
                assert!(matches!(
                    result,
                    Err(DeleteListingSourceError::Internal { .. })
                ));
            }
            let state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(
                (
                    state.locks,
                    state.blocker_reads,
                    state.deletes,
                    state.commits
                ),
                (expected_locks, expected_blocker_reads, expected_deletes, 0)
            );
        }
    }

    #[tokio::test]
    async fn should_report_transaction_failures_without_success() {
        let begin_state = Arc::new(Mutex::new(State {
            fail_begin: true,
            ..State::default()
        }));
        let source = stored();
        let begin_handler = DeleteListingSourceHandler::new(
            Uow(Arc::clone(&begin_state)),
            sources(
                Arc::clone(&begin_state),
                source.clone(),
                SourceOutcome::Found,
            ),
            Admin { allowed: true },
        );
        let begin_result = begin_handler
            .execute(
                &context(Principal::System),
                DeleteListingSourceCommand {
                    listing_source_id: source.source.id(),
                },
            )
            .await;
        assert!(matches!(
            begin_result,
            Err(DeleteListingSourceError::BeginTransactionFailed)
        ));
        let begin_counts = {
            let begin_state = begin_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                begin_state.locks,
                begin_state.blocker_reads,
                begin_state.deletes,
                begin_state.commits,
            )
        };
        assert_eq!(begin_counts, (0, 0, 0, 0));

        let commit_state = Arc::new(Mutex::new(State {
            fail_commit: true,
            ..State::default()
        }));
        let commit_handler = DeleteListingSourceHandler::new(
            Uow(Arc::clone(&commit_state)),
            sources(
                Arc::clone(&commit_state),
                source.clone(),
                SourceOutcome::Found,
            ),
            Admin { allowed: true },
        );
        let commit_result = commit_handler
            .execute(
                &context(Principal::System),
                DeleteListingSourceCommand {
                    listing_source_id: source.source.id(),
                },
            )
            .await;
        assert!(matches!(
            commit_result,
            Err(DeleteListingSourceError::CommitTransactionFailed)
        ));
        let commit_state = commit_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(
            (
                commit_state.locks,
                commit_state.blocker_reads,
                commit_state.deletes,
                commit_state.commits
            ),
            (1, 1, 1, 0)
        );
    }
}
