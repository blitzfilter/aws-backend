use super::*;
use crate::scraper::scraper_service::util::hash::{
    fingerprint_scraper_context, hash_html, hash_main_fragment,
};
use sha2::{Digest, Sha256};

#[tokio::test]
async fn should_skip_fetching_and_return_none_when_hashes_match() {
    let id = listing_source_id();
    let url = product_url();
    let html = sample_html();
    let matching_hash = hash_main_fragment(&html).unwrap_or_else(|| hash_html(&html));

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = html.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });

    let schema = listing_source_product_schemas(id);
    let schema_fingerprint = fingerprint_scraper_context(&schema.product_schemas, None)
        .unwrap_or_else(|error| panic!("test schema must serialize: {error}"));
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let schema = schema.clone();
            Box::pin(async move { Ok(Some(schema)) })
        });
    let norm_svc = MockProductListingNormalizationService::new();
    let expected_raw_input_sha256 = vec![5; 32];
    let expected_raw_input_sha256_for_mock = expected_raw_input_sha256.clone();
    let mut cand_svc = MockScraperCandidateService::new();
    cand_svc
        .expect_touch_scraped()
        .once()
        .withf(move |_, _, _, _, actual_raw_input_sha256| {
            actual_raw_input_sha256.as_deref()
                == Some(expected_raw_input_sha256_for_mock.as_slice())
        })
        .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let result = service
        .scrape(
            &id,
            &url,
            None,
            Some(&matching_hash),
            Some(&schema_fingerprint),
            Some(expected_raw_input_sha256.as_slice()),
        )
        .await
        .unwrap();

    assert!(result.is_none());
}

#[test]
fn should_hash_main_fragment_when_main_tag_exists() {
    let html = "<html><body><main><h1>Hello</h1></main></body></html>";
    let hash = hash_main_fragment(html).expect("should find <main> tag");

    let mut hasher = Sha256::new();
    hasher.update("<h1>Hello</h1>".as_bytes());
    let expected: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    assert_eq!(hash, expected);
}

#[test]
fn should_return_none_from_hash_main_fragment_when_main_tag_missing() {
    let html = "<html><body><section>No main</section></body></html>";
    assert!(hash_main_fragment(html).is_none());
}

#[test]
fn should_hash_full_html_when_main_tag_missing() {
    let html = "<html><body><section>No main</section></body></html>";
    let hash = hash_html(html);

    let mut hasher = Sha256::new();
    hasher.update(html.as_bytes());
    let expected: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    assert_eq!(hash, expected);
}
