use super::*;
use crate::scraper::scraper_service::image_validation::{ImageValidation, ImageValidator};
use crate::scraper::scraper_service::util::hash::{
    fingerprint_scraper_context, hash_html, hash_main_fragment,
};
use sha2::{Digest, Sha256};

struct ValidImage;

#[async_trait::async_trait]
impl ImageValidator for ValidImage {
    async fn validate(&self, _: &Url) -> ImageValidation {
        ImageValidation::Valid
    }
}

#[rstest::rstest]
#[case::no_capture(None)]
#[case::old_capture(Some(vec![5; 32]))]
#[case::removed_capture(Some(crate::scraper::raw_input::crawler_verified_removal_input(&product_url()).unwrap().hash().unwrap().as_bytes().to_vec()))]
#[tokio::test]
async fn should_extract_for_capture_when_local_fingerprints_match(
    #[case] expected_raw_input_sha256: Option<Vec<u8>>,
) {
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
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();
    let expected = prepared_product(url.clone());
    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |_, _, _| {
            let prepared = expected.clone();
            Box::pin(async move { Ok(normalization_success(prepared, 0)) })
        });
    let mut cand_svc = MockScraperCandidateService::new();
    cand_svc.expect_touch_scraped().never();
    cand_svc.expect_mark_as_scraped().never();

    let mut service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );
    service.image_validator = Box::new(ValidImage);

    let result = service
        .scrape(
            &id,
            &url,
            None,
            Some(&matching_hash),
            Some(&schema_fingerprint),
            expected_raw_input_sha256.as_deref(),
        )
        .await
        .unwrap();

    let scraped = result.expect("matching local hashes cannot prove current business custody");
    assert_eq!(scraped.hash, matching_hash);
    assert_eq!(scraped.schema_fingerprint, schema_fingerprint);
    assert_eq!(
        scraped.raw_input.operation(),
        product_listing_normalization::RawProductListingOperation::Upsert,
    );
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
