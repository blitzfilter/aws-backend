use super::*;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::pipeline::scrape_product::is_redirect_to_non_product_page;

#[test]
fn should_detect_same_host_product_redirect_to_homepage() {
    let original = Url::parse("https://example.com/products/123").unwrap();
    let final_url = Url::parse("https://example.com/").unwrap();

    assert!(is_redirect_to_non_product_page(&original, &final_url, None));
}

#[test]
fn should_detect_www_variant_redirect_to_homepage() {
    let original = Url::parse("https://example.com/products/123").unwrap();
    let final_url = Url::parse("https://www.example.com/").unwrap();

    assert!(is_redirect_to_non_product_page(&original, &final_url, None));
}

#[test]
fn should_detect_same_host_product_redirect_to_category_page_when_pattern_does_not_match() {
    let original =
        Url::parse("https://www.ebth.com/items/7055109-digital-print-television-poster-dirt")
            .unwrap();
    let final_url = Url::parse(
        "https://www.ebth.com/collectibles/memorabilia/television-and-movie-memorabilia",
    )
    .unwrap();

    assert!(is_redirect_to_non_product_page(
        &original,
        &final_url,
        Some(r"/items/")
    ));
}

#[test]
fn should_not_detect_redirect_to_another_matching_product_page() {
    let original = Url::parse("https://example.com/products/123").unwrap();
    let final_url = Url::parse("https://example.com/products/456").unwrap();

    assert!(!is_redirect_to_non_product_page(
        &original,
        &final_url,
        Some(r"/products/")
    ));
}

#[test]
fn should_not_detect_equivalent_url_normalization_as_homepage_redirect() {
    let original = Url::parse("https://example.com/products/123#details").unwrap();
    let final_url = Url::parse("https://example.com/products/123").unwrap();

    assert!(!is_redirect_to_non_product_page(
        &original,
        &final_url,
        Some(r"/products/")
    ));
}

#[test]
fn should_not_reject_non_homepage_redirect_when_pattern_is_missing() {
    let original = Url::parse("https://example.com/products/123").unwrap();
    let final_url = Url::parse("https://example.com/collections/chairs").unwrap();

    assert!(!is_redirect_to_non_product_page(
        &original, &final_url, None
    ));
}

#[test]
fn should_not_reject_non_homepage_redirect_when_pattern_is_invalid() {
    let original = Url::parse("https://example.com/products/123").unwrap();
    let final_url = Url::parse("https://example.com/collections/chairs").unwrap();

    assert!(!is_redirect_to_non_product_page(
        &original,
        &final_url,
        Some("[")
    ));
}

#[tokio::test]
async fn should_withdraw_product_when_product_url_redirects_to_homepage() {
    let id = listing_source_id();
    let url = product_url();
    let homepage = Url::parse("https://example.com/").unwrap();

    let mut fetcher = MockHtmlFetcher::new();
    let final_url = homepage.clone();
    fetcher.expect_fetch().once().returning(move |_| {
        let final_url = final_url.clone();
        Box::pin(async move { Ok(fetch_result_for(sample_html(), final_url)) })
    });

    let schema_svc = MockProductListingSchemaService::new();
    let norm_svc = MockProductListingNormalizationService::new();

    let mut cand_svc = MockScraperCandidateService::new();
    let url_for_set_presence = url.clone();
    cand_svc
        .expect_set_disposition()
        .never()
        .withf(
            move |received_listing_source_id, received_url, received_state, _| {
                *received_listing_source_id == id
                    && received_url == &url_for_set_presence
                    && *received_state == CrawlerDisposition::Active
            },
        )
        .returning(|_, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let err = service
        .scrape(&id, &url, None, None, None, None)
        .await
        .unwrap_err();

    assert!(matches!(err, ScraperError::ProductListingRemoved { .. }));
}

#[tokio::test]
async fn should_withdraw_product_when_redirected_url_does_not_match_product_pattern() {
    let id = listing_source_id();
    let url = Url::parse("https://www.ebth.com/items/7055109-digital-print-television-poster-dirt")
        .unwrap();
    let category = Url::parse(
        "https://www.ebth.com/collectibles/memorabilia/television-and-movie-memorabilia",
    )
    .unwrap();

    let mut fetcher = MockHtmlFetcher::new();
    let final_url = category.clone();
    fetcher.expect_fetch().once().returning(move |_| {
        let final_url = final_url.clone();
        Box::pin(async move { Ok(fetch_result_for(sample_html(), final_url)) })
    });

    let schema_svc = MockProductListingSchemaService::new();
    let norm_svc = MockProductListingNormalizationService::new();

    let mut cand_svc = MockScraperCandidateService::new();
    let url_for_set_presence = url.clone();
    cand_svc
        .expect_set_disposition()
        .never()
        .withf(
            move |received_listing_source_id, received_url, received_state, _| {
                *received_listing_source_id == id
                    && received_url == &url_for_set_presence
                    && *received_state == CrawlerDisposition::Active
            },
        )
        .returning(|_, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let err = service
        .scrape(&id, &url, Some(r"/items/"), None, None, None)
        .await
        .unwrap_err();

    assert!(matches!(err, ScraperError::ProductListingRemoved { .. }));
}
