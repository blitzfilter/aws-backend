use crate::error::NormalizationFailureScope;
use crate::price::normalize_machine_decimal_price;
use crate::text::{detect_description_language, localize_normalized_title};
use crate::{
    AvailabilityNormalizationError, DateTimeField, DateTimeNormalizationError,
    ImageUrlNormalizationError, ListingAvailabilityQuickCheck, NormalizationError, PriceField,
    PriceNormalizationError, ProductListingNormalizationInput, RawProductListingOperation,
    detect_language, normalize_date_time, normalize_description, normalize_image_urls,
    normalize_price, normalize_product_listing_price,
    normalize_source_listing_id_with_url_sha_fallback, normalize_title, quick_check_availability,
};
use localization::{Language, Localized};
use money::{Currency, Price};
use product_listing_core::{
    description::Description, product_listing_image::ProductListingImage,
    product_listing_price::ProductListingPrice, source_listing_id::SourceListingId, title::Title,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::collections::BTreeMap;
use strum::IntoEnumIterator;
use time::OffsetDateTime;
use url::Url;

pub const PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V1: u16 = 1;
pub const PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V2: u16 = 2;

/// Stable provider-neutral encoding for raw price patch values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum ProductListingRawValuesPriceFormat {
    DisplayText,
    MachineDecimal,
}

impl ProductListingRawValuesPriceFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DisplayText => "DISPLAY_TEXT",
            Self::MachineDecimal => "MACHINE_DECIMAL",
        }
    }
}

impl Serialize for ProductListingRawValuesPriceFormat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProductListingRawValuesPriceFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let code = String::deserialize(deserializer)?;
        Self::iter()
            .find(|format| format.as_str() == code.as_str())
            .ok_or_else(|| D::Error::custom("raw price format is unsupported"))
    }
}

/// Provider-neutral protocol for one raw field update.
///
/// `CLEAR` and `UNCHANGED` remain distinct from `SET`, including when a set value
/// normalizes to an empty canonical value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", content = "value", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductListingRawValuesPatch<T> {
    Set(T),
    Clear,
    Unchanged,
}

/// Frozen provider-neutral raw values for an UPSERT normalization input at schema version 1.
///
/// V1 price patches always use display-text parsing. Source adapters map provider payloads to
/// this contract before constructing [`ProductListingNormalizationInput`]. Each mutable listing
/// field carries the generic set/clear/unchanged protocol. Dynamic attributes use source-selected
/// names and do not introduce provider-specific fields into this contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductListingRawValuesV1 {
    pub source_listing_id: String,
    pub title: ProductListingRawValuesPatch<String>,
    pub description: ProductListingRawValuesPatch<Vec<String>>,
    pub price: ProductListingRawValuesPatch<String>,
    pub price_estimate_min: ProductListingRawValuesPatch<String>,
    pub price_estimate_max: ProductListingRawValuesPatch<String>,
    pub availability: ProductListingRawValuesPatch<String>,
    pub url: ProductListingRawValuesPatch<String>,
    pub images: ProductListingRawValuesPatch<Vec<String>>,
    pub auction_start: ProductListingRawValuesPatch<String>,
    pub auction_end: ProductListingRawValuesPatch<String>,
    #[serde(default)]
    pub attributes: BTreeMap<String, ProductListingRawValuesPatch<Vec<String>>>,
}

/// Provider-neutral raw values for an UPSERT normalization input at schema version 2.
///
/// `priceFormat` is required and applies to the main and estimate price patches. `DISPLAY_TEXT`
/// keeps V1 display parsing, while `MACHINE_DECIMAL` uses strict unsigned ASCII decimal parsing
/// with the normalization-context fallback currency for nonblank `SET` values. Blank `SET` values
/// normalize to `CLEAR`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductListingRawValuesV2 {
    pub source_listing_id: String,
    pub title: ProductListingRawValuesPatch<String>,
    pub description: ProductListingRawValuesPatch<Vec<String>>,
    pub price_format: ProductListingRawValuesPriceFormat,
    pub price: ProductListingRawValuesPatch<String>,
    pub price_estimate_min: ProductListingRawValuesPatch<String>,
    pub price_estimate_max: ProductListingRawValuesPatch<String>,
    pub availability: ProductListingRawValuesPatch<String>,
    pub url: ProductListingRawValuesPatch<String>,
    pub images: ProductListingRawValuesPatch<Vec<String>>,
    pub auction_start: ProductListingRawValuesPatch<String>,
    pub auction_end: ProductListingRawValuesPatch<String>,
    #[serde(default)]
    pub attributes: BTreeMap<String, ProductListingRawValuesPatch<Vec<String>>>,
}

#[derive(Debug)]
struct RawValuesForNormalization {
    source_listing_id: String,
    title: ProductListingRawValuesPatch<String>,
    description: ProductListingRawValuesPatch<Vec<String>>,
    price_format: ProductListingRawValuesPriceFormat,
    price: ProductListingRawValuesPatch<String>,
    price_estimate_min: ProductListingRawValuesPatch<String>,
    price_estimate_max: ProductListingRawValuesPatch<String>,
    availability: ProductListingRawValuesPatch<String>,
    url: ProductListingRawValuesPatch<String>,
    images: ProductListingRawValuesPatch<Vec<String>>,
    auction_start: ProductListingRawValuesPatch<String>,
    auction_end: ProductListingRawValuesPatch<String>,
    attributes: BTreeMap<String, ProductListingRawValuesPatch<Vec<String>>>,
}

impl From<ProductListingRawValuesV1> for RawValuesForNormalization {
    fn from(raw: ProductListingRawValuesV1) -> Self {
        Self {
            source_listing_id: raw.source_listing_id,
            title: raw.title,
            description: raw.description,
            price_format: ProductListingRawValuesPriceFormat::DisplayText,
            price: raw.price,
            price_estimate_min: raw.price_estimate_min,
            price_estimate_max: raw.price_estimate_max,
            availability: raw.availability,
            url: raw.url,
            images: raw.images,
            auction_start: raw.auction_start,
            auction_end: raw.auction_end,
            attributes: raw.attributes,
        }
    }
}

impl From<ProductListingRawValuesV2> for RawValuesForNormalization {
    fn from(raw: ProductListingRawValuesV2) -> Self {
        Self {
            source_listing_id: raw.source_listing_id,
            title: raw.title,
            description: raw.description,
            price_format: raw.price_format,
            price: raw.price,
            price_estimate_min: raw.price_estimate_min,
            price_estimate_max: raw.price_estimate_max,
            availability: raw.availability,
            url: raw.url,
            images: raw.images,
            auction_start: raw.auction_start,
            auction_end: raw.auction_end,
            attributes: raw.attributes,
        }
    }
}

/// Generic normalization inputs that are not provider payload fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductListingNormalizationContextV1 {
    pub base_url: String,
    #[serde(default)]
    pub fallback_currency: Option<String>,
    #[serde(default)]
    pub fallback_language: Option<String>,
}

/// Deterministically normalized values for an UPSERT observation.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingRawValuesResolved {
    pub source_listing_id: SourceListingId,
    pub title: ProductListingRawValuesPatch<Localized<Language, Title>>,
    pub description: ProductListingRawValuesPatch<Localized<Language, Description>>,
    pub price: ProductListingRawValuesPatch<ProductListingPrice>,
    pub price_estimate_min: ProductListingRawValuesPatch<Price>,
    pub price_estimate_max: ProductListingRawValuesPatch<Price>,
    pub availability: ProductListingRawValuesPatch<ListingAvailabilityQuickCheck>,
    pub url: ProductListingRawValuesPatch<Url>,
    pub images: ProductListingRawValuesPatch<Vec<ProductListingImage>>,
    pub auction_start: ProductListingRawValuesPatch<OffsetDateTime>,
    pub auction_end: ProductListingRawValuesPatch<OffsetDateTime>,
    pub attributes: BTreeMap<String, ProductListingRawValuesPatch<Vec<String>>>,
}

/// One complete raw-values normalization result.
#[derive(Debug)]
pub enum ProductListingRawValuesNormalizationOutcome {
    Resolved(Box<ProductListingRawValuesResolved>),
    Invalid(ProductListingRawValuesNormalizationError),
    Delete,
}

/// Typed reason why an UPSERT raw-values input could not normalize.
#[derive(Debug, thiserror::Error)]
pub enum ProductListingRawValuesNormalizationError {
    #[error("raw values schema version is unsupported")]
    UnsupportedRawValuesSchemaVersion { version: u16 },
    #[error("raw values do not match the V1 contract")]
    InvalidRawValuesV1(#[source] serde_json::Error),
    #[error("raw values do not match the V2 contract")]
    InvalidRawValuesV2(#[source] serde_json::Error),
    #[error("normalization context does not match the V1 contract")]
    InvalidNormalizationContextV1(#[source] serde_json::Error),
    #[error("normalization context base URL is invalid")]
    InvalidBaseUrl(#[source] url::ParseError),
    #[error("listing URL is invalid")]
    InvalidUrl(#[source] url::ParseError),
    #[error(
        "normalization context fallback currency is required for nonblank machine-decimal prices"
    )]
    MachineDecimalFallbackCurrencyRequired,
    #[error("normalization context fallback currency is unsupported")]
    UnsupportedFallbackCurrency,
    #[error("normalization context fallback language is unsupported")]
    UnsupportedFallbackLanguage,
    #[error("source listing ID or text is invalid")]
    Text(#[source] NormalizationError),
    #[error("price is invalid")]
    Price(#[source] NormalizationError),
    #[error("image URL is invalid")]
    ImageUrl(#[source] NormalizationError),
    #[error("auction date-time is invalid")]
    DateTime(#[source] NormalizationError),
    #[error("availability is invalid")]
    Availability(#[source] NormalizationError),
}

impl ProductListingRawValuesNormalizationError {
    /// Distinguishes terminal candidate data from fail-closed normalizer system failures.
    pub const fn failure_scope(&self) -> NormalizationFailureScope {
        match self {
            Self::Text(error)
            | Self::Price(error)
            | Self::ImageUrl(error)
            | Self::DateTime(error)
            | Self::Availability(error) => error.failure_scope(),
            _ => NormalizationFailureScope::CandidateData,
        }
    }
}

/// Pure raw-values normalizer for supported schema versions.
///
/// DELETE inputs deliberately bypass raw-values decoding and field normalization. Their source
/// record identity belongs to the capture input, not an UPSERT field projection.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProductListingRawValuesNormalizer;

impl ProductListingRawValuesNormalizer {
    pub const fn new() -> Self {
        Self
    }

    pub fn normalize(
        &self,
        input: &ProductListingNormalizationInput,
    ) -> ProductListingRawValuesNormalizationOutcome {
        if input.operation() == RawProductListingOperation::Delete {
            return ProductListingRawValuesNormalizationOutcome::Delete;
        }

        match self.normalize_upsert(input) {
            Ok(resolved) => {
                ProductListingRawValuesNormalizationOutcome::Resolved(Box::new(resolved))
            }
            Err(error) => ProductListingRawValuesNormalizationOutcome::Invalid(error),
        }
    }

    fn normalize_upsert(
        &self,
        input: &ProductListingNormalizationInput,
    ) -> Result<ProductListingRawValuesResolved, ProductListingRawValuesNormalizationError> {
        let raw: RawValuesForNormalization = match input.raw_values_schema_version() {
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V1 => {
                serde_json::from_value::<ProductListingRawValuesV1>(
                    input.raw_values().value().clone(),
                )
                .map(RawValuesForNormalization::from)
                .map_err(ProductListingRawValuesNormalizationError::InvalidRawValuesV1)?
            }
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V2 => {
                serde_json::from_value::<ProductListingRawValuesV2>(
                    input.raw_values().value().clone(),
                )
                .map(RawValuesForNormalization::from)
                .map_err(ProductListingRawValuesNormalizationError::InvalidRawValuesV2)?
            }
            version => {
                return Err(
                    ProductListingRawValuesNormalizationError::UnsupportedRawValuesSchemaVersion {
                        version,
                    },
                );
            }
        };
        let context: ProductListingNormalizationContextV1 = serde_json::from_value(
            input.normalization_context().value().clone(),
        )
        .map_err(ProductListingRawValuesNormalizationError::InvalidNormalizationContextV1)?;
        let base_url = Url::parse(context.base_url.as_str())
            .map_err(ProductListingRawValuesNormalizationError::InvalidBaseUrl)?;
        let fallback_currency = context
            .fallback_currency
            .as_deref()
            .map(|code| {
                Currency::from_code(code)
                    .ok_or(ProductListingRawValuesNormalizationError::UnsupportedFallbackCurrency)
            })
            .transpose()?;
        let price_format = raw.price_format;
        let fallback_language = context
            .fallback_language
            .as_deref()
            .map(|code| {
                Language::from_code(code)
                    .ok_or(ProductListingRawValuesNormalizationError::UnsupportedFallbackLanguage)
            })
            .transpose()?;

        let description_language = match &raw.description {
            ProductListingRawValuesPatch::Set(fragments) => {
                detect_description_language(fragments).or(fallback_language)
            }
            ProductListingRawValuesPatch::Clear | ProductListingRawValuesPatch::Unchanged => {
                fallback_language
            }
        };
        let source_listing_id = normalize_source_listing_id_with_url_sha_fallback(
            raw.source_listing_id.as_str(),
            &base_url,
        )
        .map_err(ProductListingRawValuesNormalizationError::Text)?;
        let title = normalize_title_patch(raw.title, description_language)?;
        let description = normalize_description_patch(
            raw.description,
            title_language(&title).or(fallback_language),
        )?;
        let price = normalize_product_listing_price_patch(
            raw.price,
            fallback_currency,
            price_format,
            PriceField::Price,
        )?;
        let price_estimate_min = normalize_price_patch(
            raw.price_estimate_min,
            fallback_currency,
            price_format,
            PriceField::EstimateMin,
        )?;
        let price_estimate_max = normalize_price_patch(
            raw.price_estimate_max,
            fallback_currency,
            price_format,
            PriceField::EstimateMax,
        )?;
        let availability = normalize_availability_patch(raw.availability)?;
        let url = normalize_url_patch(raw.url, &base_url)?;
        let images = normalize_images_patch(raw.images, &base_url)?;
        let auction_start =
            normalize_date_time_patch(raw.auction_start, DateTimeField::AuctionStart)?;
        let auction_end = normalize_date_time_patch(raw.auction_end, DateTimeField::AuctionEnd)?;

        Ok(ProductListingRawValuesResolved {
            source_listing_id,
            title,
            description,
            price,
            price_estimate_min,
            price_estimate_max,
            availability,
            url,
            images,
            auction_start,
            auction_end,
            attributes: raw.attributes,
        })
    }
}

fn title_language(
    title: &ProductListingRawValuesPatch<Localized<Language, Title>>,
) -> Option<Language> {
    match title {
        ProductListingRawValuesPatch::Set(title) => Some(title.localization),
        ProductListingRawValuesPatch::Clear | ProductListingRawValuesPatch::Unchanged => None,
    }
}

fn normalize_title_patch(
    patch: ProductListingRawValuesPatch<String>,
    description_language: Option<Language>,
) -> Result<
    ProductListingRawValuesPatch<Localized<Language, Title>>,
    ProductListingRawValuesNormalizationError,
> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) => {
            let title = normalize_title(raw.as_str())
                .map_err(ProductListingRawValuesNormalizationError::Text)?;
            let title_language = detect_language(title.as_ref());
            localize_normalized_title(title, title_language, description_language)
                .map(ProductListingRawValuesPatch::Set)
                .map_err(ProductListingRawValuesNormalizationError::Text)
        }
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn normalize_description_patch(
    patch: ProductListingRawValuesPatch<Vec<String>>,
    fallback_language: Option<Language>,
) -> Result<
    ProductListingRawValuesPatch<Localized<Language, Description>>,
    ProductListingRawValuesNormalizationError,
> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) => normalize_description(raw, fallback_language)
            .map(|description| match description {
                Some(description) => ProductListingRawValuesPatch::Set(description),
                None => ProductListingRawValuesPatch::Clear,
            })
            .map_err(ProductListingRawValuesNormalizationError::Text),
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn normalize_product_listing_price_patch(
    patch: ProductListingRawValuesPatch<String>,
    fallback_currency: Option<Currency>,
    price_format: ProductListingRawValuesPriceFormat,
    field: PriceField,
) -> Result<
    ProductListingRawValuesPatch<ProductListingPrice>,
    ProductListingRawValuesNormalizationError,
> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) if raw.trim().is_empty() => {
            Ok(ProductListingRawValuesPatch::Clear)
        }
        ProductListingRawValuesPatch::Set(raw) => match price_format {
            ProductListingRawValuesPriceFormat::DisplayText => {
                normalize_product_listing_price(Some(raw.as_str()), fallback_currency)
                    .map(|price| match price {
                        Some(price) => ProductListingRawValuesPatch::Set(price),
                        None => ProductListingRawValuesPatch::Clear,
                    })
                    .map_err(|error| map_price_error(error, field))
            }
            ProductListingRawValuesPriceFormat::MachineDecimal => {
                let currency = fallback_currency.ok_or(
                    ProductListingRawValuesNormalizationError::MachineDecimalFallbackCurrencyRequired,
                )?;
                normalize_machine_decimal_price(raw.as_str(), currency)
                    .map(ProductListingPrice::from)
                    .map(ProductListingRawValuesPatch::Set)
                    .map_err(|error| map_price_error(error, field))
            }
        },
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn normalize_price_patch(
    patch: ProductListingRawValuesPatch<String>,
    fallback_currency: Option<Currency>,
    price_format: ProductListingRawValuesPriceFormat,
    field: PriceField,
) -> Result<ProductListingRawValuesPatch<Price>, ProductListingRawValuesNormalizationError> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) if raw.trim().is_empty() => {
            Ok(ProductListingRawValuesPatch::Clear)
        }
        ProductListingRawValuesPatch::Set(raw) => match price_format {
            ProductListingRawValuesPriceFormat::DisplayText => {
                normalize_price(Some(raw.as_str()), fallback_currency)
                    .map(|price| match price {
                        Some(price) => ProductListingRawValuesPatch::Set(price),
                        None => ProductListingRawValuesPatch::Clear,
                    })
                    .map_err(|error| map_price_error(error, field))
            }
            ProductListingRawValuesPriceFormat::MachineDecimal => {
                let currency = fallback_currency.ok_or(
                    ProductListingRawValuesNormalizationError::MachineDecimalFallbackCurrencyRequired,
                )?;
                normalize_machine_decimal_price(raw.as_str(), currency)
                    .map(ProductListingRawValuesPatch::Set)
                    .map_err(|error| map_price_error(error, field))
            }
        },
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn normalize_availability_patch(
    patch: ProductListingRawValuesPatch<String>,
) -> Result<
    ProductListingRawValuesPatch<ListingAvailabilityQuickCheck>,
    ProductListingRawValuesNormalizationError,
> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) => quick_check_availability(raw.as_str())
            .map(ProductListingRawValuesPatch::Set)
            .map_err(map_availability_error),
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn normalize_url_patch(
    patch: ProductListingRawValuesPatch<String>,
    base_url: &Url,
) -> Result<ProductListingRawValuesPatch<Url>, ProductListingRawValuesNormalizationError> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) => Url::parse(raw.as_str())
            .or_else(|_| base_url.join(raw.as_str()))
            .map(ProductListingRawValuesPatch::Set)
            .map_err(ProductListingRawValuesNormalizationError::InvalidUrl),
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn normalize_images_patch(
    patch: ProductListingRawValuesPatch<Vec<String>>,
    base_url: &Url,
) -> Result<
    ProductListingRawValuesPatch<Vec<ProductListingImage>>,
    ProductListingRawValuesNormalizationError,
> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) => normalize_image_urls(raw, base_url)
            .map(ProductListingRawValuesPatch::Set)
            .map_err(map_image_error),
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn normalize_date_time_patch(
    patch: ProductListingRawValuesPatch<String>,
    field: DateTimeField,
) -> Result<ProductListingRawValuesPatch<OffsetDateTime>, ProductListingRawValuesNormalizationError>
{
    match patch {
        ProductListingRawValuesPatch::Set(raw) => normalize_date_time(Some(raw.as_str()))
            .map(|date_time| match date_time {
                Some(date_time) => ProductListingRawValuesPatch::Set(date_time),
                None => ProductListingRawValuesPatch::Clear,
            })
            .map_err(|error| map_date_time_error(error, field)),
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn map_price_error(
    error: PriceNormalizationError,
    field: PriceField,
) -> ProductListingRawValuesNormalizationError {
    let error = match error {
        PriceNormalizationError::UnknownCurrency => {
            NormalizationError::PriceUnknownCurrency { field }
        }
        PriceNormalizationError::ParseFailure => NormalizationError::PriceParseError { field },
    };
    ProductListingRawValuesNormalizationError::Price(error)
}

fn map_image_error(error: ImageUrlNormalizationError) -> ProductListingRawValuesNormalizationError {
    let error = match error {
        ImageUrlNormalizationError::InvalidUrl(source) => {
            NormalizationError::InvalidImageUrl(source)
        }
    };
    ProductListingRawValuesNormalizationError::ImageUrl(error)
}

fn map_date_time_error(
    _: DateTimeNormalizationError,
    field: DateTimeField,
) -> ProductListingRawValuesNormalizationError {
    ProductListingRawValuesNormalizationError::DateTime(NormalizationError::DateTimeParseError {
        field,
    })
}

fn map_availability_error(
    error: AvailabilityNormalizationError,
) -> ProductListingRawValuesNormalizationError {
    let error = match error {
        AvailabilityNormalizationError::InputTooLong { len, max } => {
            NormalizationError::AvailabilityTextTooLong { len, max }
        }
        AvailabilityNormalizationError::EmbeddedNul => {
            NormalizationError::AvailabilityTextEmbeddedNul
        }
        AvailabilityNormalizationError::RegexSetCompilationFailed => {
            NormalizationError::AvailabilityRegexSetCompilationFailed
        }
    };
    ProductListingRawValuesNormalizationError::Availability(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        NormalizationContext, RawProductListingPayloadFormat, RawProductListingValues,
        SourcePayload,
    };
    use rstest::rstest;
    use serde_json::json;
    use strum::IntoEnumIterator;

    fn input(
        operation: RawProductListingOperation,
        raw_values_schema_version: u16,
        raw_values: serde_json::Value,
        context: serde_json::Value,
    ) -> Result<ProductListingNormalizationInput, crate::NormalizationInputError> {
        ProductListingNormalizationInput::new(
            operation,
            RawProductListingPayloadFormat::CrawlerExtractedProduct,
            1,
            raw_values_schema_version,
            SourcePayload::new(json!({}))?,
            RawProductListingValues::new(raw_values)?,
            NormalizationContext::new(context)?,
        )
    }

    fn set(value: impl Serialize) -> serde_json::Value {
        json!({"action": "SET", "value": value})
    }

    fn clear() -> serde_json::Value {
        json!({"action": "CLEAR"})
    }

    fn unchanged() -> serde_json::Value {
        json!({"action": "UNCHANGED"})
    }

    fn context() -> serde_json::Value {
        json!({
            "baseUrl": "https://example.test/catalogue/",
            "fallbackCurrency": "EUR",
            "fallbackLanguage": "en"
        })
    }

    fn upsert_values() -> serde_json::Value {
        json!({
            "sourceListingId": "listing-123",
            "title": set("An antique ceramic vase from England"),
            "description": set(["This ceramic vase has a documented provenance."]),
            "price": set("100"),
            "priceEstimateMin": set("EUR 90"),
            "priceEstimateMax": set("EUR 120"),
            "availability": set("sold out"),
            "url": set("listings/123"),
            "images": set(["/images/one.jpg", "/images/one.jpg"]),
            "auctionStart": set("2026-01-01T12:00:00Z"),
            "auctionEnd": set("2026-01-02T12:00:00Z"),
            "attributes": {
                "material": set(["ceramic"]),
                "condition": unchanged()
            }
        })
    }

    fn v2_upsert_values(price_format: &str) -> serde_json::Value {
        let mut values = upsert_values();
        values["priceFormat"] = json!(price_format);
        values
    }

    #[test]
    fn should_serialize_v1_raw_values_with_explicit_patch_protocol_and_dynamic_attributes()
    -> Result<(), serde_json::Error> {
        let values = ProductListingRawValuesV1 {
            source_listing_id: "listing-123".to_owned(),
            title: ProductListingRawValuesPatch::Set("Vase".to_owned()),
            description: ProductListingRawValuesPatch::Clear,
            price: ProductListingRawValuesPatch::Unchanged,
            price_estimate_min: ProductListingRawValuesPatch::Unchanged,
            price_estimate_max: ProductListingRawValuesPatch::Unchanged,
            availability: ProductListingRawValuesPatch::Clear,
            url: ProductListingRawValuesPatch::Set("listing/123".to_owned()),
            images: ProductListingRawValuesPatch::Unchanged,
            auction_start: ProductListingRawValuesPatch::Unchanged,
            auction_end: ProductListingRawValuesPatch::Unchanged,
            attributes: BTreeMap::from([(
                "condition".to_owned(),
                ProductListingRawValuesPatch::Set(vec!["restored".to_owned()]),
            )]),
        };

        let json = serde_json::to_value(&values)?;
        assert_eq!(json["title"]["action"], "SET");
        assert_eq!(json["description"]["action"], "CLEAR");
        assert_eq!(json["price"]["action"], "UNCHANGED");
        assert!(json.get("priceFormat").is_none());
        assert_eq!(json["attributes"]["condition"]["value"][0], "restored");
        assert_eq!(
            serde_json::from_value::<ProductListingRawValuesV1>(json)?,
            values
        );
        Ok(())
    }

    #[test]
    fn should_reject_price_format_when_raw_values_schema_is_v1()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values();
        raw_values["priceFormat"] = json!("DISPLAY_TEXT");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V1,
            raw_values,
            context(),
        )?;

        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::InvalidRawValuesV1(_)
            )
        ));
        Ok(())
    }

    #[test]
    fn should_use_exact_v2_price_format_codes() -> Result<(), serde_json::Error> {
        let codes = ProductListingRawValuesPriceFormat::iter()
            .map(ProductListingRawValuesPriceFormat::as_str)
            .collect::<Vec<_>>();
        assert_eq!(codes, ["DISPLAY_TEXT", "MACHINE_DECIMAL"]);
        assert_eq!(
            codes.len(),
            codes
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        );

        for price_format in ProductListingRawValuesPriceFormat::iter() {
            let encoded = serde_json::to_value(price_format)?;
            assert_eq!(json!(price_format.as_str()), encoded);
            assert_eq!(
                price_format,
                serde_json::from_value::<ProductListingRawValuesPriceFormat>(encoded)?
            );
        }
        assert!(
            serde_json::from_value::<ProductListingRawValuesPriceFormat>(json!("machine_decimal"))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn should_serialize_v2_raw_values_with_required_explicit_price_format()
    -> Result<(), serde_json::Error> {
        let values: ProductListingRawValuesV2 =
            serde_json::from_value(v2_upsert_values("MACHINE_DECIMAL"))?;
        assert_eq!(
            ProductListingRawValuesPriceFormat::MachineDecimal,
            values.price_format
        );

        let json = serde_json::to_value(&values)?;
        assert_eq!(json["priceFormat"], "MACHINE_DECIMAL");
        assert_eq!(
            values,
            serde_json::from_value::<ProductListingRawValuesV2>(json)?
        );
        Ok(())
    }

    #[test]
    fn should_resolve_all_set_v1_upsert_fields_using_generic_context()
    -> Result<(), crate::NormalizationInputError> {
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V1,
            upsert_values(),
            context(),
        )?;

        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input);
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = outcome else {
            panic!("V1 UPSERT should resolve");
        };

        assert_eq!(resolved.source_listing_id.to_string(), "listing-123");
        assert_eq!(
            resolved.price,
            ProductListingRawValuesPatch::Set(ProductListingPrice::from(Price::new(
                10_000u64.into(),
                Currency::Eur,
            )))
        );
        assert_eq!(
            resolved.availability,
            ProductListingRawValuesPatch::Set(ListingAvailabilityQuickCheck::Resolved(
                product_listing_core::listing_availability::ListingAvailability::SoldOut
            ))
        );
        assert!(matches!(
            &resolved.url,
            ProductListingRawValuesPatch::Set(url)
                if url.as_str() == "https://example.test/catalogue/listings/123"
        ));
        let ProductListingRawValuesPatch::Set(images) = &resolved.images else {
            panic!("image patch should be set");
        };
        assert_eq!(images.len(), 1);
        assert_eq!(
            images[0].url().as_str(),
            "https://example.test/images/one.jpg"
        );
        assert!(matches!(
            resolved.attributes.get("material"),
            Some(ProductListingRawValuesPatch::Set(values)) if values == &["ceramic"]
        ));
        assert!(matches!(
            resolved.attributes.get("condition"),
            Some(ProductListingRawValuesPatch::Unchanged)
        ));
        Ok(())
    }

    #[rstest]
    #[case("£8,800", Currency::Gbp, 880_000_u64)]
    #[case("1.234,56", Currency::Eur, 123_456_u64)]
    fn should_preserve_v1_crawler_display_price_parsing(
        #[case] raw_price: &str,
        #[case] expected_currency: Currency,
        #[case] expected_minor_units: u64,
    ) -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values();
        raw_values["price"] = set(raw_price);
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V1,
            raw_values,
            context(),
        )?;

        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input);
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = outcome else {
            panic!("V1 crawler display price should resolve");
        };
        assert_eq!(
            ProductListingRawValuesPatch::Set(ProductListingPrice::from(Price::new(
                expected_minor_units.into(),
                expected_currency,
            ))),
            resolved.price
        );
        Ok(())
    }

    #[rstest]
    #[case("Price on request")]
    #[case("POA")]
    #[case("Preis auf Anfrage")]
    #[case("Prix sur demande")]
    #[case("Prezzo su richiesta")]
    #[case("Precio a consultar")]
    fn should_resolve_display_price_on_request_markers(
        #[case] raw_price: &str,
    ) -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values();
        raw_values["price"] = set(raw_price);
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V1,
            raw_values,
            context(),
        )?;

        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input);
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = outcome else {
            panic!("display marker should resolve");
        };
        assert_eq!(
            ProductListingRawValuesPatch::Set(ProductListingPrice::OnRequest),
            resolved.price
        );
        Ok(())
    }

    #[test]
    fn should_clear_display_text_request_markers_for_monetary_estimates()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values();
        raw_values["priceEstimateMin"] = set("Price on request");
        raw_values["priceEstimateMax"] = set("POA");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V1,
            raw_values,
            context(),
        )?;

        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input);
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = outcome else {
            panic!("request markers in estimate fields should resolve to clear patches");
        };
        assert_eq!(
            ProductListingRawValuesPatch::Clear,
            resolved.price_estimate_min
        );
        assert_eq!(
            ProductListingRawValuesPatch::Clear,
            resolved.price_estimate_max
        );
        Ok(())
    }

    #[test]
    fn should_resolve_v2_display_text_with_the_display_parser()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = v2_upsert_values("DISPLAY_TEXT");
        raw_values["price"] = set("1.234,56");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V2,
            raw_values,
            context(),
        )?;

        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input);
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = outcome else {
            panic!("V2 display text should resolve");
        };
        assert_eq!(
            ProductListingRawValuesPatch::Set(ProductListingPrice::from(Price::new(
                123_456_u64.into(),
                Currency::Eur,
            ))),
            resolved.price
        );
        Ok(())
    }

    #[test]
    fn should_resolve_v2_machine_decimals_to_exact_minor_units()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = v2_upsert_values("MACHINE_DECIMAL");
        raw_values["price"] = set("42.000");
        raw_values["priceEstimateMin"] = set("42.5");
        raw_values["priceEstimateMax"] = set("0");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V2,
            raw_values,
            context(),
        )?;

        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input);
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = outcome else {
            panic!("V2 machine decimals should resolve");
        };
        assert_eq!(
            ProductListingRawValuesPatch::Set(ProductListingPrice::from(Price::new(
                4_200_u64.into(),
                Currency::Eur,
            ))),
            resolved.price
        );
        assert_eq!(
            ProductListingRawValuesPatch::Set(Price::new(4_250_u64.into(), Currency::Eur)),
            resolved.price_estimate_min
        );
        assert_eq!(
            ProductListingRawValuesPatch::Set(Price::new(0_u64.into(), Currency::Eur)),
            resolved.price_estimate_max
        );
        Ok(())
    }

    #[rstest]
    #[case("price")]
    #[case("priceEstimateMin")]
    #[case("priceEstimateMax")]
    fn should_require_v2_machine_decimal_fallback_currency_when_any_price_patch_is_nonblank(
        #[case] field: &str,
    ) -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = v2_upsert_values("MACHINE_DECIMAL");
        raw_values["price"] = clear();
        raw_values["priceEstimateMin"] = clear();
        raw_values["priceEstimateMax"] = clear();
        raw_values[field] = set("42.00");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V2,
            raw_values,
            json!({"baseUrl": "https://example.test/catalogue/"}),
        )?;

        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::MachineDecimalFallbackCurrencyRequired
            )
        ));
        Ok(())
    }

    #[test]
    fn should_clear_blank_v2_machine_decimal_price_patches_when_fallback_currency_is_absent()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = v2_upsert_values("MACHINE_DECIMAL");
        raw_values["price"] = set(" ");
        raw_values["priceEstimateMin"] = set("");
        raw_values["priceEstimateMax"] = set("\t");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V2,
            raw_values,
            json!({"baseUrl": "https://example.test/catalogue/"}),
        )?;

        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input);
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = outcome else {
            panic!("blank V2 machine-decimal prices should resolve");
        };
        assert_eq!(ProductListingRawValuesPatch::Clear, resolved.price);
        assert_eq!(
            ProductListingRawValuesPatch::Clear,
            resolved.price_estimate_min
        );
        assert_eq!(
            ProductListingRawValuesPatch::Clear,
            resolved.price_estimate_max
        );
        Ok(())
    }

    #[test]
    fn should_reject_v2_machine_decimal_with_nonzero_excess_precision()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = v2_upsert_values("MACHINE_DECIMAL");
        raw_values["price"] = set("42.001");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V2,
            raw_values,
            context(),
        )?;

        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::Price(
                    NormalizationError::PriceParseError {
                        field: PriceField::Price
                    }
                )
            )
        ));
        Ok(())
    }

    #[test]
    fn should_preserve_clear_and_unchanged_without_normalizing_them()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values();
        raw_values["title"] = unchanged();
        raw_values["description"] = clear();
        raw_values["price"] = clear();
        raw_values["availability"] = unchanged();
        raw_values["images"] = clear();
        raw_values["auctionStart"] = unchanged();
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V1,
            raw_values,
            context(),
        )?;

        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input);
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = outcome else {
            panic!("patch-only UPSERT should resolve");
        };

        assert_eq!(resolved.title, ProductListingRawValuesPatch::Unchanged);
        assert_eq!(resolved.description, ProductListingRawValuesPatch::Clear);
        assert_eq!(resolved.price, ProductListingRawValuesPatch::Clear);
        assert_eq!(
            resolved.availability,
            ProductListingRawValuesPatch::Unchanged
        );
        assert_eq!(resolved.images, ProductListingRawValuesPatch::Clear);
        assert_eq!(
            resolved.auction_start,
            ProductListingRawValuesPatch::Unchanged
        );
        Ok(())
    }

    #[test]
    fn should_classify_availability_regex_configuration_failure_as_system() {
        let error =
            map_availability_error(AvailabilityNormalizationError::RegexSetCompilationFailed);

        assert_eq!(
            crate::error::NormalizationFailureScope::System,
            error.failure_scope()
        );
        assert!(matches!(
            error,
            ProductListingRawValuesNormalizationError::Availability(
                NormalizationError::AvailabilityRegexSetCompilationFailed
            )
        ));
    }

    #[test]
    fn should_classify_invalid_candidate_data_as_candidate_data() {
        assert_eq!(
            crate::error::NormalizationFailureScope::CandidateData,
            ProductListingRawValuesNormalizationError::Text(NormalizationError::TitleEmpty)
                .failure_scope()
        );
    }

    #[test]
    fn should_return_typed_invalid_outcome_when_composed_price_normalizer_fails()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values();
        raw_values["price"] = set("100");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V1,
            raw_values,
            json!({"baseUrl": "https://example.test/catalogue/"}),
        )?;

        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::Price(
                    NormalizationError::PriceUnknownCurrency {
                        field: PriceField::Price
                    }
                )
            )
        ));
        Ok(())
    }

    #[test]
    fn should_return_typed_invalid_outcome_for_invalid_raw_values_patch()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values();
        raw_values["title"] = json!({"action": "SET"});
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V1,
            raw_values,
            context(),
        )?;

        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::InvalidRawValuesV1(_)
            )
        ));
        Ok(())
    }

    #[test]
    fn should_return_typed_invalid_outcome_for_v2_without_explicit_price_format()
    -> Result<(), crate::NormalizationInputError> {
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V2,
            upsert_values(),
            context(),
        )?;

        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::InvalidRawValuesV2(_)
            )
        ));
        Ok(())
    }

    #[test]
    fn should_return_typed_invalid_outcome_for_invalid_normalization_context()
    -> Result<(), crate::NormalizationInputError> {
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION_V1,
            upsert_values(),
            json!({}),
        )?;

        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::InvalidNormalizationContextV1(_)
            )
        ));
        Ok(())
    }

    #[test]
    fn should_return_typed_invalid_outcome_for_unsupported_raw_values_schema()
    -> Result<(), crate::NormalizationInputError> {
        let input = input(
            RawProductListingOperation::Upsert,
            3,
            upsert_values(),
            context(),
        )?;

        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::UnsupportedRawValuesSchemaVersion {
                    version: 3
                }
            )
        ));
        Ok(())
    }

    #[test]
    fn should_return_delete_without_decoding_raw_values_or_context()
    -> Result<(), crate::NormalizationInputError> {
        let input = input(RawProductListingOperation::Delete, 99, json!({}), json!({}))?;

        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&input),
            ProductListingRawValuesNormalizationOutcome::Delete
        ));
        Ok(())
    }
}
