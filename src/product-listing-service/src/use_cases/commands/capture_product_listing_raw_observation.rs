use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingRawCaptureWrite,
    ProductListingRawCaptureWriteError, ProductListingRawCaptureWriteOutcome,
    ProductListingRawCaptureWriter, ProductListingRawCaptureWriterFactory,
    ProductListingRawIngestionMethod, ProductListingRawProviderReceipt, SourceRecordKeySha256,
};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::ListingSourceId;
use product_listing_normalization::{
    NormalizationInputError, ProductListingNormalizationInput, RawProductListingProvenance,
};
use sha2::{Digest, Sha256};
use std::time::Instant;
use time::OffsetDateTime;
use user_core::user_id::UserId;

pub const MAX_SOURCE_RECORD_KEY_UTF8_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub struct CaptureProductListingRawObservationCommand {
    pub listing_source_id: ListingSourceId,
    pub ingestion_method: ProductListingRawIngestionMethod,
    pub source_record_key: String,
    pub input: ProductListingNormalizationInput,
    pub provenance: RawProductListingProvenance,
    pub source_event_id: Option<String>,
    pub source_occurred_at: Option<OffsetDateTime>,
    pub provider_receipt: Option<ProductListingRawProviderReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureProductListingRawObservationResult {
    Changed {
        product_listing_raw_stream_id: crate::ports::ProductListingRawStreamId,
        product_listing_raw_revision_id: crate::ports::ProductListingRawRevisionId,
        revision: u64,
    },
    Unchanged {
        product_listing_raw_stream_id: crate::ports::ProductListingRawStreamId,
        latest_revision: u64,
    },
    Duplicate {
        product_listing_raw_stream_id: crate::ports::ProductListingRawStreamId,
        latest_revision: u64,
    },
    Stale {
        product_listing_raw_stream_id: crate::ports::ProductListingRawStreamId,
        latest_revision: u64,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureProductListingRawObservationError {
    #[error("authenticated actor required to capture raw product listing input")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("source record key exceeds the maximum UTF-8 byte length")]
    SourceRecordKeyTooLong { len: usize, max: usize },
    #[error("source record key contains an embedded NUL")]
    SourceRecordKeyEmbeddedNul,
    #[error("raw product listing input is invalid")]
    InvalidInput {
        #[source]
        source: BoxError,
    },
    #[error("listing source not found")]
    ListingSourceNotFound,
    #[error("partner product listing authorization is temporarily unavailable")]
    PartnerAuthorizationTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("partner product listing authorization failed internally")]
    PartnerAuthorizationInternal {
        #[source]
        source: BoxError,
    },
    #[error("raw product listing source-record key hash collision")]
    SourceRecordKeyHashCollision,
    #[error("provider receipt conflicts with existing source evidence")]
    ProviderReceiptDigestConflict,
    #[error("provider source order conflicts with existing source evidence")]
    ProviderSourceOrderConflict,
    #[error("failed to begin raw product listing capture transaction")]
    BeginTransactionFailed,
    #[error("raw product listing capture failed")]
    CaptureFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit raw product listing capture transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CaptureProductListingRawObservationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CaptureProductListingRawObservationCommand,
    ) -> Result<CaptureProductListingRawObservationResult, CaptureProductListingRawObservationError>;
}

pub struct CaptureProductListingRawObservationHandler<U, W, A> {
    unit_of_work: U,
    writer: W,
    authorizer: A,
}

impl<U, W, A> CaptureProductListingRawObservationHandler<U, W, A> {
    pub fn new(unit_of_work: U, writer: W, authorizer: A) -> Self {
        Self {
            unit_of_work,
            writer,
            authorizer,
        }
    }
}

impl<U, W, A> CaptureProductListingRawObservationHandler<U, W, A>
where
    U: UnitOfWork,
    W: ProductListingRawCaptureWriterFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    async fn capture(
        &self,
        context: &OperationContext,
        command: CaptureProductListingRawObservationCommand,
    ) -> Result<CaptureProductListingRawObservationResult, CaptureProductListingRawObservationError>
    {
        validate_source_record_key(&command.source_record_key)?;
        let input_sha256 = command
            .input
            .hash()
            .map_err(CaptureProductListingRawObservationError::from)?;
        let source_record_key_sha256 =
            SourceRecordKeySha256::new(Sha256::digest(command.source_record_key.as_bytes()).into());

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CaptureProductListingRawObservationError::BeginTransactionFailed)?;

        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(&mut tx)
                .authorize(actor_id, command.listing_source_id)
                .await
                .map_err(CaptureProductListingRawObservationError::from)?;
        }

        let outcome = self
            .writer
            .in_transaction(&mut tx)
            .capture(ProductListingRawCaptureWrite {
                listing_source_id: command.listing_source_id,
                ingestion_method: command.ingestion_method,
                source_record_key: command.source_record_key,
                source_record_key_sha256,
                input: command.input,
                input_sha256,
                provenance: command.provenance,
                source_event_id: command.source_event_id,
                source_occurred_at: command.source_occurred_at,
                provider_receipt: command.provider_receipt,
            })
            .await
            .map_err(CaptureProductListingRawObservationError::from)?;

        tx.commit()
            .await
            .map_err(|_| CaptureProductListingRawObservationError::CommitTransactionFailed)?;

        Ok(match outcome {
            ProductListingRawCaptureWriteOutcome::Changed {
                product_listing_raw_stream_id,
                product_listing_raw_revision_id,
                revision,
            } => CaptureProductListingRawObservationResult::Changed {
                product_listing_raw_stream_id,
                product_listing_raw_revision_id,
                revision,
            },
            ProductListingRawCaptureWriteOutcome::Unchanged {
                product_listing_raw_stream_id,
                latest_revision,
            } => CaptureProductListingRawObservationResult::Unchanged {
                product_listing_raw_stream_id,
                latest_revision,
            },
            ProductListingRawCaptureWriteOutcome::Duplicate {
                product_listing_raw_stream_id,
                latest_revision,
            } => CaptureProductListingRawObservationResult::Duplicate {
                product_listing_raw_stream_id,
                latest_revision,
            },
            ProductListingRawCaptureWriteOutcome::Stale {
                product_listing_raw_stream_id,
                latest_revision,
            } => CaptureProductListingRawObservationResult::Stale {
                product_listing_raw_stream_id,
                latest_revision,
            },
        })
    }
}

#[async_trait::async_trait]
impl<U, W, A> CaptureProductListingRawObservationUseCase
    for CaptureProductListingRawObservationHandler<U, W, A>
where
    U: UnitOfWork,
    W: ProductListingRawCaptureWriterFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "capture_product_listing_raw_observation",
        skip_all,
        fields(
            listing_source_id = %command.listing_source_id,
            ingestion_method = command.ingestion_method.as_str(),
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CaptureProductListingRawObservationCommand,
    ) -> Result<CaptureProductListingRawObservationResult, CaptureProductListingRawObservationError>
    {
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<CaptureProductListingRawObservationError>()?;
        let listing_source_id = command.listing_source_id;
        let ingestion_method = command.ingestion_method.as_str();
        let source_payload_bytes = command.input.source_payload().value().to_string().len();
        let raw_values_bytes = command.input.raw_values().value().to_string().len();
        let normalization_context_bytes = command
            .input
            .normalization_context()
            .value()
            .to_string()
            .len();
        let provenance_bytes = command.provenance.value().to_string().len();
        let started = Instant::now();
        let result = self.capture(context, command).await;

        match &result {
            Ok(CaptureProductListingRawObservationResult::Changed {
                product_listing_raw_stream_id,
                revision,
                ..
            }) => tracing::info!(
                metric = "product_listing_raw_capture",
                raw_capture_attempts = 1_u64,
                raw_revision_inserts = 1_u64,
                unchanged_captures = 0_u64,
                duplicate_captures = 0_u64,
                stale_captures = 0_u64,
                capture_latency_ms = started.elapsed().as_millis() as u64,
                source_payload_bytes,
                raw_values_bytes,
                normalization_context_bytes,
                provenance_bytes,
                listing_source_id = %listing_source_id,
                ingestion_method,
                product_listing_raw_stream_id = %product_listing_raw_stream_id.as_uuid(),
                revision,
                outcome = "changed",
                "raw product listing capture metric"
            ),
            Ok(CaptureProductListingRawObservationResult::Unchanged {
                product_listing_raw_stream_id,
                latest_revision,
            }) => tracing::info!(
                metric = "product_listing_raw_capture",
                raw_capture_attempts = 1_u64,
                raw_revision_inserts = 0_u64,
                unchanged_captures = 1_u64,
                duplicate_captures = 0_u64,
                stale_captures = 0_u64,
                capture_latency_ms = started.elapsed().as_millis() as u64,
                source_payload_bytes,
                raw_values_bytes,
                normalization_context_bytes,
                provenance_bytes,
                listing_source_id = %listing_source_id,
                ingestion_method,
                product_listing_raw_stream_id = %product_listing_raw_stream_id.as_uuid(),
                revision = latest_revision,
                outcome = "unchanged",
                "raw product listing capture metric"
            ),
            Ok(CaptureProductListingRawObservationResult::Duplicate {
                product_listing_raw_stream_id,
                latest_revision,
            }) => tracing::info!(
                metric = "product_listing_raw_capture",
                raw_capture_attempts = 1_u64,
                raw_revision_inserts = 0_u64,
                unchanged_captures = 0_u64,
                duplicate_captures = 1_u64,
                stale_captures = 0_u64,
                capture_latency_ms = started.elapsed().as_millis() as u64,
                source_payload_bytes,
                raw_values_bytes,
                normalization_context_bytes,
                provenance_bytes,
                listing_source_id = %listing_source_id,
                ingestion_method,
                product_listing_raw_stream_id = %product_listing_raw_stream_id.as_uuid(),
                revision = latest_revision,
                outcome = "duplicate",
                "raw product listing capture metric"
            ),
            Ok(CaptureProductListingRawObservationResult::Stale {
                product_listing_raw_stream_id,
                latest_revision,
            }) => tracing::info!(
                metric = "product_listing_raw_capture",
                raw_capture_attempts = 1_u64,
                raw_revision_inserts = 0_u64,
                unchanged_captures = 0_u64,
                duplicate_captures = 0_u64,
                stale_captures = 1_u64,
                capture_latency_ms = started.elapsed().as_millis() as u64,
                source_payload_bytes,
                raw_values_bytes,
                normalization_context_bytes,
                provenance_bytes,
                listing_source_id = %listing_source_id,
                ingestion_method,
                product_listing_raw_stream_id = %product_listing_raw_stream_id.as_uuid(),
                revision = latest_revision,
                outcome = "stale",
                "raw product listing capture metric"
            ),
            Err(error) => tracing::warn!(
                metric = "product_listing_raw_capture",
                raw_capture_attempts = 1_u64,
                raw_revision_inserts = 0_u64,
                unchanged_captures = 0_u64,
                duplicate_captures = 0_u64,
                stale_captures = 0_u64,
                capture_latency_ms = started.elapsed().as_millis() as u64,
                source_payload_bytes,
                raw_values_bytes,
                normalization_context_bytes,
                provenance_bytes,
                listing_source_id = %listing_source_id,
                ingestion_method,
                outcome = "failure",
                error_code = capture_error_code(error),
                "raw product listing capture metric"
            ),
        }

        result
    }
}

fn validate_source_record_key(
    source_record_key: &str,
) -> Result<(), CaptureProductListingRawObservationError> {
    if source_record_key.len() > MAX_SOURCE_RECORD_KEY_UTF8_BYTES {
        return Err(
            CaptureProductListingRawObservationError::SourceRecordKeyTooLong {
                len: source_record_key.len(),
                max: MAX_SOURCE_RECORD_KEY_UTF8_BYTES,
            },
        );
    }
    if source_record_key.contains('\0') {
        return Err(CaptureProductListingRawObservationError::SourceRecordKeyEmbeddedNul);
    }
    Ok(())
}

fn capture_error_code(error: &CaptureProductListingRawObservationError) -> &'static str {
    match error {
        CaptureProductListingRawObservationError::AuthenticatedActorRequired => {
            "AUTHENTICATED_ACTOR_REQUIRED"
        }
        CaptureProductListingRawObservationError::Forbidden => "FORBIDDEN",
        CaptureProductListingRawObservationError::SourceRecordKeyTooLong { .. } => {
            "SOURCE_RECORD_KEY_TOO_LONG"
        }
        CaptureProductListingRawObservationError::SourceRecordKeyEmbeddedNul => {
            "SOURCE_RECORD_KEY_EMBEDDED_NUL"
        }
        CaptureProductListingRawObservationError::InvalidInput { .. } => "INVALID_INPUT",
        CaptureProductListingRawObservationError::ListingSourceNotFound => {
            "LISTING_SOURCE_NOT_FOUND"
        }
        CaptureProductListingRawObservationError::PartnerAuthorizationTemporarilyUnavailable {
            ..
        } => "PARTNER_AUTHORIZATION_TEMPORARILY_UNAVAILABLE",
        CaptureProductListingRawObservationError::PartnerAuthorizationInternal { .. } => {
            "PARTNER_AUTHORIZATION_INTERNAL"
        }
        CaptureProductListingRawObservationError::SourceRecordKeyHashCollision => {
            "SOURCE_RECORD_KEY_HASH_COLLISION"
        }
        CaptureProductListingRawObservationError::ProviderReceiptDigestConflict => {
            "PROVIDER_RECEIPT_DIGEST_CONFLICT"
        }
        CaptureProductListingRawObservationError::ProviderSourceOrderConflict => {
            "PROVIDER_SOURCE_ORDER_CONFLICT"
        }
        CaptureProductListingRawObservationError::BeginTransactionFailed => {
            "BEGIN_TRANSACTION_FAILED"
        }
        CaptureProductListingRawObservationError::CaptureFailed { .. } => "CAPTURE_FAILED",
        CaptureProductListingRawObservationError::CommitTransactionFailed => {
            "COMMIT_TRANSACTION_FAILED"
        }
    }
}

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

impl From<OperationAuthorizationError> for CaptureProductListingRawObservationError {
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

impl From<NormalizationInputError> for CaptureProductListingRawObservationError {
    fn from(error: NormalizationInputError) -> Self {
        Self::InvalidInput {
            source: box_error(error),
        }
    }
}

impl From<PartnerProductListingAuthorizationError> for CaptureProductListingRawObservationError {
    fn from(error: PartnerProductListingAuthorizationError) -> Self {
        match error {
            PartnerProductListingAuthorizationError::ListingSourceNotFound => {
                Self::ListingSourceNotFound
            }
            PartnerProductListingAuthorizationError::Forbidden => Self::Forbidden,
            PartnerProductListingAuthorizationError::TemporarilyUnavailable { source } => {
                Self::PartnerAuthorizationTemporarilyUnavailable { source }
            }
            PartnerProductListingAuthorizationError::Internal { source } => {
                Self::PartnerAuthorizationInternal { source }
            }
        }
    }
}

impl From<ProductListingRawCaptureWriteError> for CaptureProductListingRawObservationError {
    fn from(error: ProductListingRawCaptureWriteError) -> Self {
        match error {
            ProductListingRawCaptureWriteError::SourceRecordKeyHashCollision => {
                Self::SourceRecordKeyHashCollision
            }
            ProductListingRawCaptureWriteError::ProviderReceiptDigestConflict => {
                Self::ProviderReceiptDigestConflict
            }
            ProductListingRawCaptureWriteError::ProviderSourceOrderConflict => {
                Self::ProviderSourceOrderConflict
            }
            ProductListingRawCaptureWriteError::CaptureFailed { source } => {
                Self::CaptureFailed { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use product_listing_normalization::{
        NormalizationContext, RawProductListingOperation, RawProductListingPayloadFormat,
        RawProductListingValues, SourcePayload,
    };
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn should_capture_and_commit_for_system_principal() {
        let committed = Arc::new(Mutex::new(false));
        let writes = Arc::new(Mutex::new(0_usize));
        let provider_receipts = Arc::new(Mutex::new(Vec::new()));
        let handler = CaptureProductListingRawObservationHandler::new(
            TestUnitOfWork(Arc::clone(&committed)),
            TestWriterFactory {
                writes: Arc::clone(&writes),
                provider_receipts: Arc::clone(&provider_receipts),
                outcome: TestCaptureOutcome::Changed,
            },
            TestAuthorizerFactory,
        );

        let result = handler.execute(&system_context(), command()).await;

        assert!(matches!(
            result,
            Ok(CaptureProductListingRawObservationResult::Changed { revision: 1, .. })
        ));
        assert!(*lock(&committed));
        assert_eq!(1, *lock(&writes));
        assert_eq!(1, lock(&provider_receipts).len());
        assert!(lock(&provider_receipts)[0].is_none());
    }

    #[tokio::test]
    async fn should_map_duplicate_and_stale_raw_capture_outcomes() {
        let duplicate = execute_with_outcome(TestCaptureOutcome::Duplicate).await;
        assert!(matches!(
            duplicate,
            Ok(CaptureProductListingRawObservationResult::Duplicate {
                latest_revision: 1,
                ..
            })
        ));

        let stale = execute_with_outcome(TestCaptureOutcome::Stale).await;
        assert!(matches!(
            stale,
            Ok(CaptureProductListingRawObservationResult::Stale {
                latest_revision: 1,
                ..
            })
        ));
    }

    #[test]
    fn should_map_provider_receipt_conflicts_to_stable_capture_errors() {
        let digest_conflict = CaptureProductListingRawObservationError::from(
            ProductListingRawCaptureWriteError::ProviderReceiptDigestConflict,
        );
        assert_eq!(
            "PROVIDER_RECEIPT_DIGEST_CONFLICT",
            capture_error_code(&digest_conflict)
        );
        assert!(matches!(
            digest_conflict,
            CaptureProductListingRawObservationError::ProviderReceiptDigestConflict
        ));

        let source_order_conflict = CaptureProductListingRawObservationError::from(
            ProductListingRawCaptureWriteError::ProviderSourceOrderConflict,
        );
        assert_eq!(
            "PROVIDER_SOURCE_ORDER_CONFLICT",
            capture_error_code(&source_order_conflict)
        );
        assert!(matches!(
            source_order_conflict,
            CaptureProductListingRawObservationError::ProviderSourceOrderConflict
        ));
    }

    #[tokio::test]
    async fn should_not_invoke_writer_when_source_record_key_contains_embedded_nul() {
        let committed = Arc::new(Mutex::new(false));
        let writes = Arc::new(Mutex::new(0_usize));
        let handler = CaptureProductListingRawObservationHandler::new(
            TestUnitOfWork(Arc::clone(&committed)),
            TestWriterFactory {
                writes: Arc::clone(&writes),
                provider_receipts: Arc::new(Mutex::new(Vec::new())),
                outcome: TestCaptureOutcome::Changed,
            },
            TestAuthorizerFactory,
        );
        let mut invalid_command = command();
        invalid_command.source_record_key = "valid\0invalid".to_owned();

        let result = handler.execute(&system_context(), invalid_command).await;

        assert!(matches!(
            result,
            Err(CaptureProductListingRawObservationError::SourceRecordKeyEmbeddedNul)
        ));
        assert!(!*lock(&committed));
        assert_eq!(0, *lock(&writes));
    }

    #[test]
    fn should_exclude_partner_api_from_raw_ingestion_methods() {
        assert_eq!(
            "WEB_CRAWL",
            ProductListingRawIngestionMethod::WebCrawl.as_str()
        );
        assert_eq!(
            "SHOPIFY",
            ProductListingRawIngestionMethod::Shopify.as_str()
        );
        assert_eq!(
            "WOOCOMMERCE",
            ProductListingRawIngestionMethod::Woocommerce.as_str()
        );
    }

    fn command() -> CaptureProductListingRawObservationCommand {
        let input = ProductListingNormalizationInput::new(
            RawProductListingOperation::Upsert,
            RawProductListingPayloadFormat::ShopifyProduct,
            1,
            1,
            SourcePayload::new(json!({"unknown": true}))
                .unwrap_or_else(|error| panic!("source payload: {error}")),
            RawProductListingValues::new(json!({"title": "Vase"}))
                .unwrap_or_else(|error| panic!("raw values: {error}")),
            NormalizationContext::new(json!({}))
                .unwrap_or_else(|error| panic!("normalization context: {error}")),
        )
        .unwrap_or_else(|error| panic!("normalization input: {error}"));
        CaptureProductListingRawObservationCommand {
            listing_source_id: ListingSourceId::new(),
            ingestion_method: ProductListingRawIngestionMethod::Shopify,
            source_record_key: "123".to_owned(),
            input,
            provenance: RawProductListingProvenance::new(json!({"deliveryId": "one"}))
                .unwrap_or_else(|error| panic!("provenance: {error}")),
            source_event_id: Some("one".to_owned()),
            source_occurred_at: None,
            provider_receipt: None,
        }
    }

    fn system_context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    async fn execute_with_outcome(
        outcome: TestCaptureOutcome,
    ) -> Result<CaptureProductListingRawObservationResult, CaptureProductListingRawObservationError>
    {
        let handler = CaptureProductListingRawObservationHandler::new(
            TestUnitOfWork(Arc::new(Mutex::new(false))),
            TestWriterFactory {
                writes: Arc::new(Mutex::new(0)),
                provider_receipts: Arc::new(Mutex::new(Vec::new())),
                outcome,
            },
            TestAuthorizerFactory,
        );

        handler.execute(&system_context(), command()).await
    }

    fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct TestTransaction(Arc<Mutex<bool>>);

    #[async_trait::async_trait]
    impl Transaction for TestTransaction {
        async fn commit(self) -> Result<(), application::transaction::TransactionError> {
            *lock(&self.0) = true;
            Ok(())
        }
    }

    struct TestUnitOfWork(Arc<Mutex<bool>>);

    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTransaction;

        async fn begin(&self) -> Result<Self::Tx, application::transaction::TransactionError> {
            Ok(TestTransaction(Arc::clone(&self.0)))
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum TestCaptureOutcome {
        Changed,
        Duplicate,
        Stale,
    }

    struct TestWriterFactory {
        writes: Arc<Mutex<usize>>,
        provider_receipts: Arc<Mutex<Vec<Option<ProductListingRawProviderReceipt>>>>,
        outcome: TestCaptureOutcome,
    }

    struct TestWriter<'a> {
        writes: &'a Mutex<usize>,
        provider_receipts: &'a Mutex<Vec<Option<ProductListingRawProviderReceipt>>>,
        outcome: TestCaptureOutcome,
    }

    impl ProductListingRawCaptureWriterFactory<TestTransaction> for TestWriterFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TestTransaction,
        ) -> impl ProductListingRawCaptureWriter + 'tx {
            TestWriter {
                writes: &self.writes,
                provider_receipts: &self.provider_receipts,
                outcome: self.outcome,
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRawCaptureWriter for TestWriter<'_> {
        async fn capture(
            &mut self,
            write: ProductListingRawCaptureWrite,
        ) -> Result<ProductListingRawCaptureWriteOutcome, ProductListingRawCaptureWriteError>
        {
            *lock(self.writes) += 1;
            lock(self.provider_receipts).push(write.provider_receipt);
            let product_listing_raw_stream_id =
                crate::ports::ProductListingRawStreamId::from_uuid(uuid::Uuid::new_v4());

            Ok(match self.outcome {
                TestCaptureOutcome::Changed => ProductListingRawCaptureWriteOutcome::Changed {
                    product_listing_raw_stream_id,
                    product_listing_raw_revision_id:
                        crate::ports::ProductListingRawRevisionId::from_uuid(uuid::Uuid::new_v4()),
                    revision: 1,
                },
                TestCaptureOutcome::Duplicate => ProductListingRawCaptureWriteOutcome::Duplicate {
                    product_listing_raw_stream_id,
                    latest_revision: 1,
                },
                TestCaptureOutcome::Stale => ProductListingRawCaptureWriteOutcome::Stale {
                    product_listing_raw_stream_id,
                    latest_revision: 1,
                },
            })
        }
    }

    struct TestAuthorizerFactory;

    struct TestAuthorizer;

    impl PartnerProductListingAuthorizerFactory<TestTransaction> for TestAuthorizerFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TestTransaction,
        ) -> impl PartnerProductListingAuthorizer + 'tx {
            TestAuthorizer
        }
    }

    #[async_trait::async_trait]
    impl PartnerProductListingAuthorizer for TestAuthorizer {
        async fn authorize(
            &mut self,
            _: UserId,
            _: ListingSourceId,
        ) -> Result<(), PartnerProductListingAuthorizationError> {
            Ok(())
        }
    }
}
