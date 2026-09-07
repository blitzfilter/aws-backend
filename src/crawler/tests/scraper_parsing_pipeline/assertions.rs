use crawler::scraper::css_selector::product_schema::{
    ProductCssSelectorSchema, RawExtractedProduct,
};

use crawler::scraper::normalization::product_normalization_service::{
    ProductListingNormalizationService, ProductListingNormalizationServiceImpl,
};
use crawler::scraper::scraper_service::rank_applicable_schema_indices;
use money::Currency;

use scraper::Html;

use url::Url;

use crate::expectation_types::{NormalizedExpectation, RawExpectation};

pub fn assert_extraction(
    schemas: &[ProductCssSelectorSchema],
    schema_index: usize,
    html_src: &str,
    expected: &RawExpectation,
) {
    let html = Html::parse_document(html_src);
    let ranked_indices = rank_applicable_schema_indices(schemas, html_src);
    assert_eq!(ranked_indices.first().copied(), Some(schema_index));
    let schema = schemas
        .get(schema_index)
        .unwrap_or_else(|| panic!("schema index {schema_index} out of range"));
    let result: RawExtractedProduct = schema
        .apply(&html)
        .unwrap_or_else(|e| panic!("schema apply failed: {e}"));

    assert_eq!(
        result.source_listing_id, expected.source_listing_id,
        "source_listing_id"
    );
    assert_eq!(result.title, expected.title, "title");
    assert_eq!(result.description, expected.description, "description");
    assert_eq!(result.price.as_deref(), expected.price.as_deref(), "price");
    assert_eq!(
        result.price_estimate_min.as_deref(),
        expected.price_estimate_min.as_deref(),
        "price_estimate_min"
    );
    assert_eq!(
        result.price_estimate_max.as_deref(),
        expected.price_estimate_max.as_deref(),
        "price_estimate_max"
    );
    assert_eq!(result.state, expected.state, "state");
    assert_eq!(result.images, expected.images, "images");
    assert_eq!(
        result.auction_start.as_deref(),
        expected.auction_start.as_deref(),
        "auction_start"
    );
    assert_eq!(
        result.auction_end.as_deref(),
        expected.auction_end.as_deref(),
        "auction_end"
    );
}

pub async fn assert_normalized(
    schema: &ProductCssSelectorSchema,
    html_src: &str,
    url: &str,
    expected: &NormalizedExpectation,
) {
    let html = Html::parse_document(html_src);
    let raw = schema
        .apply(&html)
        .unwrap_or_else(|e| panic!("schema apply failed: {e}"));

    let norm_svc = ProductListingNormalizationServiceImpl::new();

    let product_url = Url::parse(url).expect("test URL must be valid");
    let default_currency = schema.default_currency.map(Currency::from);
    let result = norm_svc
        .normalize(raw, product_url, default_currency)
        .await
        .unwrap_or_else(|e| panic!("normalization failed: {e}"))
        .prepared;

    assert_eq!(
        result.source_listing_id.to_string(),
        expected.source_listing_id,
        "source_listing_id"
    );
    assert_eq!(result.title.payload.as_ref(), expected.title, "title");
    assert_eq!(
        result.description.as_ref().map(|d| d.payload.as_ref()),
        expected.description.as_deref(),
        "description"
    );
    assert_eq!(result.price, expected.price, "price");
    assert_eq!(
        result.price_estimate_min, expected.price_estimate_min,
        "price_estimate_min"
    );
    assert_eq!(
        result.price_estimate_max, expected.price_estimate_max,
        "price_estimate_max"
    );
    assert_eq!(
        result.availability.availability(),
        expected.availability,
        "availability"
    );
    assert_eq!(result.url.as_str(), expected.url, "url");
    let result_image_urls: Vec<&str> = result.images.iter().map(|i| i.url().as_str()).collect();
    let expected_images: Vec<&str> = expected.images.iter().map(|i| i.as_str()).collect();
    assert_eq!(result_image_urls, expected_images, "images");
    assert_eq!(
        result.auction_start, expected.auction_start,
        "auction_start"
    );
    assert_eq!(result.auction_end, expected.auction_end, "auction_end");
}
