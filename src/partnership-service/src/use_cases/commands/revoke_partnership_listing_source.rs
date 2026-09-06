use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::*,
};
use application::{
    error::{BoxError, static_error},
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use listing_source_core::ListingSourceId;
use listing_source_service::ports::ListingSourceRepositoryError;
use partnership_core::partnership_id::PartnershipId;
use user_service::ports::UserAdminReaderFactory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevokePartnershipListingSourceCommand {
    pub partnership_id: PartnershipId,
    pub listing_source_id: ListingSourceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokePartnershipListingSourceOutcome {
    Removed,
    AlreadyAbsent,
}

impl RevokePartnershipListingSourceOutcome {
    pub fn changed(self) -> bool {
        matches!(self, Self::Removed)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::AlreadyAbsent => "already_absent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevokePartnershipListingSourceResult {
    pub outcome: RevokePartnershipListingSourceOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum RevokePartnershipListingSourceError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("partnership not found")]
    PartnershipNotFound,
    #[error("listing source not found")]
    ListingSourceNotFound,
    #[error("temporary Partnership ListingSource grant revocation failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted Partnership ListingSource grant revocation state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal Partnership ListingSource grant revocation failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin transaction")]
    BeginTransactionFailed,
    #[error("failed to commit transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait RevokePartnershipListingSourceUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: RevokePartnershipListingSourceCommand,
    ) -> Result<RevokePartnershipListingSourceResult, RevokePartnershipListingSourceError>;
}

pub struct RevokePartnershipListingSourceHandler<U, P, S, G, A> {
    unit_of_work: U,
    partnerships: P,
    sources: S,
    grants: G,
    admins: A,
}

impl<U, P, S, G, A> RevokePartnershipListingSourceHandler<U, P, S, G, A> {
    pub fn new(unit_of_work: U, partnerships: P, sources: S, grants: G, admins: A) -> Self {
        Self {
            unit_of_work,
            partnerships,
            sources,
            grants,
            admins,
        }
    }
}

#[async_trait::async_trait]
impl<U, P, S, G, A> RevokePartnershipListingSourceUseCase
    for RevokePartnershipListingSourceHandler<U, P, S, G, A>
where
    U: UnitOfWork,
    P: PartnershipRepositoryFactory<U::Tx>,
    S: ListingSourceRepositoryFactory<U::Tx>,
    G: ListingSourceGrantRepositoryFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "revoke_partnership_listing_source",
        skip_all,
        fields(
            partnership_id = %command.partnership_id,
            listing_source_id = %command.listing_source_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            changed = tracing::field::Empty,
            grant_outcome = tracing::field::Empty,
            outcome = tracing::field::Empty,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: RevokePartnershipListingSourceCommand,
    ) -> Result<RevokePartnershipListingSourceResult, RevokePartnershipListingSourceError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let result = async {
            let mut tx = self
                .unit_of_work
                .begin()
                .await
                .map_err(|_| RevokePartnershipListingSourceError::BeginTransactionFailed)?;

            authorize_admin(context, &mut tx, &self.admins).await?;

            self.partnerships
                .in_transaction(&mut tx)
                .find_by_id(command.partnership_id)
                .await?
                .ok_or(RevokePartnershipListingSourceError::PartnershipNotFound)?;

            self.sources
                .in_transaction(&mut tx)
                .find_by_id(command.listing_source_id)
                .await?
                .ok_or(RevokePartnershipListingSourceError::ListingSourceNotFound)?;

            let outcome = self
                .grants
                .in_transaction(&mut tx)
                .remove_source_access(command.partnership_id, command.listing_source_id)
                .await?;

            tx.commit()
                .await
                .map_err(|_| RevokePartnershipListingSourceError::CommitTransactionFailed)?;

            Ok(RevokePartnershipListingSourceResult {
                outcome: match outcome {
                    ListingSourceGrantRemoveOutcome::Removed => {
                        RevokePartnershipListingSourceOutcome::Removed
                    }
                    ListingSourceGrantRemoveOutcome::AlreadyAbsent => {
                        RevokePartnershipListingSourceOutcome::AlreadyAbsent
                    }
                },
            })
        }
        .await;

        let actor_id = context.principal.actor_id();
        match &result {
            Ok(result) => {
                let changed = result.outcome.changed();
                let grant_outcome = result.outcome.as_str();
                tracing::Span::current().record("changed", changed);
                tracing::Span::current().record("grant_outcome", grant_outcome);
                tracing::Span::current().record("outcome", "success");
                tracing::info!(
                    event = "partnership.listing_source_grant.revoked",
                    action = "revoke_partnership_listing_source",
                    actor_type = context.principal.kind(),
                    actor_id = actor_id.as_deref().unwrap_or(""),
                    target_type = "partnership_listing_source_grant",
                    partnership_id = %command.partnership_id,
                    listing_source_id = %command.listing_source_id,
                    changed,
                    grant_outcome,
                    request_id = %context.request_id,
                    correlation_id = %context.correlation_id,
                    outcome = "success",
                );
            }
            Err(error) => {
                tracing::Span::current().record("changed", "unknown");
                tracing::Span::current().record("grant_outcome", "unknown");
                tracing::Span::current().record("outcome", "failure");
                tracing::warn!(
                    event = "partnership.listing_source_grant.revoked",
                    action = "revoke_partnership_listing_source",
                    actor_type = context.principal.kind(),
                    actor_id = actor_id.as_deref().unwrap_or(""),
                    target_type = "partnership_listing_source_grant",
                    partnership_id = %command.partnership_id,
                    listing_source_id = %command.listing_source_id,
                    changed = "unknown",
                    grant_outcome = "unknown",
                    request_id = %context.request_id,
                    correlation_id = %context.correlation_id,
                    error_category = %error,
                    outcome = "failure",
                );
            }
        }

        result
    }
}

impl From<AdminAuthorizationError> for RevokePartnershipListingSourceError {
    fn from(value: AdminAuthorizationError) -> Self {
        match value {
            AdminAuthorizationError::Forbidden => Self::Forbidden,
            AdminAuthorizationError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AdminAuthorizationError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            AdminAuthorizationError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<PartnershipRepositoryError> for RevokePartnershipListingSourceError {
    fn from(value: PartnershipRepositoryError) -> Self {
        match value {
            PartnershipRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnershipRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<ListingSourceRepositoryError> for RevokePartnershipListingSourceError {
    fn from(value: ListingSourceRepositoryError) -> Self {
        match value {
            ListingSourceRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ListingSourceRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            ListingSourceRepositoryError::Internal { source } => Self::Internal { source },
            ListingSourceRepositoryError::ConcurrencyConflict => Self::Internal {
                source: static_error("unexpected listing source concurrency"),
            },
            ListingSourceRepositoryError::SlugConflict { source }
            | ListingSourceRepositoryError::ShopifyDomainConflict { source } => {
                Self::Internal { source }
            }
        }
    }
}

impl From<PartnershipGrantError> for RevokePartnershipListingSourceError {
    fn from(value: PartnershipGrantError) -> Self {
        match value {
            PartnershipGrantError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipGrantError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        error::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
        patch_field::PatchField,
        transaction::TransactionError,
    };
    use domain_primitives::versioned::Versioned;
    use listing_source_core::{
        ListingSource, ListingSourceName, ListingSourcePresentation, NewListingSource,
    };
    use listing_source_service::ports::{
        ListingSourceIngestionConfigurations, ListingSourceRepository, ListingSourceStorageVersion,
        StoredListingSource,
    };
    use partnership_core::partnership::{NewPartnership, Partnership};
    use party_core::party_id::PartyId;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::{role::UserRole, user_id::UserId};
    use user_service::ports::{UserAdminActorView, UserAdminReader};

    #[derive(Default)]
    struct State {
        partnership: Option<Partnership>,
        source: Option<StoredListingSource>,
        admin: Option<UserAdminActorView>,
        remove_outcome: Option<ListingSourceGrantRemoveOutcome>,
        remove_error: Option<PartnershipGrantError>,
        begin_fails: bool,
        commit_fails: bool,
        partnership_reads: usize,
        source_reads: usize,
        remove_calls: usize,
        commit_attempts: usize,
        commits: usize,
    }

    struct FakeUnitOfWork {
        state: Arc<Mutex<State>>,
    }

    struct FakeTransaction {
        state: Arc<Mutex<State>>,
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.state);
            state.commit_attempts += 1;
            if state.commit_fails {
                return Err(TransactionError::CommitFailed);
            }
            state.commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            if lock(&self.state).begin_fails {
                return Err(TransactionError::BeginFailed);
            }
            Ok(FakeTransaction {
                state: Arc::clone(&self.state),
            })
        }
    }

    #[derive(Clone)]
    struct FakeFactories {
        state: Arc<Mutex<State>>,
    }

    struct FakePartnershipRepository {
        state: Arc<Mutex<State>>,
    }

    struct FakeSourceRepository {
        state: Arc<Mutex<State>>,
    }

    struct FakeGrantRepository {
        state: Arc<Mutex<State>>,
    }

    struct FakeAdminReader {
        state: Arc<Mutex<State>>,
    }

    impl PartnershipRepositoryFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PartnershipRepository + 'tx {
            FakePartnershipRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    impl ListingSourceRepositoryFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ListingSourceRepository + 'tx {
            FakeSourceRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    impl ListingSourceGrantRepositoryFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ListingSourceGrantRepository + 'tx {
            FakeGrantRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    impl UserAdminReaderFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl UserAdminReader + 'tx {
            FakeAdminReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnershipRepository for FakePartnershipRepository {
        async fn find_by_id(
            &mut self,
            partnership_id: PartnershipId,
        ) -> Result<Option<VersionedPartnership>, PartnershipRepositoryError> {
            let mut state = lock(&self.state);
            state.partnership_reads += 1;
            Ok(state
                .partnership
                .clone()
                .filter(|partnership| partnership.id() == partnership_id)
                .map(|partnership| Versioned::new(partnership, PartnershipStorageVersion::INITIAL)))
        }

        async fn find_or_create_for_party(
            &mut self,
            _party_id: PartyId,
            _new_partnership_id: PartnershipId,
        ) -> Result<VersionedPartnership, PartnershipRepositoryError> {
            Err(PartnershipRepositoryError::Internal {
                source: static_error("unexpected partnership creation"),
            })
        }
    }

    #[async_trait::async_trait]
    impl ListingSourceRepository for FakeSourceRepository {
        async fn find_by_id(
            &mut self,
            listing_source_id: ListingSourceId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            let mut state = lock(&self.state);
            state.source_reads += 1;
            Ok(state
                .source
                .clone()
                .filter(|source| source.source.id() == listing_source_id))
        }

        async fn find_by_slug(
            &mut self,
            _slug: &listing_source_core::ListingSourceSlugId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Err(ListingSourceRepositoryError::Internal {
                source: static_error("unexpected listing source slug lookup"),
            })
        }

        async fn insert(
            &mut self,
            _source: &ListingSource,
            _configuration: &ListingSourceIngestionConfigurations,
            _woocommerce_webhook_secret: Option<&str>,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            Err(ListingSourceRepositoryError::Internal {
                source: static_error("unexpected listing source insert"),
            })
        }

        async fn update(
            &mut self,
            _source: &ListingSource,
            _configuration: &ListingSourceIngestionConfigurations,
            _woocommerce_webhook_secret: PatchField<&str>,
            _expected: ListingSourceStorageVersion,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            Err(ListingSourceRepositoryError::Internal {
                source: static_error("unexpected listing source update"),
            })
        }
    }

    #[async_trait::async_trait]
    impl ListingSourceGrantRepository for FakeGrantRepository {
        async fn grant_source_access(
            &mut self,
            _partnership_id: PartnershipId,
            _listing_source_id: ListingSourceId,
        ) -> Result<ListingSourceGrantOutcome, PartnershipGrantError> {
            Err(PartnershipGrantError::Internal {
                source: static_error("unexpected listing source grant"),
            })
        }

        async fn remove_source_access(
            &mut self,
            _partnership_id: PartnershipId,
            _listing_source_id: ListingSourceId,
        ) -> Result<ListingSourceGrantRemoveOutcome, PartnershipGrantError> {
            let mut state = lock(&self.state);
            state.remove_calls += 1;
            if let Some(error) = state.remove_error.take() {
                return Err(error);
            }
            Ok(state
                .remove_outcome
                .unwrap_or(ListingSourceGrantRemoveOutcome::Removed))
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeAdminReader {
        async fn find_admin_actor(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, user_service::ports::UserAdminReadError> {
            Ok(lock(&self.state).admin.clone())
        }
    }

    fn lock(state: &Arc<Mutex<State>>) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn source(
        listing_source_id: ListingSourceId,
        operator_party_id: PartyId,
    ) -> StoredListingSource {
        StoredListingSource {
            source: ListingSource::create(NewListingSource {
                id: listing_source_id,
                name: ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("invalid test ListingSource name: {error}")),
                operator_party_id,
                ingestion_methods: std::collections::HashSet::new(),
                presentation: ListingSourcePresentation::default(),
                referral_configuration: None,
            }),
            configuration: ListingSourceIngestionConfigurations::default(),
            version: ListingSourceStorageVersion::INITIAL,
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn valid_state() -> (State, PartnershipId, ListingSourceId) {
        let partnership_id = PartnershipId::new();
        let listing_source_id = ListingSourceId::new();
        let party_id = PartyId::new();
        (
            State {
                partnership: Some(Partnership::create(NewPartnership {
                    id: partnership_id,
                    party_id,
                })),
                source: Some(source(listing_source_id, party_id)),
                admin: Some(UserAdminActorView {
                    user_id: UserId::new(),
                    role: UserRole::Admin,
                }),
                remove_outcome: Some(ListingSourceGrantRemoveOutcome::Removed),
                ..State::default()
            },
            partnership_id,
            listing_source_id,
        )
    }

    type Handler = RevokePartnershipListingSourceHandler<
        FakeUnitOfWork,
        FakeFactories,
        FakeFactories,
        FakeFactories,
        FakeFactories,
    >;

    fn handler(state: Arc<Mutex<State>>) -> Handler {
        let factories = FakeFactories {
            state: Arc::clone(&state),
        };
        RevokePartnershipListingSourceHandler::new(
            FakeUnitOfWork { state },
            factories.clone(),
            factories.clone(),
            factories.clone(),
            factories,
        )
    }

    #[tokio::test]
    async fn should_remove_listing_source_access_and_commit_one_transaction() {
        let (state, partnership_id, listing_source_id) = valid_state();
        let state = Arc::new(Mutex::new(state));
        let result = handler(Arc::clone(&state))
            .execute(
                &context(),
                RevokePartnershipListingSourceCommand {
                    partnership_id,
                    listing_source_id,
                },
            )
            .await;

        assert!(matches!(
            result,
            Ok(RevokePartnershipListingSourceResult {
                outcome: RevokePartnershipListingSourceOutcome::Removed
            })
        ));
        let state = lock(&state);
        assert_eq!(1, state.partnership_reads);
        assert_eq!(1, state.source_reads);
        assert_eq!(1, state.remove_calls);
        assert_eq!(1, state.commit_attempts);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_commit_absent_listing_source_grant_as_successful_no_op() {
        let (mut state, partnership_id, listing_source_id) = valid_state();
        state.remove_outcome = Some(ListingSourceGrantRemoveOutcome::AlreadyAbsent);
        let state = Arc::new(Mutex::new(state));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(),
                RevokePartnershipListingSourceCommand {
                    partnership_id,
                    listing_source_id,
                },
            )
            .await;

        assert!(matches!(
            result,
            Ok(RevokePartnershipListingSourceResult {
                outcome: RevokePartnershipListingSourceOutcome::AlreadyAbsent
            })
        ));
        assert_eq!(1, lock(&state).commits);
    }

    #[tokio::test]
    async fn should_reject_missing_references_without_removing() {
        let (mut missing_partnership, partnership_id, listing_source_id) = valid_state();
        missing_partnership.partnership = None;
        let missing_partnership = Arc::new(Mutex::new(missing_partnership));
        let result = handler(Arc::clone(&missing_partnership))
            .execute(
                &context(),
                RevokePartnershipListingSourceCommand {
                    partnership_id,
                    listing_source_id,
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(RevokePartnershipListingSourceError::PartnershipNotFound)
        ));
        assert_eq!(0, lock(&missing_partnership).source_reads);

        let (mut missing_source, partnership_id, listing_source_id) = valid_state();
        missing_source.source = None;
        let missing_source = Arc::new(Mutex::new(missing_source));
        let result = handler(Arc::clone(&missing_source))
            .execute(
                &context(),
                RevokePartnershipListingSourceCommand {
                    partnership_id,
                    listing_source_id,
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(RevokePartnershipListingSourceError::ListingSourceNotFound)
        ));
        let state = lock(&missing_source);
        assert_eq!(0, state.remove_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_reject_non_admin_before_target_reads() {
        let (mut state, partnership_id, listing_source_id) = valid_state();
        state.admin = Some(UserAdminActorView {
            user_id: UserId::new(),
            role: UserRole::User,
        });
        let state = Arc::new(Mutex::new(state));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(),
                RevokePartnershipListingSourceCommand {
                    partnership_id,
                    listing_source_id,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(RevokePartnershipListingSourceError::Forbidden)
        ));
        let state = lock(&state);
        assert_eq!(0, state.partnership_reads);
        assert_eq!(0, state.source_reads);
        assert_eq!(0, state.remove_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_not_commit_when_grant_removal_persistence_fails() {
        let (mut state, partnership_id, listing_source_id) = valid_state();
        state.remove_error = Some(PartnershipGrantError::Internal {
            source: static_error("grant removal failed"),
        });
        let state = Arc::new(Mutex::new(state));

        let result = handler(Arc::clone(&state))
            .execute(
                &context(),
                RevokePartnershipListingSourceCommand {
                    partnership_id,
                    listing_source_id,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(RevokePartnershipListingSourceError::Internal { .. })
        ));
        let state = lock(&state);
        assert_eq!(1, state.remove_calls);
        assert_eq!(0, state.commit_attempts);
        assert_eq!(0, state.commits);
    }
}
