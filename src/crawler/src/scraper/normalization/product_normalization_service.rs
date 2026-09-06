pub use super::error::NormalizationError;

use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use money::Currency;
use product_listing_normalization::{
    AvailabilityNormalizationError, DateTimeField, DateTimeNormalizationError,
    ImageUrlNormalizationError, PriceField, PriceNormalizationError, normalize_date_time,
    normalize_description, normalize_image_urls, normalize_price,
    normalize_source_listing_id_with_url_sha_fallback, normalize_title, quick_check_availability,
};
use url::Url;

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductListingNormalizationService: Send + Sync {
    /// Normalizes crawler-extracted values using only the shared pure kernel.
    async fn normalize(
        &self,
        raw: RawExtractedProduct,
        url: Url,
        default_currency: Option<Currency>,
    ) -> ProductListingNormalizationResult;
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizationSuccess {
    /// Candidate-local prepared values. They support plausibility and ranking;
    /// canonical writes use the separate raw-input handoff.
    pub prepared: PreparedProduct,
    /// Deterministic preparation never consumes LLM budget.
    pub llm_calls_used: u32,
}

#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct NormalizationFailure {
    pub error: NormalizationError,
    pub llm_calls_used: u32,
}

pub type ProductListingNormalizationResult = Result<NormalizationSuccess, NormalizationFailure>;

/// Deterministic candidate-local product data. Source mapping stays in crawler.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedProduct {
    /// Pure crawler outcome used only for local disposition.
    pub availability: product_listing_normalization::ListingAvailabilityQuickCheck,
    pub source_listing_id: product_listing_core::source_listing_id::SourceListingId,
    pub title: localization::Localized<localization::Language, product_listing_core::title::Title>,
    pub description: Option<
        localization::Localized<
            localization::Language,
            product_listing_core::description::Description,
        >,
    >,
    pub price: Option<money::Price>,
    pub price_estimate_min: Option<money::Price>,
    pub price_estimate_max: Option<money::Price>,
    pub images: Vec<product_listing_core::product_listing_image::ProductListingImage>,
    pub auction_start: Option<time::OffsetDateTime>,
    pub auction_end: Option<time::OffsetDateTime>,
    pub raw_attributes: std::collections::BTreeMap<String, Vec<String>>,
    pub raw_state: String,
    pub url: Url,
}

pub fn prepare_product(
    raw: RawExtractedProduct,
    url: Url,
    default_currency: Option<Currency>,
) -> Result<PreparedProduct, NormalizationError> {
    let availability =
        quick_check_availability(raw.state.as_str()).map_err(map_availability_error)?;
    let source_listing_id =
        normalize_source_listing_id_with_url_sha_fallback(&raw.source_listing_id, &url)?;
    let title = normalize_title(raw.title.as_str())?;
    let title_language = product_listing_normalization::detect_language(title.as_ref());
    let description_language =
        product_listing_normalization::text::detect_description_language(&raw.description);
    let title = product_listing_normalization::text::localize_normalized_title(
        title,
        title_language,
        description_language,
    )?;
    let description = normalize_description(raw.description, title_language)?;

    let price = normalize_price(raw.price.as_deref(), default_currency)
        .map_err(|error| map_price_error(error, PriceField::Price))?;
    let price_estimate_min = normalize_price(raw.price_estimate_min.as_deref(), default_currency)
        .map_err(|error| map_price_error(error, PriceField::EstimateMin))?;
    let price_estimate_max = normalize_price(raw.price_estimate_max.as_deref(), default_currency)
        .map_err(|error| map_price_error(error, PriceField::EstimateMax))?;
    let images = normalize_image_urls(raw.images, &url).map_err(map_image_error)?;
    let auction_start = normalize_date_time(raw.auction_start.as_deref())
        .map_err(|error| map_date_time_error(error, DateTimeField::AuctionStart))?;
    let auction_end = normalize_date_time(raw.auction_end.as_deref())
        .map_err(|error| map_date_time_error(error, DateTimeField::AuctionEnd))?;

    Ok(PreparedProduct {
        availability,
        source_listing_id,
        title,
        description,
        price,
        price_estimate_min,
        price_estimate_max,
        images,
        auction_start,
        auction_end,
        raw_attributes: raw.raw_attributes,
        raw_state: raw.state,
        url,
    })
}

pub struct ProductListingNormalizationServiceImpl;

impl ProductListingNormalizationServiceImpl {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for ProductListingNormalizationServiceImpl {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ProductListingNormalizationService for ProductListingNormalizationServiceImpl {
    async fn normalize(
        &self,
        raw: RawExtractedProduct,
        url: Url,
        default_currency: Option<Currency>,
    ) -> ProductListingNormalizationResult {
        let prepared = prepare_product(raw, url, default_currency).map_err(failure)?;
        Ok(NormalizationSuccess {
            prepared,
            llm_calls_used: 0,
        })
    }
}

fn failure(error: NormalizationError) -> NormalizationFailure {
    NormalizationFailure {
        error,
        llm_calls_used: 0,
    }
}

fn map_price_error(error: PriceNormalizationError, field: PriceField) -> NormalizationError {
    match error {
        PriceNormalizationError::UnknownCurrency => {
            NormalizationError::PriceUnknownCurrency { field }
        }
        PriceNormalizationError::ParseFailure => NormalizationError::PriceParseError { field },
    }
}

fn map_image_error(error: ImageUrlNormalizationError) -> NormalizationError {
    match error {
        ImageUrlNormalizationError::InvalidUrl(source) => {
            NormalizationError::InvalidImageUrl(source)
        }
    }
}

fn map_date_time_error(_: DateTimeNormalizationError, field: DateTimeField) -> NormalizationError {
    NormalizationError::DateTimeParseError { field }
}

fn map_availability_error(error: AvailabilityNormalizationError) -> NormalizationError {
    match error {
        AvailabilityNormalizationError::InputTooLong { len, max } => {
            NormalizationError::AvailabilityTextTooLong { len, max }
        }
        AvailabilityNormalizationError::EmbeddedNul => {
            NormalizationError::AvailabilityTextEmbeddedNul
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_normalization::ListingAvailabilityQuickCheck;

    fn raw() -> RawExtractedProduct {
        RawExtractedProduct {
            source_listing_id: "listing-123".into(),
            title: "Antique ceramic vase from the early twentieth century".into(),
            description: vec![],
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: "sold out".into(),
            images: vec![],
            auction_start: None,
            auction_end: None,
            raw_attributes: Default::default(),
        }
    }

    #[tokio::test]
    async fn should_use_pure_availability_normalization_without_llm_budget() {
        let result = ProductListingNormalizationServiceImpl::new()
            .normalize(
                raw(),
                Url::parse("https://example.com/listings/123")
                    .unwrap_or_else(|error| panic!("test URL must parse: {error}")),
                None,
            )
            .await
            .unwrap_or_else(|error| panic!("product must normalize: {error}"));

        assert_eq!(result.llm_calls_used, 0);
        assert_eq!(
            result.prepared.availability,
            ListingAvailabilityQuickCheck::Resolved(
                product_listing_core::listing_availability::ListingAvailability::SoldOut
            )
        );
    }
}
