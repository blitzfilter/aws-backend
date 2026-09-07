//! Scraper parsing pipeline integration tests.
//!
//! All test cases are driven by fixture JSON and ListingSource schema-cache files:
//!   `tests/fixtures/fixtures.json`
//!
//! Each element in the JSON array is one test case and contains:
//!   - `html`         — path to the HTML fixture (relative to crate root)
//!   - `schema` or `schemas_file` — one schema or a ListingSource schema cache
//!   - `raw`          — expected raw extraction output and state evidence
//!   - `normalized`   — expected pure-normalizer output

//!
//! To add a new ListingSource or a new variant (e.g. sold vs available):
//!   1. Drop the HTML file in `tests/fixtures/html/<listing_source>[_variant].html`.
//!   2. Add all ListingSource cached schemas to `tests/fixtures/schemas/<listing_source>.json`.
//!   3. Append an entry to `tests/fixtures/fixtures.json` with `schemas_file`
//!      and the expected `schema_index`.
//!      No Rust code changes needed.

#[path = "scraper_parsing_pipeline/assertions.rs"]
mod assertions;
#[path = "scraper_parsing_pipeline/expectation_types.rs"]
mod expectation_types;
#[path = "scraper_parsing_pipeline/scraper_parsing_pipeline_case.rs"]
mod scraper_parsing_pipeline_case;

use assertions::{assert_extraction, assert_normalized};
use scraper_parsing_pipeline_case::load_all_fixtures;

#[test]
fn should_extract_product_for_all_fixtures() {
    for fixture in load_all_fixtures() {
        let html = fixture.load_html_source();
        assert_extraction(&fixture.schemas, fixture.schema_index, &html, &fixture.raw);
    }
}

#[tokio::test]
async fn should_normalize_product_for_all_fixtures() {
    for fixture in load_all_fixtures() {
        let html = fixture.load_html_source();
        assert_normalized(
            &fixture.schemas[fixture.schema_index],
            &html,
            &fixture.normalized.url,
            &fixture.normalized,
        )
        .await;
    }
}
