use crate::ports::{
    ActiveSearchFilterMatchCandidate, ActiveSearchFilterMatchCandidateReadError,
    ActiveSearchFilterMatchCandidateReader, ActiveSearchFilterMatchCandidateReaderFactory,
    SearchFilterIndex, SearchFilterIndexError, SearchFilterMatchCandidate,
    SearchFilterMatchPersistOutcome, SearchFilterMatchWriteError, SearchFilterMatchWriter,
    SearchFilterMatchWriterFactory,
};
use crate::product_match_evaluator::{
    ProductListingMatchEvaluationOutcome, ProductListingMatchEvaluationRequest,
    evaluate_product_matches,
};
use application::error::{BoxError, box_error};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
#[cfg(test)]
use fxrate_core::FxRateSnapshot;
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
#[cfg(test)]
use large_language_model::StructuredGenerationRequest;
use large_language_model::{LargeLanguageModel, LargeLanguageModelError};
use product_listing_core::{
    listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
    product_listing::ProductListingPriceValuationBasis, product_listing_id::ProductListingId,
    product_listing_price::ProductListingPrice,
};
use product_listing_service::ports::{
    ProductListingCurrentEventCheck, ProductListingCurrentEventCheckError,
    ProductListingCurrentEventGuard, ProductListingCurrentEventGuardFactory,
    ProductListingPercolationInput, ProductListingPercolationValuation,
    ProductListingPricesByCurrency, ProductListingSearchFilterMatchSource,
    ProductListingSearchFilterMatchSourceReadError, ProductListingSearchFilterMatchSourceReader,
    ProductListingSearchFilterMatchSourceReaderFactory,
};

#[cfg(test)]
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_core::{PriceMatchValuation, SearchFilterProductListingMatch};
use std::num::NonZeroUsize;

const MAX_CONCURRENT_LLM_REQUESTS: NonZeroUsize = match NonZeroUsize::new(4) {
    Some(value) => value,
    None => NonZeroUsize::MIN,
};

#[derive(Debug, Clone, PartialEq)]
pub struct MatchProductListingEventCommand {
    pub origin_event_id: EventId,
    pub product_listing_id: ProductListingId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchProductListingEventOutcome {
    Processed,
    DuplicateAlreadyPersisted,
    StaleSourceSkipped,
    InactiveSourceSkipped,
    SourceNotFound,
    IgnoredEventType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchProductListingEventResult {
    pub outcome: MatchProductListingEventOutcome,
    pub percolated_count: usize,
    pub persisted_match_count: usize,
    /// Enhanced candidates that were not evaluated. Their failures are explicit
    /// operational outcomes; they never make a plain percolation match implicit.
    pub enhanced_evaluation_failure_count: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum MatchProductListingEventError {
    #[error("failed to begin product source read transaction")]
    BeginSourceReadTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("product source read failed")]
    ProductListingSourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("product source persisted state is invalid")]
    ProductListingSourceStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("product source does not match requested event or product")]
    ProductListingSourceMismatch,
    #[error("sale FX snapshot is missing from canonical persisted storage")]
    SaleSnapshotNotFound { fx_rate_id: FxRateId },
    #[error("event-effective FX snapshot is missing from canonical persisted storage")]
    EventSnapshotNotFound {
        origin_event_time: time::OffsetDateTime,
    },
    #[error("sale FX snapshot read failed")]
    SaleSnapshotReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("event-effective FX snapshot read failed")]
    EventSnapshotReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("sale FX snapshot persisted state is invalid")]
    SaleSnapshotStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("event-effective FX snapshot persisted state is invalid")]
    EventSnapshotStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("event-time FX conversion failed")]
    EventValuationConversionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit product source read transaction")]
    CommitSourceReadTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter percolation failed")]
    PercolationFailed {
        #[source]
        source: BoxError,
    },
    #[error("product match evaluation failed")]
    ProductListingMatchEvaluationFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search filter match transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("product current event check failed")]
    ProductListingCurrentEventCheckFailed {
        #[source]
        source: BoxError,
    },
    #[error("active search filter match candidate read failed")]
    CandidateReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("active search filter match candidate state is invalid")]
    CandidateStateInvalid {
        #[source]
        source: BoxError,
    },

    #[error("search filter match persistence failed")]
    MatchPersistenceFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted search filter match state is invalid")]
    PersistedMatchStateInvalid {
        #[source]
        source: BoxError,
    },

    #[error("failed to commit search filter match transaction")]
    CommitTransactionFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait MatchProductListingEventUseCase: Send + Sync {
    async fn execute(
        &self,
        command: MatchProductListingEventCommand,
    ) -> Result<MatchProductListingEventResult, MatchProductListingEventError>;
}

pub struct MatchProductListingEventHandler<U, S, G, F, I, E, R, W> {
    unit_of_work: U,
    sources: S,
    current_event_guard: G,
    fx_rates: F,
    index: I,
    evaluator: E,
    candidates: R,
    matches: W,
}

impl<U, S, G, F, I, E, R, W> MatchProductListingEventHandler<U, S, G, F, I, E, R, W> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        unit_of_work: U,
        sources: S,
        current_event_guard: G,
        fx_rates: F,
        index: I,
        evaluator: E,
        candidates: R,
        matches: W,
    ) -> Self {
        Self {
            unit_of_work,
            sources,
            current_event_guard,
            fx_rates,
            index,
            evaluator,
            candidates,
            matches,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, G, F, I, E, R, W> MatchProductListingEventUseCase
    for MatchProductListingEventHandler<U, S, G, F, I, E, R, W>
where
    U: UnitOfWork,
    S: ProductListingSearchFilterMatchSourceReaderFactory<U::Tx>,
    G: ProductListingCurrentEventGuardFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    I: SearchFilterIndex,
    E: LargeLanguageModel,
    R: ActiveSearchFilterMatchCandidateReaderFactory<U::Tx>,
    W: SearchFilterMatchWriterFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "match_product_event",
        skip_all,
        fields(
            origin_event_id = %command.origin_event_id,
            product_listing_id = %command.product_listing_id,
        )
    )]
    async fn execute(
        &self,
        command: MatchProductListingEventCommand,
    ) -> Result<MatchProductListingEventResult, MatchProductListingEventError> {
        let product =
            load_product_source(&self.unit_of_work, &self.sources, &self.fx_rates, &command)
                .await?;
        let product = match product {
            ProductListingSourceReadOutcome::Missing => {
                return Ok(MatchProductListingEventResult {
                    outcome: MatchProductListingEventOutcome::SourceNotFound,
                    percolated_count: 0,
                    persisted_match_count: 0,
                    enhanced_evaluation_failure_count: 0,
                });
            }
            ProductListingSourceReadOutcome::IgnoredEventType => {
                return Ok(MatchProductListingEventResult {
                    outcome: MatchProductListingEventOutcome::IgnoredEventType,
                    percolated_count: 0,
                    persisted_match_count: 0,
                    enhanced_evaluation_failure_count: 0,
                });
            }
            ProductListingSourceReadOutcome::Stale => {
                return Ok(MatchProductListingEventResult {
                    outcome: MatchProductListingEventOutcome::StaleSourceSkipped,
                    percolated_count: 0,
                    persisted_match_count: 0,
                    enhanced_evaluation_failure_count: 0,
                });
            }
            ProductListingSourceReadOutcome::Inactive => {
                return Ok(MatchProductListingEventResult {
                    outcome: MatchProductListingEventOutcome::InactiveSourceSkipped,
                    percolated_count: 0,
                    persisted_match_count: 0,
                    enhanced_evaluation_failure_count: 0,
                });
            }
            ProductListingSourceReadOutcome::Current(product) => *product,
        };

        let price_match_valuation =
            product
                .valuation
                .as_ref()
                .map(|valuation| PriceMatchValuation {
                    basis: valuation.basis,
                    fx_rate_id: valuation.fx_rate_id,
                });
        let percolated = self
            .index
            .percolate(&product)
            .await
            .map_err(percolation_error)?;
        let percolated_count = percolated.len();
        let evaluated = evaluate_candidates(
            &self.evaluator,
            &product.source,
            percolated,
            price_match_valuation,
        )
        .await;
        let candidates = evaluated.candidates;
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            MatchProductListingEventError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;
        let current_event = self
            .current_event_guard
            .in_transaction(&mut tx)
            .lock_and_check(command.product_listing_id, command.origin_event_id)
            .await
            .map_err(product_current_event_check_error)?;
        if current_event == ProductListingCurrentEventCheck::Stale {
            return Ok(MatchProductListingEventResult {
                outcome: MatchProductListingEventOutcome::StaleSourceSkipped,
                percolated_count,
                persisted_match_count: 0,
                enhanced_evaluation_failure_count: evaluated.enhanced_evaluation_failure_count,
            });
        }

        let mut candidates = if candidates.is_empty() {
            Vec::new()
        } else {
            self.candidates
                .in_transaction(&mut tx)
                .find_active(&candidates)
                .await
                .map_err(candidate_read_error)?
        };
        sort_candidates(&mut candidates);

        let mut persisted_match_count = 0;
        let mut duplicate_match_count = 0;
        for candidate in candidates {
            let product_match = SearchFilterProductListingMatch {
                user_id: candidate.user_id,
                user_search_filter_id: candidate.search_filter_id,
                user_search_filter_name: Some(candidate.search_filter_name),
                product_listing_id: command.product_listing_id,
                origin_event_id: command.origin_event_id,
                price_match_valuation: candidate.price_match_valuation,
                enhanced_match_reason: candidate.enhanced_match_reason,
                feedback: None,
            };
            let outcome = self
                .matches
                .in_transaction(&mut tx)
                .insert_if_absent(&product_match)
                .await
                .map_err(match_write_error)?;
            match outcome {
                SearchFilterMatchPersistOutcome::Inserted => persisted_match_count += 1,
                SearchFilterMatchPersistOutcome::AlreadyExists => duplicate_match_count += 1,
            }
        }

        tx.commit().await.map_err(|source| {
            MatchProductListingEventError::CommitTransactionFailed {
                source: box_error(source),
            }
        })?;

        if let Some(error) = evaluated.retryable_error {
            return Err(product_match_evaluation_error(error));
        }

        Ok(MatchProductListingEventResult {
            outcome: if persisted_match_count == 0 && duplicate_match_count > 0 {
                MatchProductListingEventOutcome::DuplicateAlreadyPersisted
            } else {
                MatchProductListingEventOutcome::Processed
            },
            percolated_count,
            persisted_match_count,
            enhanced_evaluation_failure_count: evaluated.enhanced_evaluation_failure_count,
        })
    }
}

enum ProductListingSourceReadOutcome {
    Missing,
    IgnoredEventType,
    Stale,
    Inactive,
    Current(Box<ProductListingPercolationSource>),
}

type ProductListingPercolationSource = ProductListingPercolationInput;

async fn load_product_source<U, S, F>(
    unit_of_work: &U,
    sources: &S,
    fx_rates: &F,
    command: &MatchProductListingEventCommand,
) -> Result<ProductListingSourceReadOutcome, MatchProductListingEventError>
where
    U: UnitOfWork,
    S: ProductListingSearchFilterMatchSourceReaderFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    let mut tx = unit_of_work.begin().await.map_err(|source| {
        MatchProductListingEventError::BeginSourceReadTransactionFailed {
            source: box_error(source),
        }
    })?;
    let source = sources
        .in_transaction(&mut tx)
        .find_source(command.origin_event_id, command.product_listing_id)
        .await
        .map_err(product_source_read_error)?;
    let outcome = match source {
        None => ProductListingSourceReadOutcome::Missing,
        Some(product)
            if product.event_id != command.origin_event_id
                || product.product_listing_id != command.product_listing_id =>
        {
            return Err(MatchProductListingEventError::ProductListingSourceMismatch);
        }
        Some(product) if !product.event_kind.is_percolation_trigger() => {
            ProductListingSourceReadOutcome::IgnoredEventType
        }
        Some(product) if product.current_event_id != command.origin_event_id => {
            ProductListingSourceReadOutcome::Stale
        }
        Some(product) if product.lifecycle == ListingLifecycle::Withdrawn => {
            ProductListingSourceReadOutcome::Inactive
        }
        Some(product) => {
            let valuation = match product.pricing.price {
                Some(ProductListingPrice::Monetary(source_price)) => {
                    let (basis, snapshot) = match product.sale_observation.filter(|_| {
                        product.availability == Some(ListingAvailability::SoldOut)
                            || product.lifecycle == ListingLifecycle::Withdrawn
                    }) {
                        Some(observation) => (
                            ProductListingPriceValuationBasis::SaleObservation,
                            fx_rates
                                .in_transaction(&mut tx)
                                .find_by_id(observation.fx_rate_id())
                                .await
                                .map_err(sale_snapshot_read_error)?
                                .ok_or(MatchProductListingEventError::SaleSnapshotNotFound {
                                    fx_rate_id: observation.fx_rate_id(),
                                })?,
                        ),
                        None => (
                            ProductListingPriceValuationBasis::Event,
                            fx_rates
                                .in_transaction(&mut tx)
                                .find_latest_at_or_before(product.origin_event_time)
                                .await
                                .map_err(event_snapshot_read_error)?
                                .ok_or(MatchProductListingEventError::EventSnapshotNotFound {
                                    origin_event_time: product.origin_event_time,
                                })?,
                        ),
                    };
                    let prices =
                        ProductListingPricesByCurrency::convert_all(&snapshot, source_price)
                            .map_err(|source| {
                                MatchProductListingEventError::EventValuationConversionFailed {
                                    source: box_error(source),
                                }
                            })?;
                    Some(ProductListingPercolationValuation {
                        basis,
                        fx_rate_id: snapshot.id(),
                        effective_at: snapshot.captured_at(),
                        prices,
                    })
                }
                None | Some(ProductListingPrice::OnRequest) => None,
            };
            ProductListingSourceReadOutcome::Current(Box::new(ProductListingPercolationInput {
                source: product,
                valuation,
            }))
        }
    };
    tx.commit().await.map_err(|source| {
        MatchProductListingEventError::CommitSourceReadTransactionFailed {
            source: box_error(source),
        }
    })?;
    Ok(outcome)
}

struct EvaluatedCandidates {
    candidates: Vec<SearchFilterMatchCandidate>,
    enhanced_evaluation_failure_count: usize,
    retryable_error: Option<LargeLanguageModelError>,
}

async fn evaluate_candidates<E>(
    llm: &E,
    product: &ProductListingSearchFilterMatchSource,
    mut filters: Vec<crate::ports::SearchFilterView>,
    price_match_valuation: Option<PriceMatchValuation>,
) -> EvaluatedCandidates
where
    E: LargeLanguageModel,
{
    filters.retain(|filter| {
        filter.state == search_filter_core::search_filter_state::SearchFilterState::Active
    });
    filters.sort_by_key(|filter| filter.search_filter_id.to_string());
    filters.dedup_by(|left, right| left.search_filter_id == right.search_filter_id);

    let evaluations = filters
        .iter()
        .filter_map(|filter| {
            filter
                .search
                .enhanced_search_description
                .as_deref()
                .map(|search_description| ProductListingMatchEvaluationRequest {
                    key: filter.search_filter_id,
                    product,
                    search_description,
                    search_language: filter.search.language,
                })
        })
        .collect();
    let mut enhanced_evaluations =
        evaluate_product_matches(llm, evaluations, MAX_CONCURRENT_LLM_REQUESTS)
            .await
            .into_iter()
            .map(|evaluation| (evaluation.key, evaluation.outcome))
            .collect::<std::collections::HashMap<_, _>>();

    let mut candidates = Vec::with_capacity(filters.len());
    let mut enhanced_evaluation_failure_count = 0;
    let mut retryable_error = None;
    for filter in filters {
        let enhanced_match_reason = if filter.search.enhanced_search_description.is_some() {
            match enhanced_evaluations.remove(&filter.search_filter_id) {
                Some(ProductListingMatchEvaluationOutcome::Matched(reason)) => Some(reason),
                Some(ProductListingMatchEvaluationOutcome::Rejected) => continue,
                Some(ProductListingMatchEvaluationOutcome::RetryableFailure(error)) => {
                    enhanced_evaluation_failure_count += 1;
                    tracing::warn!(
                        user_search_filter_id = %filter.search_filter_id,
                        error_category = %error,
                        "enhanced product match evaluation failed; plain and successful candidates remain eligible"
                    );
                    if retryable_error.is_none() {
                        retryable_error = Some(error);
                    }
                    continue;
                }
                Some(ProductListingMatchEvaluationOutcome::PermanentFailure(error)) => {
                    enhanced_evaluation_failure_count += 1;
                    tracing::warn!(
                        user_search_filter_id = %filter.search_filter_id,
                        error_category = %error,
                        "enhanced product match evaluation failed; plain and successful candidates remain eligible"
                    );
                    continue;
                }
                None => {
                    enhanced_evaluation_failure_count += 1;
                    tracing::warn!(
                        user_search_filter_id = %filter.search_filter_id,
                        "enhanced product match evaluator omitted a candidate; plain and successful candidates remain eligible"
                    );
                    continue;
                }
            }
        } else {
            None
        };
        candidates.push(SearchFilterMatchCandidate {
            user_id: filter.user_id,
            search_filter_id: filter.search_filter_id,
            price_match_valuation: if filter.search.price_query.is_some() {
                price_match_valuation
            } else {
                None
            },
            enhanced_match_reason,
            expected_search: filter.search,
            expected_embedding: filter.embedding,
        });
    }
    EvaluatedCandidates {
        candidates,
        enhanced_evaluation_failure_count,
        retryable_error,
    }
}

fn sort_candidates(candidates: &mut Vec<ActiveSearchFilterMatchCandidate>) {
    candidates.sort_by_key(|candidate| {
        (
            candidate.user_id.to_string(),
            candidate.search_filter_id.to_string(),
        )
    });
    candidates.dedup_by(|left, right| {
        left.user_id == right.user_id && left.search_filter_id == right.search_filter_id
    });
}

fn product_source_read_error(
    error: ProductListingSearchFilterMatchSourceReadError,
) -> MatchProductListingEventError {
    match error {
        ProductListingSearchFilterMatchSourceReadError::InvalidPersistedState { source } => {
            MatchProductListingEventError::ProductListingSourceStateInvalid { source }
        }
        error => MatchProductListingEventError::ProductListingSourceReadFailed {
            source: box_error(error),
        },
    }
}

fn sale_snapshot_read_error(error: FxRateSnapshotRepositoryError) -> MatchProductListingEventError {
    match error {
        FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
            MatchProductListingEventError::SaleSnapshotStateInvalid { source }
        }
        error => MatchProductListingEventError::SaleSnapshotReadFailed {
            source: box_error(error),
        },
    }
}

fn event_snapshot_read_error(
    error: FxRateSnapshotRepositoryError,
) -> MatchProductListingEventError {
    match error {
        FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
            MatchProductListingEventError::EventSnapshotStateInvalid { source }
        }
        error => MatchProductListingEventError::EventSnapshotReadFailed {
            source: box_error(error),
        },
    }
}

fn percolation_error(error: SearchFilterIndexError) -> MatchProductListingEventError {
    MatchProductListingEventError::PercolationFailed {
        source: box_error(error),
    }
}

fn product_match_evaluation_error(error: LargeLanguageModelError) -> MatchProductListingEventError {
    MatchProductListingEventError::ProductListingMatchEvaluationFailed {
        source: box_error(error),
    }
}

fn product_current_event_check_error(
    error: ProductListingCurrentEventCheckError,
) -> MatchProductListingEventError {
    MatchProductListingEventError::ProductListingCurrentEventCheckFailed {
        source: box_error(error),
    }
}

fn candidate_read_error(
    error: ActiveSearchFilterMatchCandidateReadError,
) -> MatchProductListingEventError {
    match error {
        ActiveSearchFilterMatchCandidateReadError::InvalidPersistedState { source } => {
            MatchProductListingEventError::CandidateStateInvalid { source }
        }
        error => MatchProductListingEventError::CandidateReadFailed {
            source: box_error(error),
        },
    }
}

fn match_write_error(error: SearchFilterMatchWriteError) -> MatchProductListingEventError {
    match error {
        SearchFilterMatchWriteError::InvalidPersistedState { source } => {
            MatchProductListingEventError::PersistedMatchStateInvalid { source }
        }
        error => MatchProductListingEventError::MatchPersistenceFailed {
            source: box_error(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        SearchFilterIndexQuery, SearchFilterProjectionWriteOutcome, SearchFilterView,
    };
    use application::transaction::TransactionError;
    use domain_primitives::query::range_query::RangeQuery;
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use fxrate_service::ports::{
        FxRateSnapshotInsertOutcome, FxRateSnapshotRepository, FxRateSnapshotRepositoryError,
        FxRateSnapshotRepositoryFactory,
    };
    use indexmap::IndexSet;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::Language;
    use money::{Currency, MonetaryAmount, Price};
    use product_listing_core::{
        listing_availability::ListingAvailability,
        listing_lifecycle::ListingLifecycle,
        product_listing::{ListingSaleObservation, ProductListingPricing},
        product_listing_image::ProductListingImage,
        product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
    };
    use product_listing_service::ports::{
        ListingSourceSummary, ProductListingCurrentEventCheck,
        ProductListingCurrentEventCheckError, ProductListingCurrentEventGuard,
        ProductListingCurrentEventGuardFactory, ProductListingSearchFilterMatchSource,
        ProductListingSearchFilterMatchSourceEventKind,
    };
    use search_filter_core::user_search_filter_id::UserSearchFilterId;
    use search_filter_core::user_search_filter_name::UserSearchFilterName;
    use std::sync::{Arc, Mutex};
    use strum::IntoEnumIterator;
    use tokio::sync::Notify;
    use user_core::user_id::UserId;

    use time::OffsetDateTime;
    use url::Url;

    #[derive(Default)]
    struct State {
        committed: usize,
        persisted: Vec<SearchFilterProductListingMatch>,
        active_reads: usize,
        candidate_requests: Vec<SearchFilterMatchCandidate>,
        current_filter: Option<SearchFilterView>,
        sale_snapshot_reads: usize,
        event_snapshot_reads: usize,
        sale_snapshot: Option<FxRateSnapshot>,
        event_snapshot: Option<FxRateSnapshot>,
        current_event_id: Option<EventId>,
        percolations: usize,
    }

    #[derive(Clone, Default)]
    struct FakeUnitOfWork(Arc<Mutex<State>>);

    struct FakeTransaction(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = self.0.lock().map_err(|_| TransactionError::CommitFailed)?;
            state.committed += 1;
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

    struct Sources(Vec<ProductListingSearchFilterMatchSource>);

    struct ReadingSource(Vec<ProductListingSearchFilterMatchSource>);

    #[async_trait::async_trait]
    impl ProductListingSearchFilterMatchSourceReader for ReadingSource {
        async fn find_source(
            &mut self,
            event_id: EventId,
            product_listing_id: ProductListingId,
        ) -> Result<
            Option<ProductListingSearchFilterMatchSource>,
            ProductListingSearchFilterMatchSourceReadError,
        > {
            Ok(self
                .0
                .iter()
                .find(|source| {
                    source.event_id == event_id && source.product_listing_id == product_listing_id
                })
                .cloned())
        }
    }

    impl ProductListingSearchFilterMatchSourceReaderFactory<FakeTransaction> for Sources {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ProductListingSearchFilterMatchSourceReader + 'tx {
            ReadingSource(self.0.clone())
        }
    }

    #[derive(Clone)]
    struct CurrentEventGuards(Arc<Mutex<State>>);

    struct CheckingCurrentEvent<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl ProductListingCurrentEventGuard for CheckingCurrentEvent<'_> {
        async fn lock_and_check(
            &mut self,
            _product_listing_id: ProductListingId,
            expected_event_id: EventId,
        ) -> Result<ProductListingCurrentEventCheck, ProductListingCurrentEventCheckError> {
            let state =
                self.0
                    .lock()
                    .map_err(|_| ProductListingCurrentEventCheckError::CheckFailed {
                        source: box_error(std::io::Error::other("test mutex poisoned")),
                    })?;
            Ok(match state.current_event_id {
                Some(current_event_id) if current_event_id != expected_event_id => {
                    ProductListingCurrentEventCheck::Stale
                }
                Some(_) | None => ProductListingCurrentEventCheck::Current,
            })
        }
    }

    impl ProductListingCurrentEventGuardFactory<FakeTransaction> for CurrentEventGuards {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ProductListingCurrentEventGuard + 'tx {
            CheckingCurrentEvent(&self.0)
        }
    }

    struct Index {
        filters: Vec<SearchFilterView>,
        state: Arc<Mutex<State>>,
    }

    #[async_trait::async_trait]
    impl SearchFilterIndex for Index {
        async fn upsert(
            &self,
            _projection: &crate::ports::SearchFilterProjection,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn delete(
            &self,
            _id: UserSearchFilterId,
            _source_version: i64,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn percolate(
            &self,
            _input: &ProductListingPercolationInput,
        ) -> Result<Vec<SearchFilterView>, SearchFilterIndexError> {
            self.state
                .lock()
                .map_err(|_| SearchFilterIndexError::PercolateFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                })?
                .percolations += 1;
            Ok(self.filters.clone())
        }

        async fn query(
            &self,
            _query: &SearchFilterIndexQuery,
        ) -> Result<
            application::pagination::CursoredResult<SearchFilterView, serde_json::Value>,
            SearchFilterIndexError,
        > {
            Ok(Default::default())
        }
    }

    struct BlockingIndex {
        filters: Vec<SearchFilterView>,
        percolation_started: Arc<Notify>,
        resume_percolation: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl SearchFilterIndex for BlockingIndex {
        async fn upsert(
            &self,
            _projection: &crate::ports::SearchFilterProjection,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn delete(
            &self,
            _id: UserSearchFilterId,
            _source_version: i64,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn percolate(
            &self,
            _input: &ProductListingPercolationInput,
        ) -> Result<Vec<SearchFilterView>, SearchFilterIndexError> {
            self.percolation_started.notify_one();
            self.resume_percolation.notified().await;
            Ok(self.filters.clone())
        }

        async fn query(
            &self,
            _query: &SearchFilterIndexQuery,
        ) -> Result<
            application::pagination::CursoredResult<SearchFilterView, serde_json::Value>,
            SearchFilterIndexError,
        > {
            Ok(Default::default())
        }
    }

    #[derive(Clone)]
    struct FxRates(Arc<Mutex<State>>);

    struct ReadingFxRates<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for ReadingFxRates<'_> {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state =
                self.0
                    .lock()
                    .map_err(|_| FxRateSnapshotRepositoryError::ReadFailed {
                        source: box_error(std::io::Error::other("test mutex poisoned")),
                    })?;
            state.event_snapshot_reads += 1;
            Ok(state.event_snapshot.clone())
        }

        async fn find_by_id(
            &mut self,
            _id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state =
                self.0
                    .lock()
                    .map_err(|_| FxRateSnapshotRepositoryError::ReadFailed {
                        source: box_error(std::io::Error::other("test mutex poisoned")),
                    })?;
            state.sale_snapshot_reads += 1;
            Ok(state.sale_snapshot.clone())
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
        ) -> Result<FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError> {
            Ok(FxRateSnapshotInsertOutcome::Duplicate)
        }
    }

    impl FxRateSnapshotRepositoryFactory<FakeTransaction> for FxRates {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl FxRateSnapshotRepository + 'tx {
            ReadingFxRates(&self.0)
        }
    }

    struct Evaluator;
    struct PermanentlyFailingEvaluator;

    struct PausedEvaluator {
        entered: Arc<tokio::sync::Notify>,
        resume: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl LargeLanguageModel for PausedEvaluator {
        async fn generate<Output>(
            &self,
            _request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: serde::de::DeserializeOwned + Send,
        {
            self.entered.notify_one();
            self.resume.notified().await;
            serde_json::from_str(r#"{"matches":true,"reason":"Matches the evaluated search"}"#)
                .map_err(|source| LargeLanguageModelError::InvalidResponse {
                    source: box_error(source),
                })
        }
    }

    #[async_trait::async_trait]
    impl LargeLanguageModel for Evaluator {
        async fn generate<Output>(
            &self,
            _request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: serde::de::DeserializeOwned + Send,
        {
            serde_json::from_str(r#"{"matches":false}"#).map_err(|source| {
                LargeLanguageModelError::InvalidResponse {
                    source: box_error(source),
                }
            })
        }
    }

    #[async_trait::async_trait]
    impl LargeLanguageModel for PermanentlyFailingEvaluator {
        async fn generate<Output>(
            &self,
            _request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: serde::de::DeserializeOwned + Send,
        {
            Err(LargeLanguageModelError::Permanent {
                source: box_error(std::io::Error::other("invalid Vertex request")),
            })
        }
    }

    #[derive(Clone)]
    struct Candidates(Arc<Mutex<State>>);

    struct ReadingActiveCandidates<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl ActiveSearchFilterMatchCandidateReader for ReadingActiveCandidates<'_> {
        async fn find_active(
            &mut self,
            candidates: &[SearchFilterMatchCandidate],
        ) -> Result<Vec<ActiveSearchFilterMatchCandidate>, ActiveSearchFilterMatchCandidateReadError>
        {
            let mut state = self.0.lock().map_err(|_| {
                ActiveSearchFilterMatchCandidateReadError::ReadFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                }
            })?;
            state.active_reads += 1;
            state.candidate_requests.extend_from_slice(candidates);
            Ok(candidates
                .iter()
                .filter(|candidate| {
                    state.current_filter.as_ref().is_none_or(|current| {
                        current.state == SearchFilterState::Active
                            && current.search == candidate.expected_search
                            && current.embedding == candidate.expected_embedding
                    })
                })
                .map(|candidate| ActiveSearchFilterMatchCandidate {
                    user_id: candidate.user_id,
                    search_filter_id: candidate.search_filter_id,
                    search_filter_name: UserSearchFilterName::from(
                        candidate.search_filter_id.to_string(),
                    ),
                    price_match_valuation: candidate.price_match_valuation,
                    enhanced_match_reason: candidate.enhanced_match_reason.clone(),
                })
                .collect())
        }
    }

    impl ActiveSearchFilterMatchCandidateReaderFactory<FakeTransaction> for Candidates {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ActiveSearchFilterMatchCandidateReader + 'tx {
            ReadingActiveCandidates(&self.0)
        }
    }

    #[derive(Clone)]
    struct Matches(Arc<Mutex<State>>);

    struct WritingMatches<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl SearchFilterMatchWriter for WritingMatches<'_> {
        async fn insert_if_absent(
            &mut self,
            product_match: &SearchFilterProductListingMatch,
        ) -> Result<SearchFilterMatchPersistOutcome, SearchFilterMatchWriteError> {
            let mut state =
                self.0
                    .lock()
                    .map_err(|_| SearchFilterMatchWriteError::WriteFailed {
                        source: box_error(std::io::Error::other("test mutex poisoned")),
                    })?;
            if state.persisted.iter().any(|persisted| {
                persisted.user_search_filter_id == product_match.user_search_filter_id
                    && persisted.product_listing_id == product_match.product_listing_id
            }) {
                return Ok(SearchFilterMatchPersistOutcome::AlreadyExists);
            }
            state.persisted.push(product_match.clone());
            Ok(SearchFilterMatchPersistOutcome::Inserted)
        }
    }

    impl SearchFilterMatchWriterFactory<FakeTransaction> for Matches {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl SearchFilterMatchWriter + 'tx {
            WritingMatches(&self.0)
        }
    }

    fn product() -> Result<ProductListingSearchFilterMatchSource, url::ParseError> {
        let url = Url::parse("https://example.test/product")?;
        let event_id = EventId::new();
        Ok(ProductListingSearchFilterMatchSource {
            event_id,
            event_kind: ProductListingSearchFilterMatchSourceEventKind::Domain,
            origin_event_time: OffsetDateTime::UNIX_EPOCH,
            current_event_id: event_id,
            projection_version: 1,
            product_listing_id: product_listing_core::product_listing_id::ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("product-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source: ListingSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
                slug_id: ListingSourceSlugId::raw("source")
                    .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
            },
            source_listing_id: SourceListingId::try_from("product")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            product_title: None,
            product_description: None,
            titles: std::collections::HashMap::new(),
            descriptions: std::collections::HashMap::new(),
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
        })
    }

    fn fx_snapshot(generation: i64, captured_at: OffsetDateTime) -> FxRateSnapshot {
        let quotes = Currency::iter().map(|currency| {
            FxRateQuote::new(
                currency,
                match currency {
                    Currency::Eur => FX_RATE_SCALE,
                    Currency::Gbp => 850_000,
                    Currency::Usd => 1_100_000,
                    Currency::Jpy => 160_000_000,
                    _ => 1_250_000,
                },
            )
        });
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            captured_at,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            quotes,
        )
        .unwrap_or_else(|error| panic!("valid snapshot: {error}"))
        .into_persisted(
            FxRateGeneration::try_from(generation)
                .unwrap_or_else(|error| panic!("valid generation: {error}")),
        )
    }

    fn filter(user_id: UserId, search_filter_id: UserSearchFilterId) -> SearchFilterView {
        SearchFilterView {
            search_filter_id,
            user_id,
            name: UserSearchFilterName::from("daily"),
            notifications: true,
            state: SearchFilterState::Active,
            search: product_listing_core::product_listing_search::ProductListingSearch::new(
                Language::En,
                Currency::Eur,
            ),
            embedding: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn matching_handler(
        state: Arc<Mutex<State>>,
        sources: Vec<ProductListingSearchFilterMatchSource>,
        search_filter: SearchFilterView,
    ) -> MatchProductListingEventHandler<
        FakeUnitOfWork,
        Sources,
        CurrentEventGuards,
        FxRates,
        Index,
        Evaluator,
        Candidates,
        Matches,
    > {
        MatchProductListingEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(sources),
            CurrentEventGuards(Arc::clone(&state)),
            FxRates(Arc::clone(&state)),
            Index {
                filters: vec![search_filter],
                state: Arc::clone(&state),
            },
            Evaluator,
            Candidates(Arc::clone(&state)),
            Matches(state),
        )
    }

    #[tokio::test]
    async fn should_revalidate_evaluated_inputs_when_filter_changes_during_paused_enhanced_work()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let product = product()?;
        let command = MatchProductListingEventCommand {
            origin_event_id: product.event_id,
            product_listing_id: product.product_listing_id,
        };
        let mut original = filter(UserId::new(), UserSearchFilterId::new());
        original.search.enhanced_search_description = Some("Antique ceramic vase".try_into()?);
        original.embedding = Some(vec![0.25; 768]);
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let handler = MatchProductListingEventHandler::new(
            FakeUnitOfWork(state.clone()),
            Sources(vec![product]),
            CurrentEventGuards(state.clone()),
            FxRates(state.clone()),
            Index {
                filters: vec![original.clone()],
                state: state.clone(),
            },
            PausedEvaluator {
                entered: entered.clone(),
                resume: resume.clone(),
            },
            Candidates(state.clone()),
            Matches(state.clone()),
        );
        let edit_during_external_work = async {
            entered.notified().await;
            {
                let mut state = state
                    .lock()
                    .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
                assert_eq!(
                    1, state.committed,
                    "source transaction must finish before external work"
                );
                assert!(state.candidate_requests.is_empty());
                let mut changed = original.clone();
                changed.search.enhanced_search_description =
                    Some("Modern steel furniture".try_into()?);
                changed.embedding = Some(vec![0.75; 768]);
                state.current_filter = Some(changed);
            }
            resume.notify_one();
            Ok::<_, Box<dyn std::error::Error>>(())
        };
        let (result, edit) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(handler.execute(command), edit_during_external_work)
        })
        .await?;
        edit?;
        let result = result?;
        assert_eq!(1, result.percolated_count);
        assert_eq!(0, result.persisted_match_count);
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(2, state.committed);
        assert!(state.persisted.is_empty());
        assert_eq!(1, state.candidate_requests.len());
        assert_eq!(original.search, state.candidate_requests[0].expected_search);
        assert_eq!(
            original.embedding,
            state.candidate_requests[0].expected_embedding
        );
        assert!(state.candidate_requests[0].enhanced_match_reason.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn should_persist_all_active_candidates_without_a_notification_quota()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let user_id = UserId::new();
        let product = product()?;
        let handler = MatchProductListingEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(vec![product.clone()]),
            CurrentEventGuards(Arc::clone(&state)),
            FxRates(Arc::clone(&state)),
            Index {
                filters: vec![
                    filter(user_id, UserSearchFilterId::new()),
                    filter(user_id, UserSearchFilterId::new()),
                ],
                state: Arc::clone(&state),
            },
            Evaluator,
            Candidates(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
        );

        let result = handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: product.event_id,
                product_listing_id: product.product_listing_id,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(2, result.percolated_count);
        assert_eq!(2, result.persisted_match_count);
        assert_eq!(2, state.committed);
        assert_eq!(1, state.active_reads);
        assert_eq!(0, state.sale_snapshot_reads);
        assert_eq!(2, state.persisted.len());
        Ok(())
    }

    #[tokio::test]
    async fn should_use_event_time_snapshot_and_persist_price_match_provenance()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let event_snapshot = fx_snapshot(1, OffsetDateTime::UNIX_EPOCH);
        state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .event_snapshot = Some(event_snapshot.clone());
        let mut source = product()?;
        source.pricing.price =
            Some(Price::new(MonetaryAmount::from(12_500_u64), Currency::Gbp).into());
        let mut saved_filter = filter(UserId::new(), UserSearchFilterId::new());
        saved_filter.search.price_query = Some(RangeQuery {
            min: Some(MonetaryAmount::from(10_000_u64)),
            max: Some(MonetaryAmount::from(20_000_u64)),
        });
        let handler = matching_handler(Arc::clone(&state), vec![source.clone()], saved_filter);

        let result = handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: source.event_id,
                product_listing_id: source.product_listing_id,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(MatchProductListingEventOutcome::Processed, result.outcome);
        assert_eq!(1, state.event_snapshot_reads);
        assert_eq!(0, state.sale_snapshot_reads);
        assert_eq!(
            Some(PriceMatchValuation {
                basis: ProductListingPriceValuationBasis::Event,
                fx_rate_id: event_snapshot.id(),
            }),
            state.persisted[0].price_match_valuation
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_use_immutable_sale_snapshot_instead_of_newer_event_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let sale_snapshot = fx_snapshot(1, OffsetDateTime::UNIX_EPOCH);
        let newer_event_snapshot =
            fx_snapshot(2, OffsetDateTime::UNIX_EPOCH + time::Duration::days(1));
        {
            let mut state = state
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
            state.sale_snapshot = Some(sale_snapshot.clone());
            state.event_snapshot = Some(newer_event_snapshot);
        }
        let mut source = product()?;
        source.pricing.price =
            Some(Price::new(MonetaryAmount::from(12_500_u64), Currency::Gbp).into());
        source.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            sale_snapshot.id(),
        ));
        source.availability = Some(ListingAvailability::SoldOut);
        let mut saved_filter = filter(UserId::new(), UserSearchFilterId::new());
        saved_filter.search.price_query = Some(RangeQuery {
            min: Some(MonetaryAmount::from(1_u64)),
            max: Some(MonetaryAmount::from(1_000_000_u64)),
        });
        let handler = matching_handler(Arc::clone(&state), vec![source.clone()], saved_filter);

        handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: source.event_id,
                product_listing_id: source.product_listing_id,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, state.sale_snapshot_reads);
        assert_eq!(0, state.event_snapshot_reads);
        assert_eq!(
            Some(PriceMatchValuation {
                basis: ProductListingPriceValuationBasis::SaleObservation,
                fx_rate_id: sale_snapshot.id(),
            }),
            state.persisted[0].price_match_valuation
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_when_event_effective_snapshot_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = product()?;
        source.pricing.price = Some(Price::new(MonetaryAmount::from(1_u64), Currency::Eur).into());
        let handler = matching_handler(
            Arc::new(Mutex::new(State::default())),
            vec![source.clone()],
            filter(UserId::new(), UserSearchFilterId::new()),
        );

        let error = handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: source.event_id,
                product_listing_id: source.product_listing_id,
            })
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("missing snapshot must fail"))?;
        assert!(matches!(
            error,
            MatchProductListingEventError::EventSnapshotNotFound { .. }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_when_sale_snapshot_is_missing() -> Result<(), Box<dyn std::error::Error>> {
        let mut source = product()?;
        source.pricing.price = Some(Price::new(MonetaryAmount::from(1_u64), Currency::Eur).into());
        source.sale_observation = Some(ListingSaleObservation::new(
            OffsetDateTime::UNIX_EPOCH,
            FxRateId::new(),
        ));
        source.availability = Some(ListingAvailability::SoldOut);
        let handler = matching_handler(
            Arc::new(Mutex::new(State::default())),
            vec![source.clone()],
            filter(UserId::new(), UserSearchFilterId::new()),
        );

        let error = handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: source.event_id,
                product_listing_id: source.product_listing_id,
            })
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("missing sale snapshot must fail"))?;
        assert!(matches!(
            error,
            MatchProductListingEventError::SaleSnapshotNotFound { .. }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_persist_plain_candidate_when_enhanced_candidate_fails_permanently()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let user_id = UserId::new();
        let product = product()?;
        let plain = filter(user_id, UserSearchFilterId::new());
        let mut enhanced = filter(user_id, UserSearchFilterId::new());
        enhanced.search.enhanced_search_description = Some(
            product_listing_core::product_listing_search::EnhancedSearchDescription::try_from(
                "only paintings",
            )?,
        );
        let handler = MatchProductListingEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(vec![product.clone()]),
            CurrentEventGuards(Arc::clone(&state)),
            FxRates(Arc::clone(&state)),
            Index {
                filters: vec![plain.clone(), enhanced],
                state: Arc::clone(&state),
            },
            PermanentlyFailingEvaluator,
            Candidates(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
        );

        let result = handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: product.event_id,
                product_listing_id: product.product_listing_id,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, result.persisted_match_count);
        assert_eq!(1, result.enhanced_evaluation_failure_count);
        assert_eq!(1, state.persisted.len());
        assert_eq!(
            plain.search_filter_id,
            state.persisted[0].user_search_filter_id
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_enhanced_candidate_when_evaluator_does_not_match()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let user_id = UserId::new();
        let mut enhanced = filter(user_id, UserSearchFilterId::new());
        enhanced.search.enhanced_search_description = Some(
            product_listing_core::product_listing_search::EnhancedSearchDescription::try_from(
                "only paintings",
            )?,
        );
        let product = product()?;
        let handler = MatchProductListingEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(vec![product.clone()]),
            CurrentEventGuards(Arc::clone(&state)),
            FxRates(Arc::clone(&state)),
            Index {
                filters: vec![enhanced],
                state: Arc::clone(&state),
            },
            Evaluator,
            Candidates(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
        );

        let result = handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: product.event_id,
                product_listing_id: product.product_listing_id,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, result.percolated_count);
        assert_eq!(0, result.persisted_match_count);
        assert_eq!(2, state.committed);
        assert_eq!(0, state.active_reads);
        Ok(())
    }

    #[tokio::test]
    async fn should_keep_matches_order_invariant_for_domain_and_enrichment_events()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let search_filter_id = UserSearchFilterId::new();
        let mut event_a = product()?;
        let mut event_b = event_a.clone();
        event_b.event_id = EventId::new();
        event_b.current_event_id = event_b.event_id;
        event_b.event_kind = ProductListingSearchFilterMatchSourceEventKind::Enrichment;
        event_a.current_event_id = event_b.event_id;

        let state = Arc::new(Mutex::new(State::default()));
        let handler = matching_handler(
            Arc::clone(&state),
            vec![event_a.clone(), event_b.clone()],
            filter(user_id, search_filter_id),
        );
        let stale_first = handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: event_a.event_id,
                product_listing_id: event_a.product_listing_id,
            })
            .await?;
        let current_second = handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: event_b.event_id,
                product_listing_id: event_b.product_listing_id,
            })
            .await?;
        let redelivered_stale = handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: event_a.event_id,
                product_listing_id: event_a.product_listing_id,
            })
            .await?;
        let redelivered_current = handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: event_b.event_id,
                product_listing_id: event_b.product_listing_id,
            })
            .await?;

        assert_eq!(
            MatchProductListingEventOutcome::StaleSourceSkipped,
            stale_first.outcome
        );
        assert_eq!(
            MatchProductListingEventOutcome::Processed,
            current_second.outcome
        );
        assert_eq!(
            MatchProductListingEventOutcome::StaleSourceSkipped,
            redelivered_stale.outcome
        );
        assert_eq!(
            MatchProductListingEventOutcome::DuplicateAlreadyPersisted,
            redelivered_current.outcome
        );
        assert_eq!(
            vec![event_b.event_id],
            state
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?
                .persisted
                .iter()
                .map(|persisted| persisted.origin_event_id)
                .collect::<Vec<_>>()
        );

        let reverse_state = Arc::new(Mutex::new(State::default()));
        let reverse_handler = matching_handler(
            Arc::clone(&reverse_state),
            vec![event_a.clone(), event_b.clone()],
            filter(user_id, search_filter_id),
        );
        let current_first = reverse_handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: event_b.event_id,
                product_listing_id: event_b.product_listing_id,
            })
            .await?;
        let stale_second = reverse_handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: event_a.event_id,
                product_listing_id: event_a.product_listing_id,
            })
            .await?;

        assert_eq!(
            MatchProductListingEventOutcome::Processed,
            current_first.outcome
        );
        assert_eq!(
            MatchProductListingEventOutcome::StaleSourceSkipped,
            stale_second.outcome
        );
        assert_eq!(
            vec![event_b.event_id],
            reverse_state
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?
                .persisted
                .iter()
                .map(|persisted| persisted.origin_event_id)
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_current_withdrawn_source_before_percolation_and_match_work()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let mut product = product()?;
        product.lifecycle = ListingLifecycle::Withdrawn;
        let result = matching_handler(
            Arc::clone(&state),
            vec![product.clone()],
            filter(UserId::new(), UserSearchFilterId::new()),
        )
        .execute(MatchProductListingEventCommand {
            origin_event_id: product.event_id,
            product_listing_id: product.product_listing_id,
        })
        .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(
            MatchProductListingEventOutcome::InactiveSourceSkipped,
            result.outcome
        );
        assert_eq!(0, state.percolations);
        assert_eq!(0, state.active_reads);
        assert_eq!(0, state.sale_snapshot_reads);
        assert_eq!(0, state.event_snapshot_reads);
        assert!(state.persisted.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_distinguish_ignored_and_missing_product_sources()
    -> Result<(), Box<dyn std::error::Error>> {
        let ignored_state = Arc::new(Mutex::new(State::default()));
        let mut ignored_product = product()?;
        ignored_product.event_kind = ProductListingSearchFilterMatchSourceEventKind::Ignored;
        let ignored = matching_handler(
            Arc::clone(&ignored_state),
            vec![ignored_product.clone()],
            filter(UserId::new(), UserSearchFilterId::new()),
        )
        .execute(MatchProductListingEventCommand {
            origin_event_id: ignored_product.event_id,
            product_listing_id: ignored_product.product_listing_id,
        })
        .await?;
        let missing = matching_handler(
            Arc::new(Mutex::new(State::default())),
            Vec::new(),
            filter(UserId::new(), UserSearchFilterId::new()),
        )
        .execute(MatchProductListingEventCommand {
            origin_event_id: EventId::new(),
            product_listing_id: ignored_product.product_listing_id,
        })
        .await?;

        assert_eq!(
            MatchProductListingEventOutcome::IgnoredEventType,
            ignored.outcome
        );
        assert_eq!(
            MatchProductListingEventOutcome::SourceNotFound,
            missing.outcome
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_stale_event_when_product_advances_during_percolation()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let product = product()?;
        let percolation_started = Arc::new(Notify::new());
        let resume_percolation = Arc::new(Notify::new());
        let handler = MatchProductListingEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(vec![product.clone()]),
            CurrentEventGuards(Arc::clone(&state)),
            FxRates(Arc::clone(&state)),
            BlockingIndex {
                filters: vec![filter(UserId::new(), UserSearchFilterId::new())],
                percolation_started: Arc::clone(&percolation_started),
                resume_percolation: Arc::clone(&resume_percolation),
            },
            Evaluator,
            Candidates(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
        );

        let product_listing_id = product.product_listing_id;
        let event_id = product.event_id;
        let matching = tokio::spawn(async move {
            handler
                .execute(MatchProductListingEventCommand {
                    origin_event_id: event_id,
                    product_listing_id,
                })
                .await
        });
        percolation_started.notified().await;
        state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .current_event_id = Some(EventId::new());
        resume_percolation.notify_one();

        let result = matching.await??;
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(
            MatchProductListingEventOutcome::StaleSourceSkipped,
            result.outcome
        );
        assert_eq!(0, result.persisted_match_count);
        assert_eq!(0, state.active_reads);
        assert!(state.persisted.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_ignore_stale_product_listing_events_after_committing_source_read()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let mut product = product()?;
        product.current_event_id = EventId::new();
        let handler = MatchProductListingEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(vec![product.clone()]),
            CurrentEventGuards(Arc::clone(&state)),
            FxRates(Arc::clone(&state)),
            Index {
                filters: Vec::new(),
                state: Arc::clone(&state),
            },
            Evaluator,
            Candidates(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
        );

        let result = handler
            .execute(MatchProductListingEventCommand {
                origin_event_id: product.event_id,
                product_listing_id: product.product_listing_id,
            })
            .await?;

        assert_eq!(
            MatchProductListingEventOutcome::StaleSourceSkipped,
            result.outcome
        );
        assert_eq!(
            1,
            state
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?
                .committed
        );
        Ok(())
    }
}
