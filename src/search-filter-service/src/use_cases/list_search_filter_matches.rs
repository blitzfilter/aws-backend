use crate::ports::{
    SearchFilterMatchListQuery, SearchFilterMatchReadError, SearchFilterMatchReader,
};
use application::{
    error::{BoxError, box_error, static_error},
    operation_context::{CredentialCapability, OperationAuthorizationError, OperationContext},
    pagination::{Cursor, CursoredResult},
    transaction::{Transaction, UnitOfWork},
};
use domain_primitives::sort::SortOrder;
use fxrate_core::{FxRateId, FxRateSnapshot, FxRateSnapshotError};
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use localization::Language;
use money::Currency;
use product_listing_core::{
    listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
    product_listing_id::ProductListingId,
};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use user_core::user_id::UserId;

use product_listing_service::ports::{
    PersonalizedProductListingDetailsReadModel, ProductListingDetailsBatchReadError,
    ProductListingDetailsBatchReadRequest, ProductListingDetailsBatchReader,
};
use product_listing_service::use_cases::{
    PersonalizedProductListingDetailsView, ProductListingPricingPresentationError,
    present_product_details, redact_hidden_product,
};
use std::collections::{HashMap, HashSet};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct ListSearchFilterMatchesRequest {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub language: Language,
    pub currency: Currency,
    pub cursor: Option<Cursor<crate::ports::SearchFilterMatchCursor>>,
    pub order: SortOrder,
}

pub type ListSearchFilterMatchesResult =
    CursoredResult<PersonalizedProductListingDetailsView, crate::ports::SearchFilterMatchCursor>;

#[derive(Debug, thiserror::Error)]
pub enum ListSearchFilterMatchesError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("actor may not manage this search filter")]
    ActorMayNotManageSearchFilter,
    #[error("search filter not found")]
    SearchFilterNotFound,
    #[error("search filter match read failed")]
    SearchFilterMatchReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("matched product details read failed")]
    ProductListingDetailsReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("matched product details are invalid")]
    ProductListingDetailsInvalid {
        #[source]
        source: BoxError,
    },
    #[error("matched product is missing from the product details read")]
    MatchedProductListingMissing {
        product_listing_id: ProductListingId,
    },
    #[error("no persisted FX snapshot is available for current matched-product pricing")]
    CurrentPricingFxSnapshotMissing,
    #[error("sale valuation FX snapshot is missing")]
    SalePricingFxSnapshotMissing { fx_rate_id: FxRateId },
    #[error("FX snapshot lookup is temporarily unavailable for matched-product pricing")]
    PricingFxSnapshotUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("persisted FX snapshot is invalid for matched-product pricing")]
    PricingFxSnapshotInvalid {
        #[source]
        source: BoxError,
    },
    #[error("sale valuation FX snapshot does not match")]
    SaleFxSnapshotMismatch {
        expected: FxRateId,
        actual: FxRateId,
    },
    #[error("matched product price conversion failed")]
    ProductListingPriceConversionFailed {
        #[source]
        source: FxRateSnapshotError,
    },
    #[error("failed to begin matched-product FX transaction")]
    BeginPricingTransactionFailed,
    #[error("failed to commit matched-product FX transaction")]
    CommitPricingTransactionFailed,

    #[error("matched product could not be redacted")]
    HiddenProductListingRedactionFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ListSearchFilterMatchesUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListSearchFilterMatchesRequest,
    ) -> Result<ListSearchFilterMatchesResult, ListSearchFilterMatchesError>;
}

pub struct ListSearchFilterMatchesHandler<U, M, P, F> {
    unit_of_work: U,
    matches: M,
    product_listings: P,
    fx_rates: F,
}

impl<U, M, P, F> ListSearchFilterMatchesHandler<U, M, P, F> {
    pub fn new(unit_of_work: U, matches: M, product_listings: P, fx_rates: F) -> Self {
        Self {
            unit_of_work,
            matches,
            product_listings,
            fx_rates,
        }
    }
}

#[async_trait::async_trait]
impl<U, M, P, F> ListSearchFilterMatchesUseCase for ListSearchFilterMatchesHandler<U, M, P, F>
where
    U: UnitOfWork,
    M: SearchFilterMatchReader,
    P: ProductListingDetailsBatchReader,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "list_search_filter_matches",
        skip_all,
        fields(
            search_filter_id = %request.search_filter_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListSearchFilterMatchesRequest,
    ) -> Result<ListSearchFilterMatchesResult, ListSearchFilterMatchesError> {
        authorize_owner(context, request.user_id)?;
        let matches = self
            .matches
            .list_for_owned_filter(&SearchFilterMatchListQuery {
                user_id: request.user_id,
                search_filter_id: request.search_filter_id,
                cursor: request.cursor,
                order: request.order,
            })
            .await
            .map_err(read_error)?
            .ok_or(ListSearchFilterMatchesError::SearchFilterNotFound)?;

        if matches.items.is_empty() {
            return Ok(CursoredResult {
                items: Vec::new(),
                cursor: matches.cursor,
                total: matches.total,
            });
        }

        let product_listing_ids = matches
            .items
            .iter()
            .map(|matched| matched.product_listing_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let details = self
            .product_listings
            .find_for_user(&ProductListingDetailsBatchReadRequest {
                user_id: request.user_id,
                language: request.language,
                product_listing_ids,
                search_filter_id: request.search_filter_id,
            })
            .await
            .map_err(product_details_read_error)?;
        let factual_details = matches
            .items
            .iter()
            .map(|matched| {
                details.get(&matched.product_listing_id).cloned().ok_or(
                    ListSearchFilterMatchesError::MatchedProductListingMissing {
                        product_listing_id: matched.product_listing_id,
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        let valuation_at = OffsetDateTime::now_utc();
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListSearchFilterMatchesError::BeginPricingTransactionFailed)?;
        let pricing_snapshots =
            pricing_snapshots(&self.fx_rates, &mut tx, &factual_details, valuation_at).await?;
        let mut product_listings = factual_details
            .into_iter()
            .map(|factual_details| {
                present_with_pricing_snapshot(factual_details, &pricing_snapshots, request.currency)
            })
            .collect::<Result<Vec<_>, _>>()?;
        tx.commit()
            .await
            .map_err(|_| ListSearchFilterMatchesError::CommitPricingTransactionFailed)?;

        for product in &mut product_listings {
            let is_hidden = product
                .user_state
                .as_ref()
                .ok_or(ListSearchFilterMatchesError::ProductListingDetailsInvalid {
                    source: static_error("matched product is missing user state"),
                })?
                .search_filter
                .hidden;
            if is_hidden {
                redact_hidden_product(&mut product.item).map_err(|error| {
                    ListSearchFilterMatchesError::HiddenProductListingRedactionFailed {
                        source: box_error(error),
                    }
                })?;
            }
        }

        Ok(CursoredResult {
            items: product_listings,
            cursor: matches.cursor,
            total: matches.total,
        })
    }
}

struct PricingSnapshots {
    current: Option<FxRateSnapshot>,
    sale: HashMap<FxRateId, FxRateSnapshot>,
}

async fn pricing_snapshots<Tx, F>(
    fx_rates: &F,
    tx: &mut Tx,
    factual_details: &[PersonalizedProductListingDetailsReadModel],
    valuation_at: OffsetDateTime,
) -> Result<PricingSnapshots, ListSearchFilterMatchesError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    let sale_snapshot_ids = factual_details
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
        .collect::<HashSet<_>>();
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
                .ok_or(ListSearchFilterMatchesError::CurrentPricingFxSnapshotMissing)?,
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
    factual_details: PersonalizedProductListingDetailsReadModel,
    pricing_snapshots: &PricingSnapshots,
    currency: Currency,
) -> Result<PersonalizedProductListingDetailsView, ListSearchFilterMatchesError> {
    let snapshot = match factual_details.item.sale_observation.filter(|_| {
        factual_details.item.availability == Some(ListingAvailability::SoldOut)
            || factual_details.item.lifecycle == ListingLifecycle::Withdrawn
    }) {
        Some(observation) => pricing_snapshots
            .sale
            .get(&observation.fx_rate_id())
            .ok_or(ListSearchFilterMatchesError::SalePricingFxSnapshotMissing {
                fx_rate_id: observation.fx_rate_id(),
            })?,
        None => pricing_snapshots
            .current
            .as_ref()
            .ok_or(ListSearchFilterMatchesError::CurrentPricingFxSnapshotMissing)?,
    };
    Ok(present_product_details(
        factual_details,
        snapshot,
        currency,
    )?)
}

fn authorize_owner(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), ListSearchFilterMatchesError> {
    context
        .require()
        .credential_capability(CredentialCapability::SearchFiltersWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<ListSearchFilterMatchesError>()
}

fn read_error(error: SearchFilterMatchReadError) -> ListSearchFilterMatchesError {
    ListSearchFilterMatchesError::SearchFilterMatchReadFailed {
        source: box_error(error),
    }
}

fn product_details_read_error(
    error: ProductListingDetailsBatchReadError,
) -> ListSearchFilterMatchesError {
    match error {
        ProductListingDetailsBatchReadError::QueryFailed { source } => {
            ListSearchFilterMatchesError::ProductListingDetailsReadFailed { source }
        }
        ProductListingDetailsBatchReadError::InvalidReadModel { source } => {
            ListSearchFilterMatchesError::ProductListingDetailsInvalid { source }
        }
    }
}

impl From<FxRateSnapshotRepositoryError> for ListSearchFilterMatchesError {
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

impl From<ProductListingPricingPresentationError> for ListSearchFilterMatchesError {
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

impl From<OperationAuthorizationError> for ListSearchFilterMatchesError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => {
                Self::ActorMayNotManageSearchFilter
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        SearchFilterMatchCursor, SearchFilterMatchListItem, SearchFilterMatchReadError,
    };
    use application::{
        operation_context::{CorrelationId, Principal, RequestId},
        personalized::Personalized,
        transaction::TransactionError,
    };
    use domain_primitives::event_id::EventId;
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use indexmap::IndexSet;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use product_listing_core::product_listing::{ListingSaleObservation, ProductListingPricing};
    use product_listing_core::{
        listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
        product_listing_slug_id::ProductListingSlugId, source_listing_id::SourceListingId,
    };
    use product_listing_service::ports::{ListingSourceSummary, ProductListingDetailsReadModel};
    use product_listing_service::use_cases::ProductListingPricingValuation;
    use product_listing_service::user_state::{NotificationUserState, ProductListingUserState};
    use std::sync::{Arc, Mutex, MutexGuard};
    use strum::IntoEnumIterator;
    use time::OffsetDateTime;
    use url::Url;

    #[derive(Default)]
    struct State {
        product_requests: Vec<ProductListingDetailsBatchReadRequest>,

        begin_count: usize,
        commit_count: usize,
        latest_snapshot_requests: usize,
        find_by_id_requests: usize,
        sale_snapshot_requests: Vec<Vec<FxRateId>>,
        latest_snapshot: Option<FxRateSnapshot>,
        sale_snapshots: Vec<FxRateSnapshot>,
    }

    type SharedState = Arc<Mutex<State>>;

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[derive(Clone)]
    struct UnitOfWorkFake(SharedState);
    #[derive(Clone)]
    struct FxRateSnapshotFactoryFake(SharedState);
    struct TransactionFake(SharedState);
    struct FxRateSnapshotRepositoryFake(SharedState);

    struct MatchesReader {
        matches: Vec<SearchFilterMatchListItem>,
    }

    #[async_trait::async_trait]
    impl SearchFilterMatchReader for MatchesReader {
        async fn list_for_owned_filter(
            &self,
            _query: &SearchFilterMatchListQuery,
        ) -> Result<
            Option<CursoredResult<SearchFilterMatchListItem, SearchFilterMatchCursor>>,
            SearchFilterMatchReadError,
        > {
            Ok(Some(CursoredResult {
                items: self.matches.clone(),
                cursor: Cursor::default(),
                total: None,
            }))
        }
    }

    struct ProductListingsReader {
        state: SharedState,
        product_listings: HashMap<ProductListingId, PersonalizedProductListingDetailsReadModel>,
    }

    #[async_trait::async_trait]
    impl ProductListingDetailsBatchReader for ProductListingsReader {
        async fn find_for_user(
            &self,
            request: &ProductListingDetailsBatchReadRequest,
        ) -> Result<
            HashMap<ProductListingId, PersonalizedProductListingDetailsReadModel>,
            ProductListingDetailsBatchReadError,
        > {
            lock(&self.state).product_requests.push(request.clone());
            Ok(self.product_listings.clone())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for UnitOfWorkFake {
        type Tx = TransactionFake;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begin_count += 1;
            Ok(TransactionFake(Arc::clone(&self.0)))
        }
    }

    #[async_trait::async_trait]
    impl Transaction for TransactionFake {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.0).commit_count += 1;
            Ok(())
        }
    }

    impl FxRateSnapshotRepositoryFactory<TransactionFake> for FxRateSnapshotFactoryFake {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TransactionFake,
        ) -> impl FxRateSnapshotRepository + 'tx {
            FxRateSnapshotRepositoryFake(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for FxRateSnapshotRepositoryFake {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock(&self.0);
            state.latest_snapshot_requests += 1;
            Ok(state.latest_snapshot.clone())
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock(&self.0);
            state.latest_snapshot_requests += 1;
            Ok(state.latest_snapshot.clone())
        }

        async fn find_by_id(
            &mut self,
            _id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            lock(&self.0).find_by_id_requests += 1;
            Ok(None)
        }

        async fn find_by_ids(
            &mut self,
            ids: &[FxRateId],
        ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock(&self.0);
            state.sale_snapshot_requests.push(ids.to_vec());
            Ok(state.sale_snapshots.clone())
        }

        async fn insert(
            &mut self,
            _snapshot: &NewFxRateSnapshot,
            _source_event_id: &str,
        ) -> Result<fxrate_service::ports::FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>
        {
            Err(FxRateSnapshotRepositoryError::ReadFailed {
                source: static_error("insert is not supported by this fake"),
            })
        }
    }

    fn product(
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
                product_title: None,
                product_description: None,
                title: None,
                description: None,
                pricing: ProductListingPricing::default(),
                sale_observation: None,
                availability: Some(ListingAvailability::Available),
                lifecycle: ListingLifecycle::Active,
                content_policy: None,
                url: url.clone(),
                view_url: url,
                images: IndexSet::new(),
                auction: None,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: Some(ProductListingUserState::default()),
        })
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

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn request(user_id: UserId) -> ListSearchFilterMatchesRequest {
        ListSearchFilterMatchesRequest {
            user_id,
            search_filter_id: UserSearchFilterId::new(),
            language: Language::En,
            currency: Currency::Usd,
            cursor: None,
            order: SortOrder::Asc,
        }
    }

    fn handler(
        state: &SharedState,
        matches: Vec<SearchFilterMatchListItem>,
        product_listings: HashMap<ProductListingId, PersonalizedProductListingDetailsReadModel>,
    ) -> ListSearchFilterMatchesHandler<
        UnitOfWorkFake,
        MatchesReader,
        ProductListingsReader,
        FxRateSnapshotFactoryFake,
    > {
        ListSearchFilterMatchesHandler::new(
            UnitOfWorkFake(Arc::clone(state)),
            MatchesReader { matches },
            ProductListingsReader {
                state: Arc::clone(state),
                product_listings,
            },
            FxRateSnapshotFactoryFake(Arc::clone(state)),
        )
    }

    fn match_item(product_listing_id: ProductListingId) -> SearchFilterMatchListItem {
        SearchFilterMatchListItem {
            product_listing_id,
            created: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[tokio::test]
    async fn should_batch_fx_snapshot_reads_present_products_and_retain_canonical_user_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let current_product_listing_id = ProductListingId::new();
        let first_sale_product_listing_id = ProductListingId::new();
        let second_sale_product_listing_id = ProductListingId::new();
        let current_snapshot = snapshot(FxRateId::new())?;
        let sale_snapshot = snapshot(FxRateId::new())?;
        let expected_user_state = ProductListingUserState {
            notification: NotificationUserState {
                unseen_notification_ids: vec![Default::default()],
            },
            ..Default::default()
        };
        let mut current = product(current_product_listing_id)?;
        current.user_state = Some(expected_user_state.clone());
        let mut first_sale = product(first_sale_product_listing_id)?;
        first_sale.item.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            sale_snapshot.id(),
        ));
        first_sale.item.availability = Some(ListingAvailability::SoldOut);
        let mut second_sale = product(second_sale_product_listing_id)?;
        second_sale.item.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            sale_snapshot.id(),
        ));
        second_sale.item.availability = Some(ListingAvailability::SoldOut);
        let state = Arc::new(Mutex::new(State {
            latest_snapshot: Some(current_snapshot.clone()),
            sale_snapshots: vec![sale_snapshot.clone()],
            ..Default::default()
        }));

        let result = handler(
            &state,
            vec![
                match_item(current_product_listing_id),
                match_item(first_sale_product_listing_id),
                match_item(second_sale_product_listing_id),
            ],
            HashMap::from([
                (second_sale_product_listing_id, second_sale),
                (current_product_listing_id, current),
                (first_sale_product_listing_id, first_sale),
            ]),
        )
        .execute(&context(), request(user_id))
        .await?;

        assert_eq!(
            vec![
                current_product_listing_id,
                first_sale_product_listing_id,
                second_sale_product_listing_id
            ],
            result
                .items
                .iter()
                .map(|item| item.item.product_listing_id)
                .collect::<Vec<_>>()
        );
        assert!(matches!(
            result.items[0].item.pricing.valuation,
            ProductListingPricingValuation::Current { fx_rate_id, .. } if fx_rate_id == current_snapshot.id()
        ));
        assert_eq!(
            Some(&expected_user_state),
            result.items[0].user_state.as_ref()
        );
        assert!(result.items[1..].iter().all(|item| matches!(
            item.item.pricing.valuation,
            ProductListingPricingValuation::SaleObservation { fx_rate_id, .. } if fx_rate_id == sale_snapshot.id()
        )));
        let state = lock(&state);
        assert_eq!(1, state.product_requests.len());
        assert_eq!(1, state.begin_count);
        assert_eq!(1, state.commit_count);
        assert_eq!(1, state.latest_snapshot_requests);
        assert_eq!(0, state.find_by_id_requests);
        assert_eq!(1, state.sale_snapshot_requests.len());
        assert_eq!(
            HashSet::from([sale_snapshot.id()]),
            state.sale_snapshot_requests[0].iter().copied().collect()
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_without_fallback_when_a_sale_snapshot_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let missing_snapshot_id = FxRateId::new();
        let mut sale = product(product_listing_id)?;
        sale.item.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            missing_snapshot_id,
        ));
        sale.item.availability = Some(ListingAvailability::SoldOut);
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(
            &state,
            vec![match_item(product_listing_id)],
            HashMap::from([(product_listing_id, sale)]),
        )
        .execute(&context(), request(user_id))
        .await;

        assert!(matches!(
            result,
            Err(ListSearchFilterMatchesError::SalePricingFxSnapshotMissing { fx_rate_id })
                if fx_rate_id == missing_snapshot_id
        ));
        let state = lock(&state);
        assert_eq!(0, state.latest_snapshot_requests);
        assert_eq!(
            vec![vec![missing_snapshot_id]],
            state.sale_snapshot_requests
        );
        assert_eq!(0, state.find_by_id_requests);
        assert_eq!(0, state.commit_count);
        Ok(())
    }
}
