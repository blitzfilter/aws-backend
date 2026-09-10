//! Pure deterministic ProductListing normalization.
//!
//! Source integrations map provider fields into generic values before calling these APIs.
//! This crate performs no I/O, logging, configuration lookup, or provider interpretation.

pub mod availability;
pub mod date_time;
pub mod error;
pub mod image_url;
pub mod language;
pub mod normalization_input;
pub mod price;
pub mod raw_values_normalizer;
pub mod source_listing_id;
pub mod text;

pub use availability::{
    AvailabilityNormalizationError, ListingAvailabilityQuickCheck, quick_check_availability,
};
pub use date_time::{DateTimeNormalizationError, normalize_date_time};
pub use error::{DateTimeField, NormalizationError, PriceField};
pub use image_url::{ImageUrlNormalizationError, normalize_image_urls};
pub use language::detect_language;
pub use normalization_input::{
    JsonField, MAX_JSON_NESTING_DEPTH, MAX_NORMALIZATION_CONTEXT_JSON_BYTES,
    MAX_PROVENANCE_JSON_BYTES, MAX_RAW_VALUES_JSON_BYTES, MAX_SOURCE_PAYLOAD_JSON_BYTES,
    NORMALIZATION_INPUT_HASH_BYTES, NormalizationContext, NormalizationInputError,
    NormalizationInputHash, ProductListingNormalizationInput, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingProvenance, RawProductListingValues,
    SchemaVersionField, SourcePayload, SourcePayloadHash,
};
pub use price::{PriceNormalizationError, normalize_price, normalize_product_listing_price};
pub use raw_values_normalizer::{
    PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V1, PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V2,
    ProductListingNormalizationContextV1, ProductListingRawValuesNormalizationError,
    ProductListingRawValuesNormalizationOutcome, ProductListingRawValuesNormalizer,
    ProductListingRawValuesPatch, ProductListingRawValuesPriceFormat,
    ProductListingRawValuesResolved, ProductListingRawValuesV1, ProductListingRawValuesV2,
};
pub use source_listing_id::{
    SourceListingIdNormalizationError, normalize_source_listing_id_with_url_sha_fallback,
};
pub use text::{normalize_description, normalize_title};
