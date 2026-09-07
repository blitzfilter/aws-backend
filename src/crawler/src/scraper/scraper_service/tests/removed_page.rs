use super::*;
use crate::scraper::css_selector::removed_page_schema::{
    ListingSourceRemovedPageSchema, RemovedPageSchema,
};
use crate::scraper::css_selector::removed_page_schema_repository::MockRemovedPageSchemaRepository;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use listing_source_core::ListingSourceId;

fn removed_schema_set(listing_source_id: ListingSourceId) -> ListingSourceRemovedPageSchema {
    ListingSourceRemovedPageSchema {
        listing_source_id,
        removed_page_schemas: vec![RemovedPageSchema {
            selector: CssSelector::from("#mainCatCol h1"),
            text: Some("Sorry, the page you're looking for couldn't be found".to_string()),
            regex: None,
        }],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    }
}

#[tokio::test]
async fn should_mark_product_removed_when_stored_removed_page_schema_matches() {
    let id = listing_source_id();
    let url = product_url();
    let html = r#"<main id="mainCatCol"><h1>Sorry, the page you're looking for couldn't be found</h1></main>"#;

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(move |_| Box::pin(async move { Ok(fetch_result(html.to_string())) }));

    let mut removed_repo = MockRemovedPageSchemaRepository::new();
    removed_repo
        .expect_find_removed_page_schema()
        .once()
        .returning(move |_| {
            let row = removed_schema_set(id);
            Box::pin(async move { Ok(Some(row)) })
        });

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc.expect_find_product_schema().never();
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();

    let mut cand_svc = MockScraperCandidateService::new();
    let url_for_state = url.clone();
    cand_svc
        .expect_set_disposition()
        .never()
        .withf(
            move |received_listing_source_id, received_url, received_state, _| {
                *received_listing_source_id == id
                    && received_url == &url_for_state
                    && *received_state == CrawlerDisposition::DormantRemoved
            },
        )
        .returning(|_, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(MockProductListingNormalizationService::new()),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    )
    .with_removed_page_schema_repository(Box::new(removed_repo));

    let err = service
        .scrape(&id, &url, None, None, None, None)
        .await
        .unwrap_err();

    assert!(matches!(err, ScraperError::ProductListingRemoved { .. }));
}
