use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

pub const NORMALIZATION_INPUT_HASH_BYTES: usize = 32;
pub const MAX_SOURCE_PAYLOAD_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_RAW_VALUES_JSON_BYTES: usize = 256 * 1024;
pub const MAX_NORMALIZATION_CONTEXT_JSON_BYTES: usize = 64 * 1024;
pub const MAX_PROVENANCE_JSON_BYTES: usize = 64 * 1024;
pub const MAX_JSON_NESTING_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum RawProductListingOperation {
    Upsert,
    Delete,
}

impl RawProductListingOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Upsert => "UPSERT",
            Self::Delete => "DELETE",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        Self::iter().find(|operation| operation.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum RawProductListingPayloadFormat {
    CrawlerExtractedProduct,
    ShopifyProduct,
    WoocommerceProduct,
}

impl RawProductListingPayloadFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CrawlerExtractedProduct => "CRAWLER_EXTRACTED_PRODUCT",
            Self::ShopifyProduct => "SHOPIFY_PRODUCT",
            Self::WoocommerceProduct => "WOOCOMMERCE_PRODUCT",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        Self::iter().find(|format| format.as_str() == value)
    }
}

/// Provider-neutral raw values selected by a source implementation for current
/// deterministic normalization. The object remains open for generic contract evolution.
#[derive(Debug, Clone, PartialEq)]
pub struct RawProductListingValues(JsonObject);

impl RawProductListingValues {
    pub fn new(value: Value) -> Result<Self, NormalizationInputError> {
        JsonObject::new(value, JsonField::RawValues).map(Self)
    }

    pub const fn value(&self) -> &Value {
        self.0.value()
    }
}

/// Complete semantic provider/source object retained as evidence. Unknown keys are preserved.
#[derive(Debug, Clone, PartialEq)]
pub struct SourcePayload(JsonObject);

impl SourcePayload {
    pub fn new(value: Value) -> Result<Self, NormalizationInputError> {
        JsonObject::new(value, JsonField::SourcePayload).map(Self)
    }

    pub const fn value(&self) -> &Value {
        self.0.value()
    }

    /// Returns the key-order-stable SHA-256 digest of source payload evidence only.
    pub fn canonical_sha256(&self) -> Result<SourcePayloadHash, NormalizationInputError> {
        let canonical_json = canonical_json(self.value())?;
        Ok(SourcePayloadHash(
            Sha256::digest(canonical_json.as_bytes()).into(),
        ))
    }
}

/// SHA-256 digest of canonical source payload evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourcePayloadHash([u8; NORMALIZATION_INPUT_HASH_BYTES]);

impl SourcePayloadHash {
    pub const fn as_bytes(&self) -> &[u8; NORMALIZATION_INPUT_HASH_BYTES] {
        &self.0
    }
}

/// Provider-neutral values needed by deterministic normalization, such as a base URL.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizationContext(JsonObject);

impl NormalizationContext {
    pub fn new(value: Value) -> Result<Self, NormalizationInputError> {
        JsonObject::new(value, JsonField::NormalizationContext).map(Self)
    }

    pub const fn value(&self) -> &Value {
        self.0.value()
    }
}

/// Transport and diagnostic provenance. It is retained but excluded from input hashing.
#[derive(Debug, Clone, PartialEq)]
pub struct RawProductListingProvenance(JsonObject);

impl RawProductListingProvenance {
    pub fn new(value: Value) -> Result<Self, NormalizationInputError> {
        JsonObject::new(value, JsonField::Provenance).map(Self)
    }

    pub const fn value(&self) -> &Value {
        self.0.value()
    }
}

/// The durable provider-neutral input whose hash determines whether a raw revision changed.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingNormalizationInput {
    operation: RawProductListingOperation,
    payload_format: RawProductListingPayloadFormat,
    payload_schema_version: u16,
    raw_values_schema_version: u16,
    source_payload: SourcePayload,
    raw_values: RawProductListingValues,
    normalization_context: NormalizationContext,
}

impl ProductListingNormalizationInput {
    pub fn new(
        operation: RawProductListingOperation,
        payload_format: RawProductListingPayloadFormat,
        payload_schema_version: u16,
        raw_values_schema_version: u16,
        source_payload: SourcePayload,
        raw_values: RawProductListingValues,
        normalization_context: NormalizationContext,
    ) -> Result<Self, NormalizationInputError> {
        if payload_schema_version == 0 {
            return Err(NormalizationInputError::SchemaVersionMustBePositive {
                field: SchemaVersionField::Payload,
            });
        }
        if raw_values_schema_version == 0 {
            return Err(NormalizationInputError::SchemaVersionMustBePositive {
                field: SchemaVersionField::RawValues,
            });
        }
        Ok(Self {
            operation,
            payload_format,
            payload_schema_version,
            raw_values_schema_version,
            source_payload,
            raw_values,
            normalization_context,
        })
    }

    pub const fn operation(&self) -> RawProductListingOperation {
        self.operation
    }

    pub const fn payload_format(&self) -> RawProductListingPayloadFormat {
        self.payload_format
    }

    pub const fn payload_schema_version(&self) -> u16 {
        self.payload_schema_version
    }

    pub const fn raw_values_schema_version(&self) -> u16 {
        self.raw_values_schema_version
    }

    pub const fn source_payload(&self) -> &SourcePayload {
        &self.source_payload
    }

    pub const fn raw_values(&self) -> &RawProductListingValues {
        &self.raw_values
    }

    pub const fn normalization_context(&self) -> &NormalizationContext {
        &self.normalization_context
    }

    pub fn hash(&self) -> Result<NormalizationInputHash, NormalizationInputError> {
        let mut input = Map::new();
        input.insert(
            "action".to_owned(),
            Value::String(self.operation.as_str().to_owned()),
        );
        input.insert(
            "payloadFormat".to_owned(),
            Value::String(self.payload_format.as_str().to_owned()),
        );
        input.insert(
            "payloadSchemaVersion".to_owned(),
            Value::Number(self.payload_schema_version.into()),
        );
        input.insert(
            "rawValuesSchemaVersion".to_owned(),
            Value::Number(self.raw_values_schema_version.into()),
        );
        input.insert(
            "sourcePayload".to_owned(),
            self.source_payload.value().clone(),
        );
        input.insert("rawValues".to_owned(), self.raw_values.value().clone());
        input.insert(
            "normalizationContext".to_owned(),
            self.normalization_context.value().clone(),
        );

        let canonical_json = canonical_json(&Value::Object(input))?;
        Ok(NormalizationInputHash(
            Sha256::digest(canonical_json.as_bytes()).into(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NormalizationInputHash([u8; NORMALIZATION_INPUT_HASH_BYTES]);

impl NormalizationInputHash {
    pub const fn as_bytes(&self) -> &[u8; NORMALIZATION_INPUT_HASH_BYTES] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonField {
    SourcePayload,
    RawValues,
    NormalizationContext,
    Provenance,
}

impl JsonField {
    const fn max_bytes(self) -> usize {
        match self {
            Self::SourcePayload => MAX_SOURCE_PAYLOAD_JSON_BYTES,
            Self::RawValues => MAX_RAW_VALUES_JSON_BYTES,
            Self::NormalizationContext => MAX_NORMALIZATION_CONTEXT_JSON_BYTES,
            Self::Provenance => MAX_PROVENANCE_JSON_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaVersionField {
    Payload,
    RawValues,
}

#[derive(Debug, thiserror::Error)]
pub enum NormalizationInputError {
    #[error("raw product listing JSON field must be an object")]
    JsonNotObject { field: JsonField },
    #[error("raw product listing JSON field contains an embedded NUL")]
    JsonEmbeddedNul { field: JsonField },
    #[error("raw product listing JSON field exceeds its byte limit")]
    JsonTooLarge {
        field: JsonField,
        len: usize,
        max: usize,
    },
    #[error("raw product listing JSON field exceeds its nesting limit")]
    JsonTooDeep {
        field: JsonField,
        depth: usize,
        max: usize,
    },
    #[error("raw product listing schema version must be positive")]
    SchemaVersionMustBePositive { field: SchemaVersionField },
    #[error("raw product listing JSON could not be serialized")]
    JsonSerialization(#[source] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq)]
struct JsonObject(Value);

impl JsonObject {
    fn new(value: Value, field: JsonField) -> Result<Self, NormalizationInputError> {
        if !value.is_object() {
            return Err(NormalizationInputError::JsonNotObject { field });
        }
        let encoded =
            serde_json::to_vec(&value).map_err(NormalizationInputError::JsonSerialization)?;
        if encoded.len() > field.max_bytes() {
            return Err(NormalizationInputError::JsonTooLarge {
                field,
                len: encoded.len(),
                max: field.max_bytes(),
            });
        }
        let depth = json_depth(&value);
        if depth > MAX_JSON_NESTING_DEPTH {
            return Err(NormalizationInputError::JsonTooDeep {
                field,
                depth,
                max: MAX_JSON_NESTING_DEPTH,
            });
        }
        if json_contains_embedded_nul(&value) {
            return Err(NormalizationInputError::JsonEmbeddedNul { field });
        }
        Ok(Self(value))
    }

    const fn value(&self) -> &Value {
        &self.0
    }
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(values) => 1 + values.values().map(json_depth).max().unwrap_or(0),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => 1,
    }
}

fn json_contains_embedded_nul(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(json_contains_embedded_nul),
        Value::Object(values) => values
            .iter()
            .any(|(key, value)| key.contains('\0') || json_contains_embedded_nul(value)),
        Value::String(value) => value.contains('\0'),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn canonical_json(value: &Value) -> Result<String, NormalizationInputError> {
    let mut output = String::new();
    write_canonical_json(value, &mut output)?;
    Ok(output)
}

fn write_canonical_json(value: &Value, output: &mut String) -> Result<(), NormalizationInputError> {
    match value {
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(
                    &serde_json::to_string(key)
                        .map_err(NormalizationInputError::JsonSerialization)?,
                );
                output.push(':');
                if let Some(value) = values.get(*key) {
                    write_canonical_json(value, output)?;
                }
            }
            output.push('}');
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => output.push_str(
            &serde_json::to_string(value).map_err(NormalizationInputError::JsonSerialization)?,
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(
        source_payload: Value,
        raw_values: Value,
        context: Value,
    ) -> Result<ProductListingNormalizationInput, NormalizationInputError> {
        ProductListingNormalizationInput::new(
            RawProductListingOperation::Upsert,
            RawProductListingPayloadFormat::ShopifyProduct,
            1,
            1,
            SourcePayload::new(source_payload)?,
            RawProductListingValues::new(raw_values)?,
            NormalizationContext::new(context)?,
        )
    }

    #[test]
    fn should_return_equal_source_payload_hashes_when_object_key_order_differs()
    -> Result<(), NormalizationInputError> {
        let first = SourcePayload::new(json!({
            "event": {"metadata": {"b": 2, "a": 1}, "id": "event-1"}
        }))?;
        let second = SourcePayload::new(json!({
            "event": {"id": "event-1", "metadata": {"a": 1, "b": 2}}
        }))?;

        let first_hash = first.canonical_sha256()?;
        assert_eq!(first_hash, second.canonical_sha256()?);
        assert_eq!(first_hash.as_bytes().len(), NORMALIZATION_INPUT_HASH_BYTES);
        Ok(())
    }

    #[test]
    fn should_return_different_source_payload_hashes_when_evidence_differs()
    -> Result<(), NormalizationInputError> {
        let first = SourcePayload::new(json!({"event": {"id": "event-1", "amount": 10}}))?;
        let second = SourcePayload::new(json!({"event": {"id": "event-1", "amount": 11}}))?;

        assert_ne!(first.canonical_sha256()?, second.canonical_sha256()?);
        Ok(())
    }

    #[test]
    fn should_hash_equally_when_object_key_order_differs() -> Result<(), NormalizationInputError> {
        let first = input(
            json!({"unknown": {"b": 2, "a": 1}}),
            json!({"title": "Vase"}),
            json!({}),
        )?;
        let second = input(
            json!({"unknown": {"a": 1, "b": 2}}),
            json!({"title": "Vase"}),
            json!({}),
        )?;

        assert_eq!(first.hash()?, second.hash()?);
        Ok(())
    }

    #[test]
    fn should_hash_only_source_payload_when_other_observation_fields_differ()
    -> Result<(), NormalizationInputError> {
        let first = input(
            json!({"event": {"id": "event-1", "status": "published"}}),
            json!({"title": "Vase"}),
            json!({"fallbackCurrency": "EUR"}),
        )?;
        let second = input(
            json!({"event": {"id": "event-1", "status": "published"}}),
            json!({"title": "Bowl"}),
            json!({"fallbackCurrency": "USD"}),
        )?;
        let first_provenance = RawProductListingProvenance::new(json!({"deliveryId": "one"}))?;
        let second_provenance = RawProductListingProvenance::new(json!({"deliveryId": "two"}))?;

        assert_ne!(first.hash()?, second.hash()?);
        assert_ne!(first_provenance.value(), second_provenance.value());
        assert_eq!(
            first.source_payload().canonical_sha256()?,
            second.source_payload().canonical_sha256()?
        );
        Ok(())
    }

    #[test]
    fn should_hash_differently_when_array_order_differs() -> Result<(), NormalizationInputError> {
        let first = input(json!({}), json!({"images": ["first", "second"]}), json!({}))?;
        let second = input(json!({}), json!({"images": ["second", "first"]}), json!({}))?;
        assert_ne!(first.hash()?, second.hash()?);
        Ok(())
    }

    #[test]
    fn should_preserve_and_hash_unknown_nested_source_payload_fields()
    -> Result<(), NormalizationInputError> {
        let first_payload = json!({
            "unknownSourceField": {
                "unknownNestedKey": [
                    {"unknownLeafKey": "one"},
                    "retained source evidence"
                ]
            }
        });
        let second_payload = json!({
            "unknownSourceField": {
                "unknownNestedKey": [
                    {"unknownLeafKey": "two"},
                    "retained source evidence"
                ]
            }
        });
        let first = input(first_payload.clone(), json!({}), json!({}))?;
        let second = input(second_payload.clone(), json!({}), json!({}))?;

        assert_eq!(&first_payload, first.source_payload().value());
        assert_eq!(&second_payload, second.source_payload().value());
        assert_ne!(first.hash()?, second.hash()?);
        Ok(())
    }

    #[test]
    fn should_hash_action_raw_values_and_context() -> Result<(), NormalizationInputError> {
        let base = input(
            json!({}),
            json!({"title": "Vase"}),
            json!({"currency": "EUR"}),
        )?;
        let changed_raw_values = input(
            json!({}),
            json!({"title": "Bowl"}),
            json!({"currency": "EUR"}),
        )?;
        let changed_context = input(
            json!({}),
            json!({"title": "Vase"}),
            json!({"currency": "USD"}),
        )?;
        let delete = ProductListingNormalizationInput::new(
            RawProductListingOperation::Delete,
            RawProductListingPayloadFormat::ShopifyProduct,
            1,
            1,
            SourcePayload::new(json!({}))?,
            RawProductListingValues::new(json!({"title": "Vase"}))?,
            NormalizationContext::new(json!({"currency": "EUR"}))?,
        )?;
        assert_ne!(base.hash()?, changed_raw_values.hash()?);
        assert_ne!(base.hash()?, changed_context.hash()?);
        assert_ne!(base.hash()?, delete.hash()?);
        Ok(())
    }

    #[test]
    fn should_exclude_provenance_from_input_hash() -> Result<(), NormalizationInputError> {
        let input = input(json!({}), json!({}), json!({}))?;
        let first = RawProductListingProvenance::new(json!({"deliveryId": "one"}))?;
        let second = RawProductListingProvenance::new(json!({"deliveryId": "two"}))?;
        assert_ne!(first.value(), second.value());
        assert_eq!(input.hash()?, input.hash()?);
        Ok(())
    }

    #[test]
    fn should_reject_embedded_nul_in_json_object_keys_and_nested_strings() {
        assert_json_embedded_nul(
            SourcePayload::new(json!({"unknown": {"nested\0key": "value"}})),
            JsonField::SourcePayload,
        );
        assert_json_embedded_nul(
            SourcePayload::new(json!({"unknown": {"nested": ["bad\0value"]}})),
            JsonField::SourcePayload,
        );
        assert_json_embedded_nul(
            RawProductListingValues::new(json!({"unknown": {"nested\0key": "value"}})),
            JsonField::RawValues,
        );
        assert_json_embedded_nul(
            RawProductListingValues::new(json!({"unknown": {"nested": ["bad\0value"]}})),
            JsonField::RawValues,
        );
        assert_json_embedded_nul(
            NormalizationContext::new(json!({"unknown": {"nested\0key": "value"}})),
            JsonField::NormalizationContext,
        );
        assert_json_embedded_nul(
            NormalizationContext::new(json!({"unknown": {"nested": ["bad\0value"]}})),
            JsonField::NormalizationContext,
        );
        assert_json_embedded_nul(
            RawProductListingProvenance::new(json!({"unknown": {"nested\0key": "value"}})),
            JsonField::Provenance,
        );
        assert_json_embedded_nul(
            RawProductListingProvenance::new(json!({"unknown": {"nested": ["bad\0value"]}})),
            JsonField::Provenance,
        );
    }

    #[test]
    fn should_reject_non_object_and_zero_versions() {
        assert!(matches!(
            RawProductListingValues::new(json!([])),
            Err(NormalizationInputError::JsonNotObject {
                field: JsonField::RawValues
            })
        ));
        assert!(matches!(
            ProductListingNormalizationInput::new(
                RawProductListingOperation::Upsert,
                RawProductListingPayloadFormat::ShopifyProduct,
                0,
                1,
                SourcePayload::new(json!({}))
                    .unwrap_or_else(|error| panic!("source payload: {error}")),
                RawProductListingValues::new(json!({}))
                    .unwrap_or_else(|error| panic!("raw values: {error}")),
                NormalizationContext::new(json!({}))
                    .unwrap_or_else(|error| panic!("context: {error}")),
            ),
            Err(NormalizationInputError::SchemaVersionMustBePositive {
                field: SchemaVersionField::Payload
            })
        ));
    }

    #[test]
    fn should_reject_oversized_and_too_deep_json() {
        assert!(matches!(
            RawProductListingValues::new(json!({"value": "x".repeat(MAX_RAW_VALUES_JSON_BYTES)})),
            Err(NormalizationInputError::JsonTooLarge { .. })
        ));
        let mut value = json!(null);
        for _ in 0..MAX_JSON_NESTING_DEPTH {
            value = json!({"nested": value});
        }
        assert!(matches!(
            SourcePayload::new(value),
            Err(NormalizationInputError::JsonTooDeep { .. })
        ));
    }

    #[test]
    fn should_expose_a_fixed_width_input_hash() -> Result<(), NormalizationInputError> {
        assert_eq!(
            input(json!({}), json!({}), json!({}))?
                .hash()?
                .as_bytes()
                .len(),
            NORMALIZATION_INPUT_HASH_BYTES
        );
        Ok(())
    }

    fn assert_json_embedded_nul<T>(result: Result<T, NormalizationInputError>, field: JsonField) {
        assert!(matches!(
            result,
            Err(NormalizationInputError::JsonEmbeddedNul { field: actual_field })
                if actual_field == field
        ));
    }
}
