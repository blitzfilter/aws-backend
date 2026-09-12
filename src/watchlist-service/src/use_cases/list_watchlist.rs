use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::pagination::{Cursor, CursoredResult};
use application::transaction::{Transaction, UnitOfWork};
use fxrate_core::{FxRateId, FxRateSnapshot, FxRateSnapshotError};
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use localization::Language;
use money::Currency;
use product_listing_core::{
    listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
};
use user_core::user_id::UserId;

use product_listing_service::ports::{
    ProductListingWatchlistDetailsCursor, ProductListingWatchlistDetailsReadError,
    ProductListingWatchlistDetailsReader, ProductListingWatchlistDetailsReaderFactory,
    ProductListingWatchlistDetailsRequest,
};
use product_listing_service::use_cases::{
    PersonalizedProductListingDetailsView, ProductListingPricingPresentationError,
    present_product_details, redact_hidden_product,
};
use std::collections::{HashMap, HashSet};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct ListWatchlistRequest {
    pub user_id: UserId,
    pub language: Language,
    pub currency: Currency,
    pub cursor: Cursor<ProductListingWatchlistDetailsCursor>,
}

pub type ListWatchlistResult =
    CursoredResult<PersonalizedProductListingDetailsView, ProductListingWatchlistDetailsCursor>;

#[derive(Debug, thiserror::Error)]
pub enum ListWatchlistError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary watchlist read failure")]
    TemporarilyUnavailable,
    #[error("invalid persisted watchlist state")]
    InvalidPersistedState,
    #[error("no persisted FX snapshot is available for current product pricing")]
    CurrentPricingFxSnapshotMissing,
    #[error("sale valuation FX snapshot is missing")]
    SalePricingFxSnapshotMissing { fx_rate_id: FxRateId },
    #[error("FX snapshot lookup is temporarily unavailable for watchlist pricing")]
    PricingFxSnapshotUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("persisted FX snapshot is invalid for watchlist pricing")]
    PricingFxSnapshotInvalid {
        #[source]
        source: BoxError,
    },
    #[error("sale valuation FX snapshot does not match")]
    SaleFxSnapshotMismatch {
        expected: FxRateId,
        actual: FxRateId,
    },
    #[error("watchlist product price conversion failed")]
    ProductListingPriceConversionFailed {
        #[source]
        source: FxRateSnapshotError,
    },

    #[error("failed to begin watchlist transaction")]
    BeginTransactionFailed,
    #[error("failed to commit watchlist transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ListWatchlistUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListWatchlistRequest,
    ) -> Result<ListWatchlistResult, ListWatchlistError>;
}

pub struct ListWatchlistHandler<U, D, F> {
    unit_of_work: U,
    details_reader: D,
    fx_rates: F,
}

impl<U, D, F> ListWatchlistHandler<U, D, F> {
    pub fn new(unit_of_work: U, details_reader: D, fx_rates: F) -> Self {
        Self {
            unit_of_work,
            details_reader,
            fx_rates,
        }
    }
}

#[async_trait::async_trait]
impl<U, D, F> ListWatchlistUseCase for ListWatchlistHandler<U, D, F>
where
    U: UnitOfWork,
    D: ProductListingWatchlistDetailsReaderFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(name = "list_watchlist", skip_all, fields(user_id = %request.user_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListWatchlistRequest,
    ) -> Result<ListWatchlistResult, ListWatchlistError> {
        authorize_read(context, request.user_id)?;

        let valuation_at = OffsetDateTime::now_utc();
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListWatchlistError::BeginTransactionFailed)?;
        let cursor = Cursor {
            size: request.cursor.size.clamp(1, 100),
            search_after: request.cursor.search_after,
        };
        let factual_page = self
            .details_reader
            .in_transaction(&mut tx)
            .find_for_user(&ProductListingWatchlistDetailsRequest {
                user_id: request.user_id,
                language: request.language,
                cursor,
            })
            .await?;
        let pricing_snapshots =
            pricing_snapshots(&self.fx_rates, &mut tx, &factual_page.items, valuation_at).await?;
        let CursoredResult {
            items,
            cursor,
            total,
        } = factual_page;
        let mut page = CursoredResult {
            items: items
                .into_iter()
                .map(|factual_details| {
                    present_with_pricing_snapshot(
                        factual_details,
                        &pricing_snapshots,
                        request.currency,
                    )
                })
                .collect::<Result<_, _>>()?,
            cursor,
            total,
        };

        tx.commit()
            .await
            .map_err(|_| ListWatchlistError::CommitTransactionFailed)?;

        for product in &mut page.items {
            let user_state = product
                .user_state
                .as_ref()
                .ok_or(ListWatchlistError::InvalidPersistedState)?;
            if user_state.search_filter.hidden {
                redact_hidden_product(&mut product.item)
                    .map_err(|_| ListWatchlistError::InvalidPersistedState)?;
            }
        }

        Ok(page)
    }
}

struct PricingSnapshots {
    current: Option<FxRateSnapshot>,
    sale: HashMap<FxRateId, FxRateSnapshot>,
}

async fn pricing_snapshots<Tx, F>(
    fx_rates: &F,
    tx: &mut Tx,
    factual_details: &[product_listing_service::ports::PersonalizedProductListingDetailsReadModel],
    valuation_at: OffsetDateTime,
) -> Result<PricingSnapshots, ListWatchlistError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    let sale_snapshot_ids: HashSet<_> = factual_details
        .iter()
        .filter_map(|details| {
            details
                .item
                .sale_observation
                .filter(|_| {
                    details.item.availability == Some(ListingAvailability::SoldOut)
                        || details.item.lifecycle == ListingLifecycle::Withdrawn
                })
                .map(|observation| observation.fx_rate_id())
        })
        .collect();
    let current = if factual_details.iter().any(|details| {
        details.item.sale_observation.is_none()
            || (details.item.availability != Some(ListingAvailability::SoldOut)
                && details.item.lifecycle != ListingLifecycle::Withdrawn)
    }) {
        Some(
            fx_rates
                .in_transaction(tx)
                .find_latest_at_or_before(valuation_at)
                .await?
                .ok_or(ListWatchlistError::CurrentPricingFxSnapshotMissing)?,
        )
    } else {
        None
    };
    let sale_snapshot_ids = sale_snapshot_ids.into_iter().collect::<Vec<_>>();
    let sale = if sale_snapshot_ids.is_empty() {
        HashMap::new()
    } else {
        fx_rates
            .in_transaction(tx)
            .find_by_ids(&sale_snapshot_ids)
            .await?
            .into_iter()
            .map(|snapshot| (snapshot.id(), snapshot))
            .collect()
    };

    Ok(PricingSnapshots { current, sale })
}

fn present_with_pricing_snapshot(
    factual_details: product_listing_service::ports::PersonalizedProductListingDetailsReadModel,
    pricing_snapshots: &PricingSnapshots,
    currency: Currency,
) -> Result<PersonalizedProductListingDetailsView, ListWatchlistError> {
    let snapshot = match factual_details.item.sale_observation.filter(|_| {
        factual_details.item.availability == Some(ListingAvailability::SoldOut)
            || factual_details.item.lifecycle == ListingLifecycle::Withdrawn
    }) {
        Some(observation) => pricing_snapshots
            .sale
            .get(&observation.fx_rate_id())
            .ok_or(ListWatchlistError::SalePricingFxSnapshotMissing {
                fx_rate_id: observation.fx_rate_id(),
            })?,
        None => pricing_snapshots
            .current
            .as_ref()
            .ok_or(ListWatchlistError::CurrentPricingFxSnapshotMissing)?,
    };
    Ok(present_product_details(
        factual_details,
        snapshot,
        currency,
    )?)
}

fn authorize_read(context: &OperationContext, user_id: UserId) -> Result<(), ListWatchlistError> {
    context
        .require()
        .credential_capability(CredentialCapability::WatchlistRead)
        .user(&user_id)
        .service_or_system()
        .authorize::<ListWatchlistError>()
}

impl From<OperationAuthorizationError> for ListWatchlistError {
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

impl From<ProductListingWatchlistDetailsReadError> for ListWatchlistError {
    fn from(error: ProductListingWatchlistDetailsReadError) -> Self {
        match error {
            ProductListingWatchlistDetailsReadError::QueryFailed => Self::TemporarilyUnavailable,
            ProductListingWatchlistDetailsReadError::InvalidReadModel => {
                Self::InvalidPersistedState
            }
        }
    }
}

impl From<FxRateSnapshotRepositoryError> for ListWatchlistError {
    fn from(error: FxRateSnapshotRepositoryError) -> Self {
        match error {
            FxRateSnapshotRepositoryError::InsertFailed { source }
            | FxRateSnapshotRepositoryError::ReadFailed { source } => {
                Self::PricingFxSnapshotUnavailable { source }
            }
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
                Self::PricingFxSnapshotInvalid { source }
            }
            FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => {
                Self::CurrentPricingFxSnapshotMissing
            }
        }
    }
}

impl From<ProductListingPricingPresentationError> for ListWatchlistError {
    fn from(error: ProductListingPricingPresentationError) -> Self {
        match error {
            ProductListingPricingPresentationError::SaleObservationFxSnapshotMismatch {
                expected,
                actual,
            } => Self::SaleFxSnapshotMismatch { expected, actual },
            ProductListingPricingPresentationError::PriceConversionFailed { source } => {
                Self::ProductListingPriceConversionFailed { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::box_error;
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::personalized::Personalized;
    use application::transaction::TransactionError;
    use domain_primitives::event_id::EventId;
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::Localized;
    use money::{MonetaryAmount, Price};
    use product_listing_core::listing_availability::ListingAvailability;
    use product_listing_core::listing_lifecycle::ListingLifecycle;
    use product_listing_core::product_listing_id::ProductListingId;
    use product_listing_core::product_listing_slug_id::ProductListingSlugId;
    use product_listing_core::source_listing_id::SourceListingId;

    use product_listing_core::description::Description;
    use product_listing_core::product_listing::{ListingSaleObservation, ProductListingPricing};
    use product_listing_core::title::Title;
    use product_listing_service::ports::{
        ListingSourceSummary, PersonalizedProductListingDetailsReadModel,
        ProductListingDetailsReadModel,
    };
    use product_listing_service::use_cases::ProductListingPricingValuation;
    use product_listing_service::user_state::{NotificationUserState, ProductListingUserState};

    use std::sync::{Arc, Mutex, MutexGuard};
    use strum::IntoEnumIterator;
    use time::OffsetDateTime;
    use url::Url;

    #[derive(Default)]
    struct FakeState {
        begin_fails: bool,
        commit_fails: bool,
        details_result: Option<
            Result<
                CursoredResult<
                    PersonalizedProductListingDetailsReadModel,
                    ProductListingWatchlistDetailsCursor,
                >,
                ProductListingWatchlistDetailsReadError,
            >,
        >,
        latest_snapshot_result:
            Option<Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>>,
        sale_snapshots_result: Option<Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError>>,

        begin_count: usize,
        commit_count: usize,
        details_requests: Vec<ProductListingWatchlistDetailsRequest>,
        latest_snapshot_requests: usize,
        sale_snapshot_requests: Vec<Vec<FxRateId>>,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork(SharedState);
    #[derive(Clone)]
    struct FakeDetailsReaderFactory(SharedState);
    #[derive(Clone)]
    struct FakeFxRateSnapshotRepositoryFactory(SharedState);

    struct FakeTransaction(SharedState);
    struct FakeDetailsReader(SharedState);
    struct FakeFxRateSnapshotRepository(SharedState);

    fn state() -> SharedState {
        Arc::new(Mutex::new(FakeState::default()))
    }

    fn lock(state: &SharedState) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = lock(&self.0);
            state.begin_count += 1;
            if state.begin_fails {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(FakeTransaction(Arc::clone(&self.0)))
            }
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.0);
            state.commit_count += 1;
            if state.commit_fails {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl ProductListingWatchlistDetailsReaderFactory<FakeTransaction> for FakeDetailsReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ProductListingWatchlistDetailsReader + 'tx {
            FakeDetailsReader(Arc::clone(&self.0))
        }
    }

    impl FxRateSnapshotRepositoryFactory<FakeTransaction> for FakeFxRateSnapshotRepositoryFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl FxRateSnapshotRepository + 'tx {
            FakeFxRateSnapshotRepository(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ProductListingWatchlistDetailsReader for FakeDetailsReader {
        async fn find_for_user(
            &mut self,
            request: &ProductListingWatchlistDetailsRequest,
        ) -> Result<
            CursoredResult<
                PersonalizedProductListingDetailsReadModel,
                ProductListingWatchlistDetailsCursor,
            >,
            ProductListingWatchlistDetailsReadError,
        > {
            let mut state = lock(&self.0);
            state.details_requests.push(request.clone());
            state.details_result.take().unwrap_or_else(|| {
                Ok(CursoredResult {
                    items: Vec::new(),
                    cursor: request.cursor,
                    total: None,
                })
            })
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for FakeFxRateSnapshotRepository {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock(&self.0);
            state.latest_snapshot_requests += 1;
            state.latest_snapshot_result.take().unwrap_or(Ok(None))
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock(&self.0);
            state.latest_snapshot_requests += 1;
            state.latest_snapshot_result.take().unwrap_or(Ok(None))
        }

        async fn find_by_id(
            &mut self,
            _id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_by_ids(
            &mut self,
            ids: &[FxRateId],
        ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock(&self.0);
            state.sale_snapshot_requests.push(ids.to_vec());
            state.sale_snapshots_result.take().unwrap_or(Ok(Vec::new()))
        }

        async fn insert(
            &mut self,
            _snapshot: &fxrate_core::NewFxRateSnapshot,
            _source_event_id: &str,
        ) -> Result<fxrate_service::ports::FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>
        {
            Err(FxRateSnapshotRepositoryError::ReadFailed {
                source: box_error(std::io::Error::other("not implemented in fake")),
            })
        }
    }

    fn handler(
        state: &SharedState,
    ) -> ListWatchlistHandler<
        FakeUnitOfWork,
        FakeDetailsReaderFactory,
        FakeFxRateSnapshotRepositoryFactory,
    > {
        ListWatchlistHandler::new(
            FakeUnitOfWork(Arc::clone(state)),
            FakeDetailsReaderFactory(Arc::clone(state)),
            FakeFxRateSnapshotRepositoryFactory(Arc::clone(state)),
        )
    }

    fn context(user_id: UserId) -> OperationContext {
        OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn request(user_id: UserId) -> ListWatchlistRequest {
        ListWatchlistRequest {
            user_id,
            language: Language::En,
            currency: Currency::Usd,
            cursor: Cursor::default(),
        }
    }

    fn delegated_context(user_id: UserId, capability: bool) -> OperationContext {
        let capabilities = if capability {
            [CredentialCapability::WatchlistRead].into_iter().collect()
        } else {
            Default::default()
        };
        OperationContext {
            principal: Principal::DelegatedUser {
                user_id,
                capabilities,
            },
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn snapshot(id: FxRateId) -> Result<FxRateSnapshot, FxRateSnapshotError> {
        let snapshot = NewFxRateSnapshot::capture_eur(
            id,
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    if currency == Currency::Eur {
                        FX_RATE_SCALE
                    } else {
                        1_250_000
                    },
                )
            }),
        )?;
        Ok(snapshot.into_persisted(FxRateGeneration::try_from(1)?))
    }

    fn details(
        product_listing_id: ProductListingId,
    ) -> Result<PersonalizedProductListingDetailsReadModel, url::ParseError> {
        let url = Url::parse("https://example.test/product")?;
        Ok(Personalized {
            item: ProductListingDetailsReadModel {
                product_listing_id,
                product_listing_title_slug_id: ProductListingSlugId::raw("product-a1b2c3")
                    .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
                event_id: EventId::new(),
                source: ListingSourceSummary {
                    listing_source_id: ListingSourceId::new(),
                    name: ListingSourceName::try_from("Source").unwrap_or_else(|error| {
                        panic!("invalid test listing source name: {error}")
                    }),
                    slug_id: ListingSourceSlugId::raw("source")
                        .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
                },
                source_listing_id: SourceListingId::try_from("product")
                    .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
                product_title: Some(Localized::new(Language::En, Title::from("ProductListing"))),
                product_description: Some(Localized::new(
                    Language::En,
                    Description::from("Description"),
                )),
                title: Some(Localized::new(Language::En, Title::from("ProductListing"))),
                description: Some(Localized::new(
                    Language::En,
                    Description::from("Description"),
                )),
                pricing: ProductListingPricing {
                    price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur).into()),
                    price_estimate_min: None,
                    price_estimate_max: None,
                },
                sale_observation: None,
                availability: Some(ListingAvailability::Available),
                lifecycle: ListingLifecycle::Active,
                url: url.clone(),
                view_url: url,
                images: Default::default(),
                content_policy: None,
                auction: None,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: Some(ProductListingUserState::default()),
        })
    }

    fn page(
        items: Vec<PersonalizedProductListingDetailsReadModel>,
    ) -> CursoredResult<
        PersonalizedProductListingDetailsReadModel,
        ProductListingWatchlistDetailsCursor,
    > {
        CursoredResult {
            items,
            cursor: Cursor::default(),
            total: None,
        }
    }

    #[tokio::test]
    async fn should_use_one_current_snapshot_for_all_current_products()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let first_product_listing_id = ProductListingId::new();
        let second_product_listing_id = ProductListingId::new();
        let current_snapshot = snapshot(FxRateId::new())?;
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![
            details(first_product_listing_id)?,
            details(second_product_listing_id)?,
        ])));
        lock(&state).latest_snapshot_result = Some(Ok(Some(current_snapshot.clone())));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await?;

        assert_eq!(
            vec![first_product_listing_id, second_product_listing_id],
            result
                .items
                .iter()
                .map(|item| item.item.product_listing_id)
                .collect::<Vec<_>>()
        );
        assert!(result.items.iter().all(|item| matches!(
            item.item.pricing.valuation,
            ProductListingPricingValuation::Current { fx_rate_id, .. } if fx_rate_id == current_snapshot.id()
        )));
        let state = lock(&state);
        assert_eq!(1, state.latest_snapshot_requests);
        assert!(state.sale_snapshot_requests.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_retain_canonical_notification_user_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let expected_user_state = ProductListingUserState {
            notification: NotificationUserState {
                unseen_notification_ids: vec![Default::default()],
            },
            ..Default::default()
        };
        let mut product = details(ProductListingId::new())?;
        product.user_state = Some(expected_user_state.clone());
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![product])));
        lock(&state).latest_snapshot_result = Some(Ok(Some(snapshot(FxRateId::new())?)));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await?;

        assert_eq!(
            Some(&expected_user_state),
            result.items[0].user_state.as_ref()
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_batch_sale_observation_snapshots_without_current_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let first_snapshot = snapshot(FxRateId::new())?;
        let second_snapshot = snapshot(FxRateId::new())?;
        let mut first = details(ProductListingId::new())?;
        first.item.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            first_snapshot.id(),
        ));
        first.item.availability = Some(ListingAvailability::SoldOut);
        let mut second = details(ProductListingId::new())?;
        second.item.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            second_snapshot.id(),
        ));
        second.item.availability = Some(ListingAvailability::SoldOut);
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![first, second])));
        lock(&state).sale_snapshots_result =
            Some(Ok(vec![first_snapshot.clone(), second_snapshot.clone()]));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await?;

        assert!(matches!(
            result.items[0].item.pricing.valuation,
            ProductListingPricingValuation::SaleObservation { fx_rate_id, .. } if fx_rate_id == first_snapshot.id()
        ));
        assert!(matches!(
            result.items[1].item.pricing.valuation,
            ProductListingPricingValuation::SaleObservation { fx_rate_id, .. } if fx_rate_id == second_snapshot.id()
        ));
        let state = lock(&state);
        assert_eq!(0, state.latest_snapshot_requests);
        assert_eq!(1, state.sale_snapshot_requests.len());
        assert_eq!(
            HashSet::from([first_snapshot.id(), second_snapshot.id()]),
            state.sale_snapshot_requests[0].iter().copied().collect(),
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_use_current_and_sale_snapshots_for_mixed_page()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let current_snapshot = snapshot(FxRateId::new())?;
        let sale_snapshot = snapshot(FxRateId::new())?;
        let current = details(ProductListingId::new())?;
        let mut sale = details(ProductListingId::new())?;
        sale.item.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            sale_snapshot.id(),
        ));
        sale.item.availability = Some(ListingAvailability::SoldOut);
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![current, sale])));
        lock(&state).latest_snapshot_result = Some(Ok(Some(current_snapshot)));
        lock(&state).sale_snapshots_result = Some(Ok(vec![sale_snapshot.clone()]));

        handler(&state)
            .execute(&context(user_id), request(user_id))
            .await?;

        let state = lock(&state);
        assert_eq!(1, state.latest_snapshot_requests);
        assert_eq!(vec![vec![sale_snapshot.id()]], state.sale_snapshot_requests);
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_without_fallback_when_sale_snapshot_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let missing_snapshot_id = FxRateId::new();
        let mut sale = details(ProductListingId::new())?;
        sale.item.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            missing_snapshot_id,
        ));
        sale.item.availability = Some(ListingAvailability::SoldOut);
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![sale])));
        lock(&state).sale_snapshots_result = Some(Ok(Vec::new()));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::SalePricingFxSnapshotMissing { fx_rate_id }) if fx_rate_id == missing_snapshot_id
        ));
        let state = lock(&state);
        assert_eq!(0, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_without_fallback_when_current_snapshot_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![details(ProductListingId::new())?])));
        lock(&state).latest_snapshot_result = Some(Ok(None));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::CurrentPricingFxSnapshotMissing)
        ));
        let state = lock(&state);
        assert_eq!(0, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_typed_error_when_current_snapshot_is_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![details(ProductListingId::new())?])));
        lock(&state).latest_snapshot_result = Some(Err(
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                source: box_error(std::io::Error::other("invalid FX snapshot")),
            },
        ));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::PricingFxSnapshotInvalid { .. })
        ));
        let state = lock(&state);
        assert_eq!(0, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_commit_an_empty_watchlist() -> Result<(), ListWatchlistError> {
        let user_id = UserId::new();
        let state = state();

        handler(&state)
            .execute(&context(user_id), request(user_id))
            .await?;

        let state = lock(&state);
        assert_eq!(0, state.latest_snapshot_requests);
        assert!(state.sale_snapshot_requests.is_empty());
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_details_read_fails() {
        let user_id = UserId::new();
        let state = state();
        lock(&state).details_result =
            Some(Err(ProductListingWatchlistDetailsReadError::QueryFailed));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::TemporarilyUnavailable)
        ));
        let state = lock(&state);
        assert_eq!(0, state.commit_count);
        assert_eq!(0, state.latest_snapshot_requests);
    }

    #[tokio::test]
    async fn should_return_commit_failure() -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let state = state();
        lock(&state).commit_fails = true;
        lock(&state).details_result = Some(Ok(page(vec![details(ProductListingId::new())?])));
        lock(&state).latest_snapshot_result = Some(Ok(Some(snapshot(FxRateId::new())?)));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::CommitTransactionFailed)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_missing_canonical_user_state() -> Result<(), Box<dyn std::error::Error>>
    {
        let user_id = UserId::new();
        let mut product = details(ProductListingId::new())?;
        product.user_state = None;
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![product])));
        lock(&state).latest_snapshot_result = Some(Ok(Some(snapshot(FxRateId::new())?)));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::InvalidPersistedState)
        ));
        let state = lock(&state);
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_begin_transaction_when_delegated_user_lacks_capability() {
        let user_id = UserId::new();
        let state = state();

        let result = handler(&state)
            .execute(&delegated_context(user_id, false), request(user_id))
            .await;

        assert!(matches!(result, Err(ListWatchlistError::Forbidden)));
        assert_eq!(0, lock(&state).begin_count);
    }

    #[tokio::test]
    async fn should_allow_delegated_user_with_watchlist_read_capability()
    -> Result<(), ListWatchlistError> {
        let user_id = UserId::new();
        let state = state();

        handler(&state)
            .execute(&delegated_context(user_id, true), request(user_id))
            .await?;

        assert_eq!(1, lock(&state).commit_count);
        Ok(())
    }
}
