use crate::ports::{
    SearchFilterMatchNotificationSource, SearchFilterMatchNotificationSourceReadError,
    SearchFilterMatchNotificationSourceReader, SearchFilterMatchNotificationSourceReaderFactory,
    SearchFilterMonthlyMatchQuotaReadError, SearchFilterMonthlyMatchQuotaReader,
    SearchFilterMonthlyMatchQuotaReaderFactory,
};
use crate::tier_policy::monthly_match_quota;
use application::{
    error::{BoxError, box_error},
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use domain_primitives::event_id::EventId;
use notification_core::notification::{NotificationContent, ProductListingNotificationSnapshot};
use notification_service::ports::notification_creator::{
    ExternalDeliveryRequest, NewNotification, NotificationCreationError,
    NotificationCreationOutcome, NotificationCreator, NotificationCreatorFactory,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{
    ProductListingContentAssessmentReadError, ProductListingContentAssessmentSnapshotReader,
    ProductListingContentAssessmentSnapshotReaderFactory, ProductListingSearchFilterMatchSource,
    ProductListingSearchFilterMatchSourceReadError, ProductListingSearchFilterMatchSourceReader,
    ProductListingSearchFilterMatchSourceReaderFactory,
};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use user_core::user_id::UserId;
use user_service::ports::{
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};

#[derive(Debug, Clone, PartialEq)]
pub struct GenerateSearchFilterMatchNotificationCommand {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub product_listing_id: ProductListingId,
    pub origin_event_id: EventId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateSearchFilterMatchNotificationResult {
    Created,
    AlreadyExists,
    SuppressedByQuota,
    SuppressedForMissingUser,
    SuppressedForMissingMatch,
    SuppressedForStaleMatch,

    SuppressedForMissingProductListing,
    SuppressedForWithdrawnProductListing,
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateSearchFilterMatchNotificationError {
    #[error("failed to begin search filter notification selection transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification source read failed")]
    MatchSourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification source persisted state is invalid")]
    MatchSourceStateInvalid {
        #[source]
        source: BoxError,
    },

    #[error("product notification source read failed")]
    ProductListingSourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("product notification source persisted state is invalid")]
    ProductListingSourceStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("product notification source does not match the requested event or product")]
    ProductListingSourceMismatch,
    #[error("product content assessment snapshot read failed")]
    ContentAssessmentReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("product content assessment snapshot persisted state is invalid")]
    ContentAssessmentStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("user tier entitlement lock failed")]
    UserTierEntitlementsLockFailed {
        #[source]
        source: BoxError,
    },
    #[error("monthly search filter notification quota read failed")]
    MonthlyMatchQuotaReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit search filter notification selection transaction")]
    CommitTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification creation failed")]
    NotificationCreateFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait GenerateSearchFilterMatchNotificationUseCase: Send + Sync {
    async fn execute(
        &self,
        command: GenerateSearchFilterMatchNotificationCommand,
    ) -> Result<
        GenerateSearchFilterMatchNotificationResult,
        GenerateSearchFilterMatchNotificationError,
    >;
}

pub struct GenerateSearchFilterMatchNotificationHandler<U, M, P, Q, A, C, N> {
    unit_of_work: U,
    matches: M,
    product_listings: P,
    quotas: Q,
    tier_entitlements: A,
    content_assessments: C,
    notifications: N,
}

impl<U, M, P, Q, A, C, N> GenerateSearchFilterMatchNotificationHandler<U, M, P, Q, A, C, N> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        unit_of_work: U,
        matches: M,
        product_listings: P,
        quotas: Q,
        tier_entitlements: A,
        content_assessments: C,
        notifications: N,
    ) -> Self {
        Self {
            unit_of_work,
            matches,
            product_listings,
            quotas,
            tier_entitlements,
            content_assessments,
            notifications,
        }
    }
}

#[async_trait::async_trait]
impl<U, M, P, Q, A, C, N> GenerateSearchFilterMatchNotificationUseCase
    for GenerateSearchFilterMatchNotificationHandler<U, M, P, Q, A, C, N>
where
    U: UnitOfWork,
    M: SearchFilterMatchNotificationSourceReaderFactory<U::Tx>,
    P: ProductListingSearchFilterMatchSourceReaderFactory<U::Tx>,
    Q: SearchFilterMonthlyMatchQuotaReaderFactory<U::Tx>,
    A: UserTierEntitlementsFactory<U::Tx>,
    C: ProductListingContentAssessmentSnapshotReaderFactory<U::Tx>,
    N: NotificationCreatorFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "generate_search_filter_match_notification",
        skip_all,
        fields(
            origin_event_id = %command.origin_event_id,
            product_listing_id = %command.product_listing_id,
            user_id = %command.user_id,
            search_filter_id = %command.search_filter_id,
        )
    )]
    async fn execute(
        &self,
        command: GenerateSearchFilterMatchNotificationCommand,
    ) -> Result<
        GenerateSearchFilterMatchNotificationResult,
        GenerateSearchFilterMatchNotificationError,
    > {
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            GenerateSearchFilterMatchNotificationError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;

        let match_source = self
            .matches
            .in_transaction(&mut tx)
            .find_source(
                command.user_id,
                command.search_filter_id,
                command.product_listing_id,
                command.origin_event_id,
            )
            .await
            .map_err(match_source_read_error)?;
        let Some(match_source) = match_source else {
            tx.commit().await.map_err(commit_error)?;
            return Ok(GenerateSearchFilterMatchNotificationResult::SuppressedForMissingMatch);
        };
        if !match_source_matches_command(&match_source, &command) {
            tx.commit().await.map_err(commit_error)?;
            return Ok(GenerateSearchFilterMatchNotificationResult::SuppressedForStaleMatch);
        }

        let product = self
            .product_listings
            .in_transaction(&mut tx)
            .find_source(command.origin_event_id, command.product_listing_id)
            .await
            .map_err(product_source_read_error)?;
        let Some(product) = product else {
            tx.commit().await.map_err(commit_error)?;
            return Ok(
                GenerateSearchFilterMatchNotificationResult::SuppressedForMissingProductListing,
            );
        };
        if product.event_id != command.origin_event_id
            || product.product_listing_id != command.product_listing_id
        {
            return Err(GenerateSearchFilterMatchNotificationError::ProductListingSourceMismatch);
        }
        if product.lifecycle != product_listing_core::listing_lifecycle::ListingLifecycle::Active {
            tx.commit().await.map_err(commit_error)?;
            return Ok(
                GenerateSearchFilterMatchNotificationResult::SuppressedForWithdrawnProductListing,
            );
        }

        let content_policy = self
            .content_assessments
            .in_transaction(&mut tx)
            .find_current_for_product_listing(command.product_listing_id)
            .await
            .map_err(content_assessment_read_error)?;

        let tier = self
            .tier_entitlements
            .in_transaction(&mut tx)
            .lock_user_tier(match_source.user_id)
            .await
            .map_err(tier_entitlements_error)?;
        let Some(tier) = tier else {
            tx.commit().await.map_err(commit_error)?;
            return Ok(GenerateSearchFilterMatchNotificationResult::SuppressedForMissingUser);
        };
        let rank = self
            .quotas
            .in_transaction(&mut tx)
            .notification_selection_rank_for_user_in_month(
                match_source.user_id,
                match_source.matched_at,
                match_source.origin_event_id,
            )
            .await
            .map_err(monthly_match_quota_error)?;

        if rank > monthly_match_quota(tier) {
            tx.commit().await.map_err(commit_error)?;
            return Ok(GenerateSearchFilterMatchNotificationResult::SuppressedByQuota);
        }

        let outcome = create_notification(
            &mut self.notifications.in_transaction(&mut tx),
            match_source,
            product,
            content_policy,
        )
        .await?;
        tx.commit().await.map_err(commit_error)?;

        match outcome {
            NotificationCreationOutcome::Inserted { .. } => {
                Ok(GenerateSearchFilterMatchNotificationResult::Created)
            }
            NotificationCreationOutcome::Duplicate => {
                Ok(GenerateSearchFilterMatchNotificationResult::AlreadyExists)
            }
        }
    }
}

fn match_source_matches_command(
    source: &SearchFilterMatchNotificationSource,
    command: &GenerateSearchFilterMatchNotificationCommand,
) -> bool {
    source.user_id == command.user_id
        && source.search_filter_id == command.search_filter_id
        && source.product_listing_id == command.product_listing_id
        && source.origin_event_id == command.origin_event_id
}

async fn create_notification(
    notifications: &mut impl NotificationCreator,
    match_source: SearchFilterMatchNotificationSource,
    product: ProductListingSearchFilterMatchSource,
    content_policy: Option<product_listing_core::content_policy::ContentPolicyDecision>,
) -> Result<NotificationCreationOutcome, GenerateSearchFilterMatchNotificationError> {
    let notification = NewNotification {
        notification: notification_core::notification::Notification::new(
            Default::default(),
            match_source.user_id,
            NotificationContent::SearchFilter {
                origin_event_id: match_source.origin_event_id,
                product_listing_id: product.product_listing_id,
                user_search_filter_id: match_source.search_filter_id,
                snapshot: ProductListingNotificationSnapshot {
                    listing_source_id: product.source.listing_source_id,
                    source_listing_id: product.source_listing_id,
                    listing_source_slug_id: product.source.slug_id,
                    product_listing_title_slug_id: product.product_listing_title_slug_id,
                    listing_source_name: product.source.name,
                    title: (!product.titles.is_empty()).then_some(product.titles),
                    image: product.image.map(|image| image.url().clone()),
                    content_policy,
                    url: product.url,
                    view_url: product.view_url,
                },
                user_search_filter_name: match_source.search_filter_name,
            },
        ),
        external_delivery: if match_source.external_delivery_requested {
            ExternalDeliveryRequest::Requested
        } else {
            ExternalDeliveryRequest::None
        },
    };
    let mut outcomes = notifications.create_many(&[notification]).await.map_err(
        |source: NotificationCreationError| {
            GenerateSearchFilterMatchNotificationError::NotificationCreateFailed {
                source: box_error(source),
            }
        },
    )?;
    outcomes.pop().ok_or_else(|| {
        GenerateSearchFilterMatchNotificationError::NotificationCreateFailed {
            source: box_error(std::io::Error::other(
                "notification creator returned no outcome",
            )),
        }
    })
}

fn match_source_read_error(
    error: SearchFilterMatchNotificationSourceReadError,
) -> GenerateSearchFilterMatchNotificationError {
    match error {
        SearchFilterMatchNotificationSourceReadError::InvalidPersistedState { source } => {
            GenerateSearchFilterMatchNotificationError::MatchSourceStateInvalid { source }
        }
        error => GenerateSearchFilterMatchNotificationError::MatchSourceReadFailed {
            source: box_error(error),
        },
    }
}

fn product_source_read_error(
    error: ProductListingSearchFilterMatchSourceReadError,
) -> GenerateSearchFilterMatchNotificationError {
    match error {
        ProductListingSearchFilterMatchSourceReadError::InvalidPersistedState { source } => {
            GenerateSearchFilterMatchNotificationError::ProductListingSourceStateInvalid { source }
        }
        error => GenerateSearchFilterMatchNotificationError::ProductListingSourceReadFailed {
            source: box_error(error),
        },
    }
}

fn content_assessment_read_error(
    error: ProductListingContentAssessmentReadError,
) -> GenerateSearchFilterMatchNotificationError {
    match error {
        ProductListingContentAssessmentReadError::InvalidPersistedState { source } => {
            GenerateSearchFilterMatchNotificationError::ContentAssessmentStateInvalid { source }
        }
        error => GenerateSearchFilterMatchNotificationError::ContentAssessmentReadFailed {
            source: box_error(error),
        },
    }
}

fn tier_entitlements_error(
    error: UserTierEntitlementsError,
) -> GenerateSearchFilterMatchNotificationError {
    match error {
        UserTierEntitlementsError::LockFailed { source }
        | UserTierEntitlementsError::ReconciliationFailed { source } => {
            GenerateSearchFilterMatchNotificationError::UserTierEntitlementsLockFailed { source }
        }
    }
}

fn monthly_match_quota_error(
    error: SearchFilterMonthlyMatchQuotaReadError,
) -> GenerateSearchFilterMatchNotificationError {
    GenerateSearchFilterMatchNotificationError::MonthlyMatchQuotaReadFailed {
        source: box_error(error),
    }
}

fn commit_error(source: TransactionError) -> GenerateSearchFilterMatchNotificationError {
    GenerateSearchFilterMatchNotificationError::CommitTransactionFailed {
        source: box_error(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexSet;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use product_listing_core::{
        content_policy::{ContentPolicyDecision, SensitiveContentCategory},
        listing_availability::ListingAvailability,
        listing_lifecycle::ListingLifecycle,
        product_listing::ProductListingPricing,
        product_listing_image::ProductListingImage,
        product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
    };
    use product_listing_service::ports::{
        ListingSourceSummary, ProductListingContentAssessmentReadError,
        ProductListingContentAssessmentSnapshotReader,
        ProductListingContentAssessmentSnapshotReaderFactory,
        ProductListingSearchFilterMatchSourceEventKind,
    };
    use search_filter_core::user_search_filter_name::UserSearchFilterName;
    use std::{
        error::Error,
        sync::{Arc, Mutex},
    };
    use time::OffsetDateTime;
    use url::Url;
    use user_core::tier::UserTier;

    #[derive(Default)]
    struct State {
        commits: usize,
        quota_reads: usize,
        notification_commit_counts: Vec<usize>,
        notification_content_policies: Vec<Option<ContentPolicyDecision>>,
        content_assessment_reads: usize,
    }

    #[derive(Clone)]
    struct TestUnitOfWork(Arc<Mutex<State>>);

    struct TestTransaction(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl Transaction for TestTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = self.0.lock().map_err(|_| TransactionError::CommitFailed)?;
            state.commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(TestTransaction(Arc::clone(&self.0)))
        }
    }

    enum MatchSources {
        Found(Option<SearchFilterMatchNotificationSource>),
        QueryFailure,
    }
    struct MatchReader<'a>(&'a MatchSources);

    #[async_trait::async_trait]
    impl SearchFilterMatchNotificationSourceReader for MatchReader<'_> {
        async fn find_source(
            &mut self,
            _user_id: UserId,
            _search_filter_id: UserSearchFilterId,
            _product_listing_id: ProductListingId,
            _origin_event_id: EventId,
        ) -> Result<
            Option<SearchFilterMatchNotificationSource>,
            SearchFilterMatchNotificationSourceReadError,
        > {
            match self.0 {
                MatchSources::Found(source) => Ok(source.clone()),
                MatchSources::QueryFailure => {
                    Err(SearchFilterMatchNotificationSourceReadError::ReadFailed {
                        source: box_error(std::io::Error::other("query failed")),
                    })
                }
            }
        }
    }

    impl SearchFilterMatchNotificationSourceReaderFactory<TestTransaction> for MatchSources {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl SearchFilterMatchNotificationSourceReader + 'tx {
            MatchReader(self)
        }
    }

    struct ProductListingSources(Option<ProductListingSearchFilterMatchSource>);
    struct ProductListingReader(Option<ProductListingSearchFilterMatchSource>);

    #[async_trait::async_trait]
    impl ProductListingSearchFilterMatchSourceReader for ProductListingReader {
        async fn find_source(
            &mut self,
            _event_id: EventId,
            _product_listing_id: ProductListingId,
        ) -> Result<
            Option<ProductListingSearchFilterMatchSource>,
            ProductListingSearchFilterMatchSourceReadError,
        > {
            Ok(self.0.clone())
        }
    }

    impl ProductListingSearchFilterMatchSourceReaderFactory<TestTransaction> for ProductListingSources {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl ProductListingSearchFilterMatchSourceReader + 'tx {
            ProductListingReader(self.0.clone())
        }
    }

    #[derive(Clone, Copy)]
    enum ContentAssessmentOutcome {
        Found(Option<ContentPolicyDecision>),
        QueryFailure,
        InvalidPersistedState,
    }

    struct ContentAssessments {
        state: Arc<Mutex<State>>,
        outcome: ContentAssessmentOutcome,
    }

    struct ContentAssessmentReader {
        state: Arc<Mutex<State>>,
        outcome: ContentAssessmentOutcome,
    }

    #[async_trait::async_trait]
    impl ProductListingContentAssessmentSnapshotReader for ContentAssessmentReader {
        async fn find_current_for_product_listing(
            &mut self,
            _product_listing_id: ProductListingId,
        ) -> Result<Option<ContentPolicyDecision>, ProductListingContentAssessmentReadError>
        {
            if let Ok(mut state) = self.state.lock() {
                state.content_assessment_reads += 1;
            }
            match self.outcome {
                ContentAssessmentOutcome::Found(decision) => Ok(decision),
                ContentAssessmentOutcome::QueryFailure => {
                    Err(ProductListingContentAssessmentReadError::QueryFailed {
                        source: box_error(std::io::Error::other("assessment query failed")),
                    })
                }
                ContentAssessmentOutcome::InvalidPersistedState => Err(
                    ProductListingContentAssessmentReadError::InvalidPersistedState {
                        source: box_error(std::io::Error::other("assessment state invalid")),
                    },
                ),
            }
        }
    }

    impl ProductListingContentAssessmentSnapshotReaderFactory<TestTransaction> for ContentAssessments {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl ProductListingContentAssessmentSnapshotReader + 'tx {
            ContentAssessmentReader {
                state: Arc::clone(&self.state),
                outcome: self.outcome,
            }
        }
    }

    fn content_assessments(
        state: &Arc<Mutex<State>>,
        outcome: ContentAssessmentOutcome,
    ) -> ContentAssessments {
        ContentAssessments {
            state: Arc::clone(state),
            outcome,
        }
    }

    struct Quotas(Arc<Mutex<State>>);
    struct QuotaReader(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl SearchFilterMonthlyMatchQuotaReader for QuotaReader {
        async fn notification_selection_rank_for_user_in_month(
            &mut self,
            _user_id: UserId,
            _matched_at: OffsetDateTime,
            _origin_event_id: EventId,
        ) -> Result<usize, SearchFilterMonthlyMatchQuotaReadError> {
            if let Ok(mut state) = self.0.lock() {
                state.quota_reads += 1;
            }
            Ok(1)
        }
    }

    impl SearchFilterMonthlyMatchQuotaReaderFactory<TestTransaction> for Quotas {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl SearchFilterMonthlyMatchQuotaReader + 'tx {
            QuotaReader(Arc::clone(&self.0))
        }
    }

    struct Tiers;
    struct TierReader;

    #[async_trait::async_trait]
    impl UserTierEntitlements for TierReader {
        async fn lock_user_tier(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserTier>, UserTierEntitlementsError> {
            Ok(Some(UserTier::Free))
        }

        async fn reconcile_for_tier(
            &mut self,
            _user_id: UserId,
            _tier: UserTier,
        ) -> Result<(), UserTierEntitlementsError> {
            Ok(())
        }
    }

    impl UserTierEntitlementsFactory<TestTransaction> for Tiers {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl UserTierEntitlements + 'tx {
            TierReader
        }
    }

    struct Notifications(Arc<Mutex<State>>);
    struct TestNotificationCreator(Arc<Mutex<State>>);

    impl NotificationCreatorFactory<TestTransaction> for Notifications {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl NotificationCreator + 'tx {
            TestNotificationCreator(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl NotificationCreator for TestNotificationCreator {
        async fn create_many(
            &mut self,
            notifications: &[NewNotification],
        ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError> {
            if let Ok(mut state) = self.0.lock() {
                let commits = state.commits;
                state.notification_commit_counts.push(commits);
                state
                    .notification_content_policies
                    .extend(notifications.iter().filter_map(|notification| {
                        match notification.notification.content() {
                            NotificationContent::SearchFilter { snapshot, .. } => {
                                Some(snapshot.content_policy)
                            }
                            _ => None,
                        }
                    }));
            }
            Ok(notifications
                .iter()
                .map(|notification| NotificationCreationOutcome::Inserted {
                    notification_id: notification.notification.notification_id(),
                })
                .collect())
        }
    }

    struct DuplicateNotifications;
    struct DuplicateNotificationCreator;

    impl NotificationCreatorFactory<TestTransaction> for DuplicateNotifications {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl NotificationCreator + 'tx {
            DuplicateNotificationCreator
        }
    }

    #[async_trait::async_trait]
    impl NotificationCreator for DuplicateNotificationCreator {
        async fn create_many(
            &mut self,
            notifications: &[NewNotification],
        ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError> {
            Ok(notifications
                .iter()
                .map(|_| NotificationCreationOutcome::Duplicate)
                .collect())
        }
    }

    fn sources() -> Result<
        (
            GenerateSearchFilterMatchNotificationCommand,
            SearchFilterMatchNotificationSource,
            ProductListingSearchFilterMatchSource,
        ),
        url::ParseError,
    > {
        let user_id = UserId::new();
        let search_filter_id = UserSearchFilterId::new();
        let product_listing_id = ProductListingId::new();
        let origin_event_id = EventId::new();
        let command = GenerateSearchFilterMatchNotificationCommand {
            user_id,
            search_filter_id,
            product_listing_id,
            origin_event_id,
        };
        let match_source = SearchFilterMatchNotificationSource {
            user_id,
            search_filter_id,
            product_listing_id,
            origin_event_id,
            search_filter_name: UserSearchFilterName::from("daily"),
            matched_at: OffsetDateTime::UNIX_EPOCH,
            external_delivery_requested: true,
        };
        let url = Url::parse("https://example.test/product")?;
        let product = ProductListingSearchFilterMatchSource {
            event_id: origin_event_id,
            event_kind: ProductListingSearchFilterMatchSourceEventKind::Domain,
            origin_event_time: OffsetDateTime::UNIX_EPOCH,
            current_event_id: origin_event_id,
            projection_version: 1,
            product_listing_id,
            product_listing_title_slug_id: ProductListingSlugId::raw("product-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source: ListingSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
                slug_id: ListingSourceSlugId::raw("source")
                    .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
            },
            source_listing_id: SourceListingId::try_from("sku-1")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            product_title: None,
            product_description: None,
            titles: Default::default(),
            descriptions: Default::default(),
            pricing: ProductListingPricing::default(),
            sale_observation: None,
            availability: Some(ListingAvailability::Available),
            lifecycle: ListingLifecycle::Active,
            url: url.clone(),
            view_url: url,
            image: None,
            images: IndexSet::<ProductListingImage>::new(),
            embedding: None,
            auction: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        };
        Ok((command, match_source, product))
    }

    #[tokio::test]
    async fn should_create_notification_before_committing_selection() -> Result<(), Box<dyn Error>>
    {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, match_source, product) = sources()?;
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductListingSources(Some(product)),
            Quotas(Arc::clone(&state)),
            Tiers,
            content_assessments(
                &state,
                ContentAssessmentOutcome::Found(Some(ContentPolicyDecision::Allowed)),
            ),
            Notifications(Arc::clone(&state)),
        );

        assert_eq!(
            GenerateSearchFilterMatchNotificationResult::Created,
            handler.execute(command).await?
        );
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, state.commits);
        assert_eq!(vec![0], state.notification_commit_counts);
        assert_eq!(
            vec![Some(ContentPolicyDecision::Allowed)],
            state.notification_content_policies
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_snapshot_requires_consent_content_policy() -> Result<(), Box<dyn Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, match_source, product) = sources()?;
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductListingSources(Some(product)),
            Quotas(Arc::clone(&state)),
            Tiers,
            content_assessments(
                &state,
                ContentAssessmentOutcome::Found(Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany,
                ))),
            ),
            Notifications(Arc::clone(&state)),
        );

        assert_eq!(
            GenerateSearchFilterMatchNotificationResult::Created,
            handler.execute(command).await?
        );
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(
            vec![Some(ContentPolicyDecision::RequiresConsent(
                SensitiveContentCategory::NaziGermany,
            ))],
            state.notification_content_policies
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_report_exact_match_redelivery_as_deduplicated() -> Result<(), Box<dyn Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, match_source, product) = sources()?;
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductListingSources(Some(product)),
            Quotas(Arc::clone(&state)),
            Tiers,
            content_assessments(&state, ContentAssessmentOutcome::Found(None)),
            DuplicateNotifications,
        );

        assert_eq!(
            GenerateSearchFilterMatchNotificationResult::AlreadyExists,
            handler.execute(command).await?
        );
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, state.commits);
        Ok(())
    }

    #[tokio::test]
    async fn should_suppress_stale_match_source_without_creating_notification()
    -> Result<(), Box<dyn Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, mut match_source, product) = sources()?;
        match_source.origin_event_id = EventId::new();
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductListingSources(Some(product)),
            Quotas(Arc::clone(&state)),
            Tiers,
            content_assessments(&state, ContentAssessmentOutcome::Found(None)),
            Notifications(Arc::clone(&state)),
        );

        assert_eq!(
            GenerateSearchFilterMatchNotificationResult::SuppressedForStaleMatch,
            handler.execute(command).await?
        );
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, state.commits);
        assert!(state.notification_commit_counts.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_suppress_withdrawn_current_product_before_assessment_snapshot()
    -> Result<(), Box<dyn Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, match_source, mut product) = sources()?;
        product.lifecycle = ListingLifecycle::Withdrawn;
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductListingSources(Some(product)),
            Quotas(Arc::clone(&state)),
            Tiers,
            content_assessments(&state, ContentAssessmentOutcome::Found(None)),
            Notifications(Arc::clone(&state)),
        );

        assert_eq!(
            GenerateSearchFilterMatchNotificationResult::SuppressedForWithdrawnProductListing,
            handler.execute(command).await?
        );
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, state.commits);
        assert_eq!(0, state.content_assessment_reads);
        assert_eq!(0, state.quota_reads);
        assert!(state.notification_commit_counts.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_create_notification_when_current_product_has_unrelated_newer_event()
    -> Result<(), Box<dyn Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, match_source, mut product) = sources()?;
        product.current_event_id = EventId::new();
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductListingSources(Some(product)),
            Quotas(Arc::clone(&state)),
            Tiers,
            content_assessments(&state, ContentAssessmentOutcome::Found(None)),
            Notifications(Arc::clone(&state)),
        );

        assert_eq!(
            GenerateSearchFilterMatchNotificationResult::Created,
            handler.execute(command).await?
        );
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, state.content_assessment_reads);
        assert_eq!(1, state.quota_reads);
        assert_eq!(vec![0], state.notification_commit_counts);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_content_assessment_reader_errors() -> Result<(), Box<dyn Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, match_source, product) = sources()?;
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductListingSources(Some(product)),
            Quotas(Arc::clone(&state)),
            Tiers,
            content_assessments(&state, ContentAssessmentOutcome::QueryFailure),
            Notifications(Arc::clone(&state)),
        );

        let error = handler
            .execute(command)
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("expected error"))?;
        assert!(matches!(
            error,
            GenerateSearchFilterMatchNotificationError::ContentAssessmentReadFailed { .. }
        ));
        assert!(matches!(
            Error::source(&error).and_then(|source| {
                source.downcast_ref::<ProductListingContentAssessmentReadError>()
            }),
            Some(ProductListingContentAssessmentReadError::QueryFailed { .. })
        ));

        let (command, match_source, product) = sources()?;
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductListingSources(Some(product)),
            Quotas(Arc::clone(&state)),
            Tiers,
            content_assessments(&state, ContentAssessmentOutcome::InvalidPersistedState),
            Notifications(state),
        );

        let error = handler
            .execute(command)
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("expected error"))?;
        assert!(matches!(
            error,
            GenerateSearchFilterMatchNotificationError::ContentAssessmentStateInvalid { .. }
        ));
        assert!(Error::source(&error).is_some());
        Ok(())
    }

    #[tokio::test]
    async fn should_preserve_match_source_query_failure_in_service_error_chain()
    -> Result<(), Box<dyn Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, _, product) = sources()?;
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::QueryFailure,
            ProductListingSources(Some(product)),
            Quotas(Arc::clone(&state)),
            Tiers,
            content_assessments(&state, ContentAssessmentOutcome::Found(None)),
            Notifications(state),
        );

        let error = handler
            .execute(command)
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("expected error"))?;
        assert!(matches!(
            error,
            GenerateSearchFilterMatchNotificationError::MatchSourceReadFailed { .. }
        ));
        assert!(Error::source(&error).is_some());
        Ok(())
    }
}
