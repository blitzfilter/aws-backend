use crate::ports::{
    ProductListingSearchFilterMatchSource, ProductListingSearchFilterMatchSourceReadError,
    ProductListingSearchFilterMatchSourceReader,
    ProductListingSearchFilterMatchSourceReaderFactory, ProductListingSearchProjection,
    ProductListingSearchProjectionWriteOutcome,
};
use application::error::{BoxError, box_error};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateSnapshot;
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use product_listing_core::{
    listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
    product_listing_id::ProductListingId, product_listing_price::ProductListingPrice,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectProductListingCommand {
    pub event_id: EventId,
    pub product_listing_id: ProductListingId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectProductListingOutcome {
    Applied,
    Stale,
    MissingSource,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectProductListingResult {
    pub outcome: ProjectProductListingOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectProductListingError {
    #[error("failed to begin ProductListing projection source transaction")]
    BeginFailed {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing projection source read failed")]
    SourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing projection sale-observation FX snapshot is missing")]
    SaleObservationFxSnapshotMissing,
    #[error("ProductListing projection sale-observation FX snapshot is invalid")]
    SaleObservationFxSnapshotInvalid {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing projection sale-observation FX snapshot read failed")]
    SaleObservationFxSnapshotReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit ProductListing projection source transaction")]
    CommitFailed {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing projection target write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProjectProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        command: ProjectProductListingCommand,
    ) -> Result<ProjectProductListingResult, ProjectProductListingError>;
}

pub struct ProjectProductListingHandler<U, S, F, P> {
    unit_of_work: U,
    sources: S,
    fx_rates: F,
    projection: P,
}

impl<U, S, F, P> ProjectProductListingHandler<U, S, F, P> {
    pub fn new(unit_of_work: U, sources: S, fx_rates: F, projection: P) -> Self {
        Self {
            unit_of_work,
            sources,
            fx_rates,
            projection,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, F, P> ProjectProductListingUseCase for ProjectProductListingHandler<U, S, F, P>
where
    U: UnitOfWork,
    S: ProductListingSearchFilterMatchSourceReaderFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    P: ProductListingSearchProjection,
{
    #[tracing::instrument(name = "project_product", skip_all, fields(product_listing_id = %command.product_listing_id, event_id = %command.event_id))]
    async fn execute(
        &self,
        command: ProjectProductListingCommand,
    ) -> Result<ProjectProductListingResult, ProjectProductListingError> {
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            ProjectProductListingError::BeginFailed {
                source: box_error(source),
            }
        })?;
        let source = self
            .sources
            .in_transaction(&mut tx)
            .find_source(command.event_id, command.product_listing_id)
            .await
            .map_err(source_read_error)?;
        let Some(source) = source else {
            tx.commit()
                .await
                .map_err(|source| ProjectProductListingError::CommitFailed {
                    source: box_error(source),
                })?;
            return Ok(ProjectProductListingResult {
                outcome: ProjectProductListingOutcome::MissingSource,
            });
        };
        if source.current_event_id != command.event_id {
            tx.commit()
                .await
                .map_err(|source| ProjectProductListingError::CommitFailed {
                    source: box_error(source),
                })?;
            return Ok(ProjectProductListingResult {
                outcome: ProjectProductListingOutcome::Stale,
            });
        }
        let sale_snapshot = load_sale_snapshot(&mut tx, &self.fx_rates, &source).await?;
        tx.commit()
            .await
            .map_err(|source| ProjectProductListingError::CommitFailed {
                source: box_error(source),
            })?;

        let outcome = if source.lifecycle == ListingLifecycle::Withdrawn {
            self.projection
                .delete(source.product_listing_id, source.projection_version)
                .await
        } else {
            self.projection
                .upsert(&source, sale_snapshot.as_ref())
                .await
        }
        .map_err(|source| ProjectProductListingError::WriteFailed {
            source: box_error(source),
        })?;
        Ok(ProjectProductListingResult {
            outcome: match (source.lifecycle, outcome) {
                (
                    ListingLifecycle::Withdrawn,
                    ProductListingSearchProjectionWriteOutcome::Applied,
                ) => ProjectProductListingOutcome::Deleted,
                (_, ProductListingSearchProjectionWriteOutcome::Applied) => {
                    ProjectProductListingOutcome::Applied
                }
                (_, ProductListingSearchProjectionWriteOutcome::Stale) => {
                    ProjectProductListingOutcome::Stale
                }
            },
        })
    }
}

async fn load_sale_snapshot<Tx, F>(
    tx: &mut Tx,
    fx_rates: &F,
    source: &ProductListingSearchFilterMatchSource,
) -> Result<Option<FxRateSnapshot>, ProjectProductListingError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    let Some(observation) = source.sale_observation else {
        return Ok(None);
    };
    if source.lifecycle != ListingLifecycle::Active
        || source.availability != Some(ListingAvailability::SoldOut)
        || !matches!(source.pricing.price, Some(ProductListingPrice::Monetary(_)))
    {
        return Ok(None);
    }
    fx_rates
        .in_transaction(tx)
        .find_by_id(observation.fx_rate_id())
        .await
        .map_err(fx_rate_error)?
        .ok_or(ProjectProductListingError::SaleObservationFxSnapshotMissing)
        .map(Some)
}

fn source_read_error(
    error: ProductListingSearchFilterMatchSourceReadError,
) -> ProjectProductListingError {
    ProjectProductListingError::SourceReadFailed {
        source: match error {
            ProductListingSearchFilterMatchSourceReadError::QueryFailed { source }
            | ProductListingSearchFilterMatchSourceReadError::InvalidPersistedState { source } => {
                source
            }
        },
    }
}

fn fx_rate_error(error: FxRateSnapshotRepositoryError) -> ProjectProductListingError {
    match error {
        FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
            ProjectProductListingError::SaleObservationFxSnapshotInvalid { source }
        }
        FxRateSnapshotRepositoryError::InsertFailed { source }
        | FxRateSnapshotRepositoryError::ReadFailed { source } => {
            ProjectProductListingError::SaleObservationFxSnapshotReadFailed { source }
        }
        FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => {
            ProjectProductListingError::SaleObservationFxSnapshotMissing
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ListingSourceSummary, ProductListingSearchFilterMatchSourceEventKind,
        ProductListingSearchFilterMatchSourceRef, ProductListingSearchProjectionWriteError,
    };
    use application::error::box_error;
    use application::transaction::TransactionError;
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateId, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use indexmap::IndexSet;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::{Language, Localized};
    use money::Currency;
    use product_listing_core::{
        description::Description,
        listing_availability::ListingAvailability,
        listing_lifecycle::ListingLifecycle,
        product_listing::{ListingSaleObservation, ProductListingPricing},
        product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
        title::Title,
    };
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex, MutexGuard},
    };
    use strum::IntoEnumIterator;
    use time::OffsetDateTime;
    use url::Url;

    #[derive(Default)]
    struct FakeState {
        source_result: Option<
            Result<
                Option<ProductListingSearchFilterMatchSource>,
                ProductListingSearchFilterMatchSourceReadError,
            >,
        >,
        source_requests: Vec<(EventId, ProductListingId)>,
        fx_result: Option<Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>>,
        fx_lookup_ids: Vec<FxRateId>,
        commit_count: usize,
        upserts: Vec<(ProductListingId, Option<FxRateSnapshot>)>,
        deletes: Vec<(ProductListingId, i64)>,
        write_commit_counts: Vec<usize>,
        projection_result: Option<
            Result<
                ProductListingSearchProjectionWriteOutcome,
                ProductListingSearchProjectionWriteError,
            >,
        >,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
    }

    struct FakeTx {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeSourceFactory {
        state: SharedState,
    }

    struct FakeSourceReader {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeFxRateSnapshotFactory {
        state: SharedState,
    }

    struct FakeFxRateSnapshotRepository {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeProjection {
        state: SharedState,
    }

    fn state() -> SharedState {
        Arc::new(Mutex::new(FakeState::default()))
    }

    fn lock_state(state: &SharedState) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(FakeTx {
                state: Arc::clone(&self.state),
            })
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            lock_state(&self.state).commit_count += 1;
            Ok(())
        }
    }

    impl ProductListingSearchFilterMatchSourceReaderFactory<FakeTx> for FakeSourceFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl ProductListingSearchFilterMatchSourceReader + 'tx {
            FakeSourceReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductListingSearchFilterMatchSourceReader for FakeSourceReader {
        async fn find_source(
            &mut self,
            event_id: EventId,
            product_listing_id: ProductListingId,
        ) -> Result<
            Option<ProductListingSearchFilterMatchSource>,
            ProductListingSearchFilterMatchSourceReadError,
        > {
            let mut state = lock_state(&self.state);
            state.source_requests.push((event_id, product_listing_id));
            match state.source_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }

        async fn find_sources(
            &mut self,
            refs: &[ProductListingSearchFilterMatchSourceRef],
        ) -> Result<
            HashMap<
                ProductListingSearchFilterMatchSourceRef,
                ProductListingSearchFilterMatchSource,
            >,
            ProductListingSearchFilterMatchSourceReadError,
        > {
            let mut sources = HashMap::new();
            for reference in refs {
                if let Some(source) = self
                    .find_source(reference.event_id, reference.product_listing_id)
                    .await?
                {
                    sources.insert(*reference, source);
                }
            }
            Ok(sources)
        }
    }

    impl FxRateSnapshotRepositoryFactory<FakeTx> for FakeFxRateSnapshotFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl FxRateSnapshotRepository + 'tx {
            FakeFxRateSnapshotRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for FakeFxRateSnapshotRepository {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: time::OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_by_id(
            &mut self,
            id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock_state(&self.state);
            state.fx_lookup_ids.push(id);
            match state.fx_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }

        async fn find_by_ids(
            &mut self,
            _ids: &[FxRateId],
        ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Vec::new())
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

    #[async_trait::async_trait]
    impl ProductListingSearchProjection for FakeProjection {
        async fn upsert(
            &self,
            source: &ProductListingSearchFilterMatchSource,
            sale_snapshot: Option<&FxRateSnapshot>,
        ) -> Result<
            ProductListingSearchProjectionWriteOutcome,
            ProductListingSearchProjectionWriteError,
        > {
            let mut state = lock_state(&self.state);
            let commit_count = state.commit_count;
            state
                .upserts
                .push((source.product_listing_id, sale_snapshot.cloned()));
            state.write_commit_counts.push(commit_count);
            match state.projection_result.take() {
                Some(result) => result,
                None => Ok(ProductListingSearchProjectionWriteOutcome::Applied),
            }
        }

        async fn delete(
            &self,
            product_listing_id: ProductListingId,
            source_version: i64,
        ) -> Result<
            ProductListingSearchProjectionWriteOutcome,
            ProductListingSearchProjectionWriteError,
        > {
            let mut state = lock_state(&self.state);
            let commit_count = state.commit_count;
            state.deletes.push((product_listing_id, source_version));
            state.write_commit_counts.push(commit_count);
            match state.projection_result.take() {
                Some(result) => result,
                None => Ok(ProductListingSearchProjectionWriteOutcome::Applied),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> ProjectProductListingHandler<
        FakeUnitOfWork,
        FakeSourceFactory,
        FakeFxRateSnapshotFactory,
        FakeProjection,
    > {
        ProjectProductListingHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeSourceFactory {
                state: Arc::clone(state),
            },
            FakeFxRateSnapshotFactory {
                state: Arc::clone(state),
            },
            FakeProjection {
                state: Arc::clone(state),
            },
        )
    }

    fn source() -> Result<ProductListingSearchFilterMatchSource, url::ParseError> {
        let event_id = EventId::new();
        let url = Url::parse("https://shop.example.test/products/cabinet")?;
        let title = Title::from("Cabinet");
        Ok(ProductListingSearchFilterMatchSource {
            event_id,
            event_kind: ProductListingSearchFilterMatchSourceEventKind::Domain,
            origin_event_time: OffsetDateTime::UNIX_EPOCH,
            current_event_id: event_id,
            projection_version: 41,
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("cabinet-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source: ListingSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
                slug_id: ListingSourceSlugId::raw("source")
                    .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
            },
            source_listing_id: SourceListingId::try_from("cabinet-1")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            product_title: Some(Localized::new(Language::En, title.clone())),
            product_description: Some(Localized::new(
                Language::En,
                Description::from("Old cabinet"),
            )),
            titles: HashMap::from([(Language::En, title)]),
            descriptions: HashMap::new(),
            pricing: ProductListingPricing::default(),
            sale_observation: None,
            availability: None,
            lifecycle: ListingLifecycle::Active,
            url: url.clone(),
            view_url: url,
            image: None,
            images: IndexSet::new(),
            embedding: None,
            auction: None,
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        })
    }

    fn snapshot() -> Result<FxRateSnapshot, fxrate_core::FxRateSnapshotError> {
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            time::OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
        )
        .and_then(|snapshot| Ok(snapshot.into_persisted(FxRateGeneration::try_from(1)?)))
    }

    fn command(source: &ProductListingSearchFilterMatchSource) -> ProjectProductListingCommand {
        ProjectProductListingCommand {
            event_id: source.event_id,
            product_listing_id: source.product_listing_id,
        }
    }

    #[tokio::test]
    async fn should_upsert_active_source_without_fx_snapshot_after_commit()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let source = source()?;
        let command = command(&source);
        lock_state(&state).source_result = Some(Ok(Some(source.clone())));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductListingOutcome::Applied, result.outcome);
        let state = lock_state(&state);
        assert_eq!(
            vec![(command.event_id, command.product_listing_id)],
            state.source_requests
        );
        assert!(state.fx_lookup_ids.is_empty());
        assert_eq!(vec![(source.product_listing_id, None)], state.upserts);
        assert_eq!(vec![1], state.write_commit_counts);
        Ok(())
    }

    #[tokio::test]
    async fn should_use_exact_sale_fx_snapshot_when_source_is_sold()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let snapshot = snapshot()?;
        let mut source = source()?;
        source.pricing.price = Some(
            product_listing_core::product_listing_price::ProductListingPrice::from(
                money::Price::new(100_u64.into(), Currency::Eur),
            ),
        );
        source.availability = Some(ListingAvailability::SoldOut);
        source.sale_observation = Some(ListingSaleObservation::new(
            time::OffsetDateTime::UNIX_EPOCH,
            snapshot.id(),
        ));
        let command = command(&source);
        {
            let mut state = lock_state(&state);
            state.source_result = Some(Ok(Some(source.clone())));
            state.fx_result = Some(Ok(Some(snapshot.clone())));
        }

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductListingOutcome::Applied, result.outcome);
        let state = lock_state(&state);
        assert_eq!(vec![snapshot.id()], state.fx_lookup_ids);
        assert_eq!(
            vec![(source.product_listing_id, Some(snapshot))],
            state.upserts
        );
        assert_eq!(vec![1], state.write_commit_counts);
        Ok(())
    }

    #[tokio::test]
    async fn should_project_sold_source_without_price_without_fx_lookup()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let mut source = source()?;
        source.sale_observation = Some(ListingSaleObservation::new(
            time::OffsetDateTime::UNIX_EPOCH,
            FxRateId::new(),
        ));
        let command = command(&source);
        lock_state(&state).source_result = Some(Ok(Some(source.clone())));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductListingOutcome::Applied, result.outcome);
        let state = lock_state(&state);
        assert!(state.fx_lookup_ids.is_empty());
        assert_eq!(vec![(source.product_listing_id, None)], state.upserts);
        assert_eq!(vec![1], state.write_commit_counts);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_or_write_when_sale_observation_fx_snapshot_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let snapshot = snapshot()?;
        let mut source = source()?;
        source.pricing.price = Some(
            product_listing_core::product_listing_price::ProductListingPrice::from(
                money::Price::new(100_u64.into(), Currency::Eur),
            ),
        );
        source.availability = Some(ListingAvailability::SoldOut);
        source.sale_observation = Some(ListingSaleObservation::new(
            time::OffsetDateTime::UNIX_EPOCH,
            snapshot.id(),
        ));
        let command = command(&source);
        {
            let mut state = lock_state(&state);
            state.source_result = Some(Ok(Some(source)));
            state.fx_result = Some(Ok(None));
        }

        let result = handler(&state).execute(command).await;

        assert!(matches!(
            result,
            Err(ProjectProductListingError::SaleObservationFxSnapshotMissing)
        ));
        let state = lock_state(&state);
        assert_eq!(0, state.commit_count);
        assert!(state.upserts.is_empty());
        assert!(state.deletes.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_map_invalid_persisted_sale_observation_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let snapshot = snapshot()?;
        let mut source = source()?;
        source.pricing.price = Some(
            product_listing_core::product_listing_price::ProductListingPrice::from(
                money::Price::new(100_u64.into(), Currency::Eur),
            ),
        );
        source.availability = Some(ListingAvailability::SoldOut);
        source.sale_observation = Some(ListingSaleObservation::new(
            time::OffsetDateTime::UNIX_EPOCH,
            snapshot.id(),
        ));
        let command = command(&source);
        {
            let mut state = lock_state(&state);
            state.source_result = Some(Ok(Some(source)));
            state.fx_result = Some(Err(
                FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                    source: box_error(std::io::Error::other("invalid snapshot")),
                },
            ));
        }

        let result = handler(&state).execute(command).await;

        assert!(matches!(
            result,
            Err(ProjectProductListingError::SaleObservationFxSnapshotInvalid { .. })
        ));
        let state = lock_state(&state);
        assert_eq!(0, state.commit_count);
        assert!(state.upserts.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_commit_without_fx_lookup_or_write_when_source_is_stale()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let mut source = source()?;
        let command = command(&source);
        source.current_event_id = EventId::new();
        lock_state(&state).source_result = Some(Ok(Some(source)));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductListingOutcome::Stale, result.outcome);
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert!(state.fx_lookup_ids.is_empty());
        assert!(state.upserts.is_empty());
        assert!(state.deletes.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_commit_without_write_when_source_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let command = ProjectProductListingCommand {
            event_id: EventId::new(),
            product_listing_id: ProductListingId::new(),
        };
        lock_state(&state).source_result = Some(Ok(None));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductListingOutcome::MissingSource, result.outcome);
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert!(state.fx_lookup_ids.is_empty());
        assert!(state.upserts.is_empty());
        assert!(state.deletes.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_delete_exact_source_version_when_withdrawn_projection_is_applied()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let mut source = source()?;
        source.lifecycle = ListingLifecycle::Withdrawn;
        let command = command(&source);
        lock_state(&state).source_result = Some(Ok(Some(source.clone())));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductListingOutcome::Deleted, result.outcome);
        let state = lock_state(&state);
        assert_eq!(
            vec![(source.product_listing_id, source.projection_version)],
            state.deletes
        );
        assert!(state.upserts.is_empty());
        assert_eq!(vec![1], state.write_commit_counts);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_stale_when_withdrawn_projection_write_is_stale()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let mut source = source()?;
        source.lifecycle = ListingLifecycle::Withdrawn;
        let command = command(&source);
        {
            let mut state = lock_state(&state);
            state.source_result = Some(Ok(Some(source)));
            state.projection_result = Some(Ok(ProductListingSearchProjectionWriteOutcome::Stale));
        }

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductListingOutcome::Stale, result.outcome);
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert_eq!(1, state.deletes.len());
        Ok(())
    }

    #[tokio::test]
    async fn should_map_target_write_error_after_source_transaction_commits()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let source = source()?;
        let command = command(&source);
        {
            let mut state = lock_state(&state);
            state.source_result = Some(Ok(Some(source)));
            state.projection_result =
                Some(Err(ProductListingSearchProjectionWriteError::WriteFailed {
                    source: box_error(std::io::Error::other("target unavailable")),
                }));
        }

        let result = handler(&state).execute(command).await;

        assert!(matches!(
            result,
            Err(ProjectProductListingError::WriteFailed { .. })
        ));
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert_eq!(vec![1], state.write_commit_counts);
        Ok(())
    }
}
