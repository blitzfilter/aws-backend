use application::operation_context::OperationContext;
use listing_source_core::ListingSourceId;
use listing_source_service::ports::{
    ListingSourceReadError, WoocommerceSignatureVerification, WoocommerceSignatureVerifier,
    WoocommerceSource, WoocommerceSourceReader,
};
use product_listing_normalization::{
    NormalizationContext, NormalizationInputError, ProductListingNormalizationContextV1,
    ProductListingNormalizationInput, ProductListingRawValuesPatch, ProductListingRawValuesV1,
    RawProductListingOperation, RawProductListingPayloadFormat, RawProductListingProvenance,
    RawProductListingValues, SourcePayload,
};
use product_listing_service::ports::ProductListingRawIngestionMethod;
use product_listing_service::use_cases::{
    CaptureProductListingRawObservationCommand, CaptureProductListingRawObservationError,
    CaptureProductListingRawObservationUseCase,
};
use serde::Deserialize;
use serde_json::{Value, json};

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
    pub(crate) listing_source_id: ListingSourceId,
    pub(crate) kind: WoocommerceProductEventKind,
    pub(crate) signature: Vec<u8>,
    pub(crate) raw_body: Vec<u8>,
    pub(crate) delivery_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WoocommerceWebhookIntakeError {
    #[error("WooCommerce product payload is malformed")]
    MalformedPayload(#[source] serde_json::Error),
    #[error("WooCommerce product source payload is invalid")]
    InvalidSourcePayload(#[source] NormalizationInputError),
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

pub struct WoocommerceWebhookIntake<S, V, C> {
    sources: S,
    signature_verifier: V,
    capture: C,
}

impl<S, V, C> WoocommerceWebhookIntake<S, V, C> {
    pub fn new(sources: S, signature_verifier: V, capture: C) -> Self {
        Self {
            sources,
            signature_verifier,
            capture,
        }
    }
}

#[async_trait::async_trait]
impl<S, V, C> WoocommerceWebhookIntakeUseCase for WoocommerceWebhookIntake<S, V, C>
where
    S: WoocommerceSourceReader,
    V: WoocommerceSignatureVerifier,
    C: CaptureProductListingRawObservationUseCase,
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: WoocommerceWebhookIntakeCommand,
    ) -> Result<(), WoocommerceWebhookIntakeError> {
        let source = self
            .sources
            .find_by_id(command.listing_source_id)
            .await
            .map_err(WoocommerceWebhookIntakeError::ListingSourceRead)?
            .ok_or(WoocommerceWebhookIntakeError::ListingSourceNotFound)?;

        match self
            .signature_verifier
            .verify(
                command.listing_source_id,
                &command.raw_body,
                &command.signature,
            )
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

        let payload: Value = serde_json::from_slice(&command.raw_body)
            .map_err(WoocommerceWebhookIntakeError::MalformedPayload)?;
        let observation = command.kind.raw_observation(&source, payload)?;
        let Some(observation) = observation else {
            return Ok(());
        };
        let provenance = RawProductListingProvenance::new(json!({
            "topic": command.kind.as_topic(),
            "deliveryId": command.delivery_id,
        }))
        .map_err(WoocommerceWebhookIntakeError::InvalidSourcePayload)?;

        self.capture
            .execute(
                context,
                CaptureProductListingRawObservationCommand {
                    listing_source_id: source.listing_source_id,
                    ingestion_method: ProductListingRawIngestionMethod::Woocommerce,
                    source_record_key: observation.source_record_key,
                    input: observation.input,
                    provenance,
                    source_event_id: command.delivery_id,
                    source_occurred_at: None,
                },
            )
            .await
            .map(|_| ())
            .map_err(WoocommerceWebhookIntakeError::Capture)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct WoocommerceRawObservation {
    source_record_key: String,
    input: ProductListingNormalizationInput,
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
            product_listing_normalization::PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V1,
            source_payload,
            raw_values,
            context,
        )
        .map_err(WoocommerceWebhookIntakeError::InvalidSourcePayload)?;

        Ok(Some(WoocommerceRawObservation {
            source_record_key: product.id.to_string(),
            input,
        }))
    }
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

    let values = ProductListingRawValuesV1 {
        source_listing_id: product.id.to_string(),
        title: ProductListingRawValuesPatch::Set(title.to_owned()),
        description: description_patch(product),
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
        auction_start: ProductListingRawValuesPatch::Unchanged,
        auction_end: ProductListingRawValuesPatch::Unchanged,
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
    use listing_source_core::ListingSourceId;
    use localization::Language;
    use money::Currency;

    fn source() -> WoocommerceSource {
        WoocommerceSource {
            listing_source_id: ListingSourceId::new(),
            currency: Some(Currency::Eur),
            language: Some(Language::En),
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
