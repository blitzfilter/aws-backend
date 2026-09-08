use super::*;
use crate::scraper::normalization::product_normalization_service::ProductListingNormalizationServiceImpl;
use money::Currency;

fn numeric_price_schema(listing_source_id: ListingSourceId) -> ListingSourceProductSchema {
    let mut schemas = listing_source_product_schemas(listing_source_id);
    schemas.product_schemas[0].price = Some(ExtractionRule {
        selector: CssSelector::from("#price"),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    });
    schemas
}

async fn scrape_numeric_price(
    fallback_currency: Option<Currency>,
) -> crate::scraper::scraper_service::ScrapedProduct {
    let id = listing_source_id();
    let url = product_url();
    let html = r#"<!DOCTYPE html>
    <html>
    <body>
      <main>
        <span id="product-id">SKU-42</span>
        <h1>Biedermeier Chair</h1>
        <span id="price">5500</span>
        <span id="state">In Stock</span>
        <img src="/images/chair-640x640.jpg">
      </main>
    </body>
    </html>"#
        .to_owned();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = html.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });

    let schema = numeric_price_schema(id);
    let mut schema_service = MockProductListingSchemaService::new();
    schema_service
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let schema = schema.clone();
            Box::pin(async move { Ok(Some(schema)) })
        });

    let candidate_service = MockScraperCandidateService::new();
    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_service),
        Box::new(ProductListingNormalizationServiceImpl::new()),
        Arc::new(candidate_service),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    service
        .scrape_with_fallback_currency(&id, &url, None, None, None, None, fallback_currency)
        .await
        .unwrap_or_else(|error| panic!("numeric price scrape must succeed: {error}"))
        .unwrap_or_else(|| panic!("numeric price scrape must produce a raw observation"))
}

#[tokio::test]
async fn should_capture_numeric_price_with_operational_fallback_currency() {
    let product = scrape_numeric_price(Some(Currency::Zar)).await;

    assert_eq!(
        Some(&serde_json::json!("5500")),
        product.raw_input.source_payload().value().get("price")
    );
    assert_eq!(
        Some(&serde_json::json!({"action": "SET", "value": "5500"})),
        product.raw_input.raw_values().value().get("price")
    );
    assert_eq!(
        Some(&serde_json::json!("ZAR")),
        product
            .raw_input
            .normalization_context()
            .value()
            .get("fallbackCurrency")
    );
}

#[tokio::test]
async fn should_capture_numeric_price_evidence_without_generic_price_when_fallback_is_absent() {
    let product = scrape_numeric_price(None).await;

    assert_eq!(
        Some(&serde_json::json!("5500")),
        product.raw_input.source_payload().value().get("price")
    );
    assert_eq!(
        Some(&serde_json::json!({"action": "CLEAR"})),
        product.raw_input.raw_values().value().get("price")
    );
    assert_eq!(
        Some(&serde_json::Value::Null),
        product
            .raw_input
            .normalization_context()
            .value()
            .get("fallbackCurrency")
    );
}
