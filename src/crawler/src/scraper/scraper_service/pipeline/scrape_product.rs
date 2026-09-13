use crate::network::policy::NetworkErrorKind;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::raw_input::crawler_raw_input;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::domain::product::{ScrapedProduct, ScraperService};
use crate::scraper::scraper_service::pipeline::cached_schema_selection::ExistingSchemaSelection;
use crate::scraper::scraper_service::pipeline::fresh_schema_generation::FreshSchemaGenerationContext;
use crate::scraper::scraper_service::service::{FetchError, ScraperServiceImpl};
use crate::scraper::scraper_service::util::hash::{
    fingerprint_scraper_context, hash_html, hash_main_fragment,
};

use crate::spider::classification::url_metadata::CrawlerUrlWriteOutcome;
use crate::spider::utils::url::CrawledUrl;
use listing_source_core::ListingSourceId;
use regex::Regex;
use tracing::{debug, warn};
use url::Url;

pub(crate) fn is_redirect_to_non_product_page(
    original_url: &Url,
    final_url: &Url,
    product_url_pattern: Option<&str>,
) -> bool {
    let normalized_original = CrawledUrl::new(original_url.clone());
    let normalized_final = CrawledUrl::new(final_url.clone());

    if normalized_original.as_url() == normalized_final.as_url() {
        return false;
    }

    if !is_same_logical_host(original_url, final_url) {
        return false;
    }

    if let Some(pattern) = product_url_pattern.and_then(|raw| Regex::new(raw).ok()) {
        return !normalized_final.matches_pattern(&pattern);
    }

    is_homepage(final_url)
}

fn is_same_logical_host(left: &Url, right: &Url) -> bool {
    normalize_host(left) == normalize_host(right)
}

fn normalize_host(url: &Url) -> Option<&str> {
    url.host_str()
        .map(|host| host.strip_prefix("www.").unwrap_or(host))
}

fn is_homepage(url: &Url) -> bool {
    url.path().is_empty() || url.path() == "/"
}

impl ScraperServiceImpl {
    #[tracing::instrument(
        skip(self, expected_last_captured_raw_input_sha256),
        fields(listing_source_id = %listing_source_id, url = %url)
    )]
    pub(crate) async fn mark_url_other_best_effort(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) {
        match self
            .candidate_service
            .set_class(
                listing_source_id,
                url,
                crate::spider::classification::url_metadata::UrlClass::Other,
                expected_last_captured_raw_input_sha256,
            )
            .await
        {
            Ok(CrawlerUrlWriteOutcome::Applied) => {}
            Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                debug!(
                    crawler_url_write_outcome = "stale_noop",
                    "Skipped stale crawler URL classification"
                );
            }
            Err(error) => {
                warn!(error = %error, "Failed to mark URL as other");
            }
        }
    }

    #[allow(clippy::result_large_err)]
    async fn removed_page_schemas_for(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Vec<RemovedPageSchema>, ScraperError> {
        let mut schemas = Vec::new();

        if let Some(stored) = self
            .removed_page_schema_repository
            .find_removed_page_schema(listing_source_id)
            .await
            .map_err(ScraperError::RemovedPageSchemaDatabaseError)?
        {
            schemas.extend(stored.removed_page_schemas);
        }

        Ok(schemas)
    }

    #[allow(clippy::result_large_err)]
    async fn is_removed_page(
        &self,
        listing_source_id: &ListingSourceId,
        html: &str,
    ) -> Result<bool, ScraperError> {
        Ok(self
            .removed_page_schemas_for(listing_source_id)
            .await?
            .iter()
            .any(|schema| schema.matches(html)))
    }
}

#[async_trait::async_trait]
impl ScraperService for ScraperServiceImpl {
    #[tracing::instrument(skip(self, last_scraped_hash, last_scraped_schema_fingerprint, expected_last_captured_raw_input_sha256), fields(listing_source_id = %listing_source_id, url = %url))]
    async fn scrape(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        product_url_pattern: Option<&str>,
        last_scraped_hash: Option<&str>,
        last_scraped_schema_fingerprint: Option<&str>,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<Option<ScrapedProduct>, ScraperError> {
        self.scrape_with_fallback_currency(
            listing_source_id,
            url,
            product_url_pattern,
            last_scraped_hash,
            last_scraped_schema_fingerprint,
            expected_last_captured_raw_input_sha256,
            None,
        )
        .await
    }

    async fn scrape_with_fallback_currency(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        product_url_pattern: Option<&str>,
        _last_scraped_hash: Option<&str>,
        _last_scraped_schema_fingerprint: Option<&str>,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
        fallback_currency: Option<money::Currency>,
    ) -> Result<Option<ScrapedProduct>, ScraperError> {
        let domain = url
            .host_str()
            .ok_or_else(|| ScraperError::NoHost { url: url.clone() })?;

        if let Some(review_id) = self
            .pending_product_schema_review_id(listing_source_id)
            .await?
        {
            return Err(ScraperError::PendingSchemaReview {
                url: url.clone(),
                review_id,
            });
        }

        // 1. Fetch HTML --------------------------------------------------
        debug!(domain, "Fetching product page HTML");
        let fetched = match self.html_fetcher.fetch(url).await {
            Ok(fetched) => fetched,
            Err(FetchError::Network {
                kind: NetworkErrorKind::HttpStatus(404 | 410),
                details,
            }) => {
                return Err(ScraperError::ProductListingRemoved {
                    url: url.clone(),
                    details,
                });
            }
            Err(FetchError::Network { kind, details }) => {
                return Err(ScraperError::HttpError {
                    url: url.clone(),
                    kind,
                    details,
                });
            }
        };
        if !is_same_logical_host(url, &fetched.final_url) {
            return Err(ScraperError::HttpError {
                url: url.clone(),
                kind: NetworkErrorKind::UnsafeTarget,
                details: format!(
                    "product URL redirected outside configured crawler domain: original={url}, final={}",
                    fetched.final_url
                ),
            });
        }
        if is_redirect_to_non_product_page(url, &fetched.final_url, product_url_pattern) {
            return Err(ScraperError::ProductListingRemoved {
                url: url.clone(),
                details: format!(
                    "product URL redirected to non-product page: original={url}, final={}",
                    fetched.final_url
                ),
            });
        }
        let html = fetched.html;

        if self.is_removed_page(listing_source_id, &html).await? {
            return Err(ScraperError::ProductListingRemoved {
                url: url.clone(),
                details: "soft-404 removed page matched configured removed-page schema".to_string(),
            });
        }

        let current_hash = hash_main_fragment(&html).unwrap_or_else(|| hash_html(&html));

        // Local fingerprints cannot prove current business custody: a later capture may
        // have committed without its local mark. Re-extract even a byte-identical page;
        // the authoritative raw-capture transaction decides whether input is unchanged.
        let listing_source_product_schemas = self
            .obtain_schemas(listing_source_id, url, product_url_pattern, &html)
            .await?;

        // Select the richest cached schema that normalizes successfully.
        let selection = match self
            .select_existing_schema_with_normalization(
                listing_source_id,
                url,
                &html,
                &listing_source_product_schemas.product_schemas,
                fallback_currency,
            )
            .await?
        {
            ExistingSchemaSelection::Prepared(selection) => *selection,
            ExistingSchemaSelection::GenerateNewSchema { reason } => {
                debug!(
                    domain,
                    schemas = listing_source_product_schemas.product_schemas.len(),
                    fresh_schema_generation_reason = reason.as_str(),
                    "Cached selection exhausted; generating new schema"
                );
                self.generate_fresh_schema_for_page(FreshSchemaGenerationContext {
                    listing_source_id,
                    domain,
                    url,
                    html: &html,
                    existing_schemas: &listing_source_product_schemas.product_schemas,
                    fallback_currency,
                    expected_last_captured_raw_input_sha256,
                })
                .await?
            }
        };

        let mut effective_schemas = listing_source_product_schemas.product_schemas;
        if selection.fresh_schema {
            effective_schemas.push(selection.schema.clone());
        }
        let schema_fingerprint = fingerprint_scraper_context(&effective_schemas, fallback_currency)
            .map_err(ScraperError::SchemaFingerprint)?;
        let raw_input = crawler_raw_input(
            &selection.raw,
            &selection.validated_image_urls,
            url,
            selection.fallback_currency,
            [
                selection.prepared.price.is_some(),
                selection.prepared.price_estimate_min.is_some(),
                selection.prepared.price_estimate_max.is_some(),
            ],
        )
        .map_err(ScraperError::RawNormalizationInput)?;
        let raw_input_sha256 = raw_input
            .hash()
            .map_err(ScraperError::RawNormalizationInput)?
            .as_bytes()
            .to_vec();
        let availability = selection.prepared.availability;

        // `mark_as_scraped` is intentionally deferred until raw capture succeeds.
        debug!(domain, "Scraping complete");
        Ok(Some(ScrapedProduct {
            raw_input,
            availability,
            hash: current_hash,
            schema_fingerprint,
            raw_input_sha256,
        }))
    }
}
