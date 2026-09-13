use crate::error::NormalizationFailureScope;
use crate::price::normalize_machine_decimal_price;
use crate::text::{detect_description_language, localize_normalized_title};
use crate::{
    AvailabilityNormalizationError, ImageUrlNormalizationError, ListingAvailabilityQuickCheck,
    NormalizationError, PriceField, PriceNormalizationError, ProductListingNormalizationInput,
    RawProductListingOperation, detect_language, normalize_description, normalize_image_urls,
    normalize_price, normalize_product_listing_price,
    normalize_source_listing_id_with_url_sha_fallback, normalize_title, quick_check_availability,
};
use auction_core::{
    AuctionDescription, AuctionFormat, AuctionName, AuctionReportedStatus, AuctionSchedule,
    AuctionTime, AuctionTimeZone, InvalidAuctionDescription, InvalidAuctionFormat,
    InvalidAuctionName, InvalidAuctionReportedStatus, InvalidAuctionSchedule,
    InvalidAuctionTimeZone, ReportedCatalogueLotCount, SourceAuctionId,
};
use localization::{Language, Localized};
use money::{Currency, Price};
use product_listing_core::{
    description::Description,
    product_listing::{
        CataloguePosition, InvalidCataloguePosition, InvalidLotAuctionTiming, InvalidLotNumber,
        LotAuctionTiming, LotNumber, ProductListingAuction,
    },
    product_listing_image::ProductListingImage,
    product_listing_price::ProductListingPrice,
    source_listing_id::SourceListingId,
    title::Title,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::Value;
use std::{collections::BTreeMap, str::FromStr};
use strum::IntoEnumIterator;
use time::{
    Date, OffsetDateTime, format_description::well_known::Rfc3339, macros::format_description,
};
use url::Url;

pub const PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION: u16 = 1;

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

fn raw_patch_unchanged<T>() -> ProductListingRawValuesPatch<T> {
    ProductListingRawValuesPatch::Unchanged
}

/// Provider-neutral raw values for one current UPSERT normalization input.
///
/// `priceFormat` is required and applies to the main and estimate price patches. `DISPLAY_TEXT`
/// preserves display-price parsing, while `MACHINE_DECIMAL` uses strict unsigned ASCII decimal
/// parsing with the normalization-context fallback currency for nonblank `SET` values. Blank
/// `SET` values normalize to `CLEAR`. Each mutable listing field uses the explicit
/// set/clear/unchanged protocol. Dynamic attributes use source-selected names and do not add
/// provider-specific fields to this contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductListingRawValues {
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
    pub auction: ProductListingRawValuesPatch<ProductListingRawValuesAuction>,
    #[serde(default)]
    pub attributes: BTreeMap<String, ProductListingRawValuesPatch<Vec<String>>>,
}

/// Strict raw auction context used by a `SET` auction patch.
///
/// Every field is optional because sources commonly assert only one lot fact. A
/// `SET` replaces the complete asserted context; use the outer patch's `CLEAR`
/// and `UNCHANGED` actions for absence and no observation respectively.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductListingRawValuesAuction {
    /// Source auction identity uses the same explicit presence protocol as every mutable raw
    /// value. `CLEAR` never detaches a current membership; correction owns detachment.
    #[serde(default = "raw_patch_unchanged")]
    pub source_auction_id: ProductListingRawValuesPatch<String>,
    #[serde(default)]
    pub lot_number: Option<String>,
    #[serde(default)]
    pub catalogue_position: Option<u64>,
    /// Timing is decoded separately so an invalid optional assertion cannot reject unrelated
    /// current raw values. The outer auction object remains strict.
    #[serde(default)]
    pub timing: Option<Value>,
    /// Listing-embedded shared metadata is a candidate only. The transactional resolver applies
    /// the fill-only authority policy after this pure normalization boundary.
    #[serde(default)]
    pub auction_metadata: ProductListingRawValuesAuctionMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductListingRawValuesAuctionMetadata {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub catalogue_url: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub reported_status: Option<String>,
    #[serde(default)]
    pub reported_lot_count: Option<u32>,
    #[serde(default)]
    pub schedule: ProductListingRawValuesAuctionMetadataSchedule,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductListingRawValuesAuctionMetadataSchedule {
    #[serde(default)]
    pub bidding_opens: Option<ProductListingRawValuesAuctionTime>,
    #[serde(default)]
    pub live_starts: Option<ProductListingRawValuesAuctionTime>,
    #[serde(default)]
    pub lots_begin_closing: Option<ProductListingRawValuesAuctionTime>,
    #[serde(default)]
    pub scheduled_end: Option<ProductListingRawValuesAuctionTime>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProductListingRawValuesAuctionMetadataResolved {
    pub name: Option<Localized<Language, AuctionName>>,
    pub description: Option<Localized<Language, AuctionDescription>>,
    pub catalogue_url: Option<Url>,
    pub format: Option<AuctionFormat>,
    pub reported_status: Option<AuctionReportedStatus>,
    pub reported_lot_count: Option<ReportedCatalogueLotCount>,
    pub bidding_opens: Option<AuctionTime>,
    pub live_starts: Option<AuctionTime>,
    pub lots_begin_closing: Option<AuctionTime>,
    pub scheduled_end: Option<AuctionTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductListingRawValuesLotAuctionTiming {
    #[serde(default)]
    pub bidding_opens: Option<ProductListingRawValuesAuctionTime>,
    #[serde(default)]
    pub scheduled_closes: Option<ProductListingRawValuesAuctionTime>,
    #[serde(default)]
    pub reported_closed_at: Option<ProductListingRawValuesAuctionTime>,
}

/// A source-declared auction-time precision. `DATE` never becomes a midnight
/// instant during normalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "precision",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProductListingRawValuesAuctionTime {
    Date {
        value: String,
        #[serde(default)]
        source_timezone: Option<String>,
    },
    Instant {
        value: String,
        #[serde(default)]
        source_timezone: Option<String>,
    },
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
    pub auction: ProductListingRawValuesPatch<ProductListingAuction>,
    /// Reliable source identifier only when the outer auction context was asserted.
    pub auction_source_id: Option<SourceAuctionId>,
    /// Validated embedded shared metadata. It is actionable only alongside a reliable source key.
    pub auction_metadata: ProductListingRawValuesAuctionMetadataResolved,
    pub attributes: BTreeMap<String, ProductListingRawValuesPatch<Vec<String>>>,
    /// Safe fixed-code metadata for a non-fatal normalization loss.
    pub diagnostic: Option<ProductListingRawValuesNormalizationDiagnostic>,
}

/// Stable safe diagnostic for a non-fatal raw-values normalization loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum ProductListingRawValuesNormalizationDiagnostic {
    AuctionTimingInvalid,
    AuctionReferenceInvalid,
    MembershipChangeRequiresCorrection,
}

impl ProductListingRawValuesNormalizationDiagnostic {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuctionTimingInvalid => "AUCTION_TIMING_INVALID",
            Self::AuctionReferenceInvalid => "AUCTION_REFERENCE_INVALID",
            Self::MembershipChangeRequiresCorrection => "MEMBERSHIP_CHANGE_REQUIRES_CORRECTION",
        }
    }
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
    #[error("raw values do not match the current contract")]
    InvalidRawValues(#[source] serde_json::Error),
    #[error("normalization context does not match the current contract")]
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
    #[error("auction context is invalid")]
    Auction(#[source] ProductListingRawValuesAuctionNormalizationError),
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
            | Self::Availability(error) => error.failure_scope(),
            _ => NormalizationFailureScope::CandidateData,
        }
    }
}

/// Pure raw-values normalizer for the current schema.
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
        if input.raw_values_schema_version() != PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION {
            return Err(
                ProductListingRawValuesNormalizationError::UnsupportedRawValuesSchemaVersion {
                    version: input.raw_values_schema_version(),
                },
            );
        }
        let raw: ProductListingRawValues =
            serde_json::from_value(input.raw_values().value().clone())
                .map_err(ProductListingRawValuesNormalizationError::InvalidRawValues)?;
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
        let (auction, auction_source_id, auction_metadata, diagnostic) =
            normalize_auction_patch(raw.auction, &base_url, fallback_language)
                .map_err(ProductListingRawValuesNormalizationError::Auction)?;

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
            auction,
            auction_source_id,
            auction_metadata,
            attributes: raw.attributes,
            diagnostic,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LotAuctionTimingField {
    BiddingOpens,
    ScheduledCloses,
    ReportedClosedAt,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingRawValuesAuctionNormalizationError {
    #[error("auction timing does not match the current contract")]
    TimingContract(#[source] serde_json::Error),
    #[error("auction metadata is invalid")]
    Metadata(#[source] ProductListingRawValuesAuctionMetadataNormalizationError),
    #[error("lot number is invalid")]
    LotNumber(#[source] InvalidLotNumber),
    #[error("catalogue position is invalid")]
    CataloguePosition(#[source] InvalidCataloguePosition),
    #[error("auction {field:?} is invalid")]
    Time { field: LotAuctionTimingField },
    #[error("auction {field:?} source timezone is invalid")]
    SourceTimezone {
        field: LotAuctionTimingField,
        #[source]
        source: InvalidAuctionTimeZone,
    },
    #[error("reported auction closure must have INSTANT precision")]
    ReportedClosedAtMustBeInstant,
    #[error("auction timing is invalid")]
    Timing(#[source] InvalidLotAuctionTiming),
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingRawValuesAuctionMetadataNormalizationError {
    #[error("auction metadata name is invalid")]
    Name(#[source] InvalidAuctionName),
    #[error("auction metadata description is invalid")]
    Description(#[source] InvalidAuctionDescription),
    #[error("auction metadata name language cannot be determined")]
    NameLanguage,
    #[error("auction metadata description language cannot be determined")]
    DescriptionLanguage,
    #[error("auction metadata catalogue URL is invalid")]
    CatalogueUrl(#[source] url::ParseError),
    #[error("auction metadata format is invalid")]
    Format(#[source] InvalidAuctionFormat),
    #[error("auction metadata reported status is invalid")]
    ReportedStatus(#[source] InvalidAuctionReportedStatus),
    #[error("auction metadata {field} time is invalid")]
    Time {
        field: &'static str,
        #[source]
        source: Box<ProductListingRawValuesAuctionNormalizationError>,
    },
    #[error("auction metadata schedule is invalid")]
    Schedule(#[source] InvalidAuctionSchedule),
}

type NormalizedAuctionPatch = (
    ProductListingRawValuesPatch<ProductListingAuction>,
    Option<SourceAuctionId>,
    ProductListingRawValuesAuctionMetadataResolved,
    Option<ProductListingRawValuesNormalizationDiagnostic>,
);

fn normalize_auction_patch(
    patch: ProductListingRawValuesPatch<ProductListingRawValuesAuction>,
    base_url: &Url,
    fallback_language: Option<Language>,
) -> Result<NormalizedAuctionPatch, ProductListingRawValuesAuctionNormalizationError> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) => {
            let metadata =
                normalize_auction_metadata(raw.auction_metadata, base_url, fallback_language)
                    .map_err(ProductListingRawValuesAuctionNormalizationError::Metadata)?;
            let source_auction_id = match raw.source_auction_id {
                ProductListingRawValuesPatch::Set(value) => Some(SourceAuctionId::try_from(value)),
                ProductListingRawValuesPatch::Clear => {
                    return Ok((
                        ProductListingRawValuesPatch::Set(ProductListingAuction::new(
                            None,
                            raw.lot_number
                                .map(LotNumber::try_from)
                                .transpose()
                                .map_err(ProductListingRawValuesAuctionNormalizationError::LotNumber)?,
                            raw.catalogue_position
                                .map(CataloguePosition::try_from)
                                .transpose()
                                .map_err(ProductListingRawValuesAuctionNormalizationError::CataloguePosition)?,
                            None,
                        )),
                        None,
                        metadata,
                        Some(
                            ProductListingRawValuesNormalizationDiagnostic::MembershipChangeRequiresCorrection,
                        ),
                    ));
                }
                ProductListingRawValuesPatch::Unchanged => None,
            };
            let lot_number = raw
                .lot_number
                .map(LotNumber::try_from)
                .transpose()
                .map_err(ProductListingRawValuesAuctionNormalizationError::LotNumber)?;
            let catalogue_position = raw
                .catalogue_position
                .map(CataloguePosition::try_from)
                .transpose()
                .map_err(ProductListingRawValuesAuctionNormalizationError::CataloguePosition)?;
            let timing = match raw.timing {
                Some(raw_timing) => match serde_json::from_value(raw_timing)
                    .map_err(ProductListingRawValuesAuctionNormalizationError::TimingContract)
                    .and_then(normalize_lot_auction_timing)
                {
                    Ok(timing) => Some(timing),
                    Err(_error) => {
                        return Ok((
                            ProductListingRawValuesPatch::Unchanged,
                            None,
                            metadata,
                            Some(
                                ProductListingRawValuesNormalizationDiagnostic::AuctionTimingInvalid,
                            ),
                        ));
                    }
                },
                None => None,
            };
            match source_auction_id {
                Some(Ok(source_auction_id)) => Ok((
                    ProductListingRawValuesPatch::Set(ProductListingAuction::new(
                        None,
                        lot_number,
                        catalogue_position,
                        timing,
                    )),
                    Some(source_auction_id),
                    metadata,
                    None,
                )),
                Some(Err(_)) => Ok((
                    ProductListingRawValuesPatch::Set(ProductListingAuction::new(
                        None,
                        lot_number,
                        catalogue_position,
                        timing,
                    )),
                    None,
                    metadata,
                    Some(ProductListingRawValuesNormalizationDiagnostic::AuctionReferenceInvalid),
                )),
                None => Ok((
                    ProductListingRawValuesPatch::Set(ProductListingAuction::new(
                        None,
                        lot_number,
                        catalogue_position,
                        timing,
                    )),
                    None,
                    metadata,
                    None,
                )),
            }
        }
        ProductListingRawValuesPatch::Clear => Ok((
            ProductListingRawValuesPatch::Clear,
            None,
            ProductListingRawValuesAuctionMetadataResolved::default(),
            None,
        )),
        ProductListingRawValuesPatch::Unchanged => Ok((
            ProductListingRawValuesPatch::Unchanged,
            None,
            ProductListingRawValuesAuctionMetadataResolved::default(),
            None,
        )),
    }
}

fn normalize_auction_metadata(
    raw: ProductListingRawValuesAuctionMetadata,
    base_url: &Url,
    fallback_language: Option<Language>,
) -> Result<
    ProductListingRawValuesAuctionMetadataResolved,
    ProductListingRawValuesAuctionMetadataNormalizationError,
> {
    let description = raw
        .description
        .map(AuctionDescription::try_from)
        .transpose()
        .map_err(ProductListingRawValuesAuctionMetadataNormalizationError::Description)?;
    let description_language = description
        .as_ref()
        .and_then(|value| detect_language(value.as_ref()))
        .or(fallback_language);
    let description = description
        .map(|value| {
            description_language
                .map(|language| Localized::new(language, value))
                .ok_or(
                    ProductListingRawValuesAuctionMetadataNormalizationError::DescriptionLanguage,
                )
        })
        .transpose()?;
    let name = raw
        .name
        .map(AuctionName::try_from)
        .transpose()
        .map_err(ProductListingRawValuesAuctionMetadataNormalizationError::Name)?;
    let name = name
        .map(|value| {
            detect_language(value.as_ref())
                .or(description_language)
                .map(|language| Localized::new(language, value))
                .ok_or(ProductListingRawValuesAuctionMetadataNormalizationError::NameLanguage)
        })
        .transpose()?;
    let catalogue_url = raw
        .catalogue_url
        .map(|value| Url::parse(value.as_str()).or_else(|_| base_url.join(value.as_str())))
        .transpose()
        .map_err(ProductListingRawValuesAuctionMetadataNormalizationError::CatalogueUrl)?;
    let format = raw
        .format
        .as_deref()
        .map(AuctionFormat::from_str)
        .transpose()
        .map_err(ProductListingRawValuesAuctionMetadataNormalizationError::Format)?;
    let reported_status = raw
        .reported_status
        .as_deref()
        .map(AuctionReportedStatus::from_str)
        .transpose()
        .map_err(ProductListingRawValuesAuctionMetadataNormalizationError::ReportedStatus)?;
    let bidding_opens = normalize_auction_metadata_time(
        raw.schedule.bidding_opens,
        "biddingOpens",
        LotAuctionTimingField::BiddingOpens,
    )?;
    let live_starts = normalize_auction_metadata_time(
        raw.schedule.live_starts,
        "liveStarts",
        LotAuctionTimingField::BiddingOpens,
    )?;
    let lots_begin_closing = normalize_auction_metadata_time(
        raw.schedule.lots_begin_closing,
        "lotsBeginClosing",
        LotAuctionTimingField::ScheduledCloses,
    )?;
    let scheduled_end = normalize_auction_metadata_time(
        raw.schedule.scheduled_end,
        "scheduledEnd",
        LotAuctionTimingField::ScheduledCloses,
    )?;
    AuctionSchedule::new(
        bidding_opens.clone(),
        live_starts.clone(),
        lots_begin_closing.clone(),
        scheduled_end.clone(),
    )
    .map_err(ProductListingRawValuesAuctionMetadataNormalizationError::Schedule)?;

    Ok(ProductListingRawValuesAuctionMetadataResolved {
        name,
        description,
        catalogue_url,
        format,
        reported_status,
        reported_lot_count: raw.reported_lot_count.map(ReportedCatalogueLotCount::new),
        bidding_opens,
        live_starts,
        lots_begin_closing,
        scheduled_end,
    })
}

fn normalize_auction_metadata_time(
    raw: Option<ProductListingRawValuesAuctionTime>,
    field: &'static str,
    lot_field: LotAuctionTimingField,
) -> Result<Option<AuctionTime>, ProductListingRawValuesAuctionMetadataNormalizationError> {
    raw.map(|value| normalize_auction_time(value, lot_field))
        .transpose()
        .map_err(
            |source| ProductListingRawValuesAuctionMetadataNormalizationError::Time {
                field,
                source: Box::new(source),
            },
        )
}

fn normalize_lot_auction_timing(
    raw: ProductListingRawValuesLotAuctionTiming,
) -> Result<LotAuctionTiming, ProductListingRawValuesAuctionNormalizationError> {
    let bidding_opens = raw
        .bidding_opens
        .map(|value| normalize_auction_time(value, LotAuctionTimingField::BiddingOpens))
        .transpose()?;
    let scheduled_closes = raw
        .scheduled_closes
        .map(|value| normalize_auction_time(value, LotAuctionTimingField::ScheduledCloses))
        .transpose()?;
    let reported_closed_at = raw
        .reported_closed_at
        .map(normalize_reported_closed_at)
        .transpose()?;
    LotAuctionTiming::new(bidding_opens, scheduled_closes, reported_closed_at)
        .map_err(ProductListingRawValuesAuctionNormalizationError::Timing)
}

fn normalize_reported_closed_at(
    raw: ProductListingRawValuesAuctionTime,
) -> Result<OffsetDateTime, ProductListingRawValuesAuctionNormalizationError> {
    let time = normalize_auction_time(raw, LotAuctionTimingField::ReportedClosedAt)?;
    time.exact_instant()
        .ok_or(ProductListingRawValuesAuctionNormalizationError::ReportedClosedAtMustBeInstant)
}

fn normalize_auction_time(
    raw: ProductListingRawValuesAuctionTime,
    field: LotAuctionTimingField,
) -> Result<AuctionTime, ProductListingRawValuesAuctionNormalizationError> {
    let (value, source_timezone, date_precision) = match raw {
        ProductListingRawValuesAuctionTime::Date {
            value,
            source_timezone,
        } => (value, source_timezone, true),
        ProductListingRawValuesAuctionTime::Instant {
            value,
            source_timezone,
        } => (value, source_timezone, false),
    };
    let source_timezone = source_timezone
        .as_deref()
        .map(AuctionTimeZone::try_from)
        .transpose()
        .map_err(
            |source| ProductListingRawValuesAuctionNormalizationError::SourceTimezone {
                field,
                source,
            },
        )?;
    if date_precision {
        return Date::parse(value.trim(), &format_description!("[year]-[month]-[day]"))
            .map(|on| AuctionTime::date(on, source_timezone))
            .map_err(|_| ProductListingRawValuesAuctionNormalizationError::Time { field });
    }
    OffsetDateTime::parse(value.trim(), &Rfc3339)
        .map(|at| AuctionTime::instant(at, source_timezone))
        .map_err(|_| ProductListingRawValuesAuctionNormalizationError::Time { field })
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
    use time::{Date, Month};

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
            "priceFormat": "DISPLAY_TEXT",
            "description": set(["This ceramic vase has a documented provenance."]),
            "price": set("100"),
            "priceEstimateMin": set("EUR 90"),
            "priceEstimateMax": set("EUR 120"),
            "availability": set("sold out"),
            "url": set("listings/123"),
            "images": set(["/images/one.jpg", "/images/one.jpg"]),
            "auction": set(json!({
                "lotNumber": "42A",
                "cataloguePosition": 7,
                "timing": {
                    "biddingOpens": {"precision": "DATE", "value": "2026-01-01", "sourceTimezone": "Europe/Berlin"},
                    "scheduledCloses": {"precision": "INSTANT", "value": "2026-01-02T12:00:00Z"},
                    "reportedClosedAt": {"precision": "INSTANT", "value": "2026-01-02T12:05:00Z"}
                }
            })),
            "attributes": {
                "material": set(["ceramic"]),
                "condition": unchanged()
            }
        })
    }

    fn upsert_values_with_price_format(price_format: &str) -> serde_json::Value {
        let mut values = upsert_values();
        values["priceFormat"] = json!(price_format);
        values
    }

    #[test]
    fn should_serialize_current_raw_values_with_explicit_patch_protocol_and_dynamic_attributes()
    -> Result<(), serde_json::Error> {
        let values = ProductListingRawValues {
            source_listing_id: "listing-123".to_owned(),
            title: ProductListingRawValuesPatch::Set("Vase".to_owned()),
            description: ProductListingRawValuesPatch::Clear,
            price_format: ProductListingRawValuesPriceFormat::DisplayText,
            price: ProductListingRawValuesPatch::Unchanged,
            price_estimate_min: ProductListingRawValuesPatch::Unchanged,
            price_estimate_max: ProductListingRawValuesPatch::Unchanged,
            availability: ProductListingRawValuesPatch::Clear,
            url: ProductListingRawValuesPatch::Set("listing/123".to_owned()),
            images: ProductListingRawValuesPatch::Unchanged,
            auction: ProductListingRawValuesPatch::Unchanged,
            attributes: BTreeMap::from([(
                "condition".to_owned(),
                ProductListingRawValuesPatch::Set(vec!["restored".to_owned()]),
            )]),
        };

        let json = serde_json::to_value(&values)?;
        assert_eq!(json["title"]["action"], "SET");
        assert_eq!(json["description"]["action"], "CLEAR");
        assert_eq!(json["price"]["action"], "UNCHANGED");
        assert_eq!(json["priceFormat"], "DISPLAY_TEXT");
        assert_eq!(json["attributes"]["condition"]["value"][0], "restored");
        assert_eq!(
            serde_json::from_value::<ProductListingRawValues>(json)?,
            values
        );
        Ok(())
    }

    #[test]
    fn should_require_explicit_price_format_in_current_raw_values()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values();
        let Some(values) = raw_values.as_object_mut() else {
            panic!("raw test fixture must be an object");
        };
        values.remove("priceFormat");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            raw_values,
            context(),
        )?;

        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::InvalidRawValues(_)
            )
        ));
        Ok(())
    }

    #[test]
    fn should_use_exact_current_price_format_codes() -> Result<(), serde_json::Error> {
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
    fn should_use_stable_normalization_diagnostic_codes() {
        let codes = ProductListingRawValuesNormalizationDiagnostic::iter()
            .map(ProductListingRawValuesNormalizationDiagnostic::as_str)
            .collect::<Vec<_>>();

        assert_eq!(
            codes,
            [
                "AUCTION_TIMING_INVALID",
                "AUCTION_REFERENCE_INVALID",
                "MEMBERSHIP_CHANGE_REQUIRES_CORRECTION",
            ]
        );
        assert_eq!(
            codes.len(),
            codes
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        );
    }

    #[test]
    fn should_serialize_current_raw_values_with_required_explicit_price_format()
    -> Result<(), serde_json::Error> {
        let values: ProductListingRawValues =
            serde_json::from_value(upsert_values_with_price_format("MACHINE_DECIMAL"))?;
        assert_eq!(
            ProductListingRawValuesPriceFormat::MachineDecimal,
            values.price_format
        );

        let json = serde_json::to_value(&values)?;
        assert_eq!(json["priceFormat"], "MACHINE_DECIMAL");
        assert_eq!(
            values,
            serde_json::from_value::<ProductListingRawValues>(json)?
        );
        Ok(())
    }

    #[test]
    fn should_resolve_all_set_current_upsert_fields_using_generic_context()
    -> Result<(), crate::NormalizationInputError> {
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            upsert_values(),
            context(),
        )?;

        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input);
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = outcome else {
            panic!("current UPSERT should resolve");
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
        let ProductListingRawValuesPatch::Set(auction) = &resolved.auction else {
            panic!("auction context should resolve");
        };
        assert_eq!(Some("42A"), auction.lot_number().map(LotNumber::as_str));
        assert_eq!(
            Some(7),
            auction.catalogue_position().map(CataloguePosition::value)
        );
        let timing = auction
            .timing()
            .unwrap_or_else(|| panic!("auction timing should resolve"));
        assert_eq!(
            Some(
                Date::from_calendar_date(2026, Month::January, 1)
                    .unwrap_or_else(|error| panic!("valid date: {error}")),
            ),
            timing.bidding_opens().and_then(AuctionTime::source_date)
        );
        assert!(
            timing
                .scheduled_closes()
                .and_then(AuctionTime::exact_instant)
                .is_some()
        );
        assert_eq!(
            Some(
                OffsetDateTime::parse("2026-01-02T12:05:00Z", &Rfc3339)
                    .unwrap_or_else(|error| panic!("valid instant: {error}"))
            ),
            timing.reported_closed_at()
        );
        Ok(())
    }

    #[rstest]
    #[case("£8,800", Currency::Gbp, 880_000_u64)]
    #[case("1.234,56", Currency::Eur, 123_456_u64)]
    fn should_preserve_crawler_display_price_parsing(
        #[case] raw_price: &str,
        #[case] expected_currency: Currency,
        #[case] expected_minor_units: u64,
    ) -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values();
        raw_values["price"] = set(raw_price);
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            raw_values,
            context(),
        )?;

        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input);
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = outcome else {
            panic!("current crawler display price should resolve");
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
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
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
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
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
    fn should_resolve_current_display_text_with_the_display_parser()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values_with_price_format("DISPLAY_TEXT");
        raw_values["price"] = set("1.234,56");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            raw_values,
            context(),
        )?;

        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input);
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = outcome else {
            panic!("current display text should resolve");
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
    fn should_resolve_current_machine_decimals_to_exact_minor_units()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values_with_price_format("MACHINE_DECIMAL");
        raw_values["price"] = set("42.000");
        raw_values["priceEstimateMin"] = set("42.5");
        raw_values["priceEstimateMax"] = set("0");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            raw_values,
            context(),
        )?;

        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input);
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = outcome else {
            panic!("current machine decimals should resolve");
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
    fn should_require_machine_decimal_fallback_currency_when_any_price_patch_is_nonblank(
        #[case] field: &str,
    ) -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values_with_price_format("MACHINE_DECIMAL");
        raw_values["price"] = clear();
        raw_values["priceEstimateMin"] = clear();
        raw_values["priceEstimateMax"] = clear();
        raw_values[field] = set("42.00");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
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
    fn should_clear_blank_machine_decimal_price_patches_when_fallback_currency_is_absent()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values_with_price_format("MACHINE_DECIMAL");
        raw_values["price"] = set(" ");
        raw_values["priceEstimateMin"] = set("");
        raw_values["priceEstimateMax"] = set("\t");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            raw_values,
            json!({"baseUrl": "https://example.test/catalogue/"}),
        )?;

        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input);
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = outcome else {
            panic!("blank current machine-decimal prices should resolve");
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
    fn should_reject_machine_decimal_with_nonzero_excess_precision()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values_with_price_format("MACHINE_DECIMAL");
        raw_values["price"] = set("42.001");
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
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
        raw_values["auction"] = unchanged();
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
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
        assert_eq!(resolved.auction, ProductListingRawValuesPatch::Unchanged);
        Ok(())
    }

    #[test]
    fn should_preserve_auction_clear_and_ignore_invalid_optional_timing()
    -> Result<(), crate::NormalizationInputError> {
        let mut clear_auction = upsert_values();
        clear_auction["auction"] = clear();
        let clear_input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            clear_auction,
            context(),
        )?;
        let ProductListingRawValuesNormalizationOutcome::Resolved(clear_resolved) =
            ProductListingRawValuesNormalizer::new().normalize(&clear_input)
        else {
            panic!("clear auction patch should resolve");
        };
        assert_eq!(ProductListingRawValuesPatch::Clear, clear_resolved.auction);
        assert_eq!(None, clear_resolved.diagnostic);

        let mut invalid_timing = upsert_values();
        invalid_timing["auction"] = set(json!({
            "timing": {
                "biddingOpens": {
                    "precision": "DATE",
                    "value": "2026-05-14",
                    "sourceTimezone": "Europe/Berlin"
                },
                "scheduledCloses": {
                    "precision": "DATE",
                    "value": "2026-05-13",
                    "sourceTimezone": "Europe/Berlin"
                }
            }
        }));
        let invalid_input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            invalid_timing,
            context(),
        )?;
        let ProductListingRawValuesNormalizationOutcome::Resolved(invalid_resolved) =
            ProductListingRawValuesNormalizer::new().normalize(&invalid_input)
        else {
            panic!("invalid optional timing should not reject the raw revision");
        };
        assert!(matches!(
            invalid_resolved.title,
            ProductListingRawValuesPatch::Set(_)
        ));
        assert_eq!(
            ProductListingRawValuesPatch::Unchanged,
            invalid_resolved.auction
        );
        assert_eq!(
            Some(ProductListingRawValuesNormalizationDiagnostic::AuctionTimingInvalid),
            invalid_resolved.diagnostic
        );
        Ok(())
    }

    #[test]
    fn should_normalize_embedded_auction_metadata_with_its_own_schedule_roles()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values();
        raw_values["auction"] = set(json!({
            "sourceAuctionId": {"action": "SET", "value": "catalogue-2026-0042"},
            "auctionMetadata": {
                "name": "Autumn Decorative Arts",
                "description": "A carefully selected catalogue of decorative arts.",
                "catalogueUrl": "auctions/autumn-2026",
                "format": "TIMED",
                "reportedStatus": "SCHEDULED",
                "reportedLotCount": 42,
                "schedule": {
                    "biddingOpens": {"precision": "DATE", "value": "2026-10-01", "sourceTimezone": "Europe/Berlin"},
                    "lotsBeginClosing": {"precision": "INSTANT", "value": "2026-10-18T16:03:00Z", "sourceTimezone": "Europe/Berlin"}
                }
            }
        }));
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            raw_values,
            context(),
        )?;

        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) =
            ProductListingRawValuesNormalizer::new().normalize(&input)
        else {
            panic!("valid embedded auction metadata should resolve");
        };

        assert_eq!(
            Some("Autumn Decorative Arts"),
            resolved
                .auction_metadata
                .name
                .as_ref()
                .map(|value| value.payload.as_ref())
        );
        assert_eq!(
            Some("A carefully selected catalogue of decorative arts."),
            resolved
                .auction_metadata
                .description
                .as_ref()
                .map(|value| value.payload.as_ref())
        );
        assert_eq!(
            Some("https://example.test/catalogue/auctions/autumn-2026"),
            resolved
                .auction_metadata
                .catalogue_url
                .as_ref()
                .map(Url::as_str)
        );
        assert_eq!(Some(AuctionFormat::Timed), resolved.auction_metadata.format);
        assert_eq!(
            Some(AuctionReportedStatus::Scheduled),
            resolved.auction_metadata.reported_status
        );
        assert_eq!(
            Some(42),
            resolved
                .auction_metadata
                .reported_lot_count
                .map(ReportedCatalogueLotCount::value)
        );
        assert!(matches!(
            resolved.auction_metadata.bidding_opens,
            Some(AuctionTime::Date { .. })
        ));
        assert!(matches!(
            resolved.auction_metadata.lots_begin_closing,
            Some(AuctionTime::Instant { .. })
        ));
        Ok(())
    }

    #[test]
    fn should_reject_invalid_embedded_auction_metadata_without_accepting_an_outer_alias()
    -> Result<(), crate::NormalizationInputError> {
        let mut raw_values = upsert_values();
        raw_values["auction"] = set(json!({
            "auctionMetadata": {"format": "timed"}
        }));
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            raw_values,
            context(),
        )?;

        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::Auction(
                    ProductListingRawValuesAuctionNormalizationError::Metadata(
                        ProductListingRawValuesAuctionMetadataNormalizationError::Format(_)
                    )
                )
            )
        ));
        Ok(())
    }

    #[test]
    fn should_ignore_malformed_optional_timing_and_reject_invalid_outer_auction()
    -> Result<(), crate::NormalizationInputError> {
        let mut malformed_timing = upsert_values();
        malformed_timing["auction"] = set(json!({
            "timing": {"biddingOpens": {"value": "2026-05-13"}}
        }));
        let malformed_timing_input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            malformed_timing,
            context(),
        )?;
        let ProductListingRawValuesNormalizationOutcome::Resolved(malformed_timing_resolved) =
            ProductListingRawValuesNormalizer::new().normalize(&malformed_timing_input)
        else {
            panic!("malformed optional timing should not reject the raw revision");
        };
        assert_eq!(
            ProductListingRawValuesPatch::Unchanged,
            malformed_timing_resolved.auction
        );
        assert_eq!(
            Some(ProductListingRawValuesNormalizationDiagnostic::AuctionTimingInvalid),
            malformed_timing_resolved.diagnostic
        );

        let mut malformed_outer_auction = upsert_values();
        malformed_outer_auction["auction"] = set(json!({"unexpected": "value"}));
        let malformed_outer_auction_input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            malformed_outer_auction,
            context(),
        )?;
        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&malformed_outer_auction_input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::InvalidRawValues(_)
            )
        ));

        let mut invalid_lot_number = upsert_values();
        invalid_lot_number["auction"] = set(json!({"lotNumber": ""}));
        let invalid_lot_number_input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            invalid_lot_number,
            context(),
        )?;
        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&invalid_lot_number_input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::Auction(
                    ProductListingRawValuesAuctionNormalizationError::LotNumber(_)
                )
            )
        ));

        let mut invalid_catalogue_position = upsert_values();
        invalid_catalogue_position["auction"] = set(json!({"cataloguePosition": 0}));
        let invalid_catalogue_position_input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            invalid_catalogue_position,
            context(),
        )?;
        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&invalid_catalogue_position_input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::Auction(
                    ProductListingRawValuesAuctionNormalizationError::CataloguePosition(_)
                )
            )
        ));
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
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
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
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            raw_values,
            context(),
        )?;

        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::InvalidRawValues(_)
            )
        ));
        Ok(())
    }

    #[test]
    fn should_reject_removed_raw_values_schema_discriminator()
    -> Result<(), crate::NormalizationInputError> {
        let input = input(
            RawProductListingOperation::Upsert,
            2,
            upsert_values(),
            context(),
        )?;

        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&input),
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::UnsupportedRawValuesSchemaVersion {
                    version: 2
                }
            )
        ));
        Ok(())
    }

    #[test]
    fn should_return_typed_invalid_outcome_for_invalid_normalization_context()
    -> Result<(), crate::NormalizationInputError> {
        let input = input(
            RawProductListingOperation::Upsert,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
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
