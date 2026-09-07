use super::*;

#[tokio::test]
async fn should_return_raw_capture_and_availability_when_schema_exists_and_applies_cleanly() {
    let id = listing_source_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

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

    let result = service
        .scrape(&id, &url, None, None, None, None)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        result.availability,
        ListingAvailabilityQuickCheck::Resolved(ListingAvailability::Available)
    );
    assert_eq!(
        result
            .raw_input
            .source_payload()
            .value()
            .get("source_listing_id"),
        Some(&serde_json::json!("SKU-42"))
    );
    assert_eq!(
        result.raw_input.raw_values().value().get("url"),
        Some(&serde_json::json!({
            "action": "SET",
            "value": url.to_string(),
        }))
    );
}

#[tokio::test]
async fn should_retain_selected_raw_fields_without_canonical_product_preview() {
    let id = listing_source_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

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

    let norm = prepared_product(url.clone());
    let norm_clone = norm.clone();
    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc.expect_normalize().returning(move |_, _, _| {
        let n = norm_clone.clone();
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

    let result = service
        .scrape(&id, &url, None, None, None, None)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        result.raw_input.source_payload().value().get("title"),
        Some(&serde_json::json!("Biedermeier Chair"))
    );
    assert_eq!(
        result.raw_input.raw_values().value().get("title"),
        Some(&serde_json::json!({
            "action": "SET",
            "value": "Biedermeier Chair",
        }))
    );
}

#[tokio::test]
async fn should_preserve_raw_image_evidence_while_filtering_invalid_thumbnail_before_normalization()
{
    let id = listing_source_id();
    let url = product_url();
    let html = r#"<!DOCTYPE html>
    <html>
    <body>
      <main>
        <span id="product-id">SKU-42</span>
        <h1>Biedermeier Chair</h1>
        <span id="state">In Stock</span>
        <img src="/image-100x100.jpg">
        <img src="/image-800x600.jpg">
      </main>
    </body>
    </html>"#
        .to_string();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = html.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });

    let schema = listing_source_product_schemas(id);
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let schema = schema.clone();
            Box::pin(async move { Ok(Some(schema)) })
        });

    let expected = prepared_product(url.clone());
    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |raw, _, _| {
            assert_eq!(
                raw.images,
                vec!["https://example.com/image-800x600.jpg".to_string()]
            );
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

    let result = service
        .scrape(&id, &url, None, None, None, None)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        result.raw_input.source_payload().value().get("images"),
        Some(&serde_json::json!([
            "/image-100x100.jpg",
            "/image-800x600.jpg",
        ]))
    );
    assert_eq!(
        result.raw_input.raw_values().value().get("images"),
        Some(&serde_json::json!({
            "action": "SET",
            "value": [
                "https://example.com/image-800x600.jpg",
            ],
        }))
    );
}
