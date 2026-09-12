use crate::ports::{
    ListingSourceSummaryReader, ProductListingContentAssessmentReader,
    ProductListingEmbeddingLookup, ProductListingEmbeddingReadError, ProductListingEmbeddingReader,
    ProductListingEmbeddingReaderFactory, ProductListingPriceFilterPlan,
    ProductListingSimilarProductListingsReadError, ProductListingSimilarProductListingsReader,
    ProductListingSimilarProductListingsRequest, ProductListingUserStateReader,
};
use crate::use_cases::PersonalizedProductListingSummary;
use crate::use_cases::queries::product_listing_summary_personalization::{
    ProductListingSummaryPersonalizationError, hydrate_listing_source_summaries,
    hydrate_product_search_items,
};
use crate::use_cases::queries::search_product_listings::present_product_summaries;
use application::error::{BoxError, box_error};
use application::operation_context::{OperationContext, Principal};
use application::personalized::Personalized;
use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::ListingSourceId;
use localization::Language;
use money::Currency;

use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};

use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct GetSimilarProductListingsRequest {
    pub lookup: ProductListingEmbeddingLookup,
    pub language: Language,
    pub currency: Currency,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GetSimilarProductListingsResult {
    EmbeddingPending,
    Ready(Vec<PersonalizedProductListingSummary>),
}

#[derive(Debug, thiserror::Error)]
pub enum GetSimilarProductListingsError {
    #[error("product not found")]
    NotFound,
    #[error("product embedding query failed")]
    ProductListingEmbeddingQueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("similarity search is unavailable")]
    SimilaritySearchUnavailable,
    #[error("failed to begin get similar products transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get similar products transaction")]
    CommitTransactionFailed,
    #[error("no persisted FX snapshot is available for similar product pricing")]
    PricingFxSnapshotMissing,
    #[error("FX snapshot lookup is temporarily unavailable for similar product pricing")]
    PricingFxSnapshotUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("persisted FX snapshot is invalid for similar product pricing")]
    PricingFxSnapshotInvalid {
        #[source]
        source: BoxError,
    },
    #[error("listing source summary query failed")]
    ListingSourceSummaryQueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("listing source summary read model is invalid")]
    ListingSourceSummaryReadModelInvalid {
        #[source]
        source: BoxError,
    },
    #[error("listing source summary is missing for listing source {listing_source_id}")]
    ListingSourceSummaryMissing { listing_source_id: ListingSourceId },
    #[error("product user state query failed")]
    ProductListingUserStateQueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product user state read model is invalid")]
    ProductListingUserStateReadModelInvalid {
        #[source]
        source: BoxError,
    },

    #[error("product user state is missing")]
    ProductListingUserStateMissing,
    #[error("hidden product summary could not be constructed")]
    HiddenProductListingSummaryInvalid {
        #[source]
        source: BoxError,
    },
    #[error("product content assessment query failed")]
    ContentAssessmentQueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product content assessment state is invalid")]
    ContentAssessmentStateInvalid {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait GetSimilarProductListingsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetSimilarProductListingsRequest,
    ) -> Result<GetSimilarProductListingsResult, GetSimilarProductListingsError>;
}

pub struct GetSimilarProductListingsHandler<U, E, F, S, L, P, A> {
    unit_of_work: U,
    embedding_reader: E,
    fx_rates: F,
    similar_products_reader: S,
    listing_sources: L,
    user_states: P,
    assessments: A,
}

impl<U, E, F, S, L, P, A> GetSimilarProductListingsHandler<U, E, F, S, L, P, A> {
    pub fn new(
        unit_of_work: U,
        embedding_reader: E,
        fx_rates: F,
        similar_products_reader: S,
        listing_sources: L,
        user_states: P,
        assessments: A,
    ) -> Self {
        Self {
            unit_of_work,
            embedding_reader,
            fx_rates,
            similar_products_reader,
            listing_sources,
            user_states,
            assessments,
        }
    }
}

#[async_trait::async_trait]
impl<U, E, F, S, L, P, A> GetSimilarProductListingsUseCase
    for GetSimilarProductListingsHandler<U, E, F, S, L, P, A>
where
    U: UnitOfWork,
    E: ProductListingEmbeddingReaderFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    S: ProductListingSimilarProductListingsReader,
    L: ListingSourceSummaryReader,
    P: ProductListingUserStateReader,
    A: ProductListingContentAssessmentReader,
{
    #[tracing::instrument(
        name = "get_similar_products",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetSimilarProductListingsRequest,
    ) -> Result<GetSimilarProductListingsResult, GetSimilarProductListingsError> {
        let valuation_at = OffsetDateTime::now_utc();
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetSimilarProductListingsError::BeginTransactionFailed)?;
        let seed = self
            .embedding_reader
            .in_transaction(&mut tx)
            .find_embedding(&request.lookup)
            .await?
            .ok_or(GetSimilarProductListingsError::NotFound)?;

        let Some(embedding) = seed.embedding else {
            tx.commit()
                .await
                .map_err(|_| GetSimilarProductListingsError::CommitTransactionFailed)?;

            return Ok(GetSimilarProductListingsResult::EmbeddingPending);
        };

        let snapshot = self
            .fx_rates
            .in_transaction(&mut tx)
            .find_latest_at_or_before(valuation_at)
            .await?
            .ok_or(GetSimilarProductListingsError::PricingFxSnapshotMissing)?;
        let price_filter_plan =
            ProductListingPriceFilterPlan::compile(snapshot, request.currency, None).map_err(
                |source| GetSimilarProductListingsError::PricingFxSnapshotInvalid {
                    source: box_error(source),
                },
            )?;

        tx.commit()
            .await
            .map_err(|_| GetSimilarProductListingsError::CommitTransactionFailed)?;

        let products = self
            .similar_products_reader
            .find_similar_product_listings(&ProductListingSimilarProductListingsRequest::new(
                seed.product_listing_id,
                embedding,
                request.language,
                price_filter_plan,
            ))
            .await?;
        let mut products = hydrate_listing_source_summaries(products, &self.listing_sources)
            .await?
            .into_iter()
            .map(|item| Personalized {
                item,
                user_state: None,
            })
            .collect::<Vec<_>>();
        if let Some(user_id) = personalization_user_id(&context.principal) {
            hydrate_product_search_items(&mut products, user_id, &self.user_states).await?;
        }
        let products = present_product_summaries(products, &self.assessments)
            .await
            .map_err(GetSimilarProductListingsError::from)?;

        Ok(GetSimilarProductListingsResult::Ready(products))
    }
}

fn personalization_user_id(principal: &Principal) -> Option<user_core::user_id::UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

impl From<ProductListingEmbeddingReadError> for GetSimilarProductListingsError {
    fn from(error: ProductListingEmbeddingReadError) -> Self {
        match error {
            ProductListingEmbeddingReadError::ProductListingEmbeddingQueryFailed { source } => {
                Self::ProductListingEmbeddingQueryFailed { source }
            }
        }
    }
}

impl From<ProductListingSimilarProductListingsReadError> for GetSimilarProductListingsError {
    fn from(_: ProductListingSimilarProductListingsReadError) -> Self {
        Self::SimilaritySearchUnavailable
    }
}

impl From<FxRateSnapshotRepositoryError> for GetSimilarProductListingsError {
    fn from(error: FxRateSnapshotRepositoryError) -> Self {
        match error {
            FxRateSnapshotRepositoryError::InsertFailed { source }
            | FxRateSnapshotRepositoryError::ReadFailed { source } => {
                Self::PricingFxSnapshotUnavailable { source }
            }
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
                Self::PricingFxSnapshotInvalid { source }
            }
            FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => Self::PricingFxSnapshotMissing,
        }
    }
}

impl From<crate::ports::ProductListingContentAssessmentReadError>
    for GetSimilarProductListingsError
{
    fn from(error: crate::ports::ProductListingContentAssessmentReadError) -> Self {
        match error {
            crate::ports::ProductListingContentAssessmentReadError::QueryFailed { source } => {
                Self::ContentAssessmentQueryFailed { source }
            }
            crate::ports::ProductListingContentAssessmentReadError::InvalidPersistedState {
                source,
            } => Self::ContentAssessmentStateInvalid { source },
        }
    }
}

impl From<ProductListingSummaryPersonalizationError> for GetSimilarProductListingsError {
    fn from(error: ProductListingSummaryPersonalizationError) -> Self {
        match error {
            ProductListingSummaryPersonalizationError::ListingSourceSummaryQueryFailed {
                source,
            } => Self::ListingSourceSummaryQueryFailed { source },
            ProductListingSummaryPersonalizationError::ListingSourceSummaryReadModelInvalid {
                source,
            } => Self::ListingSourceSummaryReadModelInvalid { source },
            ProductListingSummaryPersonalizationError::ListingSourceSummaryMissing {
                listing_source_id,
            } => Self::ListingSourceSummaryMissing { listing_source_id },
            ProductListingSummaryPersonalizationError::ViewUrlInvalid { source } => {
                Self::ListingSourceSummaryReadModelInvalid { source }
            }
            ProductListingSummaryPersonalizationError::UserStateQueryFailed { source } => {
                Self::ProductListingUserStateQueryFailed { source }
            }
            ProductListingSummaryPersonalizationError::UserStateReadModelInvalid { source } => {
                Self::ProductListingUserStateReadModelInvalid { source }
            }

            ProductListingSummaryPersonalizationError::UserStateMissing { .. } => {
                Self::ProductListingUserStateMissing
            }
            ProductListingSummaryPersonalizationError::HiddenProductListingSummaryInvalid {
                source,
            } => Self::HiddenProductListingSummaryInvalid { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::ListingSourceSummary;
    use crate::ports::{ProductListingEmbedding, ProductListingSimilarProductListingsReadError};
    use crate::use_cases::{ProductListingSearchItem, ProductListingSummaryPriceValuation};
    use application::{
        error::box_error,
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use domain_primitives::event_id::EventId;
    use fxrate_core::FxRateId;
    use fxrate_core::{FX_RATE_SCALE, FxRateQuote, FxRateSource, NewFxRateSnapshot};
    use indexmap::IndexSet;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::Localized;
    use money::{Currency, MonetaryAmount, Price};
    use product_listing_core::{
        content_policy::ContentPolicyDecision, listing_availability::ListingAvailability,
        listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
        product_listing_image::ProductListingImage, product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
    };

    use crate::user_state::ProductListingUserState;
    use product_listing_core::title::Title;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, MutexGuard};
    use strum::IntoEnumIterator;
    use time::OffsetDateTime;
    use url::Url;

    #[derive(Debug, Default)]
    struct FakeState {
        begin_error: bool,
        commit_error: bool,
        find_embedding_result:
            Option<Result<Option<ProductListingEmbedding>, ProductListingEmbeddingReadError>>,
        find_similar_product_listings_result: Option<
            Result<Vec<ProductListingSearchItem>, ProductListingSimilarProductListingsReadError>,
        >,
        requested_product_listing_ids: Vec<ProductListingId>,
        requested_similar_products: Vec<ProductListingSimilarProductListingsRequest>,
        commit_count: usize,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeEmbeddingReaderFactory {
        state: SharedState,
    }

    struct FakeTx {
        state: SharedState,
    }

    struct FakeEmbeddingReader {
        state: SharedState,
    }

    #[derive(Clone, Copy)]
    struct FakeFxRateSnapshotRepositoryFactory;

    struct FakeFxRateSnapshotRepository;

    #[derive(Clone)]
    struct FakeSimilarProductsReader {
        state: SharedState,
    }

    #[derive(Clone, Copy)]
    struct EmptyUserStateReader;

    #[derive(Clone, Copy)]
    struct StaticListingSourceSummaryReader;

    #[derive(Clone, Copy)]
    struct EmptyAssessmentReader;

    #[derive(Clone)]
    struct StaticAssessmentReader {
        assessments: HashMap<ProductListingId, crate::ports::ProductListingContentAssessment>,
        requests: Arc<Mutex<Vec<Vec<ProductListingId>>>>,
    }

    #[derive(Clone)]
    struct StaticUserStateReader {
        states: HashMap<ProductListingId, ProductListingUserState>,
    }

    fn state() -> SharedState {
        Arc::new(Mutex::new(FakeState::default()))
    }

    fn lock_state(state: &SharedState) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            if lock_state(&self.state).begin_error {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(FakeTx {
                    state: Arc::clone(&self.state),
                })
            }
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock_state(&self.state);
            state.commit_count += 1;
            if state.commit_error {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl FxRateSnapshotRepositoryFactory<FakeTx> for FakeFxRateSnapshotRepositoryFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl FxRateSnapshotRepository + 'tx {
            FakeFxRateSnapshotRepository
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for FakeFxRateSnapshotRepository {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Some(test_fx_snapshot()))
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Some(test_fx_snapshot()))
        }

        async fn find_by_id(
            &mut self,
            _id: FxRateId,
        ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Some(test_fx_snapshot()))
        }

        async fn find_by_ids(
            &mut self,
            _ids: &[FxRateId],
        ) -> Result<Vec<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(vec![test_fx_snapshot()])
        }

        async fn insert(
            &mut self,
            _snapshot: &NewFxRateSnapshot,
            _source_event_id: &str,
        ) -> Result<fxrate_service::ports::FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>
        {
            Ok(fxrate_service::ports::FxRateSnapshotInsertOutcome::Duplicate)
        }
    }

    fn test_fx_snapshot() -> fxrate_core::FxRateSnapshot {
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
        )
        .and_then(|snapshot| Ok(snapshot.into_persisted(1_i64.try_into()?)))
        .unwrap_or_else(|error| panic!("test FX snapshot must be valid: {error}"))
    }

    impl ProductListingEmbeddingReaderFactory<FakeTx> for FakeEmbeddingReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl ProductListingEmbeddingReader + 'tx {
            FakeEmbeddingReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductListingEmbeddingReader for FakeEmbeddingReader {
        async fn find_embedding(
            &mut self,
            lookup: &ProductListingEmbeddingLookup,
        ) -> Result<Option<ProductListingEmbedding>, ProductListingEmbeddingReadError> {
            let product_listing_id = match lookup {
                ProductListingEmbeddingLookup::ById(product_listing_id) => *product_listing_id,
                ProductListingEmbeddingLookup::ByTitleSlug(_) => ProductListingId::new(),
            };
            let mut state = lock_state(&self.state);
            state.requested_product_listing_ids.push(product_listing_id);
            match state.find_embedding_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl ListingSourceSummaryReader for StaticListingSourceSummaryReader {
        async fn find_summaries(
            &self,
            listing_source_ids: &[ListingSourceId],
        ) -> Result<
            HashMap<ListingSourceId, crate::ports::ListingSourceSummaryWithReferral>,
            crate::ports::ListingSourceSummaryReadError,
        > {
            Ok(listing_source_ids
                .iter()
                .copied()
                .map(|listing_source_id| {
                    (
                        listing_source_id,
                        crate::ports::ListingSourceSummaryWithReferral {
                            summary: ListingSourceSummary {
                                listing_source_id,
                                name: ListingSourceName::try_from("Source").unwrap_or_else(
                                    |error| panic!("invalid test listing source name: {error}"),
                                ),
                                slug_id: ListingSourceSlugId::raw("source").unwrap_or_else(
                                    |error| panic!("valid test listing source slug: {error}"),
                                ),
                            },
                            referral_configuration: None,
                        },
                    )
                })
                .collect())
        }
    }

    #[async_trait::async_trait]
    impl ProductListingContentAssessmentReader for EmptyAssessmentReader {
        async fn find_current_assessments(
            &self,
            _product_listing_ids: &[ProductListingId],
        ) -> Result<
            HashMap<ProductListingId, crate::ports::ProductListingContentAssessment>,
            crate::ports::ProductListingContentAssessmentReadError,
        > {
            Ok(HashMap::new())
        }
    }

    #[async_trait::async_trait]
    impl ProductListingContentAssessmentReader for StaticAssessmentReader {
        async fn find_current_assessments(
            &self,
            product_listing_ids: &[ProductListingId],
        ) -> Result<
            HashMap<ProductListingId, crate::ports::ProductListingContentAssessment>,
            crate::ports::ProductListingContentAssessmentReadError,
        > {
            match self.requests.lock() {
                Ok(mut requests) => requests.push(product_listing_ids.to_vec()),
                Err(poisoned) => poisoned.into_inner().push(product_listing_ids.to_vec()),
            }
            Ok(self.assessments.clone())
        }
    }

    #[async_trait::async_trait]
    impl ProductListingUserStateReader for EmptyUserStateReader {
        async fn find_for_user(
            &self,
            _lookup: &crate::ports::ProductListingUserStateLookup,
        ) -> Result<
            HashMap<ProductListingId, ProductListingUserState>,
            crate::ports::ProductListingUserStateReadError,
        > {
            Ok(HashMap::new())
        }
    }

    #[async_trait::async_trait]
    impl ProductListingUserStateReader for StaticUserStateReader {
        async fn find_for_user(
            &self,
            _lookup: &crate::ports::ProductListingUserStateLookup,
        ) -> Result<
            HashMap<ProductListingId, ProductListingUserState>,
            crate::ports::ProductListingUserStateReadError,
        > {
            Ok(self.states.clone())
        }
    }

    #[async_trait::async_trait]
    impl ProductListingSimilarProductListingsReader for FakeSimilarProductsReader {
        async fn find_similar_product_listings(
            &self,
            request: &ProductListingSimilarProductListingsRequest,
        ) -> Result<Vec<ProductListingSearchItem>, ProductListingSimilarProductListingsReadError>
        {
            let mut state = lock_state(&self.state);
            state.requested_similar_products.push(request.clone());
            match state.find_similar_product_listings_result.take() {
                Some(result) => result,
                None => Ok(Vec::new()),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> GetSimilarProductListingsHandler<
        FakeUnitOfWork,
        FakeEmbeddingReaderFactory,
        FakeFxRateSnapshotRepositoryFactory,
        FakeSimilarProductsReader,
        StaticListingSourceSummaryReader,
        EmptyUserStateReader,
        EmptyAssessmentReader,
    > {
        GetSimilarProductListingsHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeEmbeddingReaderFactory {
                state: Arc::clone(state),
            },
            FakeFxRateSnapshotRepositoryFactory,
            FakeSimilarProductsReader {
                state: Arc::clone(state),
            },
            StaticListingSourceSummaryReader,
            EmptyUserStateReader,
            EmptyAssessmentReader,
        )
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn authenticated_context(user_id: user_core::user_id::UserId) -> OperationContext {
        OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn product_search_item(
        product_listing_id: ProductListingId,
    ) -> Result<ProductListingSearchItem, url::ParseError> {
        Ok(ProductListingSearchItem {
            product_listing_id,
            product_listing_title_slug_id: ProductListingSlugId::raw("cabinet-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            event_id: EventId::new(),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("cabinet-1")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            auction_id: None,
            has_auction_context: false,
            title: Some(Localized::new(Language::En, Title::from("Cabinet"))),
            display_price: Some(
                product_listing_core::product_listing_price::ProductListingPrice::from(Price::new(
                    MonetaryAmount::from(100_u64),
                    Currency::Eur,
                )),
            ),
            price_valuation: ProductListingSummaryPriceValuation::Current {
                fx_rate_id: FxRateId::new(),
                captured_at: OffsetDateTime::UNIX_EPOCH,
            },
            availability: Some(ListingAvailability::InStock),
            lifecycle: ListingLifecycle::Active,
            url: Url::parse("https://shop.example/products/1")?,
            images: IndexSet::new(),
            updated: OffsetDateTime::UNIX_EPOCH,
        })
    }

    fn product_search_item_with_image(
        product_listing_id: ProductListingId,
    ) -> Result<ProductListingSearchItem, url::ParseError> {
        let mut item = product_search_item(product_listing_id)?;
        item.images = IndexSet::from([ProductListingImage::new(Url::parse(
            "https://shop.example/images/cabinet.jpg",
        )?)]);
        Ok(item)
    }

    fn request() -> GetSimilarProductListingsRequest {
        GetSimilarProductListingsRequest {
            lookup: ProductListingEmbeddingLookup::ById(ProductListingId::new()),
            language: Language::En,
            currency: Currency::Eur,
        }
    }

    #[tokio::test]
    async fn should_return_embedding_pending_when_product_embedding_is_missing() {
        let state = state();
        let request = request();
        lock_state(&state).find_embedding_result = Some(Ok(Some(ProductListingEmbedding {
            product_listing_id: ProductListingId::new(),
            embedding: None,
        })));

        let result = handler(&state).execute(&context(), request.clone()).await;

        assert!(matches!(
            result,
            Ok(GetSimilarProductListingsResult::EmbeddingPending)
        ));
        assert_eq!(
            vec![match request.lookup {
                ProductListingEmbeddingLookup::ById(product_listing_id) => product_listing_id,
                ProductListingEmbeddingLookup::ByTitleSlug(_) => ProductListingId::new(),
            }],
            lock_state(&state).requested_product_listing_ids
        );
        assert_eq!(1, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_return_not_found_when_product_listing_id_is_missing() {
        let state = state();

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(GetSimilarProductListingsError::NotFound)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_return_ready_products_when_embedding_is_available() {
        let state = state();
        let product_listing_id = ProductListingId::new();
        lock_state(&state).find_embedding_result = Some(Ok(Some(ProductListingEmbedding {
            product_listing_id,
            embedding: Some(vec![0.1_f32]),
        })));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(
            matches!(result, Ok(GetSimilarProductListingsResult::Ready(products)) if products.is_empty())
        );
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert_eq!(1, state.requested_similar_products.len());
        assert_eq!(
            product_listing_id,
            state.requested_similar_products[0].product_listing_id
        );
        assert_eq!(vec![0.1_f32], state.requested_similar_products[0].embedding);
        assert_eq!(Language::En, state.requested_similar_products[0].language);
    }

    #[tokio::test]
    async fn should_hydrate_ready_similar_products_for_authenticated_user()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let user_id = user_core::user_id::UserId::new();
        let product_listing_id = ProductListingId::new();
        let mut user_state = ProductListingUserState::default();
        user_state.watchlist.watching = true;
        lock_state(&state).find_embedding_result = Some(Ok(Some(ProductListingEmbedding {
            product_listing_id: ProductListingId::new(),
            embedding: Some(vec![0.1_f32]),
        })));
        lock_state(&state).find_similar_product_listings_result =
            Some(Ok(vec![product_search_item(product_listing_id)?]));
        let handler = GetSimilarProductListingsHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(&state),
            },
            FakeEmbeddingReaderFactory {
                state: Arc::clone(&state),
            },
            FakeFxRateSnapshotRepositoryFactory,
            FakeSimilarProductsReader {
                state: Arc::clone(&state),
            },
            StaticListingSourceSummaryReader,
            StaticUserStateReader {
                states: HashMap::from([(product_listing_id, user_state)]),
            },
            EmptyAssessmentReader,
        );

        let result = handler
            .execute(&authenticated_context(user_id), request())
            .await?;

        let GetSimilarProductListingsResult::Ready(products) = result else {
            return Err(std::io::Error::other("expected ready similar products").into());
        };
        assert_eq!(
            Some(true),
            products[0]
                .user_state
                .as_ref()
                .map(|state| state.watchlist.watching)
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_present_similar_raw_items_after_one_batched_content_assessment()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let product_listing_id = ProductListingId::new();
        let item = product_search_item_with_image(product_listing_id)?;
        let image_url = item
            .images
            .first()
            .ok_or("missing raw image")?
            .url()
            .clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        lock_state(&state).find_embedding_result = Some(Ok(Some(ProductListingEmbedding {
            product_listing_id: ProductListingId::new(),
            embedding: Some(vec![0.1_f32]),
        })));
        lock_state(&state).find_similar_product_listings_result = Some(Ok(vec![item]));
        let handler = GetSimilarProductListingsHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(&state),
            },
            FakeEmbeddingReaderFactory {
                state: Arc::clone(&state),
            },
            FakeFxRateSnapshotRepositoryFactory,
            FakeSimilarProductsReader {
                state: Arc::clone(&state),
            },
            StaticListingSourceSummaryReader,
            EmptyUserStateReader,
            StaticAssessmentReader {
                assessments: HashMap::from([(
                    product_listing_id,
                    crate::ports::ProductListingContentAssessment {
                        product_listing_id,
                        source_event_id: EventId::new(),
                        decision: ContentPolicyDecision::Allowed,
                    },
                )]),
                requests: Arc::clone(&requests),
            },
        );

        let result = handler.execute(&context(), request()).await?;

        let GetSimilarProductListingsResult::Ready(products) = result else {
            return Err(std::io::Error::other("expected ready similar products").into());
        };
        assert_eq!(
            Some(ContentPolicyDecision::Allowed),
            products[0].item.content_policy
        );
        assert_eq!(
            vec![crate::use_cases::ProductListingImageView {
                url: Some(image_url),
            }],
            products[0].item.images
        );
        let requests = match requests.lock() {
            Ok(requests) => requests,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert_eq!(vec![vec![product_listing_id]], *requests);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_similar_products_reader_failure_to_unavailable_after_commit() {
        let state = state();
        lock_state(&state).find_embedding_result = Some(Ok(Some(ProductListingEmbedding {
            product_listing_id: ProductListingId::new(),
            embedding: Some(vec![0.1_f32]),
        })));
        lock_state(&state).find_similar_product_listings_result = Some(Err(
            ProductListingSimilarProductListingsReadError::SimilarProductListingsQueryFailed {
                source: box_error(std::io::Error::other("boom")),
            },
        ));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(GetSimilarProductListingsError::SimilaritySearchUnavailable)
        ));
        assert_eq!(1, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_map_embedding_query_failure() {
        let state = state();
        lock_state(&state).find_embedding_result = Some(Err(
            ProductListingEmbeddingReadError::ProductListingEmbeddingQueryFailed {
                source: box_error(std::io::Error::other("boom")),
            },
        ));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(GetSimilarProductListingsError::ProductListingEmbeddingQueryFailed { .. })
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }
}
