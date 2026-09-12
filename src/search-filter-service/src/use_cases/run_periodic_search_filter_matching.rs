use crate::ports::{
    ExistingSearchFilterMatchReader, PeriodicSearchFilterCandidate,
    PeriodicSearchFilterCandidatePageRequest, PeriodicSearchFilterCandidateReadError,
    PeriodicSearchFilterCandidateReader, PeriodicSearchFilterMatchingRunLock,
    PeriodicSearchFilterMatchingRunLockError, PeriodicSearchFilterProgress,
    PeriodicSearchFilterProgressError, PeriodicSearchFilterProgressFactory,
    PeriodicSearchFilterProgressLockOutcome, PeriodicSearchFilterProgressWriteOutcome,
    SearchFilterMatchWriteError, SearchFilterMatchWriter, SearchFilterMatchWriterFactory,
};
use crate::product_match_evaluator::{
    ProductListingMatchEvaluationOutcome, ProductListingMatchEvaluationRequest,
    evaluate_product_matches,
};
use application::error::{BoxError, box_error};
use application::pagination::Cursor;
use application::transaction::{Transaction, UnitOfWork};

use domain_primitives::query::range_query::RangeQuery;
use fxrate_core::{FxRateSnapshot, FxRateSnapshotError};
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use large_language_model::LargeLanguageModel;
use product_listing_core::{
    listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
    product_listing::ProductListingPriceValuationBasis,
};

use product_listing_core::product_listing_search::ProductListingSearch;
use product_listing_service::ports::{
    CompiledProductListingSearch, ProductListingCurrentEventCheck, ProductListingCurrentEventGuard,
    ProductListingCurrentEventGuardFactory, ProductListingCurrentEventRef,
    ProductListingPriceFilterPlan, ProductListingSearchFilterMatchSource,
    ProductListingSearchFilterMatchSourceReadError, ProductListingSearchFilterMatchSourceReader,
    ProductListingSearchFilterMatchSourceReaderFactory, ProductListingSearchFilterMatchSourceRef,
    ProductListingSearchReadError, ProductListingSearchReadRequest, ProductListingSearchReader,
};

use search_filter_core::{PriceMatchValuation, SearchFilterProductListingMatch};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use time::{Duration, OffsetDateTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodicSearchFilterMatchingPolicy {
    pub filter_page_size: NonZeroUsize,
    pub hybrid_scan_limit: NonZeroUsize,
    pub evaluation_limit: NonZeroUsize,
    pub llm_concurrency: NonZeroUsize,
    pub max_attempts: NonZeroUsize,
    pub projection_lag: Duration,
    pub replay_overlap: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunPeriodicSearchFilterMatchingCommand {
    pub started_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPeriodicSearchFilterMatchingOutcome {
    Applied(PeriodicSearchFilterMatchingReport),
    SkippedAlreadyRunning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodicSearchFilterMatchingReport {
    pub window_end: OffsetDateTime,
    pub filters_selected: usize,
    pub filters_completed: usize,
    pub filters_already_covered: usize,
    pub filters_changed_or_inactive: usize,
    pub filters_progress_superseded: usize,
    pub filter_attempts: usize,
    pub filters_retried: usize,
    pub filters_failed: usize,
    pub filters_invalid_persisted_state: usize,
    pub candidates_scanned: usize,
    pub candidates_existing: usize,
    pub candidates_missing_source: usize,
    pub candidates_stale: usize,
    pub candidates_withdrawn: usize,
    pub candidates_rejected: usize,
    pub permanent_evaluation_failures: usize,
    pub retryable_evaluation_failures: usize,
    pub matches_inserted: usize,
    pub matches_duplicate: usize,
}

#[derive(Debug, Default)]
struct FilterAttemptReport {
    candidates_scanned: usize,
    candidates_existing: usize,
    candidates_missing_source: usize,
    candidates_stale: usize,
    candidates_withdrawn: usize,
    candidates_rejected: usize,
    permanent_evaluation_failures: usize,
    retryable_evaluation_failures: usize,
    matches_inserted: usize,
    matches_duplicate: usize,
}

impl PeriodicSearchFilterMatchingReport {
    fn merge_terminal_attempt(&mut self, attempt: &FilterAttemptReport) {
        self.candidates_scanned += attempt.candidates_scanned;
        self.candidates_existing += attempt.candidates_existing;
        self.candidates_missing_source += attempt.candidates_missing_source;
        self.candidates_stale += attempt.candidates_stale;
        self.candidates_withdrawn += attempt.candidates_withdrawn;
        self.candidates_rejected += attempt.candidates_rejected;
        self.permanent_evaluation_failures += attempt.permanent_evaluation_failures;
        self.retryable_evaluation_failures += attempt.retryable_evaluation_failures;
        self.merge_durable_match_counts(attempt);
    }

    fn merge_durable_match_counts(&mut self, attempt: &FilterAttemptReport) {
        self.matches_inserted += attempt.matches_inserted;
        self.matches_duplicate += attempt.matches_duplicate;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunPeriodicSearchFilterMatchingError {
    #[error("periodic matching policy is invalid")]
    InvalidPolicy,
    #[error("failed to acquire periodic search-filter matching run lock")]
    RunLockFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to release periodic search-filter matching run lock")]
    RunLockReleaseFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin FX snapshot transaction")]
    BeginFxSnapshotTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("FX snapshot read failed")]
    FxSnapshotReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("FX snapshot is invalid")]
    FxSnapshotInvalid {
        #[source]
        source: BoxError,
    },
    #[error("no FX snapshot exists at the periodic matching window end")]
    FxSnapshotNotFound,
    #[error("failed to commit FX snapshot transaction")]
    CommitFxSnapshotTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("periodic search-filter candidate read failed")]
    CandidateReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("periodic search-filter candidate persisted state is invalid")]
    CandidateStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing search failed")]
    ProductListingSearchFailed,
    #[error("ProductListing search result is invalid")]
    ProductListingSearchResultInvalid,
    #[error("existing search-filter match read failed")]
    ExistingMatchReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin ProductListing source transaction")]
    BeginProductListingSourceTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing source read failed")]
    ProductListingSourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing source persisted state is invalid")]
    ProductListingSourceStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing source identity does not match its requested reference")]
    ProductListingSourceMismatch,
    #[error("failed to commit ProductListing source transaction")]
    CommitProductListingSourceTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin final periodic match transaction")]
    BeginFinalTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("periodic matching progress operation failed")]
    ProgressFailed {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing current event check failed")]
    ProductListingCurrentEventCheckFailed {
        #[source]
        source: BoxError,
    },
    #[error("search-filter match persistence failed")]
    MatchPersistenceFailed {
        #[source]
        source: BoxError,
    },
    #[error("search-filter match persisted state is invalid")]
    MatchStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit final periodic match transaction")]
    CommitFinalTransactionFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait RunPeriodicSearchFilterMatchingUseCase: Send + Sync {
    async fn execute(
        &self,
        command: RunPeriodicSearchFilterMatchingCommand,
    ) -> Result<RunPeriodicSearchFilterMatchingOutcome, RunPeriodicSearchFilterMatchingError>;
}

pub struct RunPeriodicSearchFilterMatchingHandler<U, L, C, F, P, X, S, E, G, W, Q> {
    unit_of_work: U,
    run_lock: L,
    candidates: C,
    fx_rates: F,
    product_listings: P,
    existing_matches: X,
    sources: S,
    evaluator: E,
    current_event_guard: G,
    matches: W,
    progress: Q,
    policy: PeriodicSearchFilterMatchingPolicy,
}

impl<U, L, C, F, P, X, S, E, G, W, Q>
    RunPeriodicSearchFilterMatchingHandler<U, L, C, F, P, X, S, E, G, W, Q>
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        unit_of_work: U,
        run_lock: L,
        candidates: C,
        fx_rates: F,
        product_listings: P,
        existing_matches: X,
        sources: S,
        evaluator: E,
        current_event_guard: G,
        matches: W,
        progress: Q,
        policy: PeriodicSearchFilterMatchingPolicy,
    ) -> Result<Self, RunPeriodicSearchFilterMatchingError> {
        if policy.evaluation_limit > policy.hybrid_scan_limit
            || policy.projection_lag.is_negative()
            || policy.replay_overlap.is_negative()
        {
            return Err(RunPeriodicSearchFilterMatchingError::InvalidPolicy);
        }
        Ok(Self {
            unit_of_work,
            run_lock,
            candidates,
            fx_rates,
            product_listings,
            existing_matches,
            sources,
            evaluator,
            current_event_guard,
            matches,
            progress,
            policy,
        })
    }
}

#[async_trait::async_trait]
impl<U, L, C, F, P, X, S, E, G, W, Q> RunPeriodicSearchFilterMatchingUseCase
    for RunPeriodicSearchFilterMatchingHandler<U, L, C, F, P, X, S, E, G, W, Q>
where
    U: UnitOfWork,
    L: PeriodicSearchFilterMatchingRunLock,
    C: PeriodicSearchFilterCandidateReader,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    P: ProductListingSearchReader,
    X: ExistingSearchFilterMatchReader,
    S: ProductListingSearchFilterMatchSourceReaderFactory<U::Tx>,
    E: LargeLanguageModel,
    G: ProductListingCurrentEventGuardFactory<U::Tx>,
    W: SearchFilterMatchWriterFactory<U::Tx>,
    Q: PeriodicSearchFilterProgressFactory<U::Tx>,
{
    #[tracing::instrument(name = "run_periodic_search_filter_matching", skip_all, fields(window_end = tracing::field::Empty))]
    async fn execute(
        &self,
        command: RunPeriodicSearchFilterMatchingCommand,
    ) -> Result<RunPeriodicSearchFilterMatchingOutcome, RunPeriodicSearchFilterMatchingError> {
        let Some(lease) = self.run_lock.try_acquire().await.map_err(run_lock_error)? else {
            return Ok(RunPeriodicSearchFilterMatchingOutcome::SkippedAlreadyRunning);
        };
        let result = self.execute_locked(command).await;
        if let Err(error) = &result {
            tracing::error!(error = %error, "search_filter.periodic_match.failed");
        }
        let release_result = lease.release().await.map_err(run_lock_error);
        match (result, release_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(release_error)) => {
                tracing::error!(error = %release_error, "search_filter.periodic_match.run_lock_release_failed");
                Err(error)
            }
            (Ok(_), Err(error)) => Err(error),
        }
    }
}

impl<U, L, C, F, P, X, S, E, G, W, Q>
    RunPeriodicSearchFilterMatchingHandler<U, L, C, F, P, X, S, E, G, W, Q>
where
    U: UnitOfWork,
    C: PeriodicSearchFilterCandidateReader,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    P: ProductListingSearchReader,
    X: ExistingSearchFilterMatchReader,
    S: ProductListingSearchFilterMatchSourceReaderFactory<U::Tx>,
    E: LargeLanguageModel,
    G: ProductListingCurrentEventGuardFactory<U::Tx>,
    W: SearchFilterMatchWriterFactory<U::Tx>,
    Q: PeriodicSearchFilterProgressFactory<U::Tx>,
{
    async fn execute_locked(
        &self,
        command: RunPeriodicSearchFilterMatchingCommand,
    ) -> Result<RunPeriodicSearchFilterMatchingOutcome, RunPeriodicSearchFilterMatchingError> {
        let window_end = command.started_at - self.policy.projection_lag;
        tracing::Span::current().record("window_end", tracing::field::display(window_end));
        tracing::info!(window_end = %window_end, "search_filter.periodic_match.started");
        let snapshot = load_snapshot(&self.unit_of_work, &self.fx_rates, window_end).await?;
        let mut report = PeriodicSearchFilterMatchingReport {
            window_end,
            filters_selected: 0,
            filters_completed: 0,
            filters_already_covered: 0,
            filters_changed_or_inactive: 0,
            filters_progress_superseded: 0,
            filter_attempts: 0,
            filters_retried: 0,
            filters_failed: 0,
            filters_invalid_persisted_state: 0,
            candidates_scanned: 0,
            candidates_existing: 0,
            candidates_missing_source: 0,
            candidates_stale: 0,
            candidates_withdrawn: 0,
            candidates_rejected: 0,
            permanent_evaluation_failures: 0,
            retryable_evaluation_failures: 0,
            matches_inserted: 0,
            matches_duplicate: 0,
        };
        let mut after = None;
        loop {
            let page = self
                .candidates
                .find_active_page(PeriodicSearchFilterCandidatePageRequest {
                    after,
                    page_size: self.policy.filter_page_size.get(),
                    eligible_at_or_before: window_end,
                })
                .await
                .map_err(candidate_read_error)?;
            if page.is_empty() {
                break;
            }
            after = page.last().map(|candidate| candidate.search_filter_id);
            for candidate in page {
                report.filters_selected += 1;
                let mut completed = false;
                let mut failed = false;
                let mut filter_candidates_scanned = 0;
                let mut filter_matches_inserted = 0;
                let mut filter_matches_duplicate = 0;
                for attempt in 1..=self.policy.max_attempts.get() {
                    report.filter_attempts += 1;
                    let mut attempt_report = FilterAttemptReport::default();
                    let result = self
                        .process_filter(&candidate, &snapshot, window_end, &mut attempt_report)
                        .await;
                    filter_candidates_scanned = attempt_report.candidates_scanned;
                    filter_matches_inserted += attempt_report.matches_inserted;
                    filter_matches_duplicate += attempt_report.matches_duplicate;
                    match result {
                        Ok(FilterOutcome::Completed) => {
                            report.merge_terminal_attempt(&attempt_report);
                            completed = true;
                            break;
                        }
                        Ok(FilterOutcome::AlreadyCovered) => {
                            report.merge_terminal_attempt(&attempt_report);
                            report.filters_already_covered += 1;
                            tracing::info!(search_filter_id = %candidate.search_filter_id, outcome = "already_covered", "search_filter.periodic_match.filter_noop");
                            completed = true;
                            break;
                        }
                        Ok(FilterOutcome::ChangedOrInactive) => {
                            report.merge_terminal_attempt(&attempt_report);
                            report.filters_changed_or_inactive += 1;
                            tracing::info!(search_filter_id = %candidate.search_filter_id, "search_filter.periodic_match.filter_changed");
                            completed = true;
                            break;
                        }
                        Ok(FilterOutcome::ProgressSuperseded) => {
                            report.merge_terminal_attempt(&attempt_report);
                            report.filters_progress_superseded += 1;
                            tracing::info!(search_filter_id = %candidate.search_filter_id, outcome = "progress_superseded", "search_filter.periodic_match.filter_noop");
                            completed = true;
                            break;
                        }
                        Ok(FilterOutcome::InvalidPersistedState) => {
                            report.merge_terminal_attempt(&attempt_report);
                            mark_invalid_persisted_state(&mut report);
                            tracing::error!(search_filter_id = %candidate.search_filter_id, outcome = "invalid_persisted_state", "search_filter.periodic_match.filter_invalid_persisted_state");
                            failed = true;
                            break;
                        }
                        Ok(FilterOutcome::Retryable) => {
                            if attempt == self.policy.max_attempts.get() {
                                report.merge_terminal_attempt(&attempt_report);
                                report.filters_failed += 1;
                                failed = true;
                            } else {
                                report.merge_durable_match_counts(&attempt_report);
                                report.filters_retried += 1;
                                retry_delay(attempt).await;
                            }
                        }
                        Err(error) if error.retry_class() == FilterRetryClass::Retryable => {
                            if attempt == self.policy.max_attempts.get() {
                                report.merge_terminal_attempt(&attempt_report);
                                report.filters_failed += 1;
                                tracing::error!(search_filter_id = %candidate.search_filter_id, attempt, error = %error, "search_filter.periodic_match.filter_failed");
                                failed = true;
                            } else {
                                report.merge_durable_match_counts(&attempt_report);
                                report.filters_retried += 1;
                                tracing::warn!(search_filter_id = %candidate.search_filter_id, attempt, error = %error, "search_filter.periodic_match.filter_retry");
                                retry_delay(attempt).await;
                            }
                        }
                        Err(error) => {
                            report.merge_terminal_attempt(&attempt_report);
                            report.filters_failed += 1;
                            tracing::error!(search_filter_id = %candidate.search_filter_id, attempt, error = %error, "search_filter.periodic_match.filter_failed");
                            failed = true;
                            break;
                        }
                    }
                }
                if completed {
                    report.filters_completed += 1;
                    tracing::info!(
                        search_filter_id = %candidate.search_filter_id,
                        filter_candidates_scanned,
                        filter_matches_inserted,
                        filter_matches_duplicate,
                        run_matches_inserted_total = report.matches_inserted,
                        "search_filter.periodic_match.filter_completed"
                    );
                } else if !failed {
                    report.filters_failed += 1;
                    tracing::warn!(
                        search_filter_id = %candidate.search_filter_id,
                        "search_filter.periodic_match.filter_failed"
                    );
                }
            }
        }
        tracing::info!(
            window_end = %report.window_end,
            filters_selected = report.filters_selected,
            filters_completed = report.filters_completed,
            filters_failed = report.filters_failed,
            candidates_scanned = report.candidates_scanned,
            candidates_withdrawn = report.candidates_withdrawn,
            matches_inserted = report.matches_inserted,
            matches_duplicate = report.matches_duplicate,
            "search_filter.periodic_match.completed"
        );
        Ok(RunPeriodicSearchFilterMatchingOutcome::Applied(report))
    }

    async fn process_filter(
        &self,
        filter: &PeriodicSearchFilterCandidate,
        snapshot: &FxRateSnapshot,
        window_end: OffsetDateTime,
        report: &mut FilterAttemptReport,
    ) -> Result<FilterOutcome, RunPeriodicSearchFilterMatchingError> {
        if window_end <= filter.matched_through {
            return Ok(FilterOutcome::AlreadyCovered);
        }
        let description = match periodic_search_description(filter) {
            Ok(description) => description,
            Err(outcome) => return Ok(outcome),
        };
        let embedding = match filter_embedding(filter) {
            Ok(embedding) => embedding,
            Err(outcome) => return Ok(outcome),
        };
        let Some(search) = periodic_search(filter, window_end, self.policy.replay_overlap) else {
            return self
                .commit_filter(filter, window_end, Vec::new(), true, report)
                .await;
        };
        let price_filter_plan = ProductListingPriceFilterPlan::compile(
            snapshot.clone(),
            search.currency,
            search.price_query,
        )
        .map_err(|source: FxRateSnapshotError| {
            RunPeriodicSearchFilterMatchingError::FxSnapshotInvalid {
                source: box_error(source),
            }
        })?;
        let request = ProductListingSearchReadRequest {
            compiled_search: CompiledProductListingSearch {
                search,
                price_filter_plan: price_filter_plan.clone(),
            },
            sort: None,
            cursor: Some(Cursor {
                size: self.policy.hybrid_scan_limit.get() as u64,
                search_after: None,
            }),
        };
        let result = self
            .product_listings
            .search_hybrid(&request, embedding)
            .await
            .map_err(product_search_error)?;
        report.candidates_scanned += result.items.len();
        let candidates = result
            .items
            .into_iter()
            .filter(|item| {
                if is_withdrawn_candidate(item.lifecycle) {
                    report.candidates_withdrawn += 1;
                    false
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();
        let ids = candidates
            .iter()
            .map(|item| item.product_listing_id)
            .collect::<Vec<_>>();
        let existing = self
            .existing_matches
            .find_existing_product_listing_ids(filter.search_filter_id, &ids)
            .await
            .map_err(
                |source| RunPeriodicSearchFilterMatchingError::ExistingMatchReadFailed {
                    source: box_error(source),
                },
            )?;
        report.candidates_existing += existing.len();
        let refs = candidates
            .into_iter()
            .filter(|item| !existing.contains(&item.product_listing_id))
            .take(self.policy.evaluation_limit.get())
            .map(|item| ProductListingSearchFilterMatchSourceRef {
                product_listing_id: item.product_listing_id,
                event_id: item.event_id,
            })
            .collect::<Vec<_>>();
        let sources = self.load_sources(&refs).await?;
        let mut evaluations = Vec::new();
        for reference in refs {
            match sources.get(&reference) {
                None => report.candidates_missing_source += 1,
                Some(source)
                    if source.product_listing_id != reference.product_listing_id
                        || source.event_id != reference.event_id =>
                {
                    return Err(RunPeriodicSearchFilterMatchingError::ProductListingSourceMismatch);
                }
                Some(source) if source.current_event_id != reference.event_id => {
                    report.candidates_stale += 1
                }
                Some(source) if is_withdrawn_candidate(source.lifecycle) => {
                    report.candidates_withdrawn += 1
                }
                Some(source) => evaluations.push(ProductListingMatchEvaluationRequest {
                    key: reference,
                    product: source,
                    search_description: description.as_ref(),
                    search_language: filter.search.language,
                }),
            }
        }
        let evaluations =
            evaluate_product_matches(&self.evaluator, evaluations, self.policy.llm_concurrency)
                .await;
        let mut retryable = false;
        let mut accepted = Vec::new();
        for evaluation in evaluations {
            match evaluation.outcome {
                ProductListingMatchEvaluationOutcome::Matched(reason) => {
                    let source = sources.get(&evaluation.key).ok_or(
                        RunPeriodicSearchFilterMatchingError::ProductListingSourceMismatch,
                    )?;
                    let valuation =
                        filter
                            .search
                            .price_query
                            .as_ref()
                            .map(|_| PriceMatchValuation {
                                basis: if has_applicable_sale_observation(source) {
                                    ProductListingPriceValuationBasis::SaleObservation
                                } else {
                                    ProductListingPriceValuationBasis::Current
                                },
                                fx_rate_id: if let Some(observation) =
                                    applicable_sale_observation(source)
                                {
                                    observation.fx_rate_id()
                                } else {
                                    price_filter_plan.fx_rate_id
                                },
                            });
                    accepted.push(SearchFilterProductListingMatch {
                        user_id: filter.user_id,
                        user_search_filter_id: filter.search_filter_id,
                        user_search_filter_name: Some(filter.name.clone()),
                        product_listing_id: source.product_listing_id,
                        origin_event_id: source.event_id,
                        price_match_valuation: valuation,
                        enhanced_match_reason: Some(reason),
                        feedback: None,
                    });
                }
                ProductListingMatchEvaluationOutcome::Rejected => report.candidates_rejected += 1,
                ProductListingMatchEvaluationOutcome::PermanentFailure(_) => {
                    report.permanent_evaluation_failures += 1
                }
                ProductListingMatchEvaluationOutcome::RetryableFailure(_) => {
                    report.retryable_evaluation_failures += 1;
                    retryable = true;
                }
            }
        }
        let outcome = self
            .commit_filter(filter, window_end, accepted, !retryable, report)
            .await?;
        Ok(
            if matches!(outcome, FilterOutcome::Completed) && retryable {
                FilterOutcome::Retryable
            } else {
                outcome
            },
        )
    }

    async fn load_sources(
        &self,
        refs: &[ProductListingSearchFilterMatchSourceRef],
    ) -> Result<
        HashMap<ProductListingSearchFilterMatchSourceRef, ProductListingSearchFilterMatchSource>,
        RunPeriodicSearchFilterMatchingError,
    > {
        if refs.is_empty() {
            return Ok(HashMap::new());
        }
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::BeginProductListingSourceTransactionFailed {
                source: box_error(source),
            }
        })?;
        let sources = self
            .sources
            .in_transaction(&mut tx)
            .find_sources(refs)
            .await
            .map_err(product_source_error)?;
        tx.commit().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::CommitProductListingSourceTransactionFailed {
                source: box_error(source),
            }
        })?;
        Ok(sources)
    }

    async fn commit_filter(
        &self,
        filter: &PeriodicSearchFilterCandidate,
        window_end: OffsetDateTime,
        matches: Vec<SearchFilterProductListingMatch>,
        advance_progress: bool,
        report: &mut FilterAttemptReport,
    ) -> Result<FilterOutcome, RunPeriodicSearchFilterMatchingError> {
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::BeginFinalTransactionFailed {
                source: box_error(source),
            }
        })?;
        let matched_through = match self
            .progress
            .in_transaction(&mut tx)
            .lock_and_read(
                filter.search_filter_id,
                filter.version,
                filter.created,
                window_end,
            )
            .await
            .map_err(progress_error)?
        {
            PeriodicSearchFilterProgressLockOutcome::Current { matched_through } => matched_through,
            PeriodicSearchFilterProgressLockOutcome::AlreadyCovered => {
                return Ok(FilterOutcome::AlreadyCovered);
            }
            PeriodicSearchFilterProgressLockOutcome::ChangedOrInactive => {
                return Ok(FilterOutcome::ChangedOrInactive);
            }
        };
        if matched_through != filter.matched_through {
            return Ok(FilterOutcome::ProgressSuperseded);
        }
        if window_end <= matched_through {
            return Ok(FilterOutcome::AlreadyCovered);
        }
        let refs = matches
            .iter()
            .map(|item| ProductListingCurrentEventRef {
                product_listing_id: item.product_listing_id,
                expected_event_id: item.origin_event_id,
            })
            .collect::<Vec<_>>();
        let checks = self
            .current_event_guard
            .in_transaction(&mut tx)
            .lock_and_check_all(&refs)
            .await
            .map_err(|source| {
                RunPeriodicSearchFilterMatchingError::ProductListingCurrentEventCheckFailed {
                    source: box_error(source),
                }
            })?;
        let matches = matches
            .into_iter()
            .filter(|item| {
                checks.get(&ProductListingCurrentEventRef {
                    product_listing_id: item.product_listing_id,
                    expected_event_id: item.origin_event_id,
                }) == Some(&ProductListingCurrentEventCheck::Current)
            })
            .collect::<Vec<_>>();
        let persisted = if matches.is_empty() {
            Default::default()
        } else {
            self.matches
                .in_transaction(&mut tx)
                .insert_all_if_absent(&matches)
                .await
                .map_err(match_write_error)?
        };
        if advance_progress {
            let progress = self
                .progress
                .in_transaction(&mut tx)
                .compare_and_set(filter.search_filter_id, matched_through, window_end)
                .await
                .map_err(progress_error)?;
            match progress {
                PeriodicSearchFilterProgressWriteOutcome::Advanced => {}
                PeriodicSearchFilterProgressWriteOutcome::AlreadyCovered => {
                    return Ok(FilterOutcome::AlreadyCovered);
                }
                PeriodicSearchFilterProgressWriteOutcome::Superseded => {
                    return Ok(FilterOutcome::ProgressSuperseded);
                }
            }
        }
        tx.commit().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::CommitFinalTransactionFailed {
                source: box_error(source),
            }
        })?;
        report.matches_inserted += persisted.inserted;
        report.matches_duplicate += persisted.already_exists;
        Ok(FilterOutcome::Completed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterOutcome {
    Completed,
    AlreadyCovered,
    ChangedOrInactive,
    ProgressSuperseded,
    InvalidPersistedState,
    Retryable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterRetryClass {
    Retryable,
    Permanent,
}

impl RunPeriodicSearchFilterMatchingError {
    const fn retry_class(&self) -> FilterRetryClass {
        match self {
            Self::ProductListingSearchFailed
            | Self::ExistingMatchReadFailed { .. }
            | Self::BeginProductListingSourceTransactionFailed { .. }
            | Self::ProductListingSourceReadFailed { .. }
            | Self::CommitProductListingSourceTransactionFailed { .. }
            | Self::BeginFinalTransactionFailed { .. }
            | Self::ProgressFailed { .. }
            | Self::ProductListingCurrentEventCheckFailed { .. }
            | Self::MatchPersistenceFailed { .. }
            | Self::CommitFinalTransactionFailed { .. } => FilterRetryClass::Retryable,
            Self::InvalidPolicy
            | Self::RunLockFailed { .. }
            | Self::RunLockReleaseFailed { .. }
            | Self::BeginFxSnapshotTransactionFailed { .. }
            | Self::FxSnapshotReadFailed { .. }
            | Self::FxSnapshotInvalid { .. }
            | Self::FxSnapshotNotFound
            | Self::CommitFxSnapshotTransactionFailed { .. }
            | Self::CandidateReadFailed { .. }
            | Self::CandidateStateInvalid { .. }
            | Self::ProductListingSearchResultInvalid
            | Self::ProductListingSourceStateInvalid { .. }
            | Self::ProductListingSourceMismatch
            | Self::MatchStateInvalid { .. } => FilterRetryClass::Permanent,
        }
    }
}

async fn retry_delay(attempt: usize) {
    let delay = if attempt == 1 {
        std::time::Duration::from_millis(250)
    } else {
        std::time::Duration::from_secs(1)
    };
    tokio::time::sleep(delay).await;
}

fn periodic_search_description(
    filter: &PeriodicSearchFilterCandidate,
) -> Result<&product_listing_core::product_listing_search::EnhancedSearchDescription, FilterOutcome>
{
    filter
        .search
        .enhanced_search_description
        .as_ref()
        .ok_or(FilterOutcome::InvalidPersistedState)
}

fn filter_embedding(filter: &PeriodicSearchFilterCandidate) -> Result<&[f32], FilterOutcome> {
    filter
        .embedding
        .as_deref()
        .ok_or(FilterOutcome::InvalidPersistedState)
}

fn mark_invalid_persisted_state(report: &mut PeriodicSearchFilterMatchingReport) {
    report.filters_failed += 1;
    report.filters_invalid_persisted_state += 1;
}

async fn load_snapshot<U, F>(
    unit_of_work: &U,
    fx_rates: &F,
    window_end: OffsetDateTime,
) -> Result<FxRateSnapshot, RunPeriodicSearchFilterMatchingError>
where
    U: UnitOfWork,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    let mut tx = unit_of_work.begin().await.map_err(|source| {
        RunPeriodicSearchFilterMatchingError::BeginFxSnapshotTransactionFailed {
            source: box_error(source),
        }
    })?;
    let snapshot = fx_rates
        .in_transaction(&mut tx)
        .find_latest_at_or_before(window_end)
        .await
        .map_err(fx_snapshot_error)?;
    tx.commit().await.map_err(|source| {
        RunPeriodicSearchFilterMatchingError::CommitFxSnapshotTransactionFailed {
            source: box_error(source),
        }
    })?;
    snapshot.ok_or(RunPeriodicSearchFilterMatchingError::FxSnapshotNotFound)
}

fn periodic_search(
    filter: &PeriodicSearchFilterCandidate,
    window_end: OffsetDateTime,
    replay_overlap: Duration,
) -> Option<ProductListingSearch> {
    let replay_candidate = filter
        .matched_through
        .checked_sub(replay_overlap)
        .unwrap_or(filter.created);
    let replay_start = replay_candidate.max(filter.created);
    let min = filter
        .search
        .updated_query
        .as_ref()
        .and_then(|range| range.min)
        .map_or(replay_start, |value| value.max(replay_start));
    let max = filter
        .search
        .updated_query
        .as_ref()
        .and_then(|range| range.max)
        .map_or(window_end, |value| value.min(window_end));
    if min > max {
        return None;
    }
    let mut search = filter.search.clone();
    search.updated_query = Some(RangeQuery {
        min: Some(min),
        max: Some(max),
    });
    if let Some(query) = search
        .enhanced_search_description
        .as_ref()
        .and_then(|description| description.as_ref().try_into().ok())
        .filter(|query| {
            !search
                .product_listing_query
                .iter()
                .any(|existing| existing == query)
        })
    {
        search.product_listing_query.push(query)
    }
    Some(search)
}

fn run_lock_error(
    error: PeriodicSearchFilterMatchingRunLockError,
) -> RunPeriodicSearchFilterMatchingError {
    match error {
        PeriodicSearchFilterMatchingRunLockError::LockFailed { source } => {
            RunPeriodicSearchFilterMatchingError::RunLockFailed { source }
        }
        PeriodicSearchFilterMatchingRunLockError::ReleaseFailed { source } => {
            RunPeriodicSearchFilterMatchingError::RunLockReleaseFailed { source }
        }
    }
}
fn candidate_read_error(
    error: PeriodicSearchFilterCandidateReadError,
) -> RunPeriodicSearchFilterMatchingError {
    match error {
        PeriodicSearchFilterCandidateReadError::InvalidPersistedState { source } => {
            RunPeriodicSearchFilterMatchingError::CandidateStateInvalid { source }
        }
        PeriodicSearchFilterCandidateReadError::ReadFailed { source } => {
            RunPeriodicSearchFilterMatchingError::CandidateReadFailed { source }
        }
    }
}
fn fx_snapshot_error(error: FxRateSnapshotRepositoryError) -> RunPeriodicSearchFilterMatchingError {
    match error {
        FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
            RunPeriodicSearchFilterMatchingError::FxSnapshotInvalid { source }
        }
        error => RunPeriodicSearchFilterMatchingError::FxSnapshotReadFailed {
            source: box_error(error),
        },
    }
}
fn product_search_error(
    error: ProductListingSearchReadError,
) -> RunPeriodicSearchFilterMatchingError {
    match error {
        ProductListingSearchReadError::ProductListingSearchQueryFailed => {
            RunPeriodicSearchFilterMatchingError::ProductListingSearchFailed
        }
        ProductListingSearchReadError::ProductListingSearchReadModelInvalid => {
            RunPeriodicSearchFilterMatchingError::ProductListingSearchResultInvalid
        }
    }
}
fn is_withdrawn_candidate(lifecycle: ListingLifecycle) -> bool {
    lifecycle == ListingLifecycle::Withdrawn
}

fn applicable_sale_observation(
    source: &ProductListingSearchFilterMatchSource,
) -> Option<product_listing_core::product_listing::ListingSaleObservation> {
    if source.availability == Some(ListingAvailability::SoldOut)
        || source.lifecycle == ListingLifecycle::Withdrawn
    {
        source.sale_observation
    } else {
        None
    }
}

fn has_applicable_sale_observation(source: &ProductListingSearchFilterMatchSource) -> bool {
    applicable_sale_observation(source).is_some()
}

fn product_source_error(
    error: ProductListingSearchFilterMatchSourceReadError,
) -> RunPeriodicSearchFilterMatchingError {
    match error {
        ProductListingSearchFilterMatchSourceReadError::InvalidPersistedState { source } => {
            RunPeriodicSearchFilterMatchingError::ProductListingSourceStateInvalid { source }
        }
        error => RunPeriodicSearchFilterMatchingError::ProductListingSourceReadFailed {
            source: box_error(error),
        },
    }
}
fn progress_error(
    error: PeriodicSearchFilterProgressError,
) -> RunPeriodicSearchFilterMatchingError {
    RunPeriodicSearchFilterMatchingError::ProgressFailed {
        source: box_error(error),
    }
}
fn match_write_error(error: SearchFilterMatchWriteError) -> RunPeriodicSearchFilterMatchingError {
    match error {
        SearchFilterMatchWriteError::InvalidPersistedState { source } => {
            RunPeriodicSearchFilterMatchingError::MatchStateInvalid { source }
        }
        error => RunPeriodicSearchFilterMatchingError::MatchPersistenceFailed {
            source: box_error(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ExistingSearchFilterMatchReadError, PeriodicSearchFilterMatchingRunLease,
        PeriodicSearchFilterProgressLockOutcome, SearchFilterMatchPersistOutcome,
    };
    use application::transaction::TransactionError;
    use domain_primitives::event_id::EventId;
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateId, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use fxrate_service::ports::FxRateSnapshotInsertOutcome;
    use indexmap::IndexSet;
    use large_language_model::{LargeLanguageModelError, StructuredGenerationRequest};
    use listing_source_core::ListingSourceId;
    use localization::Language;
    use money::Currency;
    use product_listing_core::{
        listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
        product_listing_slug_id::ProductListingSlugId, source_listing_id::SourceListingId,
    };
    use product_listing_service::use_cases::queries::search_product_listings::{
        ProductListingSearchItem, ProductListingSearchReadResult,
        ProductListingSummaryPriceValuation,
    };
    use search_filter_core::{
        search_filter_state::SearchFilterState, user_search_filter_name::UserSearchFilterName,
    };
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};
    use strum::IntoEnumIterator;
    use url::Url;
    use user_core::user_id::UserId;

    #[derive(Debug)]
    struct State {
        lock_outcome: PeriodicSearchFilterProgressLockOutcome,
        current_event_check: ProductListingCurrentEventCheck,
        product_listing_search_result: Option<ProductListingSearchReadResult>,
        evaluator_calls: usize,
        match_writer_calls: usize,
        commits: usize,
        event_checks: usize,
        persisted: Vec<SearchFilterProductListingMatch>,
        checkpoints: usize,
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

    struct NoopRunLock;
    struct NoopRunLease;

    #[async_trait::async_trait]
    impl PeriodicSearchFilterMatchingRunLease for NoopRunLease {
        async fn release(self: Box<Self>) -> Result<(), PeriodicSearchFilterMatchingRunLockError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl PeriodicSearchFilterMatchingRunLock for NoopRunLock {
        async fn try_acquire(
            &self,
        ) -> Result<
            Option<Box<dyn PeriodicSearchFilterMatchingRunLease>>,
            PeriodicSearchFilterMatchingRunLockError,
        > {
            Ok(Some(Box::new(NoopRunLease)))
        }
    }

    struct NoopCandidates;

    #[async_trait::async_trait]
    impl PeriodicSearchFilterCandidateReader for NoopCandidates {
        async fn find_active_page(
            &self,
            _request: PeriodicSearchFilterCandidatePageRequest,
        ) -> Result<Vec<PeriodicSearchFilterCandidate>, PeriodicSearchFilterCandidateReadError>
        {
            Ok(Vec::new())
        }
    }

    struct NoopFxRates;
    struct ReadingNoopFxRates;

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for ReadingNoopFxRates {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_by_id(
            &mut self,
            _id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
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

    impl FxRateSnapshotRepositoryFactory<FakeTransaction> for NoopFxRates {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl FxRateSnapshotRepository + 'tx {
            ReadingNoopFxRates
        }
    }

    struct NoopProductListings(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl ProductListingSearchReader for NoopProductListings {
        async fn search(
            &self,
            _request: &ProductListingSearchReadRequest,
        ) -> Result<
            product_listing_service::use_cases::queries::search_product_listings::ProductListingSearchReadResult,
            ProductListingSearchReadError,
        >{
            Err(ProductListingSearchReadError::ProductListingSearchQueryFailed)
        }

        async fn search_hybrid(
            &self,
            _request: &ProductListingSearchReadRequest,
            _embedding: &[f32],
        ) -> Result<
            product_listing_service::use_cases::queries::search_product_listings::ProductListingSearchReadResult,
            ProductListingSearchReadError,
        >{
            self.0
                .lock()
                .map(|state| match &state.product_listing_search_result {
                    Some(result) => result.clone(),
                    None => Default::default(),
                })
                .map_err(|_| ProductListingSearchReadError::ProductListingSearchQueryFailed)
        }
    }

    struct NoopExistingMatches;

    #[async_trait::async_trait]
    impl ExistingSearchFilterMatchReader for NoopExistingMatches {
        async fn find_existing_product_listing_ids(
            &self,
            _search_filter_id: search_filter_core::user_search_filter_id::UserSearchFilterId,
            _product_listing_ids: &[ProductListingId],
        ) -> Result<HashSet<ProductListingId>, ExistingSearchFilterMatchReadError> {
            Ok(HashSet::new())
        }
    }

    struct NoopSources;
    struct ReadingNoopSources;

    #[async_trait::async_trait]
    impl ProductListingSearchFilterMatchSourceReader for ReadingNoopSources {
        async fn find_source(
            &mut self,
            _event_id: EventId,
            _product_listing_id: ProductListingId,
        ) -> Result<
            Option<ProductListingSearchFilterMatchSource>,
            ProductListingSearchFilterMatchSourceReadError,
        > {
            Ok(None)
        }
    }

    impl ProductListingSearchFilterMatchSourceReaderFactory<FakeTransaction> for NoopSources {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ProductListingSearchFilterMatchSourceReader + 'tx {
            ReadingNoopSources
        }
    }

    struct NoopEvaluator(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl LargeLanguageModel for NoopEvaluator {
        async fn generate<Output>(
            &self,
            _request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: serde::de::DeserializeOwned + Send,
        {
            self.0
                .lock()
                .map_err(|_| LargeLanguageModelError::Permanent {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                })?
                .evaluator_calls += 1;
            Err(LargeLanguageModelError::Permanent {
                source: box_error(std::io::Error::other("unused test evaluator")),
            })
        }
    }

    #[derive(Clone)]
    struct FakeCurrentEvents(Arc<Mutex<State>>);

    struct CheckingCurrentEvents<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl ProductListingCurrentEventGuard for CheckingCurrentEvents<'_> {
        async fn lock_and_check(
            &mut self,
            _product_listing_id: ProductListingId,
            _expected_event_id: EventId,
        ) -> Result<
            ProductListingCurrentEventCheck,
            product_listing_service::ports::ProductListingCurrentEventCheckError,
        > {
            let mut state = self.0.lock().map_err(|_| {
                product_listing_service::ports::ProductListingCurrentEventCheckError::CheckFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                }
            })?;
            state.event_checks += 1;
            Ok(state.current_event_check)
        }
    }

    impl ProductListingCurrentEventGuardFactory<FakeTransaction> for FakeCurrentEvents {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ProductListingCurrentEventGuard + 'tx {
            CheckingCurrentEvents(&self.0)
        }
    }

    #[derive(Clone)]
    struct FakeMatches(Arc<Mutex<State>>);

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
            state.match_writer_calls += 1;
            state.persisted.push(product_match.clone());
            Ok(SearchFilterMatchPersistOutcome::Inserted)
        }
    }

    impl SearchFilterMatchWriterFactory<FakeTransaction> for FakeMatches {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl SearchFilterMatchWriter + 'tx {
            WritingMatches(&self.0)
        }
    }

    #[derive(Clone)]
    struct FakeProgress(Arc<Mutex<State>>);

    struct LockingProgress<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl PeriodicSearchFilterProgress for LockingProgress<'_> {
        async fn lock_and_read(
            &mut self,
            _search_filter_id: search_filter_core::user_search_filter_id::UserSearchFilterId,
            _expected_version: i64,
            _created: OffsetDateTime,
            _window_end: OffsetDateTime,
        ) -> Result<PeriodicSearchFilterProgressLockOutcome, PeriodicSearchFilterProgressError>
        {
            self.0.lock().map(|state| state.lock_outcome).map_err(|_| {
                PeriodicSearchFilterProgressError::PersistenceFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                }
            })
        }

        async fn compare_and_set(
            &mut self,
            _search_filter_id: search_filter_core::user_search_filter_id::UserSearchFilterId,
            _expected_matched_through: OffsetDateTime,
            _matched_through: OffsetDateTime,
        ) -> Result<PeriodicSearchFilterProgressWriteOutcome, PeriodicSearchFilterProgressError>
        {
            let mut state = self.0.lock().map_err(|_| {
                PeriodicSearchFilterProgressError::PersistenceFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                }
            })?;
            state.checkpoints += 1;
            Ok(PeriodicSearchFilterProgressWriteOutcome::Advanced)
        }
    }

    impl PeriodicSearchFilterProgressFactory<FakeTransaction> for FakeProgress {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PeriodicSearchFilterProgress + 'tx {
            LockingProgress(&self.0)
        }
    }

    type Handler = RunPeriodicSearchFilterMatchingHandler<
        FakeUnitOfWork,
        NoopRunLock,
        NoopCandidates,
        NoopFxRates,
        NoopProductListings,
        NoopExistingMatches,
        NoopSources,
        NoopEvaluator,
        FakeCurrentEvents,
        FakeMatches,
        FakeProgress,
    >;

    fn state(lock_outcome: PeriodicSearchFilterProgressLockOutcome) -> Arc<Mutex<State>> {
        Arc::new(Mutex::new(State {
            lock_outcome,
            current_event_check: ProductListingCurrentEventCheck::Current,
            product_listing_search_result: None,
            evaluator_calls: 0,
            match_writer_calls: 0,
            commits: 0,
            event_checks: 0,
            persisted: Vec::new(),
            checkpoints: 0,
        }))
    }

    fn handler(state: Arc<Mutex<State>>) -> Result<Handler, RunPeriodicSearchFilterMatchingError> {
        RunPeriodicSearchFilterMatchingHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            NoopRunLock,
            NoopCandidates,
            NoopFxRates,
            NoopProductListings(Arc::clone(&state)),
            NoopExistingMatches,
            NoopSources,
            NoopEvaluator(Arc::clone(&state)),
            FakeCurrentEvents(Arc::clone(&state)),
            FakeMatches(Arc::clone(&state)),
            FakeProgress(state),
            PeriodicSearchFilterMatchingPolicy {
                filter_page_size: NonZeroUsize::MIN,
                hybrid_scan_limit: NonZeroUsize::MIN,
                evaluation_limit: NonZeroUsize::MIN,
                llm_concurrency: NonZeroUsize::MIN,
                max_attempts: NonZeroUsize::MIN,
                projection_lag: Duration::ZERO,
                replay_overlap: Duration::ZERO,
            },
        )
    }

    fn filter() -> PeriodicSearchFilterCandidate {
        PeriodicSearchFilterCandidate {
            search_filter_id: search_filter_core::user_search_filter_id::UserSearchFilterId::new(),
            user_id: UserId::new(),
            name: UserSearchFilterName::from("daily"),
            version: 4,
            state: SearchFilterState::Active,
            search: ProductListingSearch::new(Language::En, Currency::Eur),
            embedding: None,
            created: OffsetDateTime::UNIX_EPOCH,
            matched_through: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn withdrawn_search_result()
    -> Result<ProductListingSearchReadResult, Box<dyn std::error::Error>> {
        let mut result = ProductListingSearchReadResult::default();
        result.items.push(ProductListingSearchItem {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("withdrawn-a1b2c3")?,
            event_id: EventId::new(),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("withdrawn-1")?,
            auction_id: None,
            has_auction_context: false,
            title: None,
            display_price: None,
            price_valuation: ProductListingSummaryPriceValuation::Current {
                fx_rate_id: FxRateId::new(),
                captured_at: OffsetDateTime::UNIX_EPOCH,
            },
            availability: None,
            lifecycle: ListingLifecycle::Withdrawn,
            url: Url::parse("https://example.test/withdrawn")?,
            images: IndexSet::new(),
            updated: OffsetDateTime::UNIX_EPOCH,
        });
        Ok(result)
    }

    #[tokio::test]
    async fn should_skip_current_withdrawn_candidate_evaluation_and_match_write()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .product_listing_search_result = Some(withdrawn_search_result()?);
        let handler = handler(Arc::clone(&state))?;
        let mut filter = filter();
        filter.embedding = Some(vec![0.1]);
        filter.search.enhanced_search_description = Some(
            product_listing_core::product_listing_search::EnhancedSearchDescription::try_from(
                "withdrawn listing",
            )?,
        );
        let mut attempt_report = FilterAttemptReport::default();

        let outcome = handler
            .process_filter(
                &filter,
                &test_snapshot(),
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(FilterOutcome::Completed, outcome);
        assert_eq!(1, attempt_report.candidates_scanned);
        assert_eq!(1, attempt_report.candidates_withdrawn);
        assert_eq!(0, state.evaluator_calls);
        assert_eq!(0, state.match_writer_calls);
        assert!(state.persisted.is_empty());
        assert_eq!(1, state.checkpoints);
        assert_eq!(1, state.commits);
        Ok(())
    }

    #[test]
    fn should_clamp_first_replay_window_to_filter_creation() {
        let mut candidate = filter();
        candidate.created = OffsetDateTime::UNIX_EPOCH + Duration::hours(10);
        candidate.matched_through = candidate.created;
        let window_end = candidate.created + Duration::hours(1);

        let search = periodic_search(&candidate, window_end, Duration::hours(2));

        assert_eq!(
            Some(RangeQuery {
                min: Some(candidate.created),
                max: Some(window_end),
            }),
            search.and_then(|search| search.updated_query)
        );
    }

    #[test]
    fn should_intersect_replay_window_with_persisted_updated_bounds() {
        let mut candidate = filter();
        candidate.created = OffsetDateTime::UNIX_EPOCH;
        candidate.matched_through = candidate.created + Duration::hours(10);
        candidate.search.updated_query = Some(RangeQuery {
            min: Some(candidate.created + Duration::hours(9)),
            max: Some(candidate.created + Duration::hours(11)),
        });
        let window_end = candidate.created + Duration::hours(12);

        let search = periodic_search(&candidate, window_end, Duration::hours(2));

        assert_eq!(
            Some(RangeQuery {
                min: Some(candidate.created + Duration::hours(9)),
                max: Some(candidate.created + Duration::hours(11)),
            }),
            search.and_then(|search| search.updated_query)
        );
    }

    fn test_snapshot() -> FxRateSnapshot {
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
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
        )
        .unwrap_or_else(|error| panic!("test snapshot invalid: {error}"))
        .into_persisted(
            FxRateGeneration::try_from(1)
                .unwrap_or_else(|error| panic!("test generation invalid: {error}")),
        )
    }

    fn accepted_match(filter: &PeriodicSearchFilterCandidate) -> SearchFilterProductListingMatch {
        SearchFilterProductListingMatch {
            user_id: filter.user_id,
            user_search_filter_id: filter.search_filter_id,
            user_search_filter_name: Some(filter.name.clone()),
            product_listing_id: ProductListingId::new(),
            origin_event_id: EventId::new(),
            price_match_valuation: None,
            enhanced_match_reason: None,
            feedback: None,
        }
    }

    fn report() -> PeriodicSearchFilterMatchingReport {
        PeriodicSearchFilterMatchingReport {
            window_end: OffsetDateTime::UNIX_EPOCH,
            filters_selected: 0,
            filters_completed: 0,
            filters_already_covered: 0,
            filters_changed_or_inactive: 0,
            filters_progress_superseded: 0,
            filter_attempts: 0,
            filters_retried: 0,
            filters_failed: 0,
            filters_invalid_persisted_state: 0,
            candidates_scanned: 0,
            candidates_existing: 0,
            candidates_missing_source: 0,
            candidates_stale: 0,
            candidates_withdrawn: 0,
            candidates_rejected: 0,
            permanent_evaluation_failures: 0,
            retryable_evaluation_failures: 0,
            matches_inserted: 0,
            matches_duplicate: 0,
        }
    }

    #[test]
    fn should_report_missing_embedding_without_advancing_progress()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        let filter = filter();
        let mut report = report();

        assert_eq!(
            filter_embedding(&filter),
            Err(FilterOutcome::InvalidPersistedState)
        );
        mark_invalid_persisted_state(&mut report);

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(report.filters_failed, 1);
        assert_eq!(report.filters_invalid_persisted_state, 1);
        assert_eq!(state.checkpoints, 0);
        assert_eq!(state.commits, 0);
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_all_filter_work_when_window_is_already_covered()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        let handler = handler(Arc::clone(&state))?;
        let filter = filter();
        let snapshot = test_snapshot();
        let mut attempt_report = FilterAttemptReport::default();

        let equal = handler
            .process_filter(
                &filter,
                &snapshot,
                filter.matched_through,
                &mut attempt_report,
            )
            .await?;
        let older = handler
            .process_filter(
                &filter,
                &snapshot,
                filter.matched_through - Duration::seconds(1),
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(FilterOutcome::AlreadyCovered, equal);
        assert_eq!(FilterOutcome::AlreadyCovered, older);
        assert_eq!(0, state.commits);
        assert_eq!(0, state.event_checks);
        assert_eq!(0, state.checkpoints);
        assert_eq!(0, attempt_report.candidates_scanned);
        Ok(())
    }

    #[tokio::test]
    async fn should_report_progress_superseded_when_selected_checkpoint_changes()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
        });
        let handler = handler(Arc::clone(&state))?;
        let filter = filter();
        let mut attempt_report = FilterAttemptReport::default();

        let outcome = handler
            .commit_filter(
                &filter,
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(2),
                vec![accepted_match(&filter)],
                true,
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(FilterOutcome::ProgressSuperseded, outcome);
        assert_eq!(0, state.event_checks);
        assert!(state.persisted.is_empty());
        assert_eq!(0, state.checkpoints);
        assert_eq!(0, state.commits);
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_revision_match_and_checkpoint_when_filter_changed_or_inactive()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::ChangedOrInactive);
        let handler = handler(Arc::clone(&state))?;
        let filter = filter();
        let mut attempt_report = FilterAttemptReport::default();

        let outcome = handler
            .commit_filter(
                &filter,
                OffsetDateTime::UNIX_EPOCH,
                vec![accepted_match(&filter)],
                true,
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(outcome, FilterOutcome::ChangedOrInactive);
        assert_eq!(state.event_checks, 0);
        assert!(state.persisted.is_empty());
        assert_eq!(state.checkpoints, 0);
        assert_eq!(state.commits, 0);
        assert_eq!(attempt_report.matches_inserted, 0);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_checkpoint_when_accepted_work_is_retryable()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        let handler = handler(Arc::clone(&state))?;
        let filter = filter();
        let accepted = accepted_match(&filter);
        let mut attempt_report = FilterAttemptReport::default();

        let outcome = handler
            .commit_filter(
                &filter,
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
                vec![accepted.clone()],
                false,
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(outcome, FilterOutcome::Completed);
        assert_eq!(state.event_checks, 1);
        assert_eq!(state.persisted, vec![accepted]);
        assert_eq!(state.checkpoints, 0);
        assert_eq!(state.commits, 1);
        assert_eq!(attempt_report.matches_inserted, 1);
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_stale_current_product_listing_match_and_advance_progress()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?
            .current_event_check = ProductListingCurrentEventCheck::Stale;
        let handler = handler(Arc::clone(&state))?;
        let filter = filter();
        let mut attempt_report = FilterAttemptReport::default();

        let outcome = handler
            .commit_filter(
                &filter,
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
                vec![accepted_match(&filter)],
                true,
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(FilterOutcome::Completed, outcome);
        assert_eq!(1, state.event_checks);
        assert!(state.persisted.is_empty());
        assert_eq!(1, state.checkpoints);
        assert_eq!(1, state.commits);
        assert_eq!(0, attempt_report.candidates_stale);
        assert_eq!(0, attempt_report.matches_inserted);
        assert_eq!(0, attempt_report.matches_duplicate);
        Ok(())
    }

    #[tokio::test]
    async fn should_persist_accepted_work_only_after_current_filter_revalidation()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        let handler = handler(Arc::clone(&state))?;
        let filter = filter();
        let accepted = accepted_match(&filter);
        let mut attempt_report = FilterAttemptReport::default();

        let outcome = handler
            .commit_filter(
                &filter,
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
                vec![accepted.clone()],
                true,
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(outcome, FilterOutcome::Completed);
        assert_eq!(state.event_checks, 1);
        assert_eq!(state.persisted, vec![accepted]);
        assert_eq!(state.checkpoints, 1);
        assert_eq!(state.commits, 1);
        assert_eq!(attempt_report.matches_inserted, 1);
        Ok(())
    }
}
