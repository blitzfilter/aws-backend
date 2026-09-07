use crate::ports::{
    WatchlistQuotaReadError, WatchlistQuotaReader, WatchlistQuotaReaderFactory,
    WatchlistRepository, WatchlistRepositoryError, WatchlistRepositoryFactory,
};
use crate::tier_policy::active_watchlist_quota;
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};
use product_listing_service::ports::{
    ProductListingLifecycleGuard, ProductListingLifecycleGuardError,
    ProductListingLifecycleGuardFactory,
};
use user_core::user_id::UserId;
use user_service::ports::{
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};
use watchlist_core::WatchlistProductListing;
use watchlist_core::watchlist_state::WatchlistState;

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateWatchlistProductListingCommand {
    pub user_id: UserId,
    pub product_listing_id: ProductListingId,
    pub notifications: Option<bool>,
    pub state: Option<WatchlistState>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateWatchlistProductListingResult {
    pub entry: WatchlistProductListing,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateWatchlistProductListingError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("watchlist entry not found")]
    NotFound,
    #[error("watchlist entry changed concurrently")]
    ConcurrencyConflict,
    #[error("user not found")]
    UserNotFound,
    #[error("product listing not found")]
    ProductListingNotFound,
    #[error("product listing is unavailable")]
    ProductListingUnavailable,
    #[error("watchlist quota exceeded: {active_count}/{quota} active entries are already in use")]
    WatchlistQuotaExceeded { active_count: usize, quota: usize },
    #[error("user tier entitlement lock failed")]
    UserTierEntitlementsLockFailed {
        #[source]
        source: BoxError,
    },
    #[error("watchlist quota read failed")]
    WatchlistQuotaReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("temporary watchlist persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted watchlist state")]
    InvalidPersistedState,
    #[error("failed to begin watchlist transaction")]
    BeginTransactionFailed,
    #[error("failed to commit watchlist transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateWatchlistProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateWatchlistProductListingCommand,
    ) -> Result<UpdateWatchlistProductListingResult, UpdateWatchlistProductListingError>;
}

pub struct UpdateWatchlistProductListingHandler<U, R, Q, A, G> {
    unit_of_work: U,
    watchlist: R,
    quotas: Q,
    tier_entitlements: A,
    product_listing_lifecycle: G,
}

impl<U, R, Q, A, G> UpdateWatchlistProductListingHandler<U, R, Q, A, G> {
    pub fn new(
        unit_of_work: U,
        watchlist: R,
        quotas: Q,
        tier_entitlements: A,
        product_listing_lifecycle: G,
    ) -> Self {
        Self {
            unit_of_work,
            watchlist,
            quotas,
            tier_entitlements,
            product_listing_lifecycle,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, Q, A, G> UpdateWatchlistProductListingUseCase
    for UpdateWatchlistProductListingHandler<U, R, Q, A, G>
where
    U: UnitOfWork,
    R: WatchlistRepositoryFactory<U::Tx>,
    Q: WatchlistQuotaReaderFactory<U::Tx>,
    A: UserTierEntitlementsFactory<U::Tx>,
    G: ProductListingLifecycleGuardFactory<U::Tx>,
{
    #[tracing::instrument(name = "update_watchlist_product", skip_all, fields(user_id = %command.user_id, product_listing_id = %command.product_listing_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateWatchlistProductListingCommand,
    ) -> Result<UpdateWatchlistProductListingResult, UpdateWatchlistProductListingError> {
        authorize_write(context, command.user_id)?;

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateWatchlistProductListingError::BeginTransactionFailed)?;
        let loaded = self
            .watchlist
            .in_transaction(&mut tx)
            .find_by_user_and_product(command.user_id, command.product_listing_id)
            .await?
            .ok_or(UpdateWatchlistProductListingError::NotFound)?;
        let expected_version = loaded.version;
        let mut entry = loaded.value;

        let reactivating =
            matches!(command.state, Some(WatchlistState::Active)) && !entry.state().is_active();
        if reactivating {
            let tier = self
                .tier_entitlements
                .in_transaction(&mut tx)
                .lock_user_tier(command.user_id)
                .await
                .map_err(tier_entitlements_error)?
                .ok_or(UpdateWatchlistProductListingError::UserNotFound)?;
            match self
                .product_listing_lifecycle
                .in_transaction(&mut tx)
                .lock_and_find_lifecycle(command.product_listing_id)
                .await
                .map_err(product_listing_lifecycle_guard_error)?
            {
                None => return Err(UpdateWatchlistProductListingError::ProductListingNotFound),
                Some(ListingLifecycle::Withdrawn) => {
                    return Err(UpdateWatchlistProductListingError::ProductListingUnavailable);
                }
                Some(ListingLifecycle::Active) => {}
            }
            if let Some(quota) = active_watchlist_quota(tier) {
                let active_count = self
                    .quotas
                    .in_transaction(&mut tx)
                    .count_active_for_user(command.user_id)
                    .await
                    .map_err(watchlist_quota_read_error)?;
                if active_count >= quota {
                    return Err(UpdateWatchlistProductListingError::WatchlistQuotaExceeded {
                        active_count,
                        quota,
                    });
                }
            }
        }
        let mut changed = false;
        if let Some(notifications) = command.notifications
            && entry.notifications() != notifications
        {
            entry.change_notifications(notifications);
            changed = true;
        }
        if let Some(state) = command.state
            && entry.state() != state
        {
            entry.change_state(state);
            changed = true;
        }
        if changed {
            entry = match self
                .watchlist
                .in_transaction(&mut tx)
                .update(&entry, expected_version)
                .await
            {
                Ok(entry) => entry.into_value(),
                Err(WatchlistRepositoryError::ConcurrencyConflict) => {
                    tracing::warn!(
                        event = "watchlist_product.update_rejected",
                        actor_type = context.principal.kind(),
                        actor_id = ?context.principal.actor_id(),
                        user_id = %command.user_id,
                        product_listing_id = %command.product_listing_id,
                        outcome = "concurrency_conflict",
                    );
                    return Err(UpdateWatchlistProductListingError::ConcurrencyConflict);
                }
                Err(error) => return Err(error.into()),
            };
        }

        tx.commit()
            .await
            .map_err(|_| UpdateWatchlistProductListingError::CommitTransactionFailed)?;
        tracing::info!(
            event = "watchlist_product.updated",
            actor_type = context.principal.kind(),
            actor_id = ?context.principal.actor_id(),
            user_id = %command.user_id,
            product_listing_id = %command.product_listing_id,
            outcome = if changed { "success" } else { "unchanged" },
        );
        Ok(UpdateWatchlistProductListingResult { entry })
    }
}

fn authorize_write(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), UpdateWatchlistProductListingError> {
    context
        .require()
        .credential_capability(CredentialCapability::WatchlistWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<UpdateWatchlistProductListingError>()
}

impl From<OperationAuthorizationError> for UpdateWatchlistProductListingError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

fn tier_entitlements_error(error: UserTierEntitlementsError) -> UpdateWatchlistProductListingError {
    match error {
        UserTierEntitlementsError::LockFailed { source }
        | UserTierEntitlementsError::ReconciliationFailed { source } => {
            UpdateWatchlistProductListingError::UserTierEntitlementsLockFailed { source }
        }
    }
}

fn product_listing_lifecycle_guard_error(
    error: ProductListingLifecycleGuardError,
) -> UpdateWatchlistProductListingError {
    match error {
        ProductListingLifecycleGuardError::LockFailed { source } => {
            UpdateWatchlistProductListingError::TemporarilyUnavailable { source }
        }
        ProductListingLifecycleGuardError::InvalidListingLifecyclePersisted => {
            UpdateWatchlistProductListingError::InvalidPersistedState
        }
    }
}

fn watchlist_quota_read_error(
    error: WatchlistQuotaReadError,
) -> UpdateWatchlistProductListingError {
    UpdateWatchlistProductListingError::WatchlistQuotaReadFailed {
        source: box_error(error),
    }
}

impl From<WatchlistRepositoryError> for UpdateWatchlistProductListingError {
    fn from(value: WatchlistRepositoryError) -> Self {
        match value {
            WatchlistRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            WatchlistRepositoryError::InvalidPersistedState => Self::InvalidPersistedState,
            WatchlistRepositoryError::LookupFailed { source }
            | WatchlistRepositoryError::InsertFailed { source }
            | WatchlistRepositoryError::UpdateFailed { source }
            | WatchlistRepositoryError::DeleteFailed { source } => {
                Self::TemporarilyUnavailable { source }
            }
            error @ WatchlistRepositoryError::AlreadyExists => Self::TemporarilyUnavailable {
                source: box_error(error),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code)]

    use super::*;

    use application::error::static_error;
    use product_listing_core::listing_lifecycle::ListingLifecycle;
    use product_listing_service::ports::{
        ProductListingLifecycleGuard, ProductListingLifecycleGuardError,
        ProductListingLifecycleGuardFactory,
    };

    use crate::ports::{
        VersionedWatchlistProductListing, WatchlistProductListingView, WatchlistQuotaReadError,
        WatchlistQuotaReader, WatchlistQuotaReaderFactory, WatchlistReadError, WatchlistReader,
        WatchlistReaderFactory, WatchlistRepository, WatchlistRepositoryError,
        WatchlistRepositoryFactory, WatchlistStorageVersion,
    };
    use application::operation_context::{
        CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
    };
    use application::transaction::{Transaction, TransactionError, UnitOfWork};
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use user_core::tier::UserTier;
    use user_service::ports::{
        UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
    };
    use watchlist_core::watchlist_state::WatchlistState;
    use watchlist_core::{NewWatchlistProductListing, WatchlistProductListing};

    #[derive(Clone, Default)]
    struct TestUnitOfWork {
        state: SharedState,
        fail_begin: bool,
        fail_commit: bool,
    }

    struct TestTransaction {
        state: SharedState,
        fail_commit: bool,
    }

    #[derive(Clone, Default)]
    struct TestWatchlistFactory {
        state: SharedState,
    }

    #[derive(Clone, Copy)]
    struct TestAccountFactory {
        tier: UserTier,
    }

    #[derive(Clone, Copy)]
    enum TestLifecycleGuardOutcome {
        Active,
        Missing,
        Withdrawn,
        LockFailed,
        InvalidPersistedState,
    }

    #[derive(Clone)]
    struct TestLifecycleGuardFactory {
        state: SharedState,
        outcome: TestLifecycleGuardOutcome,
    }

    struct TestLifecycleGuard {
        state: SharedState,
        outcome: TestLifecycleGuardOutcome,
    }

    struct TestAccountReader {
        tier: UserTier,
    }

    #[derive(Clone, Default)]
    struct SharedState {
        entries: Arc<Mutex<Vec<VersionedWatchlistProductListing>>>,
        committed: Arc<Mutex<bool>>,
        updated: Arc<Mutex<usize>>,
        deleted: Arc<Mutex<usize>>,
        lifecycle_guard_calls: Arc<Mutex<usize>>,
        force_concurrency_conflict: bool,
    }

    struct TestWatchlistPort {
        state: SharedState,
    }

    impl SharedState {
        fn with_entry(entry: WatchlistProductListing) -> Self {
            let state = Self::default();
            state.push(entry);
            state
        }

        fn push(&self, entry: WatchlistProductListing) {
            if let Ok(mut entries) = self.entries.lock() {
                entries.push(VersionedWatchlistProductListing::new(
                    entry,
                    WatchlistStorageVersion::INITIAL,
                ));
            }
        }

        fn committed(&self) -> bool {
            self.committed.lock().map(|value| *value).unwrap_or(false)
        }

        fn updated(&self) -> usize {
            self.updated.lock().map(|value| *value).unwrap_or(0)
        }

        fn deleted(&self) -> usize {
            self.deleted.lock().map(|value| *value).unwrap_or(0)
        }

        fn lifecycle_guard_calls(&self) -> usize {
            self.lifecycle_guard_calls
                .lock()
                .map(|value| *value)
                .unwrap_or(0)
        }
    }

    #[async_trait::async_trait]
    impl Transaction for TestTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            if self.fail_commit {
                return Err(TransactionError::CommitFailed);
            }
            self.state
                .committed
                .lock()
                .map(|mut committed| *committed = true)
                .map_err(|_| TransactionError::CommitFailed)
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            if self.fail_begin {
                return Err(TransactionError::BeginFailed);
            }
            Ok(TestTransaction {
                state: self.state.clone(),
                fail_commit: self.fail_commit,
            })
        }
    }

    impl<Tx> WatchlistRepositoryFactory<Tx> for TestWatchlistFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl WatchlistRepository + 'tx {
            TestWatchlistPort {
                state: self.state.clone(),
            }
        }
    }

    impl<Tx> WatchlistQuotaReaderFactory<Tx> for TestWatchlistFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl WatchlistQuotaReader + 'tx {
            TestWatchlistPort {
                state: self.state.clone(),
            }
        }
    }

    impl<Tx> UserTierEntitlementsFactory<Tx> for TestAccountFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl UserTierEntitlements + 'tx {
            TestAccountReader { tier: self.tier }
        }
    }

    impl<Tx> ProductListingLifecycleGuardFactory<Tx> for TestLifecycleGuardFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut Tx,
        ) -> impl ProductListingLifecycleGuard + 'tx {
            TestLifecycleGuard {
                state: self.state.clone(),
                outcome: self.outcome,
            }
        }
    }

    impl<Tx> WatchlistReaderFactory<Tx> for TestWatchlistFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl WatchlistReader + 'tx {
            TestWatchlistPort {
                state: self.state.clone(),
            }
        }
    }

    #[async_trait::async_trait]
    impl UserTierEntitlements for TestAccountReader {
        async fn lock_user_tier(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserTier>, UserTierEntitlementsError> {
            Ok(Some(self.tier))
        }

        async fn reconcile_for_tier(
            &mut self,
            _user_id: UserId,
            _tier: UserTier,
        ) -> Result<(), UserTierEntitlementsError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl ProductListingLifecycleGuard for TestLifecycleGuard {
        async fn lock_and_find_lifecycle(
            &mut self,
            _product_listing_id: ProductListingId,
        ) -> Result<Option<ListingLifecycle>, ProductListingLifecycleGuardError> {
            self.state
                .lifecycle_guard_calls
                .lock()
                .map(|mut calls| *calls += 1)
                .map_err(|_| ProductListingLifecycleGuardError::LockFailed {
                    source: static_error(
                        "watchlist lifecycle guard test counter mutex is poisoned",
                    ),
                })?;
            match self.outcome {
                TestLifecycleGuardOutcome::Active => Ok(Some(ListingLifecycle::Active)),
                TestLifecycleGuardOutcome::Missing => Ok(None),
                TestLifecycleGuardOutcome::Withdrawn => Ok(Some(ListingLifecycle::Withdrawn)),
                TestLifecycleGuardOutcome::LockFailed => {
                    Err(ProductListingLifecycleGuardError::LockFailed {
                        source: static_error("product listing lifecycle guard test lock failure"),
                    })
                }
                TestLifecycleGuardOutcome::InvalidPersistedState => {
                    Err(ProductListingLifecycleGuardError::InvalidListingLifecyclePersisted)
                }
            }
        }
    }

    #[async_trait::async_trait]
    impl WatchlistQuotaReader for TestWatchlistPort {
        async fn count_active_for_user(
            &mut self,
            user_id: UserId,
        ) -> Result<usize, WatchlistQuotaReadError> {
            self.state
                .entries
                .lock()
                .map_err(|_poisoned| WatchlistQuotaReadError::ReadFailed {
                    source: static_error("watchlist quota test state mutex is poisoned"),
                })
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| {
                            entry.value.user_id() == user_id && entry.value.state().is_active()
                        })
                        .count()
                })
        }
    }

    #[async_trait::async_trait]
    impl WatchlistRepository for TestWatchlistPort {
        async fn find_by_user_and_product(
            &mut self,
            user_id: UserId,
            product_listing_id: ProductListingId,
        ) -> Result<Option<VersionedWatchlistProductListing>, WatchlistRepositoryError> {
            self.state
                .entries
                .lock()
                .map_err(|_| WatchlistRepositoryError::LookupFailed {
                    source: static_error("watchlist test lookup mutex is poisoned"),
                })
                .map(|entries| {
                    entries
                        .iter()
                        .find(|entry| {
                            entry.value.user_id() == user_id
                                && entry.value.product_listing_id() == product_listing_id
                        })
                        .cloned()
                })
        }

        async fn insert(
            &mut self,
            entry: &WatchlistProductListing,
        ) -> Result<VersionedWatchlistProductListing, WatchlistRepositoryError> {
            let mut entries =
                self.state
                    .entries
                    .lock()
                    .map_err(|_| WatchlistRepositoryError::InsertFailed {
                        source: static_error("watchlist test insert mutex is poisoned"),
                    })?;
            if entries.iter().any(|existing| {
                existing.value.user_id() == entry.user_id()
                    && existing.value.product_listing_id() == entry.product_listing_id()
            }) {
                return Err(WatchlistRepositoryError::AlreadyExists);
            }
            let persisted = VersionedWatchlistProductListing::new(
                entry.clone(),
                WatchlistStorageVersion::INITIAL,
            );
            entries.push(persisted.clone());
            Ok(persisted)
        }

        async fn update(
            &mut self,
            entry: &WatchlistProductListing,
            expected_version: WatchlistStorageVersion,
        ) -> Result<VersionedWatchlistProductListing, WatchlistRepositoryError> {
            if self.state.force_concurrency_conflict {
                return Err(WatchlistRepositoryError::ConcurrencyConflict);
            }
            let mut entries =
                self.state
                    .entries
                    .lock()
                    .map_err(|_| WatchlistRepositoryError::UpdateFailed {
                        source: static_error("watchlist test update mutex is poisoned"),
                    })?;
            let Some(existing) = entries.iter_mut().find(|existing| {
                existing.value.user_id() == entry.user_id()
                    && existing.value.product_listing_id() == entry.product_listing_id()
            }) else {
                return Err(WatchlistRepositoryError::UpdateFailed {
                    source: static_error("watchlist test entry is missing"),
                });
            };
            if existing.version != expected_version {
                return Err(WatchlistRepositoryError::ConcurrencyConflict);
            }
            let persisted =
                VersionedWatchlistProductListing::new(entry.clone(), expected_version.next());
            *existing = persisted.clone();
            self.state
                .updated
                .lock()
                .map(|mut updated| *updated += 1)
                .map_err(|_| WatchlistRepositoryError::UpdateFailed {
                    source: static_error("watchlist test update counter mutex is poisoned"),
                })?;
            Ok(persisted)
        }

        async fn delete(
            &mut self,
            user_id: UserId,
            product_listing_id: ProductListingId,
            expected_version: WatchlistStorageVersion,
        ) -> Result<(), WatchlistRepositoryError> {
            let mut entries =
                self.state
                    .entries
                    .lock()
                    .map_err(|_| WatchlistRepositoryError::DeleteFailed {
                        source: static_error("watchlist test delete mutex is poisoned"),
                    })?;
            let Some(index) = entries.iter().position(|entry| {
                entry.value.user_id() == user_id
                    && entry.value.product_listing_id() == product_listing_id
            }) else {
                return Err(WatchlistRepositoryError::ConcurrencyConflict);
            };
            if entries[index].version != expected_version {
                return Err(WatchlistRepositoryError::ConcurrencyConflict);
            }
            entries.remove(index);
            self.state
                .deleted
                .lock()
                .map(|mut deleted| *deleted += 1)
                .map_err(|_| WatchlistRepositoryError::DeleteFailed {
                    source: static_error("watchlist test delete counter mutex is poisoned"),
                })
        }
    }

    #[async_trait::async_trait]
    impl WatchlistReader for TestWatchlistPort {
        async fn find_for_user(
            &mut self,
            user_id: UserId,
        ) -> Result<Vec<WatchlistProductListingView>, WatchlistReadError> {
            self.state
                .entries
                .lock()
                .map_err(|_| WatchlistReadError::ReadFailed)
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| entry.value.user_id() == user_id)
                        .map(|entry| WatchlistProductListingView {
                            user_id: entry.value.user_id(),
                            product_listing_id: entry.value.product_listing_id(),
                            notifications: entry.value.notifications(),
                            state: entry.value.state(),
                            created: OffsetDateTime::UNIX_EPOCH,
                            updated: OffsetDateTime::UNIX_EPOCH,
                        })
                        .collect()
                })
        }

        async fn find_user_ids_for_product(
            &mut self,
            product_listing_id: ProductListingId,
        ) -> Result<Vec<UserId>, WatchlistReadError> {
            self.state
                .entries
                .lock()
                .map_err(|_| WatchlistReadError::ReadFailed)
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| entry.value.product_listing_id() == product_listing_id)
                        .map(|entry| entry.value.user_id())
                        .collect()
                })
        }
    }

    fn context_for_user(user_id: UserId) -> OperationContext {
        OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn delegated_context(
        user_id: UserId,
        capabilities: BTreeSet<CredentialCapability>,
    ) -> OperationContext {
        OperationContext {
            principal: Principal::DelegatedUser {
                user_id,
                capabilities,
            },
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn entry(
        user_id: UserId,
        product_listing_id: ProductListingId,
        notifications: bool,
    ) -> WatchlistProductListing {
        WatchlistProductListing::create(NewWatchlistProductListing {
            user_id,
            product_listing_id,
            notifications,
            state: WatchlistState::Active,
        })
    }

    fn handler(
        state: SharedState,
        tier: UserTier,
        lifecycle_outcome: TestLifecycleGuardOutcome,
    ) -> UpdateWatchlistProductListingHandler<
        TestUnitOfWork,
        TestWatchlistFactory,
        TestWatchlistFactory,
        TestAccountFactory,
        TestLifecycleGuardFactory,
    > {
        UpdateWatchlistProductListingHandler::new(
            TestUnitOfWork {
                state: state.clone(),
                ..Default::default()
            },
            TestWatchlistFactory {
                state: state.clone(),
            },
            TestWatchlistFactory {
                state: state.clone(),
            },
            TestAccountFactory { tier },
            TestLifecycleGuardFactory {
                state,
                outcome: lifecycle_outcome,
            },
        )
    }

    #[tokio::test]
    async fn should_update_notifications_when_entry_exists() -> Result<(), String> {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let state = SharedState::with_entry(entry(user_id, product_listing_id, true));

        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                user_id,
                product_listing_id,
                notifications: Some(false),
                state: None,
            },
        )
        .await
        .map_err(|error| error.to_string())?;

        assert!(!result.entry.notifications());
        assert_eq!(1, state.updated());
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_persistence_when_update_command_changes_nothing() -> Result<(), String> {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let state = SharedState::with_entry(entry(user_id, product_listing_id, true));

        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                user_id,
                product_listing_id,
                notifications: Some(true),
                state: Some(WatchlistState::Active),
            },
        )
        .await
        .map_err(|error| error.to_string())?;

        assert!(result.entry.notifications());
        assert_eq!(WatchlistState::Active, result.entry.state());
        assert_eq!(0, state.updated());
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_return_concurrency_conflict_when_entry_changes_before_update() {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let mut state = SharedState::with_entry(entry(user_id, product_listing_id, true));
        state.force_concurrency_conflict = true;

        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                user_id,
                product_listing_id,
                notifications: Some(false),
                state: None,
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(UpdateWatchlistProductListingError::ConcurrencyConflict)
        ));
        assert_eq!(0, state.updated());
        assert!(!state.committed());
    }

    #[tokio::test]
    async fn should_return_not_found_when_update_entry_missing() {
        let user_id = UserId::new();
        let state = SharedState::default();

        let result = handler(state, UserTier::Free, TestLifecycleGuardOutcome::Active)
            .execute(
                &context_for_user(user_id),
                UpdateWatchlistProductListingCommand {
                    user_id,
                    product_listing_id: ProductListingId::new(),
                    notifications: Some(false),
                    state: None,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(UpdateWatchlistProductListingError::NotFound)
        ));
    }

    fn inactive_entry(
        user_id: UserId,
        product_listing_id: ProductListingId,
    ) -> WatchlistProductListing {
        let mut entry = entry(user_id, product_listing_id, true);
        entry.change_state(WatchlistState::InactiveByUser);
        entry
    }

    #[tokio::test]
    async fn should_reactivate_when_listing_is_active() -> Result<(), String> {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let state = SharedState::with_entry(inactive_entry(user_id, product_listing_id));

        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                user_id,
                product_listing_id,
                notifications: None,
                state: Some(WatchlistState::Active),
            },
        )
        .await
        .map_err(|error| error.to_string())?;

        assert_eq!(WatchlistState::Active, result.entry.state());
        assert_eq!(1, state.lifecycle_guard_calls());
        assert_eq!(1, state.updated());
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_reactivation_when_listing_is_missing_or_withdrawn() {
        for (outcome, expected_missing) in [
            (TestLifecycleGuardOutcome::Missing, true),
            (TestLifecycleGuardOutcome::Withdrawn, false),
        ] {
            let user_id = UserId::new();
            let product_listing_id = ProductListingId::new();
            let state = SharedState::with_entry(inactive_entry(user_id, product_listing_id));

            let result = handler(state.clone(), UserTier::Free, outcome)
                .execute(
                    &context_for_user(user_id),
                    UpdateWatchlistProductListingCommand {
                        user_id,
                        product_listing_id,
                        notifications: None,
                        state: Some(WatchlistState::Active),
                    },
                )
                .await;

            if expected_missing {
                assert!(matches!(
                    result,
                    Err(UpdateWatchlistProductListingError::ProductListingNotFound)
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(UpdateWatchlistProductListingError::ProductListingUnavailable)
                ));
            }
            assert_eq!(1, state.lifecycle_guard_calls());
            assert_eq!(0, state.updated());
            assert!(!state.committed());
        }
    }

    #[tokio::test]
    async fn should_preserve_lifecycle_guard_lock_failure_and_reject_invalid_persisted_state()
    -> Result<(), String> {
        for outcome in [
            TestLifecycleGuardOutcome::LockFailed,
            TestLifecycleGuardOutcome::InvalidPersistedState,
        ] {
            let user_id = UserId::new();
            let product_listing_id = ProductListingId::new();
            let state = SharedState::with_entry(inactive_entry(user_id, product_listing_id));

            let result = handler(state.clone(), UserTier::Free, outcome)
                .execute(
                    &context_for_user(user_id),
                    UpdateWatchlistProductListingCommand {
                        user_id,
                        product_listing_id,
                        notifications: None,
                        state: Some(WatchlistState::Active),
                    },
                )
                .await;

            match (outcome, result) {
                (
                    TestLifecycleGuardOutcome::LockFailed,
                    Err(UpdateWatchlistProductListingError::TemporarilyUnavailable { source }),
                ) => assert_eq!(
                    "product listing lifecycle guard test lock failure",
                    source.to_string()
                ),
                (
                    TestLifecycleGuardOutcome::InvalidPersistedState,
                    Err(UpdateWatchlistProductListingError::InvalidPersistedState),
                ) => {}
                (_, error) => {
                    return Err(format!("unexpected lifecycle guard result: {error:?}"));
                }
            }
            assert_eq!(1, state.lifecycle_guard_calls());
            assert!(!state.committed());
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_bypass_lifecycle_guard_for_retained_withdrawn_operations() -> Result<(), String>
    {
        for command in [
            UpdateWatchlistProductListingCommand {
                user_id: UserId::new(),
                product_listing_id: ProductListingId::new(),
                notifications: Some(false),
                state: None,
            },
            UpdateWatchlistProductListingCommand {
                user_id: UserId::new(),
                product_listing_id: ProductListingId::new(),
                notifications: None,
                state: Some(WatchlistState::InactiveByUser),
            },
        ] {
            let state =
                SharedState::with_entry(entry(command.user_id, command.product_listing_id, true));
            handler(
                state.clone(),
                UserTier::Free,
                TestLifecycleGuardOutcome::Withdrawn,
            )
            .execute(&context_for_user(command.user_id), command)
            .await
            .map_err(|error| error.to_string())?;

            assert_eq!(0, state.lifecycle_guard_calls());
            assert_eq!(1, state.updated());
            assert!(state.committed());
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_bypass_lifecycle_guard_for_active_idempotent_update() -> Result<(), String> {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let state = SharedState::with_entry(entry(user_id, product_listing_id, true));

        handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Withdrawn,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                user_id,
                product_listing_id,
                notifications: None,
                state: Some(WatchlistState::Active),
            },
        )
        .await
        .map_err(|error| error.to_string())?;

        assert_eq!(0, state.lifecycle_guard_calls());
        assert_eq!(0, state.updated());
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_forbid_delegated_user_without_watchlist_write() {
        let user_id = UserId::new();
        let state = SharedState::default();

        let result = handler(state, UserTier::Free, TestLifecycleGuardOutcome::Active)
            .execute(
                &delegated_context(user_id, BTreeSet::new()),
                UpdateWatchlistProductListingCommand {
                    user_id,
                    product_listing_id: ProductListingId::new(),
                    notifications: Some(false),
                    state: None,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(UpdateWatchlistProductListingError::Forbidden)
        ));
    }
}
