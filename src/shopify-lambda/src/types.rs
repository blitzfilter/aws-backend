use listing_source_service::ports::ShopifySource;
use product_listing_normalization::{
    NormalizationContext, NormalizationInputError, ProductListingNormalizationContextV1,
    ProductListingNormalizationInput, ProductListingRawValues, ProductListingRawValuesPatch,
    ProductListingRawValuesPriceFormat, RawProductListingOperation, RawProductListingPayloadFormat,
    RawProductListingValues, SourcePayload,
};
use serde::Deserialize;
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const PAYLOAD_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Deserialize)]
pub struct ShopifyEventDetail {
    pub payload: Value,
    pub metadata: ShopifyEventMetadata,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShopifyEventMetadata {
    #[serde(rename = "X-Shopify-Topic")]
    pub topic: String,
    #[serde(rename = "X-Shopify-Shop-Domain")]
    pub shop_domain: String,
    #[serde(rename = "X-Shopify-Event-Id", default)]
    pub event_id: Option<String>,
    #[serde(rename = "X-Shopify-Webhook-Id", default)]
    pub webhook_id: Option<String>,
    #[serde(rename = "X-Shopify-Triggered-At", default)]
    pub triggered_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShopifyProductPayload {
    pub id: u64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body_html: Option<String>,
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub variants: Vec<ShopifyVariantPayload>,
    #[serde(default)]
    pub images: Vec<ShopifyImagePayload>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShopifyVariantPayload {
    #[serde(default)]
    pub price: Option<String>,
    #[serde(default)]
    pub inventory_quantity: Option<i64>,
    #[serde(default)]
    pub inventory_management: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShopifyImagePayload {
    pub src: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopifyProductEventKind {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShopifyListingAction {
    Capture(ShopifyRawObservation),
    Ignore,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShopifyRawObservation {
    pub source_record_key: String,
    pub input: ProductListingNormalizationInput,
    pub source_occurred_at: Option<OffsetDateTime>,
}

#[derive(Debug, thiserror::Error)]
pub enum ShopifyProductEventError {
    #[error("Shopify product payload is malformed")]
    MalformedPayload(#[source] serde_json::Error),
    #[error("Shopify product source payload is invalid")]
    InvalidSourcePayload(#[source] NormalizationInputError),
    #[error("Shopify product title is missing")]
    MissingTitle,
    #[error("Shopify product handle is missing")]
    MissingHandle,
    #[error("Shopify listing source currency is missing for a nonblank product price")]
    MissingListingSourceCurrency,
    #[error("Shopify product updated_at is invalid")]
    InvalidUpdatedAt(#[source] time::error::Parse),
    #[error("Shopify trigger timestamp is invalid")]
    InvalidTriggeredAt(#[source] time::error::Parse),
}

impl ShopifyProductEventKind {
    /// Maps Shopify's provider vocabulary to Aura's generic raw-input contract.
    /// Unknown Shopify object keys stay in `source_payload` unchanged.
    pub fn listing_action(
        self,
        source: &ShopifySource,
        payload: Value,
    ) -> Result<ShopifyListingAction, ShopifyProductEventError> {
        self.listing_action_with_source_occurred_at(source, payload, None)
    }

    pub fn listing_action_with_source_occurred_at(
        self,
        source: &ShopifySource,
        payload: Value,
        source_occurred_at: Option<OffsetDateTime>,
    ) -> Result<ShopifyListingAction, ShopifyProductEventError> {
        let source_payload = SourcePayload::new(payload.clone())
            .map_err(ShopifyProductEventError::InvalidSourcePayload)?;
        let product = serde_json::from_value::<ShopifyProductPayload>(payload)
            .map_err(ShopifyProductEventError::MalformedPayload)?;
        let source_record_key = product.id.to_string();

        let operation = if self == Self::Delete {
            Some(RawProductListingOperation::Delete)
        } else {
            match product.status.as_deref() {
                Some("active") => Some(RawProductListingOperation::Upsert),
                Some("archived" | "draft") => Some(RawProductListingOperation::Delete),
                Some(_) | None => None,
            }
        };
        let Some(operation) = operation else {
            return Ok(ShopifyListingAction::Ignore);
        };

        let raw_values = match operation {
            RawProductListingOperation::Upsert => active_raw_values(source, &product)?,
            RawProductListingOperation::Delete => {
                RawProductListingValues::new(serde_json::json!({}))
                    .map_err(ShopifyProductEventError::InvalidSourcePayload)?
            }
        };
        let context = normalization_context(source)?;
        let input = ProductListingNormalizationInput::new(
            operation,
            RawProductListingPayloadFormat::ShopifyProduct,
            PAYLOAD_SCHEMA_VERSION,
            product_listing_normalization::PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            source_payload,
            raw_values,
            context,
        )
        .map_err(ShopifyProductEventError::InvalidSourcePayload)?;
        Ok(ShopifyListingAction::Capture(ShopifyRawObservation {
            source_record_key,
            input,
            source_occurred_at,
        }))
    }
}

pub fn source_occurred_at_from_triggered_at(
    triggered_at: Option<&str>,
) -> Result<Option<OffsetDateTime>, ShopifyProductEventError> {
    triggered_at
        .map(|value| {
            OffsetDateTime::parse(value, &Rfc3339)
                .map_err(ShopifyProductEventError::InvalidTriggeredAt)
        })
        .transpose()
}

fn active_raw_values(
    source: &ShopifySource,
    product: &ShopifyProductPayload,
) -> Result<RawProductListingValues, ShopifyProductEventError> {
    let title = product
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(ShopifyProductEventError::MissingTitle)?;
    let handle = product
        .handle
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(ShopifyProductEventError::MissingHandle)?;
    let price = patch(
        product
            .variants
            .first()
            .and_then(|variant| variant.price.clone()),
    );
    if source.currency.is_none() && matches!(&price, ProductListingRawValuesPatch::Set(_)) {
        return Err(ShopifyProductEventError::MissingListingSourceCurrency);
    }
    let raw_values = ProductListingRawValues {
        source_listing_id: product.id.to_string(),
        title: ProductListingRawValuesPatch::Set(title.to_owned()),
        description: match product.body_html.as_deref() {
            Some(html) => {
                ProductListingRawValuesPatch::Set(vec![fallbacked_html_to_markdown(html)])
            }
            None => ProductListingRawValuesPatch::Clear,
        },
        price_format: ProductListingRawValuesPriceFormat::MachineDecimal,
        price,
        price_estimate_min: ProductListingRawValuesPatch::Unchanged,
        price_estimate_max: ProductListingRawValuesPatch::Unchanged,
        availability: product_availability(product),
        url: ProductListingRawValuesPatch::Set(format!(
            "https://{}/products/{handle}",
            source.domain
        )),
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
    serde_json::to_value(raw_values)
        .map_err(NormalizationInputError::JsonSerialization)
        .and_then(RawProductListingValues::new)
        .map_err(ShopifyProductEventError::InvalidSourcePayload)
}

fn normalization_context(
    source: &ShopifySource,
) -> Result<NormalizationContext, ShopifyProductEventError> {
    let context = ProductListingNormalizationContextV1 {
        base_url: format!("https://{}/", source.domain),
        fallback_currency: source.currency.map(|currency| currency.as_str().to_owned()),
        fallback_language: source.language.map(|language| language.as_str().to_owned()),
    };
    serde_json::to_value(context)
        .map_err(NormalizationInputError::JsonSerialization)
        .and_then(NormalizationContext::new)
        .map_err(ShopifyProductEventError::InvalidSourcePayload)
}

fn patch(value: Option<String>) -> ProductListingRawValuesPatch<String> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(ProductListingRawValuesPatch::Set)
        .unwrap_or(ProductListingRawValuesPatch::Clear)
}

pub fn fallbacked_html_to_markdown(html: &str) -> String {
    match html_to_markdown_rs::convert(html, None) {
        Ok(result) => result.content.unwrap_or_else(|| html.to_owned()),
        Err(_) => html.to_owned(),
    }
}

/// Maps only reliable Shopify inventory facts. Missing and untracked inventory
/// explicitly clear Aura's current availability assertion.
pub fn product_availability(
    payload: &ShopifyProductPayload,
) -> ProductListingRawValuesPatch<String> {
    let quantities: Vec<i64> = payload
        .variants
        .iter()
        .filter_map(|variant| {
            variant
                .inventory_management
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .and(variant.inventory_quantity)
        })
        .collect();

    if quantities.iter().any(|quantity| *quantity > 0) {
        ProductListingRawValuesPatch::Set("in stock".to_owned())
    } else if !quantities.is_empty() {
        ProductListingRawValuesPatch::Set("out of stock".to_owned())
    } else {
        ProductListingRawValuesPatch::Clear
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use listing_source_core::{Domain, ListingSourceId};
    use localization::Language;
    use money::Currency;
    use serde_json::json;

    #[test]
    fn should_map_active_product_to_generic_raw_values_and_context() {
        let action = ShopifyProductEventKind::Create
            .listing_action(
                &source(),
                json!({
                    "id": 42,
                    "title": "Cabinet",
                    "body_html": "<p>Imported cabinet</p>",
                    "handle": "cabinet",
                    "status": "active",
                    "variants": [{"price": "42.00", "inventory_quantity": 1, "inventory_management": "shopify"}],
                    "images": [{"src": "https://images.example/cabinet.jpg"}],
                    "futureShopifyKey": {"nested": true}
                }),
            )
            .unwrap_or_else(|error| panic!("mapping failed: {error}"));

        let ShopifyListingAction::Capture(observation) = action else {
            panic!("active product must capture");
        };
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
            json!("USD")
        );
        assert_eq!(
            observation.input.normalization_context().value()["fallbackLanguage"],
            json!("de")
        );
        assert_eq!(
            observation.input.source_payload().value()["futureShopifyKey"]["nested"],
            json!(true)
        );
    }

    #[test]
    fn should_map_archived_draft_and_delete_to_raw_delete() {
        for (kind, status) in [
            (ShopifyProductEventKind::Delete, Some("active")),
            (ShopifyProductEventKind::Update, Some("archived")),
            (ShopifyProductEventKind::Update, Some("draft")),
        ] {
            let action = kind
                .listing_action(&source(), payload(status))
                .unwrap_or_else(|error| panic!("mapping failed: {error}"));
            assert!(matches!(
                action,
                ShopifyListingAction::Capture(ShopifyRawObservation { input, .. })
                    if input.operation() == RawProductListingOperation::Delete
                        && input.raw_values_schema_version()
                            == product_listing_normalization::PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION
            ));
        }
    }

    #[test]
    fn should_ignore_missing_or_unsupported_status_with_invalid_updated_at() {
        for status in [None, Some("published")] {
            let mut ignored_payload = payload(status);
            ignored_payload["updated_at"] = json!("not-a-timestamp");

            assert!(matches!(
                ShopifyProductEventKind::Update.listing_action(&source(), ignored_payload),
                Ok(ShopifyListingAction::Ignore)
            ));
        }
    }

    #[test]
    fn should_reject_nonblank_provider_price_when_listing_source_currency_is_missing() {
        let mut product = payload(Some("active"));
        product["variants"] = json!([{"price": "42.00"}]);

        assert!(matches!(
            ShopifyProductEventKind::Create.listing_action(&source_without_currency(), product),
            Err(ShopifyProductEventError::MissingListingSourceCurrency)
        ));
    }

    #[test]
    fn should_capture_blank_provider_price_as_clear_when_listing_source_currency_is_missing() {
        let mut product = payload(Some("active"));
        product["variants"] = json!([{"price": " \t "}]);

        let action = ShopifyProductEventKind::Create
            .listing_action(&source_without_currency(), product)
            .unwrap_or_else(|error| panic!("mapping failed: {error}"));

        let ShopifyListingAction::Capture(observation) = action else {
            panic!("blank-price product must capture");
        };
        assert_eq!(
            json!({"action": "CLEAR"}),
            observation.input.raw_values().value()["price"]
        );
    }

    #[test]
    fn should_parse_shopify_trigger_time_for_id_only_delete() {
        assert_eq!(
            Some(
                OffsetDateTime::parse("2026-09-07T10:02:00Z", &Rfc3339)
                    .unwrap_or_else(|error| panic!("timestamp: {error}")),
            ),
            source_occurred_at_from_triggered_at(Some("2026-09-07T10:02:00Z"))
                .unwrap_or_else(|error| panic!("trigger timestamp: {error}")),
        );
        assert!(matches!(
            ShopifyProductEventKind::Delete
                .listing_action_with_source_occurred_at(
                    &source(),
                    json!({"id": 42}),
                    source_occurred_at_from_triggered_at(Some("2026-09-07T10:02:00Z"))
                        .unwrap_or_else(|error| panic!("trigger timestamp: {error}")),
                ),
            Ok(ShopifyListingAction::Capture(ShopifyRawObservation {
                source_occurred_at: Some(_),
                input,
                ..
            })) if input.operation() == RawProductListingOperation::Delete
        ));
    }

    #[test]
    fn should_map_inventory_to_explicit_generic_intent() {
        assert_eq!(
            ProductListingRawValuesPatch::Set("in stock".to_owned()),
            product_availability(&payload_with_inventory(Some(1), Some("shopify")))
        );
        assert_eq!(
            ProductListingRawValuesPatch::Set("out of stock".to_owned()),
            product_availability(&payload_with_inventory(Some(0), Some("shopify")))
        );
        assert_eq!(
            ProductListingRawValuesPatch::Clear,
            product_availability(&payload_with_inventory(None, Some("shopify")))
        );
        assert_eq!(
            ProductListingRawValuesPatch::Clear,
            product_availability(&payload_with_inventory(Some(1), None))
        );
    }

    fn source() -> ShopifySource {
        ShopifySource {
            listing_source_id: ListingSourceId::new(),
            domain: Domain::try_from("partner.example")
                .unwrap_or_else(|error| panic!("invalid domain: {error}")),
            currency: Some(Currency::Usd),
            language: Some(Language::De),
        }
    }

    fn source_without_currency() -> ShopifySource {
        let mut source = source();
        source.currency = None;
        source
    }

    fn payload(status: Option<&str>) -> Value {
        json!({
            "id": 42,
            "title": "Cabinet",
            "handle": "cabinet",
            "status": status,
            "variants": [],
            "images": []
        })
    }

    fn payload_with_inventory(
        inventory_quantity: Option<i64>,
        inventory_management: Option<&str>,
    ) -> ShopifyProductPayload {
        ShopifyProductPayload {
            id: 42,
            title: Some("Cabinet".to_owned()),
            body_html: None,
            handle: Some("cabinet".to_owned()),
            status: Some("active".to_owned()),
            updated_at: None,
            variants: vec![ShopifyVariantPayload {
                price: None,
                inventory_quantity,
                inventory_management: inventory_management.map(str::to_owned),
            }],
            images: Vec::new(),
        }
    }
}
