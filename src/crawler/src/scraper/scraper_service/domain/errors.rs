use crate::network::policy::NetworkErrorKind;
use crate::scraper::css_selector::product_schema::ApplySchemaError;
use crate::scraper::css_selector::product_schema_service::ProductListingSchemaServiceError;
use crate::scraper::normalization::error::NormalizationError;
use listing_source_core::ListingSourceId;
use product_listing_normalization::NormalizationInputError;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum ScraperError {
    #[error("HTTP error while fetching '{url}': {details}")]
    HttpError {
        url: Url,
        kind: NetworkErrorKind,
        details: String,
    },

    #[error("ProductListing URL removed while fetching '{url}': {details}")]
    ProductListingRemoved { url: Url, details: String },

    #[error("URL is not a product page while scraping '{url}': {details}")]
    NotProductPage { url: Url, details: String },

    #[error("Rejected low-confidence page classification for '{url}': {details}")]
    SchemaClassificationRejected { url: Url, details: String },

    #[error("URL has no host: {url}")]
    NoHost { url: Url },

    #[error("Schema service error: {0}")]
    SchemaServiceError(#[from] ProductListingSchemaServiceError),

    #[error("Removed-page schema database error: {0}")]
    RemovedPageSchemaDatabaseError(#[source] sqlx::Error),

    #[error(
        "Schema regeneration exhausted after {attempts} attempts for '{url}' (last error: {last_error})"
    )]
    SchemaRegenerationExhausted {
        url: Url,
        attempts: u32,
        last_error: Box<ApplySchemaError>,
    },

    /// Fresh schema generation succeeded, but normalization still failed.
    #[error(
        "Fresh schema normalization failed after {attempts} attempts for '{url}': {last_norm_error}"
    )]
    FreshSchemaNormalizationFailed {
        url: Url,
        attempts: u32,
        last_norm_error: Box<NormalizationError>,
    },

    #[error("Normalization error: {0}")]
    NormalizationError(#[from] NormalizationError),

    #[error("crawler raw normalization input could not be built")]
    RawNormalizationInput(#[source] NormalizationInputError),

    #[error("crawler effective schema fingerprint could not be built")]
    SchemaFingerprint(#[source] serde_json::Error),

    #[error(
        "LLM call budget exceeded for ListingSource '{listing_source_id}' while scraping '{url}' (limit={max_calls})"
    )]
    LlmBudgetExceeded {
        listing_source_id: ListingSourceId,
        url: Url,
        max_calls: i64,
    },

    #[error("Scraping '{url}' is blocked pending product schema review '{review_id}'")]
    PendingSchemaReview { url: Url, review_id: uuid::Uuid },
}
