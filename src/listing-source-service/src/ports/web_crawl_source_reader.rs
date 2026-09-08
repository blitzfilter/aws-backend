use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use money::Currency;

use super::ListingSourceReadError;

/// Canonical ListingSource identity plus its current WebCrawl enablement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebCrawlSource {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: ListingSourceName,
    pub listing_source_slug: ListingSourceSlugId,
    pub web_crawl_enabled: bool,
    pub fallback_currency: Option<Currency>,
}

#[async_trait::async_trait]
pub trait WebCrawlSourceReader: Send + Sync {
    /// Returns a complete canonical ListingSource snapshot with derived WebCrawl enablement.
    /// This distinguishes a disabled source from a deleted canonical source.
    async fn list_sources(&self) -> Result<Vec<WebCrawlSource>, ListingSourceReadError>;
}
