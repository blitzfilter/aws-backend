use super::*;

fn invalid_schema() -> ProductCssSelectorSchema {
    let mut schema = minimal_schema();
    schema.title.selector = CssSelector::from("missing-title");
    schema
}

#[allow(clippy::result_large_err)]
async fn scrape_with_schema_service(
    schema_svc: MockProductListingSchemaService,
    budget_increments: usize,
) -> Result<
    Option<crate::scraper::scraper_service::ScrapedProduct>,
    crate::scraper::scraper_service::domain::errors::ScraperError,
> {
    let id = listing_source_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

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
    expect_budget_increment(&mut cand_svc, budget_increments);
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), CrawlerDisposition::Active);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    service.scrape(&id, &url, None, None, None).await
}

#[tokio::test]
async fn should_use_yaml_schema_when_yaml_schema_is_high_confidence_and_covers_pages() {
    let id = listing_source_id();
    let schema = listing_source_product_schemas(id);
    let schema_for_create = schema.product_schemas.first().cloned().unwrap();
    let schema_for_save = schema.clone();

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc
        .expect_create_product_schemas()
        .once()
        .returning(move |_| {
            let s = vec![schema_for_create.clone()];
            Box::pin(async move { Ok(generated_schemas(s, SchemaLlmEvaluationConfidence::High)) })
        });
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, _| {
            let s = schema_for_save.clone();
            Box::pin(async move { Ok(s) })
        });

    let result = scrape_with_schema_service(schema_svc, 1).await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn should_use_yaml_schema_when_yaml_confidence_is_low() {
    let id = listing_source_id();
    let schema = listing_source_product_schemas(id);
    let yaml_schema = schema.product_schemas.first().cloned().unwrap();
    let schema_for_save = schema.clone();

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc
        .expect_create_product_schemas()
        .once()
        .returning(move |_| {
            let s = vec![yaml_schema.clone()];
            Box::pin(async move { Ok(generated_schemas(s, SchemaLlmEvaluationConfidence::Low)) })
        });
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, _| {
            let s = schema_for_save.clone();
            Box::pin(async move { Ok(s) })
        });

    let result = scrape_with_schema_service(schema_svc, 1).await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn should_generate_fresh_schema_after_yaml_schema_does_not_cover_raw_html() {
    let id = listing_source_id();
    let schema = listing_source_product_schemas(id);
    let generated_schema = schema.product_schemas.first().cloned().unwrap();

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc
        .expect_create_product_schemas()
        .once()
        .returning(move |_| {
            Box::pin(async move {
                Ok(generated_schemas(
                    vec![invalid_schema()],
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(move |_| {
            let s = generated_schema.clone();
            Box::pin(async move {
                Ok(generated_single_product(
                    s,
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc.expect_save_product_schemas().times(2).returning(
        move |listing_source_id, product_schemas| {
            let listing_source_id = *listing_source_id;
            Box::pin(async move {
                Ok(ListingSourceProductSchema {
                    listing_source_id,
                    product_schemas,
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                })
            })
        },
    );

    let result = scrape_with_schema_service(schema_svc, 2).await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn should_not_fallback_only_because_one_extra_schema_is_unused() {
    let id = listing_source_id();
    let schema = listing_source_product_schemas(id);
    let valid_schema = schema.product_schemas.first().cloned().unwrap();
    let schema_for_save = schema.clone();

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc
        .expect_create_product_schemas()
        .once()
        .returning(move |_| {
            let schemas = vec![valid_schema.clone(), invalid_schema()];
            Box::pin(async move {
                Ok(generated_schemas(
                    schemas,
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, _| {
            let s = schema_for_save.clone();
            Box::pin(async move { Ok(s) })
        });

    let result = scrape_with_schema_service(schema_svc, 1).await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn should_not_consume_second_budget_call_when_yaml_confidence_is_low() {
    let id = listing_source_id();
    let schema = listing_source_product_schemas(id);
    let yaml_schema = schema.product_schemas.first().cloned().unwrap();
    let schema_for_save = schema.clone();

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc
        .expect_create_product_schemas()
        .once()
        .returning(move |_| {
            let s = vec![yaml_schema.clone()];
            Box::pin(async move { Ok(generated_schemas(s, SchemaLlmEvaluationConfidence::Low)) })
        });
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, _| {
            let s = schema_for_save.clone();
            Box::pin(async move { Ok(s) })
        });

    let result = scrape_with_schema_service(schema_svc, 1).await.unwrap();
    assert!(result.is_some());
}
