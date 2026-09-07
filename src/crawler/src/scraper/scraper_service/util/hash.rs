use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::scraper_service::util::html::extract_main_fragment;
use sha2::{Digest, Sha256};

/// Returns the SHA-256 hex digest of the `<main>` tag's inner content, or
/// `None` if no `<main>` tag is present.
pub(crate) fn hash_main_fragment(html: &str) -> Option<String> {
    let content = extract_main_fragment(html)?;
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Returns the SHA-256 hex digest of the full HTML string.
pub(crate) fn hash_html(html: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(html.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Fingerprint the ordered effective schema set, including selector rules, raw attributes, and
/// default-currency context. The schema structures serialize deterministically (raw keys are a
/// `BTreeMap`), so a semantic schema change forces extraction even for unchanged HTML.
pub(crate) fn fingerprint_schema_set(
    schemas: &[ProductCssSelectorSchema],
) -> Result<String, serde_json::Error> {
    let encoded = serde_json::to_vec(schemas)?;
    let digest = Sha256::digest(encoded);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}
