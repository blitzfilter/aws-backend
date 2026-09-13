use crate::ports::{
    PendingProductListingRawStreamCursor, PendingProductListingRawStreamPageRequest,
    PendingProductListingRawStreamReader, ProductListingRawNormalizationCompletion,
    ProductListingRawNormalizationHead, ProductListingRawNormalizationOutcome,
    ProductListingRawNormalizationPortError, ProductListingRawNormalizationWriter,
    ProductListingRawNormalizationWriterFactory, ProductListingRawRevisionReader,
};
use application::patch_field::PatchField;
use application::transaction::{Transaction, TransactionError, UnitOfWork};
use auction_service::ports::{
    AuctionEventAppenderFactory, AuctionMetadataPolicyRepositoryFactory, AuctionRepositoryFactory,
};
use domain_primitives::change_outcome::ChangeOutcome;
use indexmap::IndexSet;
use product_listing_normalization::error::NormalizationFailureScope;
use product_listing_normalization::{
    ListingAvailabilityQuickCheck, PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
    ProductListingRawValuesAuctionMetadataResolved, ProductListingRawValuesNormalizationDiagnostic,
    ProductListingRawValuesNormalizationError, ProductListingRawValuesNormalizationOutcome,
    ProductListingRawValuesNormalizer, ProductListingRawValuesPatch,
    ProductListingRawValuesResolved,
};
use product_listing_service::canonical_product_listing_write::{
    CanonicalProductListingUpsert, CanonicalProductListingWriteError,
    CanonicalProductListingWriter, CanonicalProductListingWriterDependencies,
};
use product_listing_service::ports::{
    ProductListingAuctionOverrideRepositoryFactory, ProductListingEventAppenderFactory,
    ProductListingRawRevisionId, ProductListingRawStreamId, ProductListingRepositoryFactory,
};
use std::time::Instant;
use time::OffsetDateTime;

pub const NORMALIZER_VERSION: u16 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizeProductListingRawRevisionMode {
    /// CDC wake-up metadata. The handler drains from the stream head and never trusts delivery order.
    RawRevision {
        product_listing_raw_stream_id: ProductListingRawStreamId,
        product_listing_raw_revision_id: ProductListingRawRevisionId,
        revision: u64,
    },
    /// Starts a bounded reconciliation traversal from its oldest pending stream.
    Reconcile,
    /// Continues a bounded reconciliation traversal from a result cursor. A result without a
    /// cursor has reached the end and the next run should use `Reconcile` to wrap safely.
    ReconcileFromCursor {
        pending_stream_cursor: PendingProductListingRawStreamCursor,
    },
    /// Drains one worker-local recovery continuation without reading or moving the page cursor.
    ReconcileContinuation {
        product_listing_raw_stream_id: ProductListingRawStreamId,
    },
}

/// A wake-up is scoped to one stream; reconciliation uses the same handler and drains streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizeProductListingRawRevisionCommand {
    pub mode: NormalizeProductListingRawRevisionMode,
    pub max_revisions_per_stream: u32,
    pub pending_stream_limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedRawRevisionResult {
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
    pub revision: u64,
    pub outcome: ProductListingRawNormalizationOutcome,
}

/// Safe metadata for a reconciliation stream that remains pending after a retryable failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingRawNormalizationStreamFailure {
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
    pub error_code: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NormalizeProductListingRawRevisionResult {
    pub revisions: Vec<NormalizedRawRevisionResult>,
    /// Per-stream reconciliation failures. Source errors remain internal to the handler.
    pub stream_failures: Vec<ProductListingRawNormalizationStreamFailure>,
    /// Present for bounded reconciliation only; it is the scanned page, not an unbounded count.
    pub pending_stream_page_count: Option<usize>,
    /// Age of the oldest stream in the bounded reconciliation page.
    pub oldest_pending_age_seconds: Option<u64>,
    /// Non-durable cursor for the next reconciliation page. `None` safely resets the traversal.
    pub next_pending_stream_cursor: Option<PendingProductListingRawStreamCursor>,
    /// Streams that need a later worker-local recovery turn after a clean capped drain.
    pub continuation_stream_ids: Vec<ProductListingRawStreamId>,
}

#[derive(Debug, thiserror::Error)]
pub enum NormalizeProductListingRawRevisionError {
    #[error("normalization work limit must be greater than zero")]
    InvalidLimit,
    #[error("failed to read pending raw product listing streams")]
    PendingStreamReadFailed {
        #[source]
        source: ProductListingRawNormalizationPortError,
    },
    #[error("failed to begin raw product listing normalization transaction")]
    BeginTransactionFailed {
        #[source]
        source: TransactionError,
    },
    #[error("raw product listing normalization storage failed")]
    PersistenceFailed {
        #[source]
        source: ProductListingRawNormalizationPortError,
    },
    #[error("raw product listing normalization stored state is invalid")]
    InvalidPersistedState {
        #[source]
        source: ProductListingRawNormalizationPortError,
    },
    #[error("raw product listing schema version is unsupported")]
    UnsupportedStoredSchemaVersion,
    #[error("raw product listing normalizer configuration is invalid")]
    NormalizationConfigurationFailed {
        #[source]
        source: ProductListingRawValuesNormalizationError,
    },
    #[error("raw product listing normalization failed")]
    CanonicalWriteFailed {
        #[source]
        source: CanonicalProductListingWriteError,
    },
    #[error("failed to commit raw product listing normalization transaction")]
    CommitTransactionFailed {
        #[source]
        source: TransactionError,
    },
}

#[derive(Debug)]
struct StreamDrainResult {
    revisions: Vec<NormalizedRawRevisionResult>,
    error: Option<NormalizeProductListingRawRevisionError>,
}

impl StreamDrainResult {
    fn completed(revisions: Vec<NormalizedRawRevisionResult>) -> Self {
        Self {
            revisions,
            error: None,
        }
    }

    fn failed(
        revisions: Vec<NormalizedRawRevisionResult>,
        error: NormalizeProductListingRawRevisionError,
    ) -> Self {
        Self {
            revisions,
            error: Some(error),
        }
    }

    fn requires_continuation(&self, max_revisions: u32) -> bool {
        self.error.is_none() && u32::try_from(self.revisions.len()) == Ok(max_revisions)
    }
}

#[async_trait::async_trait]
pub trait NormalizeProductListingRawRevisionUseCase: Send + Sync {
    async fn execute(
        &self,
        command: NormalizeProductListingRawRevisionCommand,
    ) -> Result<NormalizeProductListingRawRevisionResult, NormalizeProductListingRawRevisionError>;
}

pub struct NormalizeProductListingRawRevisionHandler<U, W, R, E, AR, AE, AP, AO, P> {
    unit_of_work: U,
    raw_normalizations: W,
    products: R,
    events: E,
    auctions: AR,
    auction_events: AE,
    auction_policies: AP,
    auction_overrides: AO,
    pending_streams: P,
    normalizer: ProductListingRawValuesNormalizer,
}

impl<U, W, R, E, AR, AE, AP, AO, P>
    NormalizeProductListingRawRevisionHandler<U, W, R, E, AR, AE, AP, AO, P>
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        unit_of_work: U,
        raw_normalizations: W,
        products: R,
        events: E,
        auctions: AR,
        auction_events: AE,
        auction_policies: AP,
        auction_overrides: AO,
        pending_streams: P,
    ) -> Self {
        Self {
            unit_of_work,
            raw_normalizations,
            products,
            events,
            auctions,
            auction_events,
            auction_policies,
            auction_overrides,
            pending_streams,
            normalizer: ProductListingRawValuesNormalizer::new(),
        }
    }
}

impl<U, W, R, E, AR, AE, AP, AO, P>
    NormalizeProductListingRawRevisionHandler<U, W, R, E, AR, AE, AP, AO, P>
where
    U: UnitOfWork,
    W: ProductListingRawNormalizationWriterFactory<U::Tx>,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    AR: AuctionRepositoryFactory<U::Tx>,
    AE: AuctionEventAppenderFactory<U::Tx>,
    AP: AuctionMetadataPolicyRepositoryFactory<U::Tx>,
    AO: ProductListingAuctionOverrideRepositoryFactory<U::Tx>,
    P: PendingProductListingRawStreamReader + ProductListingRawRevisionReader,
{
    async fn drain_stream(
        &self,
        product_listing_raw_stream_id: ProductListingRawStreamId,
        max_revisions: u32,
    ) -> StreamDrainResult {
        let mut results = Vec::new();
        for _ in 0..max_revisions {
            let candidate = match self
                .pending_streams
                .find_next_revision(product_listing_raw_stream_id)
                .await
            {
                Ok(candidate) => candidate,
                Err(error) => return StreamDrainResult::failed(results, map_port_error(error)),
            };
            let Some(candidate) = candidate else {
                return StreamDrainResult::completed(results);
            };
            if let Err(error) = validate_stored_schema(&candidate.input) {
                return StreamDrainResult::failed(results, error);
            }
            let normalized = match require_terminal_normalization_outcome(
                self.normalizer.normalize(&candidate.input),
            ) {
                Ok(normalized) => normalized,
                Err(error) => return StreamDrainResult::failed(results, error),
            };
            let mut tx = match self.unit_of_work.begin().await {
                Ok(tx) => tx,
                Err(source) => {
                    return StreamDrainResult::failed(
                        results,
                        NormalizeProductListingRawRevisionError::BeginTransactionFailed { source },
                    );
                }
            };
            let work = match self
                .raw_normalizations
                .in_transaction(&mut tx)
                .lock_next(product_listing_raw_stream_id)
                .await
            {
                Ok(work) => work,
                Err(error) => return StreamDrainResult::failed(results, map_port_error(error)),
            };
            let Some(revision) = work.next_revision else {
                return StreamDrainResult::completed(results);
            };
            if revision.product_listing_raw_revision_id != candidate.product_listing_raw_revision_id
                || revision.revision != candidate.revision
            {
                continue;
            }
            let completion = match self
                .complete_work_in_transaction(&mut tx, work.head, revision, &normalized)
                .await
            {
                Ok(completion) => completion,
                Err(error) => return StreamDrainResult::failed(results, error),
            };
            let result = NormalizedRawRevisionResult {
                product_listing_raw_stream_id,
                revision: completion.revision,
                outcome: completion.outcome,
            };
            if let Err(error) = self
                .raw_normalizations
                .in_transaction(&mut tx)
                .complete(completion)
                .await
            {
                return StreamDrainResult::failed(results, map_port_error(error));
            }
            if let Err(source) = tx.commit().await {
                return StreamDrainResult::failed(
                    results,
                    NormalizeProductListingRawRevisionError::CommitTransactionFailed { source },
                );
            }
            results.push(result);
        }
        StreamDrainResult::completed(results)
    }

    async fn complete_work_in_transaction(
        &self,
        tx: &mut U::Tx,
        head: ProductListingRawNormalizationHead,
        revision: crate::ports::ProductListingRawRevision,
        normalized: &ProductListingRawValuesNormalizationOutcome,
    ) -> Result<ProductListingRawNormalizationCompletion, NormalizeProductListingRawRevisionError>
    {
        match normalized {
            ProductListingRawValuesNormalizationOutcome::Invalid(error) => Ok(completion(
                &head,
                &revision,
                ProductListingRawNormalizationOutcome::Rejected,
                None,
                None,
                Some(normalization_error_code(error)),
            )),
            ProductListingRawValuesNormalizationOutcome::Delete => {
                let Some(product_listing_id) = head.product_listing_id else {
                    return Ok(completion(
                        &head,
                        &revision,
                        ProductListingRawNormalizationOutcome::Ignored,
                        None,
                        None,
                        None,
                    ));
                };
                let write = match CanonicalProductListingWriter::withdraw_in_transaction(
                    tx,
                    &self.products,
                    &self.events,
                    product_listing_id,
                )
                .await
                {
                    Ok(write) => write,
                    Err(CanonicalProductListingWriteError::InvalidInput { .. }) => {
                        return Ok(completion(
                            &head,
                            &revision,
                            ProductListingRawNormalizationOutcome::Rejected,
                            None,
                            None,
                            Some("CANONICAL_PRODUCT_LISTING_INVALID"),
                        ));
                    }
                    Err(source) => {
                        return Err(
                            NormalizeProductListingRawRevisionError::CanonicalWriteFailed {
                                source,
                            },
                        );
                    }
                };
                Ok(completion(
                    &head,
                    &revision,
                    if write.outcome == ChangeOutcome::Changed {
                        ProductListingRawNormalizationOutcome::Applied
                    } else {
                        ProductListingRawNormalizationOutcome::NoChange
                    },
                    Some(write.product_listing_id),
                    write.product_listing_event_id,
                    None,
                ))
            }
            ProductListingRawValuesNormalizationOutcome::Resolved(resolved) => {
                if let Some(bound_source_listing_id) = &head.source_listing_id
                    && bound_source_listing_id != &resolved.source_listing_id
                {
                    return Ok(completion(
                        &head,
                        &revision,
                        ProductListingRawNormalizationOutcome::Rejected,
                        None,
                        None,
                        Some("SOURCE_LISTING_ID_MISMATCH"),
                    ));
                }
                let mut command = canonical_upsert(head.listing_source_id, resolved.as_ref());
                command.raw_auction_capture_generation = Some(revision.capture_generation);
                let write = match CanonicalProductListingWriter::upsert_in_transaction(
                    tx,
                    CanonicalProductListingWriterDependencies {
                        products: &self.products,
                        events: &self.events,
                        auctions: &self.auctions,
                        auction_events: &self.auction_events,
                        auction_policies: &self.auction_policies,
                        auction_overrides: &self.auction_overrides,
                    },
                    head.product_listing_id,
                    command,
                )
                .await
                {
                    Ok(write) => write,
                    Err(CanonicalProductListingWriteError::InvalidInput { .. }) => {
                        return Ok(completion(
                            &head,
                            &revision,
                            ProductListingRawNormalizationOutcome::Rejected,
                            None,
                            None,
                            Some("CANONICAL_PRODUCT_LISTING_INVALID"),
                        ));
                    }
                    Err(source) => {
                        return Err(
                            NormalizeProductListingRawRevisionError::CanonicalWriteFailed {
                                source,
                            },
                        );
                    }
                };
                let mut completion = completion(
                    &head,
                    &revision,
                    if write.outcome == ChangeOutcome::Changed {
                        ProductListingRawNormalizationOutcome::Applied
                    } else {
                        ProductListingRawNormalizationOutcome::NoChange
                    },
                    Some(write.product_listing_id),
                    write.product_listing_event_id,
                    if write.auction_context_override_preserved {
                        Some("MANUAL_AUCTION_OVERRIDE_PRESERVED")
                    } else {
                        resolved
                            .diagnostic
                            .map(ProductListingRawValuesNormalizationDiagnostic::as_str)
                    },
                );
                completion.next_product_listing_id = Some(write.product_listing_id);
                completion.next_source_listing_id = Some(resolved.source_listing_id.clone());
                Ok(completion)
            }
        }
    }
}

impl<U, W, R, E, AR, AE, AP, AO, P>
    NormalizeProductListingRawRevisionHandler<U, W, R, E, AR, AE, AP, AO, P>
where
    U: UnitOfWork,
    W: ProductListingRawNormalizationWriterFactory<U::Tx>,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    AR: AuctionRepositoryFactory<U::Tx>,
    AE: AuctionEventAppenderFactory<U::Tx>,
    AP: AuctionMetadataPolicyRepositoryFactory<U::Tx>,
    AO: ProductListingAuctionOverrideRepositoryFactory<U::Tx>,
    P: PendingProductListingRawStreamReader + ProductListingRawRevisionReader,
{
    async fn execute_inner(
        &self,
        command: NormalizeProductListingRawRevisionCommand,
    ) -> Result<NormalizeProductListingRawRevisionResult, NormalizeProductListingRawRevisionError>
    {
        if command.max_revisions_per_stream == 0 || command.pending_stream_limit == 0 {
            return Err(NormalizeProductListingRawRevisionError::InvalidLimit);
        }
        let max_revisions_per_stream = command.max_revisions_per_stream;
        let pending_stream_limit = command.pending_stream_limit;
        let cursor = match command.mode {
            NormalizeProductListingRawRevisionMode::RawRevision {
                product_listing_raw_stream_id,
                product_listing_raw_revision_id: _,
                revision: _,
            } => {
                let StreamDrainResult { revisions, error } = self
                    .drain_stream(product_listing_raw_stream_id, max_revisions_per_stream)
                    .await;
                if let Some(error) = error {
                    return Err(error);
                }
                return Ok(NormalizeProductListingRawRevisionResult {
                    revisions,
                    stream_failures: Vec::new(),
                    pending_stream_page_count: None,
                    oldest_pending_age_seconds: None,
                    next_pending_stream_cursor: None,
                    continuation_stream_ids: Vec::new(),
                });
            }
            NormalizeProductListingRawRevisionMode::ReconcileContinuation {
                product_listing_raw_stream_id,
            } => {
                let drain = self
                    .drain_stream(product_listing_raw_stream_id, max_revisions_per_stream)
                    .await;
                let requires_continuation = drain.requires_continuation(max_revisions_per_stream);
                let StreamDrainResult { revisions, error } = drain;
                let stream_failures = error
                    .into_iter()
                    .map(|error| ProductListingRawNormalizationStreamFailure {
                        product_listing_raw_stream_id,
                        error_code: normalization_failure_code(&error),
                    })
                    .collect();
                return Ok(NormalizeProductListingRawRevisionResult {
                    revisions,
                    stream_failures,
                    pending_stream_page_count: None,
                    oldest_pending_age_seconds: None,
                    next_pending_stream_cursor: None,
                    continuation_stream_ids: if requires_continuation {
                        vec![product_listing_raw_stream_id]
                    } else {
                        Vec::new()
                    },
                });
            }
            NormalizeProductListingRawRevisionMode::Reconcile => None,
            NormalizeProductListingRawRevisionMode::ReconcileFromCursor {
                pending_stream_cursor,
            } => Some(pending_stream_cursor),
        };
        let pending_page =
            self.pending_streams
                .list_pending_stream_page(PendingProductListingRawStreamPageRequest {
                    limit: pending_stream_limit,
                    cursor,
                })
                .await
                .map_err(|source| {
                    NormalizeProductListingRawRevisionError::PendingStreamReadFailed { source }
                })?;
        let oldest_pending_age_seconds = pending_page
            .streams
            .iter()
            .map(|stream| stream.oldest_pending_at)
            .min()
            .and_then(pending_age_seconds);
        let mut result = NormalizeProductListingRawRevisionResult {
            revisions: Vec::new(),
            stream_failures: Vec::new(),
            pending_stream_page_count: Some(pending_page.streams.len()),
            oldest_pending_age_seconds,
            next_pending_stream_cursor: pending_page.next_cursor,
            continuation_stream_ids: Vec::new(),
        };
        for stream in pending_page.streams {
            let drain = self
                .drain_stream(
                    stream.product_listing_raw_stream_id,
                    max_revisions_per_stream,
                )
                .await;
            let requires_continuation = drain.requires_continuation(max_revisions_per_stream);
            let StreamDrainResult { revisions, error } = drain;
            result.revisions.extend(revisions);
            if requires_continuation {
                result
                    .continuation_stream_ids
                    .push(stream.product_listing_raw_stream_id);
            }
            if let Some(error) = error {
                result
                    .stream_failures
                    .push(ProductListingRawNormalizationStreamFailure {
                        product_listing_raw_stream_id: stream.product_listing_raw_stream_id,
                        error_code: normalization_failure_code(&error),
                    });
            }
        }
        Ok(result)
    }
}

#[async_trait::async_trait]
impl<U, W, R, E, AR, AE, AP, AO, P> NormalizeProductListingRawRevisionUseCase
    for NormalizeProductListingRawRevisionHandler<U, W, R, E, AR, AE, AP, AO, P>
where
    U: UnitOfWork + Send + Sync,
    W: ProductListingRawNormalizationWriterFactory<U::Tx> + Send + Sync,
    R: ProductListingRepositoryFactory<U::Tx> + Send + Sync,
    E: ProductListingEventAppenderFactory<U::Tx> + Send + Sync,
    AR: AuctionRepositoryFactory<U::Tx> + Send + Sync,
    AE: AuctionEventAppenderFactory<U::Tx> + Send + Sync,
    AP: AuctionMetadataPolicyRepositoryFactory<U::Tx> + Send + Sync,
    AO: ProductListingAuctionOverrideRepositoryFactory<U::Tx> + Send + Sync,
    P: PendingProductListingRawStreamReader + ProductListingRawRevisionReader + Send + Sync,
{
    #[tracing::instrument(name = "normalize_product_listing_raw_revision", skip_all)]
    async fn execute(
        &self,
        command: NormalizeProductListingRawRevisionCommand,
    ) -> Result<NormalizeProductListingRawRevisionResult, NormalizeProductListingRawRevisionError>
    {
        let started = Instant::now();
        let result = self.execute_inner(command).await;

        match &result {
            Ok(result) => {
                for failure in &result.stream_failures {
                    tracing::warn!(
                        metric = "product_listing_raw_normalization",
                        normalization_revisions = 0_u64,
                        normalization_failures = 1_u64,
                        normalization_batch_latency_ms = started.elapsed().as_millis() as u64,
                        product_listing_raw_stream_id = %failure.product_listing_raw_stream_id,
                        outcome = "stream_failure",
                        error_code = failure.error_code,
                        "raw product listing reconciliation stream failed"
                    );
                }
                for revision in &result.revisions {
                    tracing::info!(
                        metric = "product_listing_raw_normalization",
                        normalization_revisions = 1_u64,
                        normalization_failures = 0_u64,
                        normalization_batch_latency_ms = started.elapsed().as_millis() as u64,
                        product_listing_raw_stream_id = %revision.product_listing_raw_stream_id,
                        revision = revision.revision,
                        outcome = revision.outcome.as_str(),
                        "raw product listing normalization metric"
                    );
                }
                if let Some(pending_stream_page_count) = result.pending_stream_page_count {
                    tracing::info!(
                        metric = "product_listing_raw_normalization_backlog",
                        pending_stream_page_count,
                        oldest_pending_age_seconds = result.oldest_pending_age_seconds,
                        reconciliation_continuation_stream_count =
                            result.continuation_stream_ids.len(),
                        reconciliation_runs = 1_u64,
                        "raw product listing normalization backlog metric"
                    );
                }
            }
            Err(error) => tracing::warn!(
                metric = "product_listing_raw_normalization",
                normalization_revisions = 0_u64,
                normalization_failures = 1_u64,
                normalization_batch_latency_ms = started.elapsed().as_millis() as u64,
                outcome = "failure",
                error_code = normalization_failure_code(error),
                "raw product listing normalization metric"
            ),
        }

        result
    }
}

fn pending_age_seconds(oldest_pending_at: OffsetDateTime) -> Option<u64> {
    let age = OffsetDateTime::now_utc() - oldest_pending_at;
    u64::try_from(age.whole_seconds()).ok()
}

fn normalization_failure_code(error: &NormalizeProductListingRawRevisionError) -> &'static str {
    match error {
        NormalizeProductListingRawRevisionError::InvalidLimit => "INVALID_LIMIT",
        NormalizeProductListingRawRevisionError::PendingStreamReadFailed { .. } => {
            "PENDING_STREAM_READ_FAILED"
        }
        NormalizeProductListingRawRevisionError::BeginTransactionFailed { .. } => {
            "BEGIN_TRANSACTION_FAILED"
        }
        NormalizeProductListingRawRevisionError::PersistenceFailed { .. } => "PERSISTENCE_FAILED",
        NormalizeProductListingRawRevisionError::InvalidPersistedState { .. } => {
            "INVALID_PERSISTED_STATE"
        }
        NormalizeProductListingRawRevisionError::UnsupportedStoredSchemaVersion => {
            "UNSUPPORTED_STORED_SCHEMA_VERSION"
        }
        NormalizeProductListingRawRevisionError::NormalizationConfigurationFailed { .. } => {
            "NORMALIZATION_CONFIGURATION_FAILED"
        }
        NormalizeProductListingRawRevisionError::CanonicalWriteFailed { .. } => {
            "CANONICAL_WRITE_FAILED"
        }
        NormalizeProductListingRawRevisionError::CommitTransactionFailed { .. } => {
            "COMMIT_TRANSACTION_FAILED"
        }
    }
}

fn canonical_upsert(
    listing_source_id: listing_source_core::ListingSourceId,
    resolved: &ProductListingRawValuesResolved,
) -> CanonicalProductListingUpsert {
    CanonicalProductListingUpsert {
        listing_source_id,
        source_listing_id: resolved.source_listing_id.clone(),
        title: to_patch(&resolved.title),
        description: to_patch(&resolved.description),
        price: to_patch(&resolved.price),
        price_estimate_min: to_patch(&resolved.price_estimate_min),
        price_estimate_max: to_patch(&resolved.price_estimate_max),
        availability: availability_patch(&resolved.availability),
        url: to_patch(&resolved.url),
        images: match &resolved.images {
            ProductListingRawValuesPatch::Set(images) => {
                PatchField::Set(images.iter().cloned().collect::<IndexSet<_>>())
            }
            ProductListingRawValuesPatch::Clear => PatchField::Clear,
            ProductListingRawValuesPatch::Unchanged => PatchField::Unchanged,
        },
        auction: to_patch(&resolved.auction),
        auction_source_id: resolved.auction_source_id.clone(),
        auction_metadata: auction_metadata(&resolved.auction_metadata),
        raw_auction_capture_generation: None,
    }
}

fn auction_metadata(
    metadata: &ProductListingRawValuesAuctionMetadataResolved,
) -> auction_service::EmbeddedAuctionMetadata {
    auction_service::EmbeddedAuctionMetadata {
        name: metadata.name.clone(),
        description: metadata.description.clone(),
        catalogue_url: metadata.catalogue_url.clone(),
        format: metadata.format,
        reported_status: metadata.reported_status,
        reported_lot_count: metadata.reported_lot_count,
        bidding_opens: metadata.bidding_opens.clone(),
        live_starts: metadata.live_starts.clone(),
        lots_begin_closing: metadata.lots_begin_closing.clone(),
        scheduled_end: metadata.scheduled_end.clone(),
    }
}

fn to_patch<T: Clone>(patch: &ProductListingRawValuesPatch<T>) -> PatchField<T> {
    match patch {
        ProductListingRawValuesPatch::Set(value) => PatchField::Set(value.clone()),
        ProductListingRawValuesPatch::Clear => PatchField::Clear,
        ProductListingRawValuesPatch::Unchanged => PatchField::Unchanged,
    }
}

fn availability_patch(
    patch: &ProductListingRawValuesPatch<ListingAvailabilityQuickCheck>,
) -> PatchField<product_listing_core::listing_availability::ListingAvailability> {
    match patch {
        ProductListingRawValuesPatch::Set(ListingAvailabilityQuickCheck::Resolved(value)) => {
            PatchField::Set(*value)
        }
        ProductListingRawValuesPatch::Set(ListingAvailabilityQuickCheck::NoAssertion)
        | ProductListingRawValuesPatch::Clear => PatchField::Clear,
        ProductListingRawValuesPatch::Set(ListingAvailabilityQuickCheck::Unsupported)
        | ProductListingRawValuesPatch::Unchanged => PatchField::Unchanged,
    }
}

// The generic canonical writer needs the caller transaction. Keep that orchestration beside the
// handler rather than exposing a second inbound normalization use case.
fn validate_stored_schema(
    input: &product_listing_normalization::ProductListingNormalizationInput,
) -> Result<(), NormalizeProductListingRawRevisionError> {
    if input.payload_schema_version() != 1
        || input.raw_values_schema_version() != PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION
    {
        return Err(NormalizeProductListingRawRevisionError::UnsupportedStoredSchemaVersion);
    }
    Ok(())
}

fn completion(
    head: &ProductListingRawNormalizationHead,
    revision: &crate::ports::ProductListingRawRevision,
    outcome: ProductListingRawNormalizationOutcome,
    product_listing_id: Option<product_listing_core::product_listing_id::ProductListingId>,
    product_listing_event_id: Option<domain_primitives::event_id::EventId>,
    error_code: Option<&'static str>,
) -> ProductListingRawNormalizationCompletion {
    ProductListingRawNormalizationCompletion {
        product_listing_raw_revision_id: revision.product_listing_raw_revision_id,
        product_listing_raw_stream_id: revision.product_listing_raw_stream_id,
        revision: revision.revision,
        normalizer_version: NORMALIZER_VERSION,
        outcome,
        product_listing_id,
        product_listing_event_id,
        error_code,
        next_product_listing_id: head.product_listing_id,
        next_source_listing_id: head.source_listing_id.clone(),
    }
}

fn require_terminal_normalization_outcome(
    outcome: ProductListingRawValuesNormalizationOutcome,
) -> Result<ProductListingRawValuesNormalizationOutcome, NormalizeProductListingRawRevisionError> {
    match outcome {
        ProductListingRawValuesNormalizationOutcome::Invalid(error)
            if error.failure_scope() == NormalizationFailureScope::System =>
        {
            Err(
                NormalizeProductListingRawRevisionError::NormalizationConfigurationFailed {
                    source: error,
                },
            )
        }
        outcome => Ok(outcome),
    }
}

fn normalization_error_code(error: &ProductListingRawValuesNormalizationError) -> &'static str {
    match error {
        ProductListingRawValuesNormalizationError::InvalidRawValues(_) => "RAW_VALUES_INVALID",
        ProductListingRawValuesNormalizationError::InvalidNormalizationContextV1(_) => {
            "NORMALIZATION_CONTEXT_INVALID"
        }
        ProductListingRawValuesNormalizationError::InvalidBaseUrl(_) => {
            "NORMALIZATION_BASE_URL_INVALID"
        }
        ProductListingRawValuesNormalizationError::InvalidUrl(_) => "LISTING_URL_INVALID",
        ProductListingRawValuesNormalizationError::MachineDecimalFallbackCurrencyRequired => {
            "MACHINE_DECIMAL_FALLBACK_CURRENCY_REQUIRED"
        }
        ProductListingRawValuesNormalizationError::UnsupportedFallbackCurrency => {
            "FALLBACK_CURRENCY_UNSUPPORTED"
        }
        ProductListingRawValuesNormalizationError::UnsupportedFallbackLanguage => {
            "FALLBACK_LANGUAGE_UNSUPPORTED"
        }
        ProductListingRawValuesNormalizationError::Text(_) => "TEXT_NORMALIZATION_INVALID",
        ProductListingRawValuesNormalizationError::Price(_) => "PRICE_NORMALIZATION_INVALID",
        ProductListingRawValuesNormalizationError::ImageUrl(_) => "IMAGE_URL_NORMALIZATION_INVALID",
        ProductListingRawValuesNormalizationError::Auction(_) => "AUCTION_TIMING_INVALID",
        ProductListingRawValuesNormalizationError::Availability(_) => {
            "AVAILABILITY_NORMALIZATION_INVALID"
        }
        ProductListingRawValuesNormalizationError::UnsupportedRawValuesSchemaVersion { .. } => {
            "RAW_VALUES_SCHEMA_UNSUPPORTED"
        }
    }
}

fn map_port_error(
    error: ProductListingRawNormalizationPortError,
) -> NormalizeProductListingRawRevisionError {
    match error {
        error @ ProductListingRawNormalizationPortError::Persistence { .. } => {
            NormalizeProductListingRawRevisionError::PersistenceFailed { source: error }
        }
        error @ ProductListingRawNormalizationPortError::InvalidPersistedState { .. } => {
            NormalizeProductListingRawRevisionError::InvalidPersistedState { source: error }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::transaction::TransactionError;
    use auction_service::ports::{
        AuctionEvent, AuctionEventAppendError, AuctionEventAppender, AuctionEventAppenderFactory,
        AuctionMetadataField, AuctionMetadataPolicyAudit, AuctionMetadataPolicyRepository,
        AuctionMetadataPolicyRepositoryError, AuctionMetadataPolicyRepositoryFactory,
        AuctionRepository, AuctionRepositoryError, AuctionRepositoryFactory, StoredAuction,
    };
    use listing_source_core::ListingSourceId;
    use product_listing_core::product_listing_id::ProductListingId;
    use product_listing_normalization::{
        NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
        RawProductListingPayloadFormat, RawProductListingValues, SourcePayload,
    };
    use product_listing_service::ports::product_listing_event_appender::ProductListingEvent;
    use product_listing_service::ports::{
        ProductListingAuctionOverride, ProductListingAuctionOverrideAudit,
        ProductListingAuctionOverrideError, ProductListingAuctionOverrideRepository,
        ProductListingAuctionOverrideRepositoryFactory, ProductListingEventAppendError,
        ProductListingEventAppender, ProductListingRawRevisionId, ProductListingRepository,
        ProductListingRepositoryError,
    };
    use std::sync::{Arc, Mutex};

    struct TestTx(Arc<Mutex<bool>>);
    struct TestUnitOfWork(Arc<Mutex<bool>>);
    struct TestRawFactory(Arc<Mutex<TestRawState>>);
    struct TestRawWriter<'a>(&'a Arc<Mutex<TestRawState>>);
    struct TestRawState {
        work: Option<crate::ports::ProductListingRawNormalizationWork>,
        completions: Vec<ProductListingRawNormalizationCompletion>,
    }
    struct TestProducts;
    struct TestProductRepository;
    struct TestEvents;
    struct TestEventAppender;
    struct TestAuctions;
    struct TestAuctionRepository;
    struct TestAuctionEvents;
    struct TestAuctionEventAppender;
    struct TestAuctionPolicies;
    struct TestAuctionPolicyRepository;
    struct TestAuctionOverrides;
    struct TestAuctionOverrideRepository;
    struct TestRevisionReader(Arc<Mutex<TestRawState>>);
    struct FailingFirstPendingReader {
        state: Arc<Mutex<TestRawState>>,
        blocked_stream_id: ProductListingRawStreamId,
        healthy_stream_id: ProductListingRawStreamId,
        fail_after_healthy_completion: bool,
    }
    struct FailingPendingListReader;
    struct CappedContinuationRawFactory(Arc<Mutex<CappedContinuationState>>);
    struct CappedContinuationRawWriter<'a>(&'a Arc<Mutex<CappedContinuationState>>);
    struct CappedContinuationReader(Arc<Mutex<CappedContinuationState>>);
    struct CappedContinuationState {
        product_listing_raw_stream_id: ProductListingRawStreamId,
        product_listing_raw_revision_id: ProductListingRawRevisionId,
        listing_source_id: ListingSourceId,
        next_revision: u64,
        last_revision: u64,
        input: ProductListingNormalizationInput,
        completions: Vec<ProductListingRawNormalizationCompletion>,
    }

    fn input_with_schema_versions(
        payload_schema_version: u16,
        raw_values_schema_version: u16,
    ) -> Result<
        ProductListingNormalizationInput,
        product_listing_normalization::NormalizationInputError,
    > {
        ProductListingNormalizationInput::new(
            RawProductListingOperation::Delete,
            RawProductListingPayloadFormat::CrawlerExtractedProduct,
            payload_schema_version,
            raw_values_schema_version,
            SourcePayload::new(serde_json::json!({}))?,
            RawProductListingValues::new(serde_json::json!({}))?,
            NormalizationContext::new(serde_json::json!({}))?,
        )
    }

    fn current_upsert_input(
        auction: serde_json::Value,
    ) -> Result<
        ProductListingNormalizationInput,
        product_listing_normalization::NormalizationInputError,
    > {
        ProductListingNormalizationInput::new(
            RawProductListingOperation::Upsert,
            RawProductListingPayloadFormat::CrawlerExtractedProduct,
            1,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            SourcePayload::new(serde_json::json!({}))?,
            RawProductListingValues::new(serde_json::json!({
                "sourceListingId": "listing-123",
                "title": {"action": "SET", "value": "An antique ceramic vase"},
                "description": {"action": "CLEAR"},
                "priceFormat": "DISPLAY_TEXT",
                "price": {"action": "SET", "value": "EUR 100"},
                "priceEstimateMin": {"action": "CLEAR"},
                "priceEstimateMax": {"action": "CLEAR"},
                "availability": {"action": "CLEAR"},
                "url": {"action": "SET", "value": "listing/123"},
                "images": {"action": "CLEAR"},
                "auction": auction
            }))?,
            NormalizationContext::new(serde_json::json!({
                "baseUrl": "https://example.test/catalogue/",
                "fallbackCurrency": "EUR",
                "fallbackLanguage": "en"
            }))?,
        )
    }

    #[async_trait::async_trait]
    impl Transaction for TestTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut committed = self.0.lock().map_err(|_| TransactionError::CommitFailed)?;
            *committed = true;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(TestTx(Arc::clone(&self.0)))
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRawNormalizationWriter for TestRawWriter<'_> {
        async fn lock_next(
            &mut self,
            _: ProductListingRawStreamId,
        ) -> Result<
            crate::ports::ProductListingRawNormalizationWork,
            ProductListingRawNormalizationPortError,
        > {
            let mut state = self.0.lock().map_err(|_| {
                ProductListingRawNormalizationPortError::InvalidPersistedState {
                    source: application::error::box_error(std::io::Error::other(
                        "test lock poisoned",
                    )),
                }
            })?;
            state.work.take().ok_or_else(|| {
                ProductListingRawNormalizationPortError::InvalidPersistedState {
                    source: application::error::box_error(std::io::Error::other(
                        "test work missing",
                    )),
                }
            })
        }

        async fn complete(
            &mut self,
            completion: ProductListingRawNormalizationCompletion,
        ) -> Result<(), ProductListingRawNormalizationPortError> {
            let mut state = self.0.lock().map_err(|_| {
                ProductListingRawNormalizationPortError::InvalidPersistedState {
                    source: application::error::box_error(std::io::Error::other(
                        "test lock poisoned",
                    )),
                }
            })?;
            state.completions.push(completion);
            Ok(())
        }
    }

    impl AuctionRepositoryFactory<TestTx> for TestAuctions {
        fn in_transaction<'tx>(&'tx self, _: &'tx mut TestTx) -> impl AuctionRepository + 'tx {
            TestAuctionRepository
        }
    }

    #[async_trait::async_trait]
    impl AuctionRepository for TestAuctionRepository {
        async fn find_by_id(
            &mut self,
            _: auction_core::AuctionId,
        ) -> Result<Option<StoredAuction>, AuctionRepositoryError> {
            Ok(None)
        }

        async fn find_by_key(
            &mut self,
            _: &auction_core::AuctionKey,
        ) -> Result<Option<StoredAuction>, AuctionRepositoryError> {
            Ok(None)
        }

        async fn insert(
            &mut self,
            _: &auction_core::Auction,
        ) -> Result<StoredAuction, AuctionRepositoryError> {
            Err(AuctionRepositoryError::Internal {
                source: application::error::box_error(std::io::Error::other(
                    "unexpected auction insert in test",
                )),
            })
        }

        async fn update(
            &mut self,
            _: &auction_core::Auction,
            _: auction_service::ports::AuctionStorageVersion,
        ) -> Result<StoredAuction, AuctionRepositoryError> {
            Err(AuctionRepositoryError::Internal {
                source: application::error::box_error(std::io::Error::other(
                    "unexpected auction update in test",
                )),
            })
        }
    }

    impl AuctionEventAppenderFactory<TestTx> for TestAuctionEvents {
        fn in_transaction<'tx>(&'tx self, _: &'tx mut TestTx) -> impl AuctionEventAppender + 'tx {
            TestAuctionEventAppender
        }
    }

    #[async_trait::async_trait]
    impl AuctionEventAppender for TestAuctionEventAppender {
        async fn append(&mut self, _: &AuctionEvent) -> Result<(), AuctionEventAppendError> {
            Ok(())
        }
    }

    impl AuctionMetadataPolicyRepositoryFactory<TestTx> for TestAuctionPolicies {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TestTx,
        ) -> impl AuctionMetadataPolicyRepository + 'tx {
            TestAuctionPolicyRepository
        }
    }

    #[async_trait::async_trait]
    impl AuctionMetadataPolicyRepository for TestAuctionPolicyRepository {
        async fn find_protected_fields(
            &mut self,
            _: auction_core::AuctionId,
        ) -> Result<
            std::collections::BTreeSet<AuctionMetadataField>,
            AuctionMetadataPolicyRepositoryError,
        > {
            Ok(std::collections::BTreeSet::new())
        }

        async fn protect(
            &mut self,
            _: &AuctionMetadataPolicyAudit,
        ) -> Result<(), AuctionMetadataPolicyRepositoryError> {
            Ok(())
        }
    }

    impl ProductListingAuctionOverrideRepositoryFactory<TestTx> for TestAuctionOverrides {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TestTx,
        ) -> impl ProductListingAuctionOverrideRepository + 'tx {
            TestAuctionOverrideRepository
        }
    }

    #[async_trait::async_trait]
    impl ProductListingAuctionOverrideRepository for TestAuctionOverrideRepository {
        async fn find(
            &mut self,
            _: ProductListingId,
        ) -> Result<Option<ProductListingAuctionOverride>, ProductListingAuctionOverrideError>
        {
            Ok(None)
        }

        async fn activate(
            &mut self,
            _: &ProductListingAuctionOverrideAudit,
            _: product_listing_service::ports::ProductListingAuctionPolicyVersion,
        ) -> Result<ProductListingAuctionOverride, ProductListingAuctionOverrideError> {
            Ok(ProductListingAuctionOverride {
                version:
                    product_listing_service::ports::ProductListingAuctionPolicyVersion::default(),
                active: false,
                release_capture_generation: None,
            })
        }

        async fn release(
            &mut self,
            _: ProductListingId,
            _: product_listing_service::ports::ProductListingAuctionPolicyVersion,
            _: domain_primitives::event_id::EventId,
            _: String,
            _: time::OffsetDateTime,
        ) -> Result<ProductListingAuctionOverride, ProductListingAuctionOverrideError> {
            Ok(ProductListingAuctionOverride {
                version:
                    product_listing_service::ports::ProductListingAuctionPolicyVersion::default(),
                active: false,
                release_capture_generation: None,
            })
        }
    }

    impl ProductListingRawNormalizationWriterFactory<TestTx> for TestRawFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TestTx,
        ) -> impl ProductListingRawNormalizationWriter + 'tx {
            TestRawWriter(&self.0)
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRawNormalizationWriter for CappedContinuationRawWriter<'_> {
        async fn lock_next(
            &mut self,
            product_listing_raw_stream_id: ProductListingRawStreamId,
        ) -> Result<
            crate::ports::ProductListingRawNormalizationWork,
            ProductListingRawNormalizationPortError,
        > {
            let state = self
                .0
                .lock()
                .map_err(|_| capped_continuation_port_error("test lock poisoned"))?;
            if product_listing_raw_stream_id != state.product_listing_raw_stream_id {
                return Err(capped_continuation_port_error("unexpected raw stream"));
            }
            Ok(crate::ports::ProductListingRawNormalizationWork {
                head: ProductListingRawNormalizationHead {
                    product_listing_raw_stream_id,
                    listing_source_id: state.listing_source_id,
                    last_processed_revision: state.next_revision.saturating_sub(1),
                    product_listing_id: None,
                    source_listing_id: None,
                },
                next_revision: capped_continuation_revision(&state),
            })
        }

        async fn complete(
            &mut self,
            completion: ProductListingRawNormalizationCompletion,
        ) -> Result<(), ProductListingRawNormalizationPortError> {
            let mut state = self
                .0
                .lock()
                .map_err(|_| capped_continuation_port_error("test lock poisoned"))?;
            if completion.product_listing_raw_stream_id != state.product_listing_raw_stream_id
                || completion.revision != state.next_revision
            {
                return Err(capped_continuation_port_error("unexpected raw completion"));
            }
            state.next_revision = state
                .next_revision
                .checked_add(1)
                .ok_or_else(|| capped_continuation_port_error("test revision overflow"))?;
            state.product_listing_raw_revision_id = ProductListingRawRevisionId::new();
            state.completions.push(completion);
            Ok(())
        }
    }

    impl ProductListingRawNormalizationWriterFactory<TestTx> for CappedContinuationRawFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TestTx,
        ) -> impl ProductListingRawNormalizationWriter + 'tx {
            CappedContinuationRawWriter(&self.0)
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRepository for TestProductRepository {
        async fn find_by_id(
            &mut self,
            _: product_listing_core::product_listing_id::ProductListingId,
        ) -> Result<
            Option<product_listing_service::ports::VersionedProductListing>,
            ProductListingRepositoryError,
        > {
            Ok(None)
        }
        async fn find_by_key(
            &mut self,
            _: &product_listing_core::product_listing_id::ProductListingKey,
        ) -> Result<
            Option<product_listing_service::ports::VersionedProductListing>,
            ProductListingRepositoryError,
        > {
            Ok(None)
        }

        async fn insert(
            &mut self,
            _: &product_listing_core::product_listing::ProductListing,
            _: domain_primitives::event_id::EventId,
        ) -> Result<
            product_listing_service::ports::VersionedProductListing,
            ProductListingRepositoryError,
        > {
            Err(ProductListingRepositoryError::ProductListingInsertFailed)
        }
        async fn update(
            &mut self,
            _: &product_listing_core::product_listing::ProductListing,
            _: product_listing_service::ports::ProductListingStorageVersion,
            _: domain_primitives::event_id::EventId,
            _: product_listing_service::ports::ProductListingWriteEffects,
        ) -> Result<
            product_listing_service::ports::VersionedProductListing,
            ProductListingRepositoryError,
        > {
            Err(ProductListingRepositoryError::ProductListingUpdateFailed)
        }
    }

    impl ProductListingRepositoryFactory<TestTx> for TestProducts {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TestTx,
        ) -> impl ProductListingRepository + 'tx {
            TestProductRepository
        }
    }

    #[async_trait::async_trait]
    impl ProductListingEventAppender for TestEventAppender {
        async fn append(
            &mut self,
            _: &ProductListingEvent,
        ) -> Result<(), ProductListingEventAppendError> {
            Ok(())
        }
    }

    impl ProductListingEventAppenderFactory<TestTx> for TestEvents {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TestTx,
        ) -> impl ProductListingEventAppender + 'tx {
            TestEventAppender
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRawRevisionReader for TestRevisionReader {
        async fn find_next_revision(
            &self,
            _: ProductListingRawStreamId,
        ) -> Result<
            Option<crate::ports::ProductListingRawRevision>,
            ProductListingRawNormalizationPortError,
        > {
            let state = self.0.lock().map_err(|_| {
                ProductListingRawNormalizationPortError::InvalidPersistedState {
                    source: application::error::box_error(std::io::Error::other(
                        "test lock poisoned",
                    )),
                }
            })?;
            Ok(state
                .work
                .as_ref()
                .and_then(|work| work.next_revision.clone()))
        }
    }

    #[async_trait::async_trait]
    impl PendingProductListingRawStreamReader for TestRevisionReader {
        async fn list_pending_stream_page(
            &self,
            _: crate::ports::PendingProductListingRawStreamPageRequest,
        ) -> Result<
            crate::ports::PendingProductListingRawStreamPage,
            ProductListingRawNormalizationPortError,
        > {
            Ok(crate::ports::PendingProductListingRawStreamPage::default())
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRawRevisionReader for CappedContinuationReader {
        async fn find_next_revision(
            &self,
            product_listing_raw_stream_id: ProductListingRawStreamId,
        ) -> Result<
            Option<crate::ports::ProductListingRawRevision>,
            ProductListingRawNormalizationPortError,
        > {
            let state = self
                .0
                .lock()
                .map_err(|_| capped_continuation_port_error("test lock poisoned"))?;
            if product_listing_raw_stream_id != state.product_listing_raw_stream_id {
                return Err(capped_continuation_port_error("unexpected raw stream"));
            }
            Ok(capped_continuation_revision(&state))
        }
    }

    #[async_trait::async_trait]
    impl PendingProductListingRawStreamReader for CappedContinuationReader {
        async fn list_pending_stream_page(
            &self,
            _: crate::ports::PendingProductListingRawStreamPageRequest,
        ) -> Result<
            crate::ports::PendingProductListingRawStreamPage,
            ProductListingRawNormalizationPortError,
        > {
            let state = self
                .0
                .lock()
                .map_err(|_| capped_continuation_port_error("test lock poisoned"))?;
            Ok(crate::ports::PendingProductListingRawStreamPage {
                streams: vec![crate::ports::PendingProductListingRawStream {
                    product_listing_raw_stream_id: state.product_listing_raw_stream_id,
                    oldest_pending_at: OffsetDateTime::UNIX_EPOCH,
                }],
                next_cursor: None,
            })
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRawRevisionReader for FailingFirstPendingReader {
        async fn find_next_revision(
            &self,
            product_listing_raw_stream_id: ProductListingRawStreamId,
        ) -> Result<
            Option<crate::ports::ProductListingRawRevision>,
            ProductListingRawNormalizationPortError,
        > {
            if product_listing_raw_stream_id == self.blocked_stream_id {
                return Err(ProductListingRawNormalizationPortError::Persistence {
                    source: application::error::box_error(std::io::Error::other(
                        "transient test failure",
                    )),
                });
            }
            if product_listing_raw_stream_id != self.healthy_stream_id {
                return Ok(None);
            }
            let state = self.state.lock().map_err(|_| {
                ProductListingRawNormalizationPortError::InvalidPersistedState {
                    source: application::error::box_error(std::io::Error::other(
                        "test lock poisoned",
                    )),
                }
            })?;
            let next_revision = state
                .work
                .as_ref()
                .and_then(|work| work.next_revision.clone());
            if next_revision.is_none() && self.fail_after_healthy_completion {
                return Err(ProductListingRawNormalizationPortError::Persistence {
                    source: application::error::box_error(std::io::Error::other(
                        "transient test failure after commit",
                    )),
                });
            }
            Ok(next_revision)
        }
    }

    #[async_trait::async_trait]
    impl PendingProductListingRawStreamReader for FailingFirstPendingReader {
        async fn list_pending_stream_page(
            &self,
            _: crate::ports::PendingProductListingRawStreamPageRequest,
        ) -> Result<
            crate::ports::PendingProductListingRawStreamPage,
            ProductListingRawNormalizationPortError,
        > {
            let now = OffsetDateTime::now_utc();
            Ok(crate::ports::PendingProductListingRawStreamPage {
                streams: vec![
                    crate::ports::PendingProductListingRawStream {
                        product_listing_raw_stream_id: self.blocked_stream_id,
                        oldest_pending_at: now - time::Duration::seconds(1),
                    },
                    crate::ports::PendingProductListingRawStream {
                        product_listing_raw_stream_id: self.healthy_stream_id,
                        oldest_pending_at: now,
                    },
                ],
                next_cursor: None,
            })
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRawRevisionReader for FailingPendingListReader {
        async fn find_next_revision(
            &self,
            _: ProductListingRawStreamId,
        ) -> Result<
            Option<crate::ports::ProductListingRawRevision>,
            ProductListingRawNormalizationPortError,
        > {
            Ok(None)
        }
    }

    fn capped_continuation_revision(
        state: &CappedContinuationState,
    ) -> Option<crate::ports::ProductListingRawRevision> {
        (state.next_revision <= state.last_revision).then(|| {
            crate::ports::ProductListingRawRevision {
                product_listing_raw_revision_id: state.product_listing_raw_revision_id,
                product_listing_raw_stream_id: state.product_listing_raw_stream_id,
                revision: state.next_revision,
                capture_generation: state.next_revision,
                input: state.input.clone(),
            }
        })
    }

    fn capped_continuation_port_error(
        message: &'static str,
    ) -> ProductListingRawNormalizationPortError {
        ProductListingRawNormalizationPortError::InvalidPersistedState {
            source: application::error::box_error(std::io::Error::other(message)),
        }
    }

    #[async_trait::async_trait]
    impl PendingProductListingRawStreamReader for FailingPendingListReader {
        async fn list_pending_stream_page(
            &self,
            _: crate::ports::PendingProductListingRawStreamPageRequest,
        ) -> Result<
            crate::ports::PendingProductListingRawStreamPage,
            ProductListingRawNormalizationPortError,
        > {
            Err(ProductListingRawNormalizationPortError::Persistence {
                source: application::error::box_error(std::io::Error::other(
                    "pending stream list failed",
                )),
            })
        }
    }

    #[test]
    fn should_return_retryable_configuration_error_for_system_normalization_failure() {
        let result = require_terminal_normalization_outcome(
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::Availability(
                    product_listing_normalization::NormalizationError::AvailabilityRegexSetCompilationFailed,
                ),
            ),
        );

        assert!(matches!(
            result,
            Err(
                NormalizeProductListingRawRevisionError::NormalizationConfigurationFailed {
                    source:
                        ProductListingRawValuesNormalizationError::Availability(
                            product_listing_normalization::NormalizationError::AvailabilityRegexSetCompilationFailed
                        ),
                }
            )
        ));
    }

    #[tokio::test]
    async fn should_reject_removed_raw_values_schema_from_the_revision_reader_and_advance_stream_head()
     {
        let stream_id = ProductListingRawStreamId::new();
        let revision_id = ProductListingRawRevisionId::new();
        let input = ProductListingNormalizationInput::new(
            RawProductListingOperation::Upsert,
            RawProductListingPayloadFormat::ShopifyProduct,
            1,
            2,
            SourcePayload::new(serde_json::json!({}))
                .unwrap_or_else(|error| panic!("input: {error}")),
            RawProductListingValues::new(serde_json::json!({"sourceListingId": "only-id"}))
                .unwrap_or_else(|error| panic!("input: {error}")),
            NormalizationContext::new(serde_json::json!({}))
                .unwrap_or_else(|error| panic!("input: {error}")),
        )
        .unwrap_or_else(|error| panic!("input: {error}"));
        let state = Arc::new(Mutex::new(TestRawState {
            work: Some(crate::ports::ProductListingRawNormalizationWork {
                head: ProductListingRawNormalizationHead {
                    product_listing_raw_stream_id: stream_id,
                    listing_source_id: ListingSourceId::new(),
                    last_processed_revision: 0,
                    product_listing_id: None,
                    source_listing_id: None,
                },
                next_revision: Some(crate::ports::ProductListingRawRevision {
                    product_listing_raw_revision_id: revision_id,
                    product_listing_raw_stream_id: stream_id,
                    revision: 1,
                    capture_generation: 1,
                    input,
                }),
            }),
            completions: vec![],
        }));
        let committed = Arc::new(Mutex::new(false));
        let handler = NormalizeProductListingRawRevisionHandler::new(
            TestUnitOfWork(Arc::clone(&committed)),
            TestRawFactory(Arc::clone(&state)),
            TestProducts,
            TestEvents,
            TestAuctions,
            TestAuctionEvents,
            TestAuctionPolicies,
            TestAuctionOverrides,
            TestRevisionReader(Arc::clone(&state)),
        );

        let result = handler
            .execute(NormalizeProductListingRawRevisionCommand {
                mode: NormalizeProductListingRawRevisionMode::RawRevision {
                    product_listing_raw_stream_id: stream_id,
                    product_listing_raw_revision_id: revision_id,
                    revision: 1,
                },
                max_revisions_per_stream: 1,
                pending_stream_limit: 1,
            })
            .await;

        assert!(matches!(
            result,
            Err(NormalizeProductListingRawRevisionError::UnsupportedStoredSchemaVersion)
        ));
        assert!(matches!(committed.lock(), Ok(committed) if !*committed));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(state.completions.is_empty());
    }

    #[tokio::test]
    async fn should_report_stream_failures_without_fifo_continuations_when_reconciliation_continues()
    -> Result<(), Box<dyn std::error::Error>> {
        let blocked_stream_id = ProductListingRawStreamId::new();
        let healthy_stream_id = ProductListingRawStreamId::new();
        let healthy_revision_id = ProductListingRawRevisionId::new();
        let input = ProductListingNormalizationInput::new(
            RawProductListingOperation::Upsert,
            RawProductListingPayloadFormat::ShopifyProduct,
            1,
            1,
            SourcePayload::new(serde_json::json!({}))?,
            RawProductListingValues::new(serde_json::json!({"sourceListingId": "only-id"}))?,
            NormalizationContext::new(serde_json::json!({}))?,
        )?;
        let state = Arc::new(Mutex::new(TestRawState {
            work: Some(crate::ports::ProductListingRawNormalizationWork {
                head: ProductListingRawNormalizationHead {
                    product_listing_raw_stream_id: healthy_stream_id,
                    listing_source_id: ListingSourceId::new(),
                    last_processed_revision: 0,
                    product_listing_id: None,
                    source_listing_id: None,
                },
                next_revision: Some(crate::ports::ProductListingRawRevision {
                    product_listing_raw_revision_id: healthy_revision_id,
                    product_listing_raw_stream_id: healthy_stream_id,
                    revision: 1,
                    capture_generation: 1,
                    input,
                }),
            }),
            completions: vec![],
        }));
        let committed = Arc::new(Mutex::new(false));
        let handler = NormalizeProductListingRawRevisionHandler::new(
            TestUnitOfWork(Arc::clone(&committed)),
            TestRawFactory(Arc::clone(&state)),
            TestProducts,
            TestEvents,
            TestAuctions,
            TestAuctionEvents,
            TestAuctionPolicies,
            TestAuctionOverrides,
            FailingFirstPendingReader {
                state: Arc::clone(&state),
                blocked_stream_id,
                healthy_stream_id,
                fail_after_healthy_completion: true,
            },
        );

        let result = handler
            .execute(NormalizeProductListingRawRevisionCommand {
                mode: NormalizeProductListingRawRevisionMode::Reconcile,
                max_revisions_per_stream: 2,
                pending_stream_limit: 2,
            })
            .await?;

        assert_eq!(
            [NormalizedRawRevisionResult {
                product_listing_raw_stream_id: healthy_stream_id,
                revision: 1,
                outcome: ProductListingRawNormalizationOutcome::Rejected,
            }],
            result.revisions.as_slice()
        );
        assert_eq!(
            [
                ProductListingRawNormalizationStreamFailure {
                    product_listing_raw_stream_id: blocked_stream_id,
                    error_code: "PERSISTENCE_FAILED",
                },
                ProductListingRawNormalizationStreamFailure {
                    product_listing_raw_stream_id: healthy_stream_id,
                    error_code: "PERSISTENCE_FAILED",
                },
            ],
            result.stream_failures.as_slice()
        );
        assert_eq!(Some(2), result.pending_stream_page_count);
        assert_eq!(None, result.next_pending_stream_cursor);
        assert!(result.continuation_stream_ids.is_empty());
        assert!(matches!(committed.lock(), Ok(committed) if *committed));
        assert!(matches!(
            state.lock(),
            Ok(state) if matches!(
                state.completions.as_slice(),
                [ProductListingRawNormalizationCompletion {
                    product_listing_raw_stream_id,
                    revision: 1,
                    outcome: ProductListingRawNormalizationOutcome::Rejected,
                    ..
                }] if *product_listing_raw_stream_id == healthy_stream_id
            )
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_return_capped_reconciliation_stream_as_worker_continuation()
    -> Result<(), Box<dyn std::error::Error>> {
        let product_listing_raw_stream_id = ProductListingRawStreamId::new();
        let state = Arc::new(Mutex::new(CappedContinuationState {
            product_listing_raw_stream_id,
            product_listing_raw_revision_id: ProductListingRawRevisionId::new(),
            listing_source_id: ListingSourceId::new(),
            next_revision: 1,
            last_revision: 3,
            input: input_with_schema_versions(1, 1)?,
            completions: Vec::new(),
        }));
        let committed = Arc::new(Mutex::new(false));
        let handler = NormalizeProductListingRawRevisionHandler::new(
            TestUnitOfWork(Arc::clone(&committed)),
            CappedContinuationRawFactory(Arc::clone(&state)),
            TestProducts,
            TestEvents,
            TestAuctions,
            TestAuctionEvents,
            TestAuctionPolicies,
            TestAuctionOverrides,
            CappedContinuationReader(Arc::clone(&state)),
        );

        let first = handler
            .execute(NormalizeProductListingRawRevisionCommand {
                mode: NormalizeProductListingRawRevisionMode::Reconcile,
                max_revisions_per_stream: 2,
                pending_stream_limit: 1,
            })
            .await?;

        assert_eq!(
            [1, 2],
            first
                .revisions
                .iter()
                .map(|revision| revision.revision)
                .collect::<Vec<_>>()
                .as_slice()
        );
        assert_eq!(
            [product_listing_raw_stream_id],
            first.continuation_stream_ids.as_slice()
        );

        let continuation = handler
            .execute(NormalizeProductListingRawRevisionCommand {
                mode: NormalizeProductListingRawRevisionMode::ReconcileContinuation {
                    product_listing_raw_stream_id,
                },
                max_revisions_per_stream: 2,
                pending_stream_limit: 1,
            })
            .await?;

        assert_eq!(
            [3],
            continuation
                .revisions
                .iter()
                .map(|revision| revision.revision)
                .collect::<Vec<_>>()
                .as_slice()
        );
        assert!(continuation.continuation_stream_ids.is_empty());
        assert!(matches!(committed.lock(), Ok(committed) if *committed));
        assert!(matches!(state.lock(), Ok(state) if state.completions.len() == 3));
        Ok(())
    }

    #[tokio::test]
    async fn should_return_pending_list_failure_as_overall_error() {
        let state = Arc::new(Mutex::new(TestRawState {
            work: None,
            completions: vec![],
        }));
        let committed = Arc::new(Mutex::new(false));
        let handler = NormalizeProductListingRawRevisionHandler::new(
            TestUnitOfWork(Arc::clone(&committed)),
            TestRawFactory(state),
            TestProducts,
            TestEvents,
            TestAuctions,
            TestAuctionEvents,
            TestAuctionPolicies,
            TestAuctionOverrides,
            FailingPendingListReader,
        );

        let result = handler
            .execute(NormalizeProductListingRawRevisionCommand {
                mode: NormalizeProductListingRawRevisionMode::Reconcile,
                max_revisions_per_stream: 1,
                pending_stream_limit: 1,
            })
            .await;

        assert!(matches!(
            result,
            Err(
                NormalizeProductListingRawRevisionError::PendingStreamReadFailed {
                    source: ProductListingRawNormalizationPortError::Persistence { .. },
                }
            )
        ));
        assert!(matches!(committed.lock(), Ok(committed) if !*committed));
    }

    #[test]
    fn should_pass_normalized_embedded_auction_metadata_to_the_canonical_writer()
    -> Result<(), Box<dyn std::error::Error>> {
        let input = current_upsert_input(serde_json::json!({
            "action": "SET",
            "value": {
                "sourceAuctionId": {"action": "SET", "value": "catalogue-2026-0042"},
                "auctionMetadata": {
                    "name": "Autumn Decorative Arts",
                    "format": "TIMED",
                    "schedule": {
                        "liveStarts": {"precision": "INSTANT", "value": "2026-10-18T10:00:00Z"}
                    }
                }
            }
        }))?;
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) =
            ProductListingRawValuesNormalizer::new().normalize(&input)
        else {
            panic!("valid raw auction metadata should resolve");
        };

        let command = canonical_upsert(ListingSourceId::new(), resolved.as_ref());

        assert_eq!(
            Some("Autumn Decorative Arts"),
            command
                .auction_metadata
                .name
                .as_ref()
                .map(|value| value.payload.as_ref())
        );
        assert_eq!(
            Some(auction_core::AuctionFormat::Timed),
            command.auction_metadata.format
        );
        assert!(matches!(
            command.auction_metadata.live_starts,
            Some(auction_core::AuctionTime::Instant { .. })
        ));
        Ok(())
    }

    #[test]
    fn should_preserve_auction_clear_and_complete_invalid_timing_diagnostics_successfully()
    -> Result<(), Box<dyn std::error::Error>> {
        let clear_input = current_upsert_input(serde_json::json!({"action": "CLEAR"}))?;
        let ProductListingRawValuesNormalizationOutcome::Resolved(clear_resolved) =
            ProductListingRawValuesNormalizer::new().normalize(&clear_input)
        else {
            panic!("clear auction patch should resolve");
        };
        let listing_source_id = ListingSourceId::new();
        assert_eq!(
            PatchField::Clear,
            canonical_upsert(listing_source_id, clear_resolved.as_ref()).auction
        );

        let invalid_timing_input = current_upsert_input(serde_json::json!({
            "action": "SET",
            "value": {
                "timing": {
                    "reportedClosedAt": {"precision": "DATE", "value": "2026-05-13"}
                }
            }
        }))?;
        let ProductListingRawValuesNormalizationOutcome::Resolved(invalid_timing_resolved) =
            ProductListingRawValuesNormalizer::new().normalize(&invalid_timing_input)
        else {
            panic!("invalid optional timing should resolve");
        };
        assert_eq!(
            PatchField::Unchanged,
            canonical_upsert(listing_source_id, invalid_timing_resolved.as_ref()).auction
        );

        let stream_id = ProductListingRawStreamId::new();
        let revision = crate::ports::ProductListingRawRevision {
            product_listing_raw_revision_id: ProductListingRawRevisionId::new(),
            product_listing_raw_stream_id: stream_id,
            revision: 1,
            capture_generation: 1,
            input: invalid_timing_input,
        };
        let head = ProductListingRawNormalizationHead {
            product_listing_raw_stream_id: stream_id,
            listing_source_id,
            last_processed_revision: 0,
            product_listing_id: None,
            source_listing_id: None,
        };
        for outcome in [
            ProductListingRawNormalizationOutcome::Applied,
            ProductListingRawNormalizationOutcome::NoChange,
        ] {
            let completed = completion(
                &head,
                &revision,
                outcome,
                None,
                None,
                invalid_timing_resolved
                    .diagnostic
                    .map(ProductListingRawValuesNormalizationDiagnostic::as_str),
            );
            assert_eq!(NORMALIZER_VERSION, completed.normalizer_version);
            assert_eq!(outcome, completed.outcome);
            assert_eq!(Some("AUCTION_TIMING_INVALID"), completed.error_code);
        }
        Ok(())
    }

    #[test]
    fn should_accept_only_the_current_stored_raw_values_schema_version()
    -> Result<(), product_listing_normalization::NormalizationInputError> {
        let input = input_with_schema_versions(1, PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION)?;
        assert!(validate_stored_schema(&input).is_ok());

        for (payload_schema_version, raw_values_schema_version) in [(2, 1), (1, 2), (1, 3)] {
            let input =
                input_with_schema_versions(payload_schema_version, raw_values_schema_version)?;
            assert!(matches!(
                validate_stored_schema(&input),
                Err(NormalizeProductListingRawRevisionError::UnsupportedStoredSchemaVersion)
            ));
        }
        assert_eq!(4, NORMALIZER_VERSION);
        Ok(())
    }

    #[test]
    fn should_return_pending_age_only_for_past_captures() {
        let now = OffsetDateTime::now_utc();

        assert_eq!(
            Some(60),
            pending_age_seconds(now - time::Duration::seconds(60))
        );
        assert_eq!(None, pending_age_seconds(now + time::Duration::days(1)));
    }

    #[test]
    fn should_use_stable_failure_codes() {
        assert_eq!(
            "UNSUPPORTED_STORED_SCHEMA_VERSION",
            normalization_failure_code(
                &NormalizeProductListingRawRevisionError::UnsupportedStoredSchemaVersion
            )
        );
        assert_eq!(
            "NORMALIZATION_CONFIGURATION_FAILED",
            normalization_failure_code(
                &NormalizeProductListingRawRevisionError::NormalizationConfigurationFailed {
                    source: ProductListingRawValuesNormalizationError::Availability(
                        product_listing_normalization::NormalizationError::AvailabilityRegexSetCompilationFailed,
                    ),
                }
            )
        );
        assert_eq!(
            "CANONICAL_WRITE_FAILED",
            normalization_failure_code(
                &NormalizeProductListingRawRevisionError::CanonicalWriteFailed {
                    source: CanonicalProductListingWriteError::BoundProductListingNotFound,
                }
            )
        );
    }
}
