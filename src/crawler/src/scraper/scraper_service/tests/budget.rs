use super::*;
use crate::scraper::scraper_service::domain::errors::ScraperError;

#[tokio::test]
async fn should_return_llm_budget_exceeded_when_increment_is_rejected_on_schema_generation() {
    let id = listing_source_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc.expect_create_product_schemas().never();

    let norm_svc = MockProductListingNormalizationService::new();
    let mut cand_svc = MockScraperCandidateService::new();
    cand_svc
        .expect_try_increment_listing_source_llm_calls_with_limit()
        .once()
        .returning(|_, _, _| Box::pin(async { Ok(false) }));

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
    assert!(matches!(
        err,
        ScraperError::LlmBudgetExceeded {
            listing_source_id,
            max_calls,
            ..
        } if listing_source_id == id && max_calls == DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE
    ));
}
