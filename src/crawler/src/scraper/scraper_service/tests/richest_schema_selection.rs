use super::*;
use crate::scraper::normalization::error::NormalizationError;
use crate::scraper::scraper_service::domain::errors::ScraperError;

/// Helper to build a schema that also has a working description selector so
/// its raw extraction scores higher than a minimal schema.
fn schema_with_description() -> ProductCssSelectorSchema {
    let mut schema = minimal_schema();
    schema.description = Some(ExtractionRule {
        selector: CssSelector::from("main"),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    });
    schema
}

#[tokio::test]
async fn should_select_richer_schema_even_when_it_is_later_in_order() {
    let id = listing_source_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    let poor_schema = minimal_schema();
    let rich_schema = schema_with_description();
    // Insert the richer schema at index 1 — first in stored order is the
    // poorer schema, so naive first-success selection would pick it.
    let schema = ListingSourceProductSchema {
        listing_source_id: id,
        product_schemas: vec![poor_schema.clone(), rich_schema.clone()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = schema.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();

    let expected = prepared_product(url.clone());
    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc
        .expect_normalize()
        .times(1)
        .returning(move |raw, _, _| {
            assert!(raw.description.iter().any(|value| !value.trim().is_empty()));
            let n = expected.clone();
            Box::pin(async move { Ok(normalization_success(n, 0)) })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), CrawlerDisposition::Active);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let result = service.scrape(&id, &url, None, None, None).await.unwrap();
    assert!(result.is_some(), "richer later schema should win");
}

#[tokio::test]
async fn should_pick_earlier_schema_when_it_extracts_more_data() {
    let id = listing_source_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    // Earlier (index 0) extracts description; later (index 1) does not.
    let rich_first = schema_with_description();
    let poor_second = minimal_schema();

    let schema = ListingSourceProductSchema {
        listing_source_id: id,
        product_schemas: vec![rich_first, poor_second],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = schema.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();

    let expected = prepared_product(url.clone());
    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc
        .expect_normalize()
        .times(1)
        .returning(move |raw, _, _| {
            assert!(raw.description.iter().any(|value| !value.trim().is_empty()));
            let n = expected.clone();
            Box::pin(async move { Ok(normalization_success(n, 0)) })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), CrawlerDisposition::Active);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let result = service.scrape(&id, &url, None, None, None).await.unwrap();
    assert!(result.is_some(), "earlier richer schema should still win");
}

#[tokio::test]
async fn should_generate_fresh_schema_when_no_cached_schema_applies() {
    let id = listing_source_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    // Stored schema has selectors that don't match the sample html, so no
    // cached schema will apply.
    let mut invalid = minimal_schema();
    invalid.title.selector = CssSelector::from("missing-title");
    let schema = ListingSourceProductSchema {
        listing_source_id: id,
        product_schemas: vec![invalid],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = schema.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(|_| {
            Box::pin(async {
                Ok(generated_single_product(
                    minimal_schema(),
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, schemas| {
            Box::pin(async move {
                Ok(ListingSourceProductSchema {
                    listing_source_id: id,
                    product_schemas: schemas,
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                })
            })
        });

    let expected = prepared_product(url.clone());
    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |_, _, _| {
            let n = expected.clone();
            Box::pin(async move { Ok(normalization_success(n, 0)) })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), CrawlerDisposition::Active);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let result = service.scrape(&id, &url, None, None, None).await.unwrap();
    assert!(
        result.is_some(),
        "fresh schema should rescue when none applies"
    );
}

#[tokio::test]
async fn should_generate_fresh_schema_when_richer_candidate_normalization_fails_fixably() {
    let id = listing_source_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    // Both schemas apply; richer is index 1. Both normalization attempts
    // fail with candidate-data errors, so fresh schema generation must run.
    let poor = minimal_schema();
    let rich = schema_with_description();
    let schema = ListingSourceProductSchema {
        listing_source_id: id,
        product_schemas: vec![poor, rich],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = schema.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(|_| {
            Box::pin(async {
                Ok(generated_single_product(
                    minimal_schema(),
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, schemas| {
            Box::pin(async move {
                Ok(ListingSourceProductSchema {
                    listing_source_id: id,
                    product_schemas: schemas,
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                })
            })
        });

    let expected = prepared_product(url.clone());
    let normalize_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc
        .expect_normalize()
        .times(3) // 2 cached (candidate-data failures) + 1 generated
        .returning(move |_, _, _| {
            let n = expected.clone();
            let normalize_calls = normalize_calls.clone();
            Box::pin(async move {
                let call = normalize_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if call < 2 {
                    Err(normalization_failure(NormalizationError::TitleEmpty, 0))
                } else {
                    Ok(normalization_success(n, 0))
                }
            })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), CrawlerDisposition::Active);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let result = service.scrape(&id, &url, None, None, None).await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn should_not_persist_generated_schema_when_normalization_keeps_failing_fixably() {
    let id = listing_source_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    // No cached schema applies, so we go straight to fresh generation.
    let mut invalid = minimal_schema();
    invalid.title.selector = CssSelector::from("missing-title");
    let schema = ListingSourceProductSchema {
        listing_source_id: id,
        product_schemas: vec![invalid],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = schema.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(|_| {
            Box::pin(async {
                Ok(generated_single_product(
                    minimal_schema(),
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc.expect_normalize().returning(|_, _, _| {
        Box::pin(async { Err(normalization_failure(NormalizationError::TitleEmpty, 0)) })
    });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let err = service
        .scrape(&id, &url, None, None, None)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ScraperError::FreshSchemaNormalizationFailed {
                attempts: 1,
                ref last_norm_error,
                ..
            }
            if matches!(last_norm_error.as_ref(), NormalizationError::TitleEmpty)
        ),
        "expected FreshSchemaNormalizationFailed, got {err:?}"
    );
}
