use super::*;
use crate::scraper::css_selector::product_schema::ListingSourceProductSchema;
use crate::scraper::css_selector::product_schema_service::GeneratedSingleSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::css_selector::removed_page_schema_repository::MockRemovedPageSchemaRepository;
use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::spider::classification::url_metadata::{CrawlerUrlWriteOutcome, UrlClass};
use listing_source_core::ListingSourceId;

fn invalid_schema() -> ProductCssSelectorSchema {
    let mut schema = minimal_schema();
    schema.title.selector = CssSelector::from("missing-title");
    schema
}

fn existing_invalid_schema(listing_source_id: ListingSourceId) -> ListingSourceProductSchema {
    ListingSourceProductSchema {
        listing_source_id,
        product_schemas: vec![invalid_schema()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    }
}

fn fetcher_with_sample_html() -> MockHtmlFetcher {
    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));
    fetcher
}

fn normalizer_with_success(url: Url) -> MockProductListingNormalizationService {
    let expected = prepared_product(url);
    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |_, _, _| {
            let n = expected.clone();
            Box::pin(async move { Ok(normalization_success(n, 0)) })
        });
    norm_svc
}

#[tokio::test]
async fn should_use_yaml_only_when_single_schema_applies() {
    let id = listing_source_id();
    let url = product_url();

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = existing_invalid_schema(id);
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

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), CrawlerDisposition::Active);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher_with_sample_html()),
        Box::new(schema_svc),
        Box::new(normalizer_with_success(url.clone())),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let result = service
        .scrape(&id, &url, None, None, None, None)
        .await
        .unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn should_fail_when_fresh_schema_does_not_apply_after_initial_schema_failure() {
    let id = listing_source_id();
    let url = product_url();

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = existing_invalid_schema(id);
            Box::pin(async move { Ok(Some(s)) })
        });

    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(|_| {
            Box::pin(async {
                Ok(generated_single_product(
                    invalid_schema(),
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher_with_sample_html()),
        Box::new(schema_svc),
        Box::new(MockProductListingNormalizationService::new()),
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
        ScraperError::SchemaRegenerationExhausted {
            attempts: 1,
            ref last_error,
            ..
        } if matches!(
            last_error.as_ref(),
            crate::scraper::css_selector::product_schema::ApplySchemaError::Title(
                ExtractionError::NoElementMatched { .. }
            )
        )
    ));
}

#[tokio::test]
async fn should_fail_when_fresh_schema_application_fails() {
    let id = listing_source_id();
    let url = product_url();

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = existing_invalid_schema(id);
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(|_| {
            Box::pin(async {
                Ok(generated_single_product(
                    invalid_schema(),
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let norm_svc = MockProductListingNormalizationService::new();
    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher_with_sample_html()),
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
        ScraperError::SchemaRegenerationExhausted {
            attempts: 1,
            ref last_error,
            ..
        } if matches!(
            last_error.as_ref(),
            crate::scraper::css_selector::product_schema::ApplySchemaError::Title(
                ExtractionError::NoElementMatched { .. }
            )
        )
    ));
}

#[tokio::test]
async fn should_not_consume_second_budget_call_when_fresh_schema_does_not_apply() {
    let id = listing_source_id();
    let url = product_url();

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = existing_invalid_schema(id);
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(|_| {
            Box::pin(async {
                Ok(generated_single_product(
                    invalid_schema(),
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let norm_svc = MockProductListingNormalizationService::new();
    let mut cand_svc = MockScraperCandidateService::new();
    cand_svc
        .expect_try_increment_listing_source_llm_calls_with_limit()
        .once()
        .returning(move |_, _, _| Box::pin(async move { Ok(true) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher_with_sample_html()),
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
        ScraperError::SchemaRegenerationExhausted { attempts: 1, .. }
    ));
}

#[tokio::test]
async fn should_mark_withdrawn_when_fresh_generation_classifies_removed() {
    let id = listing_source_id();
    let url = product_url();
    let removed_html =
        r#"<main><h1 id="removed-message">ProductListing no longer available</h1></main>"#;
    let removed_schema = RemovedPageSchema {
        selector: CssSelector::from("#removed-message"),
        text: Some("ProductListing no longer available".to_string()),
        regex: None,
    };

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = removed_html.to_string();
        Box::pin(async move { Ok(fetch_result(html)) })
    });

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = existing_invalid_schema(id);
            Box::pin(async move { Ok(Some(s)) })
        });
    let removed_schema_for_generation = removed_schema.clone();
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(move |_| {
            let schema = removed_schema_for_generation.clone();
            Box::pin(async move {
                Ok(GeneratedSingleSchema::Removed {
                    schema,
                    evaluation: schema_evaluation(SchemaLlmEvaluationConfidence::High),
                })
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let mut removed_repo = MockRemovedPageSchemaRepository::new();
    removed_repo
        .expect_find_removed_page_schema()
        .times(2)
        .returning(|_| Box::pin(async { Ok(None) }));
    removed_repo
        .expect_insert_removed_page_schema()
        .once()
        .withf(move |received_listing_source_id, row| {
            *received_listing_source_id == id
                && row.removed_page_schemas == vec![removed_schema.clone()]
        })
        .returning(|_, row| {
            let row = row.clone();
            Box::pin(async move { Ok(row) })
        });
    removed_repo.expect_update_removed_page_schema().never();

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
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

#[tokio::test]
async fn should_mark_other_when_fresh_generation_classifies_not_product() {
    let id = listing_source_id();
    let url = product_url();
    let category_html = r#"<main class="category"><h1>Latest antiques</h1></main>"#;

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = category_html.to_string();
        Box::pin(async move { Ok(fetch_result(html)) })
    });

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = existing_invalid_schema(id);
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(move |_| {
            Box::pin(async move {
                Ok(GeneratedSingleSchema::NotProduct {
                    reason: "category page".to_string(),
                    evaluation: schema_evaluation(SchemaLlmEvaluationConfidence::High),
                })
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    let expected_last_captured_raw_input_sha256 = vec![4; 32];
    let expected_raw_input_sha256_for_class = expected_last_captured_raw_input_sha256.clone();
    let url_for_class = url.clone();
    cand_svc
        .expect_set_class()
        .once()
        .withf(
            move |received_listing_source_id,
                  received_url,
                  received_class,
                  expected_raw_input_sha256| {
                *received_listing_source_id == id
                    && received_url == &url_for_class
                    && *received_class == UrlClass::Other
                    && *expected_raw_input_sha256
                        == Some(expected_raw_input_sha256_for_class.as_slice())
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
    );

    let err = service
        .scrape(
            &id,
            &url,
            None,
            None,
            None,
            Some(expected_last_captured_raw_input_sha256.as_slice()),
        )
        .await
        .unwrap_err();

    assert!(matches!(err, ScraperError::NotProductPage { .. }));
}

async fn should_reject_low_confidence_fresh_classification(
    confidence: SchemaLlmEvaluationConfidence,
    removed: bool,
) {
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
        .returning(move |_| {
            let s = existing_invalid_schema(id);
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(move |_| {
            let generated = if removed {
                GeneratedSingleSchema::Removed {
                    schema: RemovedPageSchema {
                        selector: CssSelector::from("#removed"),
                        text: Some("Gone".to_string()),
                        regex: None,
                    },
                    evaluation: schema_evaluation(confidence),
                }
            } else {
                GeneratedSingleSchema::NotProduct {
                    reason: "category page".to_string(),
                    evaluation: schema_evaluation(confidence),
                }
            };
            Box::pin(async move { Ok(generated) })
        });
    schema_svc.expect_save_product_schemas().never();

    let mut removed_repo = MockRemovedPageSchemaRepository::new();
    removed_repo
        .expect_find_removed_page_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    removed_repo.expect_insert_removed_page_schema().never();
    removed_repo.expect_update_removed_page_schema().never();

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    cand_svc.expect_set_disposition().never();
    cand_svc.expect_set_class().never();

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
    assert!(matches!(
        err,
        ScraperError::SchemaClassificationRejected { .. }
    ));
}

#[tokio::test]
async fn should_reject_medium_confidence_removed_classification_without_side_effects() {
    should_reject_low_confidence_fresh_classification(SchemaLlmEvaluationConfidence::Medium, true)
        .await;
}

#[tokio::test]
async fn should_reject_low_confidence_removed_classification_without_side_effects() {
    should_reject_low_confidence_fresh_classification(SchemaLlmEvaluationConfidence::Low, true)
        .await;
}

#[tokio::test]
async fn should_reject_medium_confidence_not_product_classification_without_side_effects() {
    should_reject_low_confidence_fresh_classification(SchemaLlmEvaluationConfidence::Medium, false)
        .await;
}

#[tokio::test]
async fn should_reject_low_confidence_not_product_classification_without_side_effects() {
    should_reject_low_confidence_fresh_classification(SchemaLlmEvaluationConfidence::Low, false)
        .await;
}

#[tokio::test]
async fn should_not_change_state_or_class_when_fresh_classification_does_not_match_html() {
    let id = listing_source_id();
    let url = product_url();
    let html = r#"<main><h1>Still a weird page</h1></main>"#;

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = html.to_string();
        Box::pin(async move { Ok(fetch_result(html)) })
    });

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = existing_invalid_schema(id);
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(|_| {
            Box::pin(async {
                Ok(GeneratedSingleSchema::Removed {
                    schema: RemovedPageSchema {
                        selector: CssSelector::from("#missing"),
                        text: Some("ProductListing no longer available".to_string()),
                        regex: None,
                    },
                    evaluation: schema_evaluation(SchemaLlmEvaluationConfidence::High),
                })
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    cand_svc.expect_set_disposition().never();
    cand_svc.expect_set_class().never();

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(MockProductListingNormalizationService::new()),
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
        ScraperError::SchemaRegenerationExhausted { attempts: 1, .. }
    ));
}
