use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use listing_source_core::ListingSourceId;
use listing_source_service::ports::{
    ListingSourceReadError, WoocommerceSignatureVerification, WoocommerceSignatureVerifier,
    WoocommerceSource, WoocommerceSourceReader,
};
use product_listing_normalization::{
    NormalizationContext, NormalizationInputError, ProductListingNormalizationContextV1,
    ProductListingNormalizationInput, ProductListingRawValues, ProductListingRawValuesPatch,
    ProductListingRawValuesPriceFormat, RawProductListingOperation, RawProductListingPayloadFormat,
    RawProductListingProvenance, RawProductListingValues, SourcePayload,
};
use product_listing_service::ports::{
    ProductListingRawIngestionMethod, ProductListingRawProviderReceipt,
    ProviderReceiptDeliveryIdError, ProviderReceiptScope, ProviderReceiptScopeError,
    SourceEvidenceSha256,
};
use product_listing_service::use_cases::{
    AuthorizeProductListingRawCaptureError, AuthorizeProductListingRawCaptureRequest,
    AuthorizeProductListingRawCaptureUseCase, CaptureProductListingRawObservationCommand,
    CaptureProductListingRawObservationError, CaptureProductListingRawObservationUseCase,
};
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const PAYLOAD_SCHEMA_VERSION: u16 = 1;
const DELETE_BASE_URL: &str = "https://woocommerce.invalid/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WoocommerceProductEventKind {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone)]
pub struct WoocommerceWebhookIntakeCommand {
    pub listing_source_id: ListingSourceId,
    pub kind: WoocommerceProductEventKind,
    pub signature: Vec<u8>,
    pub raw_body: Vec<u8>,
    pub delivery_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WoocommerceWebhookIntakeError {
    #[error("authenticated actor required to intake WooCommerce product input")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("WooCommerce product payload is malformed")]
    MalformedPayload(#[source] serde_json::Error),
    #[error("WooCommerce product source payload is invalid")]
    InvalidSourcePayload(#[source] NormalizationInputError),
    #[error("WooCommerce product modification timestamp is invalid")]
    InvalidSourceTimestamp,
    #[error("WooCommerce provider receipt scope is invalid")]
    InvalidProviderReceiptScope(#[source] ProviderReceiptScopeError),
    #[error("WooCommerce provider receipt delivery ID is invalid")]
    InvalidProviderReceiptDeliveryId(#[source] ProviderReceiptDeliveryIdError),
    #[error("WooCommerce product title is missing")]
    MissingTitle,
    #[error("WooCommerce product URL is missing")]
    MissingUrl,
    #[error("listing source has no WooCommerce currency configured")]
    MissingListingSourceCurrency,
    #[error("listing source has no WooCommerce language configured")]
    MissingListingSourceLanguage,
    #[error("listing source not found")]
    ListingSourceNotFound,
    #[error("WooCommerce webhook secret is not configured")]
    WebhookSecretNotConfigured,
    #[error("WooCommerce webhook signature is invalid")]
    InvalidSignature,
    #[error("WooCommerce listing source lookup failed")]
    ListingSourceRead(#[source] ListingSourceReadError),
    #[error("WooCommerce raw product capture authorization failed")]
    Authorize(#[source] AuthorizeProductListingRawCaptureError),
    #[error("WooCommerce raw product capture failed")]
    Capture(#[source] CaptureProductListingRawObservationError),
}

#[async_trait::async_trait]
pub trait WoocommerceWebhookIntakeUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: WoocommerceWebhookIntakeCommand,
    ) -> Result<(), WoocommerceWebhookIntakeError>;
}

pub struct WoocommerceWebhookIntake<S, V, C, A> {
    sources: S,
    signature_verifier: V,
    capture: C,
    authorize: A,
}

impl<S, V, C, A> WoocommerceWebhookIntake<S, V, C, A> {
    pub fn new(sources: S, signature_verifier: V, capture: C, authorize: A) -> Self {
        Self {
            sources,
            signature_verifier,
            capture,
            authorize,
        }
    }
}

#[async_trait::async_trait]
impl<S, V, C, A> WoocommerceWebhookIntakeUseCase for WoocommerceWebhookIntake<S, V, C, A>
where
    S: WoocommerceSourceReader,
    V: WoocommerceSignatureVerifier,
    C: CaptureProductListingRawObservationUseCase,
    A: AuthorizeProductListingRawCaptureUseCase,
{
    #[tracing::instrument(
        name = "intake_woocommerce_webhook",
        skip_all,
        fields(
            listing_source_id = %command.listing_source_id,
            topic = command.kind.as_topic(),
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: WoocommerceWebhookIntakeCommand,
    ) -> Result<(), WoocommerceWebhookIntakeError> {
        require_product_listings_write(context)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let WoocommerceWebhookIntakeCommand {
            listing_source_id,
            kind,
            signature,
            raw_body,
            delivery_id,
        } = command;
        let source = self
            .sources
            .find_by_id(listing_source_id)
            .await
            .map_err(WoocommerceWebhookIntakeError::ListingSourceRead)?
            .ok_or(WoocommerceWebhookIntakeError::ListingSourceNotFound)?;

        match self
            .signature_verifier
            .verify(listing_source_id, &raw_body, &signature)
            .await
            .map_err(WoocommerceWebhookIntakeError::ListingSourceRead)?
        {
            WoocommerceSignatureVerification::Valid => {}
            WoocommerceSignatureVerification::Invalid => {
                return Err(WoocommerceWebhookIntakeError::InvalidSignature);
            }
            WoocommerceSignatureVerification::SecretNotConfigured => {
                return Err(WoocommerceWebhookIntakeError::WebhookSecretNotConfigured);
            }
        }

        let payload: Value = serde_json::from_slice(&raw_body)
            .map_err(WoocommerceWebhookIntakeError::MalformedPayload)?;
        let observation = kind.raw_observation(&source, payload)?;
        let Some(observation) = observation else {
            return self
                .authorize
                .execute(
                    context,
                    AuthorizeProductListingRawCaptureRequest {
                        listing_source_id: source.listing_source_id,
                    },
                )
                .await
                .map(|_| ())
                .map_err(WoocommerceWebhookIntakeError::Authorize);
        };
        let WoocommerceRawObservation {
            source_record_key,
            input,
            source_occurred_at,
        } = observation;
        let provider_receipt =
            provider_receipt(kind, delivery_id.as_deref(), input.source_payload())?;
        let provenance = RawProductListingProvenance::new(json!({
            "topic": kind.as_topic(),
            "deliveryId": delivery_id.as_deref(),
        }))
        .map_err(WoocommerceWebhookIntakeError::InvalidSourcePayload)?;

        self.capture
            .execute(
                context,
                CaptureProductListingRawObservationCommand {
                    listing_source_id: source.listing_source_id,
                    ingestion_method: ProductListingRawIngestionMethod::Woocommerce,
                    source_record_key,
                    input,
                    provenance,
                    source_event_id: delivery_id,
                    source_occurred_at,
                    provider_receipt,
                },
            )
            .await
            .map(|_| ())
            .map_err(WoocommerceWebhookIntakeError::Capture)
    }
}

fn require_product_listings_write(
    context: &OperationContext,
) -> Result<(), WoocommerceWebhookIntakeError> {
    context
        .require()
        .credential_capability(CredentialCapability::ProductListingsWrite)
        .authorize::<WoocommerceWebhookIntakeError>()?;
    Ok(())
}

impl From<OperationAuthorizationError> for WoocommerceWebhookIntakeError {
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

#[derive(Debug, Clone, PartialEq)]
struct WoocommerceRawObservation {
    source_record_key: String,
    input: ProductListingNormalizationInput,
    source_occurred_at: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
struct WoocommerceProductPayload {
    id: u64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    permalink: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    short_description: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    stock_status: Option<String>,
    #[serde(default)]
    date_modified_gmt: Option<String>,
    #[serde(default)]
    images: Vec<WoocommerceImagePayload>,
}

#[derive(Debug, Deserialize)]
struct WoocommerceImagePayload {
    src: String,
}

impl WoocommerceProductEventKind {
    pub(crate) const fn as_topic(self) -> &'static str {
        match self {
            Self::Create => "product.created",
            Self::Update => "product.updated",
            Self::Delete => "product.deleted",
        }
    }

    /// Maps WooCommerce provider vocabulary to the generic persisted raw-input contract.
    /// Unknown source object keys remain untouched in `source_payload`.
    fn raw_observation(
        self,
        source: &WoocommerceSource,
        payload: Value,
    ) -> Result<Option<WoocommerceRawObservation>, WoocommerceWebhookIntakeError> {
        let source_payload = SourcePayload::new(payload.clone())
            .map_err(WoocommerceWebhookIntakeError::InvalidSourcePayload)?;
        let product = serde_json::from_value::<WoocommerceProductPayload>(payload)
            .map_err(WoocommerceWebhookIntakeError::MalformedPayload)?;
        let operation = match self {
            Self::Delete => Some(RawProductListingOperation::Delete),
            Self::Create | Self::Update => match product.status.as_deref() {
                Some("publish") => Some(RawProductListingOperation::Upsert),
                Some("trash" | "draft" | "pending" | "private") => {
                    Some(RawProductListingOperation::Delete)
                }
                Some(_) | None => None,
            },
        };
        let Some(operation) = operation else {
            return Ok(None);
        };
        let source_occurred_at = parse_source_occurred_at(product.date_modified_gmt.as_deref())?;

        let raw_values = match operation {
            RawProductListingOperation::Upsert => upsert_raw_values(source, &product)?,
            RawProductListingOperation::Delete => RawProductListingValues::new(json!({}))
                .map_err(WoocommerceWebhookIntakeError::InvalidSourcePayload)?,
        };
        let context = normalization_context(source, product.permalink.as_deref(), operation)?;
        let input = ProductListingNormalizationInput::new(
            operation,
            RawProductListingPayloadFormat::WoocommerceProduct,
            PAYLOAD_SCHEMA_VERSION,
            product_listing_normalization::PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            source_payload,
            raw_values,
            context,
        )
        .map_err(WoocommerceWebhookIntakeError::InvalidSourcePayload)?;

        Ok(Some(WoocommerceRawObservation {
            source_record_key: product.id.to_string(),
            input,
            source_occurred_at,
        }))
    }
}

fn provider_receipt(
    kind: WoocommerceProductEventKind,
    delivery_id: Option<&str>,
    source_payload: &SourcePayload,
) -> Result<Option<ProductListingRawProviderReceipt>, WoocommerceWebhookIntakeError> {
    let Some(delivery_id) = delivery_id else {
        return Ok(None);
    };
    let scope = ProviderReceiptScope::new(kind.as_topic().to_owned())
        .map_err(WoocommerceWebhookIntakeError::InvalidProviderReceiptScope)?;
    let source_evidence_sha256 = source_payload
        .canonical_sha256()
        .map_err(WoocommerceWebhookIntakeError::InvalidSourcePayload)?;
    let receipt = ProductListingRawProviderReceipt::new(
        scope,
        delivery_id.to_owned(),
        SourceEvidenceSha256::new(*source_evidence_sha256.as_bytes()),
    )
    .map_err(WoocommerceWebhookIntakeError::InvalidProviderReceiptDeliveryId)?;

    Ok(Some(receipt))
}

fn parse_source_occurred_at(
    date_modified_gmt: Option<&str>,
) -> Result<Option<OffsetDateTime>, WoocommerceWebhookIntakeError> {
    let Some(value) = date_modified_gmt else {
        return Ok(None);
    };

    let parsed = if value.ends_with('Z') || value.ends_with("+00:00") {
        OffsetDateTime::parse(value, &Rfc3339)
    } else if OffsetDateTime::parse(value, &Rfc3339).is_ok() {
        return Err(WoocommerceWebhookIntakeError::InvalidSourceTimestamp);
    } else {
        OffsetDateTime::parse(&format!("{value}Z"), &Rfc3339)
    };

    parsed
        .map(Some)
        .map_err(|_| WoocommerceWebhookIntakeError::InvalidSourceTimestamp)
}

fn upsert_raw_values(
    source: &WoocommerceSource,
    product: &WoocommerceProductPayload,
) -> Result<RawProductListingValues, WoocommerceWebhookIntakeError> {
    let title = product
        .name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(WoocommerceWebhookIntakeError::MissingTitle)?;
    let permalink = product
        .permalink
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(WoocommerceWebhookIntakeError::MissingUrl)?;
    let _ = source
        .language
        .ok_or(WoocommerceWebhookIntakeError::MissingListingSourceLanguage)?;
    if product
        .price
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        && source.currency.is_none()
    {
        return Err(WoocommerceWebhookIntakeError::MissingListingSourceCurrency);
    }

    let values = ProductListingRawValues {
        source_listing_id: product.id.to_string(),
        title: ProductListingRawValuesPatch::Set(title.to_owned()),
        description: description_patch(product),
        price_format: ProductListingRawValuesPriceFormat::MachineDecimal,
        price: string_patch(product.price.clone()),
        price_estimate_min: ProductListingRawValuesPatch::Unchanged,
        price_estimate_max: ProductListingRawValuesPatch::Unchanged,
        availability: availability_patch(product.stock_status.as_deref()),
        url: ProductListingRawValuesPatch::Set(permalink.to_owned()),
        images: ProductListingRawValuesPatch::Set(
            product
                .images
                .iter()
                .map(|image| image.src.clone())
                .collect(),
        ),
        auction: ProductListingRawValuesPatch::Unchanged,
        attributes: Default::default(),
    };
    serde_json::to_value(values)
        .map_err(NormalizationInputError::JsonSerialization)
        .and_then(RawProductListingValues::new)
        .map_err(WoocommerceWebhookIntakeError::InvalidSourcePayload)
}

fn normalization_context(
    source: &WoocommerceSource,
    permalink: Option<&str>,
    operation: RawProductListingOperation,
) -> Result<NormalizationContext, WoocommerceWebhookIntakeError> {
    let base_url = match operation {
        RawProductListingOperation::Upsert => permalink
            .filter(|value| !value.trim().is_empty())
            .ok_or(WoocommerceWebhookIntakeError::MissingUrl)?,
        RawProductListingOperation::Delete => DELETE_BASE_URL,
    };
    let context = ProductListingNormalizationContextV1 {
        base_url: base_url.to_owned(),
        fallback_currency: source.currency.map(|currency| currency.as_str().to_owned()),
        fallback_language: source.language.map(|language| language.as_str().to_owned()),
    };
    serde_json::to_value(context)
        .map_err(NormalizationInputError::JsonSerialization)
        .and_then(NormalizationContext::new)
        .map_err(WoocommerceWebhookIntakeError::InvalidSourcePayload)
}

fn description_patch(
    product: &WoocommerceProductPayload,
) -> ProductListingRawValuesPatch<Vec<String>> {
    product
        .description
        .as_deref()
        .or(product.short_description.as_deref())
        .map(fallbacked_html_to_markdown)
        .filter(|value| !value.is_empty())
        .map(|value| ProductListingRawValuesPatch::Set(vec![value]))
        .unwrap_or(ProductListingRawValuesPatch::Clear)
}

fn string_patch(value: Option<String>) -> ProductListingRawValuesPatch<String> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(ProductListingRawValuesPatch::Set)
        .unwrap_or(ProductListingRawValuesPatch::Clear)
}

fn availability_patch(value: Option<&str>) -> ProductListingRawValuesPatch<String> {
    match value {
        Some("instock") => ProductListingRawValuesPatch::Set("in stock".to_owned()),
        Some("outofstock") => ProductListingRawValuesPatch::Set("out of stock".to_owned()),
        Some("onbackorder") => {
            ProductListingRawValuesPatch::Set("https://schema.org/BackOrder".to_owned())
        }
        Some(_) | None => ProductListingRawValuesPatch::Unchanged,
    }
}

fn fallbacked_html_to_markdown(html: &str) -> String {
    match html_to_markdown_rs::convert(html, None) {
        Ok(result) => result.content.unwrap_or_else(|| html.to_owned()),
        Err(_) => html.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{
        CorrelationId, CredentialCapability, Principal, RequestId,
    };
    use listing_source_core::ListingSourceId;
    use localization::Language;
    use money::Currency;
    use std::{
        collections::BTreeSet,
        sync::{Arc, Mutex},
    };
    use user_core::user_id::UserId;

    fn source() -> WoocommerceSource {
        WoocommerceSource {
            listing_source_id: ListingSourceId::new(),
            currency: Some(Currency::Eur),
            language: Some(Language::En),
        }
    }

    struct TestSourceReader {
        source: WoocommerceSource,
        calls: Arc<Mutex<usize>>,
    }

    #[async_trait::async_trait]
    impl WoocommerceSourceReader for TestSourceReader {
        async fn find_by_id(
            &self,
            _: ListingSourceId,
        ) -> Result<Option<WoocommerceSource>, ListingSourceReadError> {
            *self
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) += 1;
            Ok(Some(self.source.clone()))
        }
    }

    type SignatureVerificationCalls = Arc<Mutex<Vec<(Vec<u8>, Vec<u8>)>>>;

    struct RecordingVerifier {
        calls: SignatureVerificationCalls,
    }

    #[async_trait::async_trait]
    impl WoocommerceSignatureVerifier for RecordingVerifier {
        async fn verify(
            &self,
            _: ListingSourceId,
            body: &[u8],
            signature: &[u8],
        ) -> Result<WoocommerceSignatureVerification, ListingSourceReadError> {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((body.to_vec(), signature.to_vec()));
            Ok(WoocommerceSignatureVerification::Valid)
        }
    }

    struct RecordingCapture {
        commands: Arc<Mutex<Vec<CaptureProductListingRawObservationCommand>>>,
    }

    #[async_trait::async_trait]
    impl CaptureProductListingRawObservationUseCase for RecordingCapture {
        async fn execute(
            &self,
            _: &OperationContext,
            command: CaptureProductListingRawObservationCommand,
        ) -> Result<
            product_listing_service::use_cases::CaptureProductListingRawObservationResult,
            CaptureProductListingRawObservationError,
        > {
            self.commands
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(command);
            Ok(
                product_listing_service::use_cases::CaptureProductListingRawObservationResult::Unchanged {
                    product_listing_raw_stream_id:
                        product_listing_service::ports::ProductListingRawStreamId::new(),
                    latest_revision: 0,
                },
            )
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum AuthorizationOutcome {
        Allowed,
        Forbidden,
    }

    struct RecordingAuthorization {
        requests: Arc<Mutex<Vec<AuthorizeProductListingRawCaptureRequest>>>,
        outcome: AuthorizationOutcome,
    }

    #[async_trait::async_trait]
    impl AuthorizeProductListingRawCaptureUseCase for RecordingAuthorization {
        async fn execute(
            &self,
            _: &OperationContext,
            request: AuthorizeProductListingRawCaptureRequest,
        ) -> Result<
            product_listing_service::use_cases::AuthorizeProductListingRawCaptureResult,
            AuthorizeProductListingRawCaptureError,
        > {
            self.requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(request);
            match self.outcome {
                AuthorizationOutcome::Allowed => {
                    Ok(product_listing_service::use_cases::AuthorizeProductListingRawCaptureResult)
                }
                AuthorizationOutcome::Forbidden => {
                    Err(AuthorizeProductListingRawCaptureError::Forbidden)
                }
            }
        }
    }

    fn system_context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("woocommerce-intake-test"),
            correlation_id: CorrelationId::new("woocommerce-intake-test"),
        }
    }

    fn delegated_context(capabilities: BTreeSet<CredentialCapability>) -> OperationContext {
        OperationContext {
            principal: Principal::DelegatedUser {
                user_id: UserId::new(),
                capabilities,
            },
            request_id: RequestId::new("woocommerce-intake-test"),
            correlation_id: CorrelationId::new("woocommerce-intake-test"),
        }
    }

    fn published_product(stock_status: Option<&str>) -> Value {
        json!({
            "id": 42,
            "name": "Cabinet",
            "permalink": "https://partner.example/products/cabinet",
            "description": "<p>Cabinet description</p>",
            "price": "42.00",
            "status": "publish",
            "stock_status": stock_status,
            "images": [{"src": "https://images.example/cabinet.jpg"}],
            "futureWooKey": {"nested": true}
        })
    }

    #[tokio::test]
    async fn should_reject_missing_product_listings_write_before_provider_work() {
        let source = source();
        let listing_source_id = source.listing_source_id;
        let source_calls = Arc::new(Mutex::new(0));
        let verifier_calls = Arc::new(Mutex::new(Vec::new()));
        let capture_commands = Arc::new(Mutex::new(Vec::new()));
        let authorization_requests = Arc::new(Mutex::new(Vec::new()));
        let intake = WoocommerceWebhookIntake::new(
            TestSourceReader {
                source,
                calls: Arc::clone(&source_calls),
            },
            RecordingVerifier {
                calls: Arc::clone(&verifier_calls),
            },
            RecordingCapture {
                commands: Arc::clone(&capture_commands),
            },
            RecordingAuthorization {
                requests: Arc::clone(&authorization_requests),
                outcome: AuthorizationOutcome::Allowed,
            },
        );

        let result = intake
            .execute(
                &delegated_context(BTreeSet::new()),
                WoocommerceWebhookIntakeCommand {
                    listing_source_id,
                    kind: WoocommerceProductEventKind::Update,
                    signature: b"signature".to_vec(),
                    raw_body: b"not-json".to_vec(),
                    delivery_id: None,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(WoocommerceWebhookIntakeError::Forbidden)
        ));
        assert_eq!(
            0,
            *source_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        );
        assert!(
            verifier_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
        assert!(
            capture_commands
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
        assert!(
            authorization_requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
    }

    #[tokio::test]
    async fn should_preserve_raw_signature_bytes_and_map_receipt_timestamp_to_capture()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = source();
        let listing_source_id = source.listing_source_id;
        let source_calls = Arc::new(Mutex::new(0));
        let verifier_calls = Arc::new(Mutex::new(Vec::new()));
        let capture_commands = Arc::new(Mutex::new(Vec::new()));
        let authorization_requests = Arc::new(Mutex::new(Vec::new()));
        let intake = WoocommerceWebhookIntake::new(
            TestSourceReader {
                source,
                calls: Arc::clone(&source_calls),
            },
            RecordingVerifier {
                calls: Arc::clone(&verifier_calls),
            },
            RecordingCapture {
                commands: Arc::clone(&capture_commands),
            },
            RecordingAuthorization {
                requests: Arc::clone(&authorization_requests),
                outcome: AuthorizationOutcome::Allowed,
            },
        );
        let raw_body = br#"{
            "id": 42,
            "name": "Cabinet",
            "permalink": "https://partner.example/products/cabinet",
            "status": "publish",
            "stock_status": "instock",
            "images": [],
            "date_modified_gmt": "2026-09-06T12:34:56",
            "futureWooKey": {"nested": true}
        }"#
        .to_vec();
        let expected_source_evidence_sha256 =
            *SourcePayload::new(serde_json::from_slice(&raw_body)?)?
                .canonical_sha256()?
                .as_bytes();

        intake
            .execute(
                &system_context(),
                WoocommerceWebhookIntakeCommand {
                    listing_source_id,
                    kind: WoocommerceProductEventKind::Update,
                    signature: b"signature".to_vec(),
                    raw_body: raw_body.clone(),
                    delivery_id: Some("delivery-42".to_owned()),
                },
            )
            .await?;

        assert_eq!(
            1,
            *source_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        );
        assert_eq!(
            vec![(raw_body, b"signature".to_vec())],
            *verifier_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        );
        assert!(
            authorization_requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
        let commands = capture_commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(1, commands.len());
        let command = commands
            .first()
            .ok_or_else(|| std::io::Error::other("capture command is missing"))?;
        let receipt = command
            .provider_receipt
            .as_ref()
            .ok_or_else(|| std::io::Error::other("provider receipt is missing"))?;
        assert_eq!("product.updated", receipt.scope().as_str());
        assert_eq!("delivery-42", receipt.delivery_id());
        assert_eq!(
            &expected_source_evidence_sha256,
            receipt.source_evidence_sha256().as_bytes()
        );
        assert_eq!(
            Some(OffsetDateTime::parse("2026-09-06T12:34:56Z", &Rfc3339)?),
            command.source_occurred_at
        );
        Ok(())
    }

    #[test]
    fn should_scope_woocommerce_receipts_by_topic_and_omit_absent_delivery_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let source_payload = SourcePayload::new(json!({"id": 42}))?;
        assert!(
            provider_receipt(WoocommerceProductEventKind::Create, None, &source_payload)?.is_none()
        );

        for (kind, topic) in [
            (WoocommerceProductEventKind::Create, "product.created"),
            (WoocommerceProductEventKind::Update, "product.updated"),
            (WoocommerceProductEventKind::Delete, "product.deleted"),
        ] {
            let receipt = provider_receipt(kind, Some("delivery-42"), &source_payload)?
                .ok_or_else(|| std::io::Error::other("provider receipt is missing"))?;
            assert_eq!(topic, receipt.scope().as_str());
        }

        Ok(())
    }

    #[test]
    fn should_accept_no_offset_and_explicit_utc_woocommerce_modified_gmt_timestamps()
    -> Result<(), Box<dyn std::error::Error>> {
        for value in [
            "2026-09-06T12:34:56",
            "2026-09-06T12:34:56Z",
            "2026-09-06T12:34:56+00:00",
        ] {
            assert_eq!(
                Some(OffsetDateTime::parse("2026-09-06T12:34:56Z", &Rfc3339)?),
                parse_source_occurred_at(Some(value))?
            );
        }

        Ok(())
    }

    #[test]
    fn should_reject_non_utc_woocommerce_modified_gmt_offsets() {
        for value in [
            "2026-09-06T12:34:56+00:01",
            "2026-09-06T12:34:56-00:01",
            "2026-09-06T12:34:56+01:00",
            "2026-09-06T12:34:56-01:00",
            "2026-09-06T12:34:56-00:00",
        ] {
            assert!(
                matches!(
                    parse_source_occurred_at(Some(value)),
                    Err(WoocommerceWebhookIntakeError::InvalidSourceTimestamp)
                ),
                "expected {value} to be rejected"
            );
        }
    }

    #[test]
    fn should_reject_invalid_woocommerce_modified_gmt_timestamp() {
        let mut payload = published_product(Some("instock"));
        payload["date_modified_gmt"] = json!("not-a-gmt-timestamp");

        assert!(matches!(
            WoocommerceProductEventKind::Update.raw_observation(&source(), payload),
            Err(WoocommerceWebhookIntakeError::InvalidSourceTimestamp)
        ));
    }

    #[tokio::test]
    async fn should_authorize_ignored_update_without_capturing_or_parsing_timestamp()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = source();
        let listing_source_id = source.listing_source_id;
        let source_calls = Arc::new(Mutex::new(0));
        let verifier_calls = Arc::new(Mutex::new(Vec::new()));
        let capture_commands = Arc::new(Mutex::new(Vec::new()));
        let authorization_requests = Arc::new(Mutex::new(Vec::new()));
        let intake = WoocommerceWebhookIntake::new(
            TestSourceReader {
                source,
                calls: Arc::clone(&source_calls),
            },
            RecordingVerifier {
                calls: Arc::clone(&verifier_calls),
            },
            RecordingCapture {
                commands: Arc::clone(&capture_commands),
            },
            RecordingAuthorization {
                requests: Arc::clone(&authorization_requests),
                outcome: AuthorizationOutcome::Allowed,
            },
        );
        let raw_body = br#"{"id":42,"name":"Cabinet","permalink":"https://partner.example/products/cabinet","date_modified_gmt":"not-a-gmt-timestamp"}"#.to_vec();

        intake
            .execute(
                &delegated_context(BTreeSet::from([CredentialCapability::ProductListingsWrite])),
                WoocommerceWebhookIntakeCommand {
                    listing_source_id,
                    kind: WoocommerceProductEventKind::Update,
                    signature: b"signature".to_vec(),
                    raw_body,
                    delivery_id: None,
                },
            )
            .await?;

        assert_eq!(
            1,
            *source_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        );
        assert_eq!(
            1,
            verifier_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len()
        );
        assert_eq!(
            vec![AuthorizeProductListingRawCaptureRequest { listing_source_id }],
            *authorization_requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        );
        assert!(
            capture_commands
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_ignored_update_when_source_authorization_is_denied() {
        let source = source();
        let listing_source_id = source.listing_source_id;
        let source_calls = Arc::new(Mutex::new(0));
        let verifier_calls = Arc::new(Mutex::new(Vec::new()));
        let capture_commands = Arc::new(Mutex::new(Vec::new()));
        let authorization_requests = Arc::new(Mutex::new(Vec::new()));
        let intake = WoocommerceWebhookIntake::new(
            TestSourceReader {
                source,
                calls: Arc::clone(&source_calls),
            },
            RecordingVerifier {
                calls: Arc::clone(&verifier_calls),
            },
            RecordingCapture {
                commands: Arc::clone(&capture_commands),
            },
            RecordingAuthorization {
                requests: Arc::clone(&authorization_requests),
                outcome: AuthorizationOutcome::Forbidden,
            },
        );

        let result = intake
            .execute(
                &delegated_context(BTreeSet::from([CredentialCapability::ProductListingsWrite])),
                WoocommerceWebhookIntakeCommand {
                    listing_source_id,
                    kind: WoocommerceProductEventKind::Update,
                    signature: b"signature".to_vec(),
                    raw_body: br#"{"id":42,"status":"future-status"}"#.to_vec(),
                    delivery_id: None,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(WoocommerceWebhookIntakeError::Authorize(
                AuthorizeProductListingRawCaptureError::Forbidden
            ))
        ));
        assert_eq!(
            1,
            *source_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        );
        assert_eq!(
            1,
            verifier_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len()
        );
        assert_eq!(
            vec![AuthorizeProductListingRawCaptureRequest { listing_source_id }],
            *authorization_requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        );
        assert!(
            capture_commands
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
    }

    #[test]
    fn should_map_published_product_to_generic_raw_values_and_context() {
        let observation = WoocommerceProductEventKind::Create
            .raw_observation(&source(), published_product(Some("instock")))
            .unwrap_or_else(|error| panic!("mapping failed: {error}"));
        let observation = observation.unwrap_or_else(|| panic!("published product must capture"));

        assert_eq!(observation.source_record_key, "42");
        assert_eq!(
            observation.input.operation(),
            RawProductListingOperation::Upsert
        );
        assert_eq!(
            product_listing_normalization::PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            observation.input.raw_values_schema_version()
        );
        assert_eq!(
            observation.input.raw_values().value()["priceFormat"],
            json!("MACHINE_DECIMAL")
        );
        assert_eq!(
            observation.input.raw_values().value()["availability"],
            json!({"action": "SET", "value": "in stock"})
        );
        assert_eq!(
            observation.input.normalization_context().value()["fallbackCurrency"],
            json!("EUR")
        );
        assert_eq!(
            observation.input.normalization_context().value()["fallbackLanguage"],
            json!("en")
        );
        assert_eq!(
            observation.input.source_payload().value()["futureWooKey"]["nested"],
            json!(true)
        );
    }

    #[test]
    fn should_map_delete_and_non_published_statuses_to_raw_delete() {
        for (kind, status) in [
            (WoocommerceProductEventKind::Delete, Some("publish")),
            (WoocommerceProductEventKind::Update, Some("trash")),
            (WoocommerceProductEventKind::Update, Some("draft")),
            (WoocommerceProductEventKind::Update, Some("pending")),
            (WoocommerceProductEventKind::Update, Some("private")),
        ] {
            let mut payload = published_product(Some("instock"));
            payload["status"] = status.map_or(Value::Null, |value| json!(value));
            let observation = kind
                .raw_observation(&source(), payload)
                .unwrap_or_else(|error| panic!("mapping failed: {error}"));
            assert!(matches!(
                observation,
                Some(WoocommerceRawObservation { input, .. })
                    if input.operation() == RawProductListingOperation::Delete
                        && input.raw_values_schema_version()
                            == product_listing_normalization::PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION
            ));
        }
    }

    #[test]
    fn should_ignore_missing_or_unsupported_status_without_capture() {
        for status in [None, Some("future-status")] {
            let mut payload = published_product(Some("instock"));
            payload["status"] = status.map_or(Value::Null, |value| json!(value));
            assert!(matches!(
                WoocommerceProductEventKind::Update.raw_observation(&source(), payload),
                Ok(None)
            ));
        }
    }

    #[test]
    fn should_map_supported_and_unsupported_stock_statuses_to_explicit_generic_intent() {
        assert_eq!(
            ProductListingRawValuesPatch::Set("in stock".to_owned()),
            availability_patch(Some("instock"))
        );
        assert_eq!(
            ProductListingRawValuesPatch::Set("out of stock".to_owned()),
            availability_patch(Some("outofstock"))
        );
        assert_eq!(
            ProductListingRawValuesPatch::Set("https://schema.org/BackOrder".to_owned()),
            availability_patch(Some("onbackorder"))
        );
        assert_eq!(
            ProductListingRawValuesPatch::Unchanged,
            availability_patch(Some("unsupported"))
        );
        assert_eq!(
            ProductListingRawValuesPatch::Unchanged,
            availability_patch(None)
        );
    }
}
