use crate::scraper::scraper_service::domain::errors::ScraperError;
use listing_source_core::ListingSourceId;
use money::Currency;
use product_listing_normalization::{
    ListingAvailabilityQuickCheck, ProductListingNormalizationInput,
};
use url::Url;

/// Result of a successful scrape — the raw normalization input together with
/// metadata needed to mark the URL as scraped after durable raw capture.
#[derive(Debug)]
pub struct ScrapedProduct {
    /// Complete source-neutral normalization input. The worker later performs canonical writes.
    pub raw_input: ProductListingNormalizationInput,
    /// Pure crawler quick-check used only for local disposition.
    pub availability: ListingAvailabilityQuickCheck,
    /// SHA-256 of the page's `<main>` fragment (or full HTML) that was used to
    /// detect whether the page had changed.
    pub hash: String,
    /// Deterministic fingerprint of the effective ordered schema set.
    pub schema_fingerprint: String,
    /// Shared provider-neutral raw-input hash used for local change detection.
    pub raw_input_sha256: Vec<u8>,
}

// ---------------------------------------------------------------------------
// ScraperService trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
#[mockall::automock]
pub trait ScraperService: Send + Sync {
    /// Fetch the product page at `url`, extract structured data using the CSS
    /// selector schema for `listing_source_id`, validate a plausible extraction, and return a
    /// [`ScrapedProduct`]. The caller captures it before calling
    /// [`crate::scraper::candidate_service::ScraperCandidateService::mark_as_scraped`].
    async fn scrape(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        product_url_pattern: Option<&str>,
        last_scraped_hash: Option<&str>,
        last_scraped_schema_fingerprint: Option<&str>,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<Option<ScrapedProduct>, ScraperError>;

    #[allow(clippy::too_many_arguments)]
    async fn scrape_with_fallback_currency(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        product_url_pattern: Option<&str>,
        last_scraped_hash: Option<&str>,
        last_scraped_schema_fingerprint: Option<&str>,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
        fallback_currency: Option<Currency>,
    ) -> Result<Option<ScrapedProduct>, ScraperError> {
        let _ = fallback_currency;
        self.scrape(
            listing_source_id,
            url,
            product_url_pattern,
            last_scraped_hash,
            last_scraped_schema_fingerprint,
            expected_last_captured_raw_input_sha256,
        )
        .await
    }
}
