use crate::ports::{
    PendingProductListingRawStreamReader, ProductListingRawNormalizationCompletion,
    ProductListingRawNormalizationHead, ProductListingRawNormalizationOutcome,
    ProductListingRawNormalizationPortError, ProductListingRawNormalizationWriter,
    ProductListingRawNormalizationWriterFactory, ProductListingRawRevisionReader,
};
use application::patch_field::PatchField;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::change_outcome::ChangeOutcome;
use indexmap::IndexSet;
use product_listing_normalization::{
    ListingAvailabilityQuickCheck, ProductListingRawValuesNormalizationError,
    ProductListingRawValuesNormalizationOutcome, ProductListingRawValuesNormalizer,
    ProductListingRawValuesPatch, ProductListingRawValuesResolved,
};
use product_listing_service::canonical_product_listing_write::{
    CanonicalProductListingUpsert, CanonicalProductListingWriteError, CanonicalProductListingWriter,
};
use product_listing_service::ports::{
    ProductListingEventAppenderFactory, ProductListingRawRevisionId, ProductListingRawStreamId,
    ProductListingRepositoryFactory,
};

pub const NORMALIZER_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizeProductListingRawRevisionMode {
    /// CDC wake-up metadata. The handler drains from the stream head and never trusts delivery order.
    RawRevision {
        product_listing_raw_stream_id: ProductListingRawStreamId,
        product_listing_raw_revision_id: ProductListingRawRevisionId,
        revision: u64,
    },
    Reconcile,
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NormalizeProductListingRawRevisionResult {
    pub revisions: Vec<NormalizedRawRevisionResult>,
}

#[derive(Debug, thiserror::Error)]
pub enum NormalizeProductListingRawRevisionError {
    #[error("normalization work limit must be greater than zero")]
    InvalidLimit,
    #[error("failed to read pending raw product listing streams")]
    PendingStreamReadFailed,
    #[error("failed to begin raw product listing normalization transaction")]
    BeginTransactionFailed,
    #[error("raw product listing normalization storage failed")]
    PersistenceFailed,
    #[error("raw product listing normalization stored state is invalid")]
    InvalidPersistedState,
    #[error("raw product listing schema version is unsupported")]
    UnsupportedStoredSchemaVersion,
    #[error("raw product listing normalization failed")]
    CanonicalWriteFailed,
    #[error("failed to commit raw product listing normalization transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait NormalizeProductListingRawRevisionUseCase: Send + Sync {
    async fn execute(
        &self,
        command: NormalizeProductListingRawRevisionCommand,
    ) -> Result<NormalizeProductListingRawRevisionResult, NormalizeProductListingRawRevisionError>;
}

pub struct NormalizeProductListingRawRevisionHandler<U, W, R, E, P> {
    unit_of_work: U,
    raw_normalizations: W,
    products: R,
    events: E,
    pending_streams: P,
    normalizer: ProductListingRawValuesNormalizer,
}

impl<U, W, R, E, P> NormalizeProductListingRawRevisionHandler<U, W, R, E, P> {
    pub fn new(
        unit_of_work: U,
        raw_normalizations: W,
        products: R,
        events: E,
        pending_streams: P,
    ) -> Self {
        Self {
            unit_of_work,
            raw_normalizations,
            products,
            events,
            pending_streams,
            normalizer: ProductListingRawValuesNormalizer::new(),
        }
    }
}

impl<U, W, R, E, P> NormalizeProductListingRawRevisionHandler<U, W, R, E, P>
where
    U: UnitOfWork,
    W: ProductListingRawNormalizationWriterFactory<U::Tx>,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    P: PendingProductListingRawStreamReader + ProductListingRawRevisionReader,
{
    async fn drain_stream(
        &self,
        product_listing_raw_stream_id: ProductListingRawStreamId,
        max_revisions: u32,
    ) -> Result<Vec<NormalizedRawRevisionResult>, NormalizeProductListingRawRevisionError> {
        let mut results = Vec::new();
        for _ in 0..max_revisions {
            let Some(candidate) = self
                .pending_streams
                .find_next_revision(product_listing_raw_stream_id)
                .await
                .map_err(map_port_error)?
            else {
                return Ok(results);
            };
            validate_stored_schema(&candidate.input)?;
            let normalized = self.normalizer.normalize(&candidate.input);
            let mut tx = self
                .unit_of_work
                .begin()
                .await
                .map_err(|_| NormalizeProductListingRawRevisionError::BeginTransactionFailed)?;
            let work = self
                .raw_normalizations
                .in_transaction(&mut tx)
                .lock_next(product_listing_raw_stream_id)
                .await
                .map_err(map_port_error)?;
            let Some(revision) = work.next_revision else {
                return Ok(results);
            };
            if revision.product_listing_raw_revision_id != candidate.product_listing_raw_revision_id
                || revision.revision != candidate.revision
            {
                continue;
            }
            let completion = self
                .complete_work_in_transaction(&mut tx, work.head, revision, &normalized)
                .await?;
            let result = NormalizedRawRevisionResult {
                product_listing_raw_stream_id,
                revision: completion.revision,
                outcome: completion.outcome,
            };
            self.raw_normalizations
                .in_transaction(&mut tx)
                .complete(completion)
                .await
                .map_err(map_port_error)?;
            tx.commit()
                .await
                .map_err(|_| NormalizeProductListingRawRevisionError::CommitTransactionFailed)?;
            results.push(result);
        }
        Ok(results)
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
                    Err(_) => {
                        return Err(NormalizeProductListingRawRevisionError::CanonicalWriteFailed);
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
                let command = canonical_upsert(head.listing_source_id, resolved.as_ref());
                let write = match CanonicalProductListingWriter::upsert_in_transaction(
                    tx,
                    &self.products,
                    &self.events,
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
                    Err(_) => {
                        return Err(NormalizeProductListingRawRevisionError::CanonicalWriteFailed);
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
                    None,
                );
                completion.next_product_listing_id = Some(write.product_listing_id);
                completion.next_source_listing_id = Some(resolved.source_listing_id.clone());
                Ok(completion)
            }
        }
    }
}

#[async_trait::async_trait]
impl<U, W, R, E, P> NormalizeProductListingRawRevisionUseCase
    for NormalizeProductListingRawRevisionHandler<U, W, R, E, P>
where
    U: UnitOfWork + Send + Sync,
    W: ProductListingRawNormalizationWriterFactory<U::Tx> + Send + Sync,
    R: ProductListingRepositoryFactory<U::Tx> + Send + Sync,
    E: ProductListingEventAppenderFactory<U::Tx> + Send + Sync,
    P: PendingProductListingRawStreamReader + ProductListingRawRevisionReader + Send + Sync,
{
    #[tracing::instrument(name = "normalize_product_listing_raw_revision", skip_all)]
    async fn execute(
        &self,
        command: NormalizeProductListingRawRevisionCommand,
    ) -> Result<NormalizeProductListingRawRevisionResult, NormalizeProductListingRawRevisionError>
    {
        if command.max_revisions_per_stream == 0 || command.pending_stream_limit == 0 {
            return Err(NormalizeProductListingRawRevisionError::InvalidLimit);
        }
        let streams = match command.mode {
            NormalizeProductListingRawRevisionMode::RawRevision {
                product_listing_raw_stream_id,
                product_listing_raw_revision_id: _,
                revision: _,
            } => vec![product_listing_raw_stream_id],
            NormalizeProductListingRawRevisionMode::Reconcile => self
                .pending_streams
                .list_pending_streams(command.pending_stream_limit)
                .await
                .map_err(|_| NormalizeProductListingRawRevisionError::PendingStreamReadFailed)?,
        };
        let mut result = NormalizeProductListingRawRevisionResult::default();
        for stream in streams {
            result.revisions.extend(
                self.drain_stream(stream, command.max_revisions_per_stream)
                    .await?,
            );
        }
        Ok(result)
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
        auction_start: to_patch(&resolved.auction_start),
        auction_end: to_patch(&resolved.auction_end),
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
    if input.payload_schema_version() != 1 || input.raw_values_schema_version() != 1 {
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

fn normalization_error_code(error: &ProductListingRawValuesNormalizationError) -> &'static str {
    match error {
        ProductListingRawValuesNormalizationError::InvalidRawValuesV1(_) => "RAW_VALUES_INVALID",
        ProductListingRawValuesNormalizationError::InvalidNormalizationContextV1(_) => {
            "NORMALIZATION_CONTEXT_INVALID"
        }
        ProductListingRawValuesNormalizationError::InvalidBaseUrl(_) => {
            "NORMALIZATION_BASE_URL_INVALID"
        }
        ProductListingRawValuesNormalizationError::InvalidUrl(_) => "LISTING_URL_INVALID",
        ProductListingRawValuesNormalizationError::UnsupportedFallbackCurrency => {
            "FALLBACK_CURRENCY_UNSUPPORTED"
        }
        ProductListingRawValuesNormalizationError::UnsupportedFallbackLanguage => {
            "FALLBACK_LANGUAGE_UNSUPPORTED"
        }
        ProductListingRawValuesNormalizationError::Text(_) => "TEXT_NORMALIZATION_INVALID",
        ProductListingRawValuesNormalizationError::Price(_) => "PRICE_NORMALIZATION_INVALID",
        ProductListingRawValuesNormalizationError::ImageUrl(_) => "IMAGE_URL_NORMALIZATION_INVALID",
        ProductListingRawValuesNormalizationError::DateTime(_) => "DATE_TIME_NORMALIZATION_INVALID",
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
        ProductListingRawNormalizationPortError::Persistence { .. } => {
            NormalizeProductListingRawRevisionError::PersistenceFailed
        }
        ProductListingRawNormalizationPortError::InvalidPersistedState { .. } => {
            NormalizeProductListingRawRevisionError::InvalidPersistedState
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::transaction::TransactionError;
    use listing_source_core::ListingSourceId;
    use product_listing_normalization::{
        NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
        RawProductListingPayloadFormat, RawProductListingValues, SourcePayload,
    };
    use product_listing_service::ports::product_listing_event_appender::ProductListingEvent;
    use product_listing_service::ports::{
        ProductListingEventAppendError, ProductListingEventAppender, ProductListingRawRevisionId,
        ProductListingRepository, ProductListingRepositoryError,
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
    struct TestRevisionReader(Arc<Mutex<TestRawState>>);

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

    impl ProductListingRawNormalizationWriterFactory<TestTx> for TestRawFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TestTx,
        ) -> impl ProductListingRawNormalizationWriter + 'tx {
            TestRawWriter(&self.0)
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
        async fn find_by_listing_source_and_url(
            &mut self,
            _: listing_source_core::ListingSourceId,
            _: &url::Url,
        ) -> Result<
            Vec<product_listing_service::ports::VersionedProductListing>,
            ProductListingRepositoryError,
        > {
            Ok(vec![])
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
        async fn list_pending_streams(
            &self,
            _: u32,
        ) -> Result<Vec<ProductListingRawStreamId>, ProductListingRawNormalizationPortError>
        {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn should_reject_invalid_raw_values_and_advance_stream_head() {
        let stream_id = ProductListingRawStreamId::from_uuid(uuid::Uuid::new_v4());
        let revision_id = ProductListingRawRevisionId::from_uuid(uuid::Uuid::new_v4());
        let input = ProductListingNormalizationInput::new(
            RawProductListingOperation::Upsert,
            RawProductListingPayloadFormat::ShopifyProduct,
            1,
            1,
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
                    listing_source_id: ListingSourceId::from(uuid::Uuid::new_v4()),
                    last_processed_revision: 0,
                    product_listing_id: None,
                    source_listing_id: None,
                },
                next_revision: Some(crate::ports::ProductListingRawRevision {
                    product_listing_raw_revision_id: revision_id,
                    product_listing_raw_stream_id: stream_id,
                    revision: 1,
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
            Ok(NormalizeProductListingRawRevisionResult { ref revisions })
                if revisions.as_slice() == [NormalizedRawRevisionResult {
                    product_listing_raw_stream_id: stream_id,
                    revision: 1,
                    outcome: ProductListingRawNormalizationOutcome::Rejected,
                }]
        ));
        assert!(matches!(committed.lock(), Ok(committed) if *committed));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(matches!(
            state.completions.as_slice(),
            [ProductListingRawNormalizationCompletion {
                outcome: ProductListingRawNormalizationOutcome::Rejected,
                error_code: Some("RAW_VALUES_INVALID"),
                ..
            }]
        ));
    }
}
