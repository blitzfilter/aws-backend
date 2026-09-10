use super::*;
use crate::scraper::css_selector::rule::IMAGE_CANDIDATE_SEPARATOR;
use crate::scraper::raw_input::{crawler_provenance, crawler_raw_input};
use crate::scraper::scraper_service::domain::product::ScrapedProduct;
use crate::scraper::scraper_service::extraction::engine::apply_schema;
use crate::scraper::scraper_service::image_validation::{
    ImageValidation, ImageValidator, filter_valid_image_urls,
};
use crate::service::raw_capture::ProductListingRawCaptureItem;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use platform_postgres::SqlxUnitOfWork;
use product_listing_normalization::{
    ProductListingRawValuesNormalizationOutcome, ProductListingRawValuesNormalizer,
    ProductListingRawValuesPatch,
};
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingRawCaptureWriterFactory,
};
use product_listing_service::use_cases::{
    CaptureProductListingRawObservationHandler, CaptureProductListingRawObservationResult,
    CaptureProductListingRawObservationUseCase,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

struct PrimaryImageValidator {
    primary_validation: ImageValidation,
}

#[async_trait::async_trait]
impl ImageValidator for PrimaryImageValidator {
    async fn validate(&self, url: &Url) -> ImageValidation {
        if url.path().ends_with("/one-primary.jpg") {
            self.primary_validation
        } else {
            ImageValidation::Unknown
        }
    }
}

fn image_schema() -> ProductCssSelectorSchema {
    let mut schema = minimal_schema();
    schema.images = ExtractionRule {
        selector: CssSelector::from("img"),
        additional_selectors: Vec::new(),
        extract: ExtractionKind::ImageUrl,
        cardinality: ExtractionCardinality::All,
    };
    schema
}

fn invalid_schema() -> ProductCssSelectorSchema {
    let mut schema = image_schema();
    schema.title.selector = CssSelector::from("missing-title");
    schema
}

fn image_evidence_html() -> String {
    r#"<!DOCTYPE html>
    <html><body><main>
      <span id="product-id">SKU-42</span>
      <h1>Biedermeier Chair</h1>
      <span id="state">In Stock</span>
      <img data-large_image="/images/one-primary.jpg" src="/images/one-800x600.jpg">
      <img data-large_image="/images/two-100x100.jpg" src="/images/two-640x480.jpg">
      <img data-large_image="/images/one-primary.jpg" src="/images/one-800x600.jpg">
    </main></body></html>"#
        .to_string()
}

fn source_image_groups() -> Vec<String> {
    vec![
        format!("/images/one-primary.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/one-800x600.jpg"),
        format!("/images/two-100x100.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/two-640x480.jpg"),
        format!("/images/one-primary.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/one-800x600.jpg"),
    ]
}

fn expected_prepared_images() -> Vec<String> {
    vec![
        "https://example.com/images/one-800x600.jpg".to_string(),
        "https://example.com/images/two-640x480.jpg".to_string(),
    ]
}

fn normalizer_with_expected_images(url: Url) -> MockProductListingNormalizationService {
    let expected = prepared_product(url);
    let expected_images = expected_prepared_images();
    let mut normalizer = MockProductListingNormalizationService::new();
    normalizer
        .expect_normalize()
        .once()
        .returning(move |raw, _, _| {
            assert_eq!(raw.images, expected_images);
            let expected = expected.clone();
            Box::pin(async move { Ok(normalization_success(expected, 0)) })
        });
    normalizer
}

fn raw_value_image_urls(
    input: &product_listing_normalization::ProductListingNormalizationInput,
) -> Vec<&str> {
    let Some(images) = input.raw_values().value()["images"]["value"].as_array() else {
        panic!("crawler raw values must contain an image array");
    };

    images
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("crawler raw image values must be strings"))
        })
        .collect()
}

fn assert_worker_image_projection(
    input: &product_listing_normalization::ProductListingNormalizationInput,
    expected_image_urls: &[String],
) {
    let resolved = match ProductListingRawValuesNormalizer::new().normalize(input) {
        ProductListingRawValuesNormalizationOutcome::Resolved(resolved) => resolved,
        ProductListingRawValuesNormalizationOutcome::Invalid(_) => {
            panic!("crawler raw image projection must normalize")
        }
        ProductListingRawValuesNormalizationOutcome::Delete => {
            panic!("crawler UPSERT image projection must not normalize as DELETE")
        }
    };
    let images = match &resolved.images {
        ProductListingRawValuesPatch::Set(images) => images,
        ProductListingRawValuesPatch::Clear | ProductListingRawValuesPatch::Unchanged => {
            panic!("crawler raw image projection must set images")
        }
    };

    assert_eq!(
        images
            .iter()
            .map(|image| image.url().as_str().to_owned())
            .collect::<Vec<_>>(),
        expected_image_urls.to_vec()
    );
}

fn assert_raw_capture_image_projection(scraped: &ScrapedProduct) {
    let expected_image_urls = expected_prepared_images();
    assert_eq!(
        scraped.raw_input.source_payload().value().get("images"),
        Some(&serde_json::json!(source_image_groups()))
    );
    assert_eq!(
        raw_value_image_urls(&scraped.raw_input),
        expected_image_urls
            .iter()
            .map(|url| url.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        raw_value_image_urls(&scraped.raw_input)
            .iter()
            .all(|url| !url.contains(IMAGE_CANDIDATE_SEPARATOR))
    );
    assert_worker_image_projection(&scraped.raw_input, &expected_image_urls);
}

async fn scrape_cached_schema_image_evidence(
    id: listing_source_core::ListingSourceId,
) -> ScrapedProduct {
    let url = product_url();
    let html = image_evidence_html();
    let schemas = ListingSourceProductSchema {
        listing_source_id: id,
        product_schemas: vec![image_schema()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = html.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });
    let mut schema_service = MockProductListingSchemaService::new();
    schema_service
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let schemas = schemas.clone();
            Box::pin(async move { Ok(Some(schemas)) })
        });
    schema_service
        .expect_generate_single_schema_for_page()
        .never();
    schema_service.expect_save_product_schemas().never();

    let mut candidate_service = MockScraperCandidateService::new();
    expect_successful_bookkeeping(
        &mut candidate_service,
        id,
        url.clone(),
        CrawlerDisposition::Active,
    );
    let mut service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_service),
        Box::new(normalizer_with_expected_images(url.clone())),
        Arc::new(candidate_service),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );
    service.image_validator = Box::new(PrimaryImageValidator {
        primary_validation: ImageValidation::Invalid,
    });

    service
        .scrape(&id, &url, None, None, None, None)
        .await
        .unwrap_or_else(|error| panic!("cached schema scrape must succeed: {error}"))
        .unwrap_or_else(|| panic!("cached schema scrape must produce a capture"))
}

#[tokio::test]
async fn should_preserve_source_image_evidence_and_capture_validated_images_after_cached_schema_validation()
 {
    let scraped = scrape_cached_schema_image_evidence(listing_source_id()).await;

    assert_raw_capture_image_projection(&scraped);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_persist_cached_schema_image_evidence_in_raw_revision() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "crawler-image-evidence").await;
    let candidate_url = product_url();
    let scraped = scrape_cached_schema_image_evidence(listing_source_id).await;
    let capture_item = ProductListingRawCaptureItem::crawler(
        listing_source_id,
        &candidate_url,
        scraped.raw_input,
        crawler_provenance(None, None)
            .unwrap_or_else(|error| panic!("crawler provenance must be valid: {error}")),
    );
    let capture_handler = CaptureProductListingRawObservationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRawCaptureWriterFactory::new(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
    );
    let context = crawler_service_context(listing_source_id);

    let outcome = capture_handler
        .execute(&context, capture_item.command)
        .await
        .unwrap_or_else(|error| panic!("capture scraped raw observation: {error}"));
    assert!(matches!(
        outcome,
        CaptureProductListingRawObservationResult::Changed { revision: 1, .. }
    ));

    let stored_revisions: Vec<(serde_json::Value, serde_json::Value)> = sqlx::query_as(
        "SELECT source_payload, raw_values FROM product_listing_raw_revisions ORDER BY revision",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_else(|error| panic!("read stored raw revision: {error}"));
    assert_eq!(stored_revisions.len(), 1);
    let Some((source_payload, raw_values)) = stored_revisions.into_iter().next() else {
        panic!("capture must store one raw revision");
    };

    assert_eq!(
        source_payload.get("images"),
        Some(&serde_json::json!(source_image_groups()))
    );
    assert_eq!(
        raw_values.get("images"),
        Some(&serde_json::json!({
            "action": "SET",
            "value": expected_prepared_images(),
        }))
    );
    let Some(stored_images) = raw_values
        .get("images")
        .and_then(|images| images.get("value"))
        .and_then(serde_json::Value::as_array)
    else {
        panic!("stored raw image values must be an array");
    };
    let stored_image_urls = stored_images
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("stored raw image values must be strings"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        stored_image_urls,
        expected_prepared_images()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    assert!(
        stored_image_urls
            .iter()
            .all(|url| !url.contains(IMAGE_CANDIDATE_SEPARATOR))
    );
}

fn crawler_service_context(
    listing_source_id: listing_source_core::ListingSourceId,
) -> OperationContext {
    let key = format!("crawler-raw-capture:{listing_source_id}");
    OperationContext {
        principal: Principal::Service("crawler".to_owned()),
        request_id: RequestId::new(key.clone()),
        correlation_id: CorrelationId::new(key),
    }
}

async fn seed_listing_source(
    pool: &sqlx::PgPool,
    slug: &str,
) -> listing_source_core::ListingSourceId {
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = listing_source_core::ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed party: {error}"));
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(slug)
    .bind(slug)
    .bind(party_id)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed listing source: {error}"));
    listing_source_id
}

#[tokio::test]
async fn should_preserve_source_image_evidence_and_capture_validated_images_after_fresh_schema_validation()
 {
    let id = listing_source_id();
    let url = product_url();
    let html = image_evidence_html();
    let existing_schemas = ListingSourceProductSchema {
        listing_source_id: id,
        product_schemas: vec![invalid_schema()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = html.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });
    let mut schema_service = MockProductListingSchemaService::new();
    schema_service
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let schemas = existing_schemas.clone();
            Box::pin(async move { Ok(Some(schemas)) })
        });
    schema_service
        .expect_generate_single_schema_for_page()
        .once()
        .returning(|_| {
            Box::pin(async {
                Ok(generated_single_product(
                    image_schema(),
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_service
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

    let mut candidate_service = MockScraperCandidateService::new();
    expect_budget_increment(&mut candidate_service, 1);
    expect_successful_bookkeeping(
        &mut candidate_service,
        id,
        url.clone(),
        CrawlerDisposition::Active,
    );
    let mut service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_service),
        Box::new(normalizer_with_expected_images(url.clone())),
        Arc::new(candidate_service),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );
    service.image_validator = Box::new(PrimaryImageValidator {
        primary_validation: ImageValidation::Invalid,
    });

    let scraped = service
        .scrape(&id, &url, None, None, None, None)
        .await
        .unwrap_or_else(|error| panic!("fresh schema scrape must succeed: {error}"))
        .unwrap_or_else(|| panic!("fresh schema scrape must produce a capture"));

    assert_raw_capture_image_projection(&scraped);
}

#[tokio::test]
async fn should_hash_validated_image_projection_without_mutating_source_evidence() {
    let url = product_url();
    let raw = apply_schema(&image_schema(), &image_evidence_html())
        .unwrap_or_else(|error| panic!("image evidence schema must apply: {error}"));
    let source_images = raw.images.clone();

    let accepted_primary = filter_valid_image_urls(
        raw.images.clone(),
        &url,
        &PrimaryImageValidator {
            primary_validation: ImageValidation::Valid,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("valid image probe must select images: {error}"));
    let rejected_primary = filter_valid_image_urls(
        raw.images.clone(),
        &url,
        &PrimaryImageValidator {
            primary_validation: ImageValidation::Invalid,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("invalid image probe must select fallback images: {error}"));

    assert_eq!(
        accepted_primary,
        vec![
            "https://example.com/images/one-primary.jpg".to_string(),
            "https://example.com/images/two-640x480.jpg".to_string(),
        ]
    );
    assert_eq!(rejected_primary, expected_prepared_images());
    assert_ne!(accepted_primary, rejected_primary);
    assert_eq!(raw.images, source_images);
    assert_eq!(raw.images, source_image_groups());

    let accepted_input =
        crawler_raw_input(&raw, &accepted_primary, &url, None, [true, false, false])
            .unwrap_or_else(|error| panic!("accepted image projection must build: {error}"));
    let rejected_input =
        crawler_raw_input(&raw, &rejected_primary, &url, None, [true, false, false])
            .unwrap_or_else(|error| panic!("fallback image projection must build: {error}"));

    assert_eq!(
        accepted_input.source_payload().value().get("images"),
        Some(&serde_json::json!(source_image_groups()))
    );
    assert_eq!(
        rejected_input.source_payload().value().get("images"),
        Some(&serde_json::json!(source_image_groups()))
    );
    assert_eq!(
        accepted_input.source_payload().value(),
        rejected_input.source_payload().value()
    );
    let accepted_raw_value_images = raw_value_image_urls(&accepted_input);
    let rejected_raw_value_images = raw_value_image_urls(&rejected_input);
    assert_eq!(
        accepted_raw_value_images,
        accepted_primary
            .iter()
            .map(|url| url.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        rejected_raw_value_images,
        rejected_primary
            .iter()
            .map(|url| url.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        accepted_raw_value_images
            .iter()
            .chain(rejected_raw_value_images.iter())
            .all(|url| !url.contains(IMAGE_CANDIDATE_SEPARATOR))
    );
    assert_worker_image_projection(&accepted_input, &accepted_primary);
    assert_worker_image_projection(&rejected_input, &rejected_primary);

    let accepted_hash = accepted_input
        .hash()
        .unwrap_or_else(|error| panic!("accepted image projection must hash: {error}"));
    let rejected_hash = rejected_input
        .hash()
        .unwrap_or_else(|error| panic!("fallback image projection must hash: {error}"));

    assert_ne!(accepted_hash, rejected_hash);
}
