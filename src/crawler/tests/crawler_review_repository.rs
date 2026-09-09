use crawler::review::model::{
    PAGE_ROLE_PRIMARY, PAGE_ROLE_TRIGGERING_GENERATION_PAGE, STATUS_APPROVED,
    STATUS_PENDING_REVIEW, SchemaPageReference, SchemaReviewPageInput,
};
use crawler::review::model::{UrlPatternDecision, UrlPatternReviewCandidate};
use crawler::review::repository::{
    CrawlerReviewRepository, ReviewRepositoryError, SchemaReviewWithStatusInput,
};
use crawler::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use crawler::scraper::css_selector::product_schema_repository::{
    ListingSourceProductSchemaRepository, ListingSourceProductSchemaRepositoryImpl,
};
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use crawler::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use regex::Regex;
use serde_json::json;
use sqlx::{AssertSqlSafe, PgPool};
use test_api::*;
use time::OffsetDateTime;
use uuid::Uuid;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

fn rule(selector: &str) -> ExtractionRule {
    ExtractionRule {
        selector: selector.into(),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    }
}

fn image_rule(selector: &str) -> ExtractionRule {
    ExtractionRule {
        selector: selector.into(),
        additional_selectors: vec![],
        extract: ExtractionKind::Attribute { name: "src".into() },
        cardinality: ExtractionCardinality::All,
    }
}

fn schema(title_selector: &str) -> ProductCssSelectorSchema {
    ProductCssSelectorSchema {
        source_listing_id: Some(rule("span.id")),
        title: rule(title_selector),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        state: rule("span.state"),
        images: image_rule("img.product"),
        auction_start: None,
        auction_end: None,
        raw_attributes: Default::default(),
    }
}

async fn insert_listing_source(pool: &PgPool, listing_source_id: ListingSourceId) {
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
         VALUES ($1, 'Test source', 'test-source', TRUE, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_domain(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    domain: &str,
) -> CrawlerDomainId {
    let domain_id = CrawlerDomainId::new();
    sqlx::query(
        "INSERT INTO listing_source_domains (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, $3, $3)",
    )
    .bind(domain_id.as_uuid())
    .bind(listing_source_id.as_uuid())
    .bind(domain)
    .execute(pool)
    .await
    .unwrap();
    domain_id
}

fn review_pages() -> Vec<SchemaReviewPageInput> {
    vec![SchemaReviewPageInput {
        url: "https://example.com/products/1".to_string(),
        role: PAGE_ROLE_PRIMARY.to_string(),
        raw_html: "<html><body><span class=\"id\">SKU</span><h1>Title</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>".to_string(),
    }]
}

#[aura_integration_test(services = [POSTGRES])]
async fn fresh_generation_review_page_role_is_persisted() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;

    let review_id = review_repository
        .create_schema_review_with_status(SchemaReviewWithStatusInput {
            listing_source_id: &listing_source_id,
            reason: "fresh_schema_generation",
            schemas: &[schema("h1")],
            pages: vec![SchemaReviewPageInput {
                url: "https://example.com/products/1".to_string(),
                role: PAGE_ROLE_TRIGGERING_GENERATION_PAGE.to_string(),
                raw_html: "<html><body><h1>Title</h1></body></html>".to_string(),
            }],
            validation_summary: json!({}),
            status: STATUS_PENDING_REVIEW,
            notes: None,
        })
        .await
        .unwrap();

    assert_eq!(review_page_count(&pool, review_id).await, 1);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_not_define_database_defaults_for_crawler_object_ids() {
    let pool = get_postgres_client().await;
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*)::bigint, COUNT(column_default)::bigint \
         FROM information_schema.columns \
         WHERE table_schema = current_schema() \
           AND (table_name, column_name) IN ( \
             ('listing_source_domains', 'domain_id'), \
             ('crawler_reviews', 'review_id'), \
             ('crawler_review_pages', 'review_page_id'), \
             ('crawler_review_urls', 'review_url_id') \
           )",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(counts, (4, 0));
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_persist_uuid_v7_for_generated_review_page_and_url_ids() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "generated-ids.example.com").await;

    let schema_review_id = repository
        .create_schema_review(
            &listing_source_id,
            "initial_schema_generation",
            &[schema("h1")],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap();
    let schema_detail = repository.get_review(schema_review_id).await.unwrap();

    assert_eq!(
        7,
        schema_detail.review.review_id.as_uuid().get_version_num()
    );
    assert_eq!(
        7,
        schema_detail.pages[0]
            .review_page_id
            .as_uuid()
            .get_version_num()
    );

    let pattern_review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "url_pattern_generation",
            Some(&Regex::new("/product/").unwrap()),
            &["https://generated-ids.example.com/product/1".to_owned()],
            None,
        )
        .await
        .unwrap();
    let pattern_detail = repository.get_review(pattern_review_id).await.unwrap();

    assert_eq!(
        7,
        pattern_detail.review.review_id.as_uuid().get_version_num()
    );
    assert_eq!(
        7,
        pattern_detail.urls[0]
            .review_url_id
            .as_uuid()
            .get_version_num()
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_surface_invalid_persisted_v4_review_id() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let invalid_persisted_review_id =
        Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap();
    sqlx::query(
        "INSERT INTO crawler_reviews (review_id, listing_source_id, artifact_type, status, reason, candidate_payload, validation_summary) \
         VALUES ($1, $2, 'PRODUCT_SCHEMA', 'PENDING_REVIEW', 'corrupt test row', '{}'::jsonb, '{}'::jsonb)",
    )
    .bind(invalid_persisted_review_id)
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let result = repository.list_reviews(10).await;

    assert!(matches!(result, Err(sqlx::Error::Decode(_))));
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_surface_invalid_persisted_v4_review_page_id() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let review_id = repository
        .create_schema_review(
            &listing_source_id,
            "initial_schema_generation",
            &[schema("h1")],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap();
    let invalid_persisted_page_id =
        Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap();
    sqlx::query(
        "INSERT INTO crawler_review_pages (review_page_id, review_id, url, role, html_hash) \
         VALUES ($1, $2, 'https://example.com/products/corrupt', 'SEED', 'corrupt')",
    )
    .bind(invalid_persisted_page_id)
    .bind(review_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let result = repository.get_review_pages(review_id).await;

    assert!(matches!(result, Err(sqlx::Error::Decode(_))));
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_surface_invalid_persisted_v4_review_url_id() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "corrupt-review-url.example.com").await;
    let review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "url_pattern_generation",
            Some(&Regex::new("/product/").unwrap()),
            &["https://corrupt-review-url.example.com/product/1".to_owned()],
            None,
        )
        .await
        .unwrap();
    let invalid_persisted_url_id = Uuid::parse_str("00000000-0000-4000-8000-000000000003").unwrap();
    sqlx::query(
        "INSERT INTO crawler_review_urls (review_url_id, review_id, url) \
         VALUES ($1, $2, 'https://corrupt-review-url.example.com/product/corrupt')",
    )
    .bind(invalid_persisted_url_id)
    .bind(review_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let result = repository.get_review(review_id).await;

    assert!(matches!(
        result,
        Err(ReviewRepositoryError::Database(sqlx::Error::Decode(_)))
    ));
}

async fn pending_review_count(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    artifact_type: &str,
) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM crawler_reviews
         WHERE listing_source_id = $1 AND artifact_type = $2 AND status = 'PENDING_REVIEW'",
    )
    .bind(listing_source_id.as_uuid())
    .bind(artifact_type)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn review_page_count(pool: &PgPool, review_id: CrawlerReviewId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM crawler_review_pages WHERE review_id = $1")
        .bind(review_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn review_url_count(pool: &PgPool, review_id: CrawlerReviewId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM crawler_review_urls WHERE review_id = $1")
        .bind(review_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn review_count(pool: &PgPool, listing_source_id: ListingSourceId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM crawler_reviews WHERE listing_source_id = $1")
        .bind(listing_source_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_roll_back_schema_review_when_evidence_insert_fails() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    sqlx::query(
        "CREATE FUNCTION fail_crawler_review_page_insert() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN RAISE EXCEPTION 'injected review-page failure'; END; $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_crawler_review_page_insert \
         BEFORE INSERT ON crawler_review_pages FOR EACH ROW \
         WHEN (NEW.url = 'https://rollback-review.example/fail') \
         EXECUTE FUNCTION fail_crawler_review_page_insert()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let result = repository
        .create_schema_review(
            &listing_source_id,
            "initial_schema_generation",
            &[schema("h1")],
            vec![SchemaReviewPageInput {
                url: "https://rollback-review.example/fail".to_owned(),
                role: PAGE_ROLE_PRIMARY.to_string(),
                raw_html: "<html></html>".to_owned(),
            }],
            json!({}),
        )
        .await;

    assert!(result.is_err());
    assert_eq!(review_count(&pool, listing_source_id).await, 0);
    sqlx::query("DROP TRIGGER fail_crawler_review_page_insert ON crawler_review_pages")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION fail_crawler_review_page_insert()")
        .execute(&pool)
        .await
        .unwrap();
}

#[aura_integration_test(services = [POSTGRES])]
async fn approved_schema_candidate_edit_updates_live_schema_and_audit_payload() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let schema_repository = ListingSourceProductSchemaRepositoryImpl::new(&pool);
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;

    let initial_schema = schema("h1.old");
    let now = OffsetDateTime::now_utc();
    schema_repository
        .insert_product_schema(
            &listing_source_id,
            &ListingSourceProductSchema {
                listing_source_id,
                product_schemas: vec![initial_schema.clone()],
                created: now,
                updated: now,
            },
        )
        .await
        .unwrap();

    let review_id = review_repository
        .create_schema_review_with_status(SchemaReviewWithStatusInput {
            listing_source_id: &listing_source_id,
            reason: "initial_schema_generation",
            schemas: &[initial_schema],
            pages: vec![SchemaReviewPageInput {
                url: "https://example.com/products/1".to_string(),
                role: PAGE_ROLE_PRIMARY.to_string(),
                raw_html: "<html><body><span class=\"id\">SKU</span><h1 class=\"new\">Title</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>".to_string(),
            }],
            validation_summary: json!({
                "auto_schema_evaluation": {
                    "decision": "APPROVE",
                    "confidence": "HIGH",
                    "approved_by_llm": true,
                    "summary": "ok"
                }
            }),
            status: STATUS_APPROVED,
            notes: Some("Auto-approved by LLM schema evaluator"),
        })
        .await
        .unwrap();

    let updated_schema = schema("h1.new");
    review_repository
        .update_candidate_payload(review_id, json!({ "schemas": [updated_schema.clone()] }))
        .await
        .unwrap();

    let live = schema_repository
        .find_product_schema(&listing_source_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live.product_schemas, vec![updated_schema.clone()]);

    let detail = review_repository.get_review(review_id).await.unwrap();
    assert_eq!(
        detail.review.candidate_payload,
        json!({ "schemas": [updated_schema] })
    );
    let edits = detail.review.validation_summary["manual_schema_edits"]
        .as_array()
        .expect("manual edits should be recorded");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["source"], "review_console");
    assert_eq!(edits[0]["operation"], "approved_schema_live_update");
}

#[aura_integration_test(services = [POSTGRES])]
async fn schema_review_creation_rejects_invalid_typed_schema() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;

    let result = repository
        .create_schema_review(
            &listing_source_id,
            "initial_schema_generation",
            &[schema("[")],
            review_pages(),
            json!({}),
        )
        .await;

    assert!(matches!(
        result,
        Err(ReviewRepositoryError::InvalidProductSchemaCandidate)
    ));
    assert_eq!(review_count(&pool, listing_source_id).await, 0);
}

#[aura_integration_test(services = [POSTGRES])]
async fn approved_schema_field_edit_rejects_invalid_rule_without_mutating_live_or_audit_state() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let schema_repository = ListingSourceProductSchemaRepositoryImpl::new(&pool);
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let original_schema = schema("h1.original");
    let now = OffsetDateTime::now_utc();
    schema_repository
        .insert_product_schema(
            &listing_source_id,
            &ListingSourceProductSchema {
                listing_source_id,
                product_schemas: vec![original_schema.clone()],
                created: now,
                updated: now,
            },
        )
        .await
        .unwrap();
    let review_id = review_repository
        .create_schema_review_with_status(SchemaReviewWithStatusInput {
            listing_source_id: &listing_source_id,
            reason: "initial_schema_generation",
            schemas: std::slice::from_ref(&original_schema),
            pages: review_pages(),
            validation_summary: json!({ "auto_schema_evaluation": { "decision": "APPROVE" } }),
            status: STATUS_APPROVED,
            notes: None,
        })
        .await
        .unwrap();
    let before = review_repository.get_review(review_id).await.unwrap();

    let result = review_repository
        .update_schema_field(review_id, 0, "title", Some(rule("[")))
        .await;

    assert!(matches!(
        result,
        Err(ReviewRepositoryError::InvalidProductSchemaCandidate)
    ));
    assert_eq!(
        schema_repository
            .find_product_schema(&listing_source_id)
            .await
            .unwrap()
            .unwrap()
            .product_schemas,
        vec![original_schema]
    );
    let after = review_repository.get_review(review_id).await.unwrap();
    assert_eq!(
        after.review.candidate_payload,
        before.review.candidate_payload
    );
    assert_eq!(
        after.review.validation_summary,
        before.review.validation_summary
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn schema_matrix_write_skips_stale_candidate_snapshot() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let review_id = repository
        .create_schema_review(
            &listing_source_id,
            "initial_schema_generation",
            &[schema("h1")],
            review_pages(),
            json!({
                "auto_schema_evaluation": { "decision": "APPROVE" },
                "manual_schema_edits": []
            }),
        )
        .await
        .unwrap();

    let pages = repository.get_review_pages(review_id).await.unwrap();
    let review_page_id = pages[0].review_page_id;
    let matrix = repository
        .evaluate_schema_matrix_for_live_pages(
            review_id,
            pages
                .into_iter()
                .map(|page| {
                    (
                        page,
                        "<html><body><span class=\"id\">SKU</span><h1>Title</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>".to_owned(),
                    )
                })
                .collect(),
        )
        .await
        .unwrap();
    assert_eq!(matrix.matrix.review_id, Some(review_id));
    assert_eq!(
        matrix.matrix.candidates[0].pages[0].page_reference,
        SchemaPageReference::Persisted { review_page_id }
    );
    repository
        .update_candidate_payload(review_id, json!({ "schemas": [schema("h2")] }))
        .await
        .unwrap();
    let candidate_version: i64 =
        sqlx::query_scalar("SELECT candidate_version FROM crawler_reviews WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(candidate_version, 2);

    let stale_store = repository
        .store_schema_matrix_if_current(review_id, &matrix)
        .await;
    assert!(matches!(
        stale_store,
        Err(ReviewRepositoryError::CandidateChangedDuringEvaluation)
    ));

    let validation_summary: serde_json::Value =
        sqlx::query_scalar("SELECT validation_summary FROM crawler_reviews WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        validation_summary,
        json!({
            "auto_schema_evaluation": { "decision": "APPROVE" },
            "manual_schema_edits": []
        })
    );

    let pages = repository.get_review_pages(review_id).await.unwrap();
    let fresh_matrix = repository
        .evaluate_schema_matrix_for_live_pages(
            review_id,
            pages
                .into_iter()
                .map(|page| {
                    (
                        page,
                        "<html><body><span class=\"id\">SKU</span><h2>Title</h2><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>".to_owned(),
                    )
                })
                .collect(),
        )
        .await
        .unwrap();
    repository
        .store_schema_matrix_if_current(review_id, &fresh_matrix)
        .await
        .unwrap();

    let validation_summary: serde_json::Value =
        sqlx::query_scalar("SELECT validation_summary FROM crawler_reviews WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        validation_summary["schema_matrix"]["review_id"],
        json!(review_id)
    );
    assert_eq!(
        validation_summary["schema_matrix"]["candidates"][0]["pages"][0]["page_reference"]["kind"],
        "persisted"
    );
    assert_eq!(
        validation_summary["schema_matrix"]["candidates"][0]["pages"][0]["page_reference"]["review_page_id"],
        json!(review_page_id)
    );
    assert_eq!(
        validation_summary["auto_schema_evaluation"]["decision"],
        "APPROVE"
    );
    assert_eq!(validation_summary["manual_schema_edits"], json!([]));
}

#[aura_integration_test(services = [POSTGRES])]
async fn concurrent_schema_reviews_return_same_pending_review_without_duplicate_pages() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;

    let schema = schema("h1");
    let first_repo = review_repository.clone();
    let first_schema = schema.clone();
    let second_repo = review_repository.clone();
    let second_schema = schema.clone();

    let (first, second) = tokio::join!(
        async move {
            first_repo
                .create_schema_review(
                    &listing_source_id,
                    "initial_schema_generation",
                    &[first_schema],
                    review_pages(),
                    json!({ "source": "first" }),
                )
                .await
        },
        async move {
            second_repo
                .create_schema_review(
                    &listing_source_id,
                    "initial_schema_generation",
                    &[second_schema],
                    review_pages(),
                    json!({ "source": "second" }),
                )
                .await
        }
    );

    let first_id = first.unwrap();
    let second_id = second.unwrap();

    assert_eq!(first_id, second_id);
    assert_eq!(
        pending_review_count(&pool, listing_source_id, "PRODUCT_SCHEMA").await,
        1
    );
    assert_eq!(review_page_count(&pool, first_id).await, 1);
}

#[aura_integration_test(services = [POSTGRES])]
async fn concurrent_url_pattern_reviews_return_same_pending_review_without_duplicate_urls() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "review-pattern.example.com").await;

    let pattern = Regex::new("/product/").unwrap();
    let urls = vec![
        "https://example.com/product/1".to_string(),
        "https://example.com/about".to_string(),
    ];
    let first_repo = review_repository.clone();
    let first_urls = urls.clone();
    let second_repo = review_repository.clone();
    let second_urls = urls.clone();

    let (first, second) = tokio::join!(
        async move {
            first_repo
                .create_url_pattern_review(
                    &listing_source_id,
                    &domain_id,
                    "url_pattern_generation",
                    Some(&pattern),
                    &first_urls,
                    None,
                )
                .await
        },
        async move {
            let pattern = Regex::new("/product/").unwrap();
            second_repo
                .create_url_pattern_review(
                    &listing_source_id,
                    &domain_id,
                    "url_pattern_generation",
                    Some(&pattern),
                    &second_urls,
                    None,
                )
                .await
        }
    );

    let first_id = first.unwrap();
    let second_id = second.unwrap();

    assert_eq!(first_id, second_id);
    assert_eq!(
        pending_review_count(&pool, listing_source_id, "URL_PATTERN").await,
        1
    );
    assert_eq!(review_url_count(&pool, first_id).await, 2);
}

#[aura_integration_test(services = [POSTGRES])]
async fn invalid_edited_url_pattern_is_rejected_without_changing_live_pattern() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "typed-pattern.example.com").await;
    let pattern = Regex::new("/products/").unwrap();
    let review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "url_pattern_generation",
            Some(&pattern),
            &["https://typed-pattern.example.com/products/1".to_owned()],
            None,
        )
        .await
        .unwrap();

    let result = repository
        .update_candidate_payload(
            review_id,
            json!(UrlPatternReviewCandidate {
                decision: UrlPatternDecision::Pattern {
                    value: "[".to_owned(),
                },
                current_pattern: None,
            }),
        )
        .await;

    assert!(matches!(
        result,
        Err(ReviewRepositoryError::InvalidUrlPatternCandidate)
    ));
    let live_pattern: Option<String> =
        sqlx::query_scalar("SELECT url_pattern FROM listing_source_domains WHERE domain_id = $1")
            .bind(domain_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(live_pattern.is_none());
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_roll_back_pattern_approval_when_status_update_fails() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "atomic-pattern.example.com").await;
    sqlx::query(
        "UPDATE listing_source_domains SET url_pattern = '/old/', url_pattern_state = 'MATCHED' \
         WHERE domain_id = $1",
    )
    .bind(domain_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "url_pattern_generation",
            Some(&Regex::new("/new/").unwrap()),
            &["https://atomic-pattern.example.com/new/1".to_owned()],
            Some(&Regex::new("/old/").unwrap()),
        )
        .await
        .unwrap();
    sqlx::query(
        "CREATE FUNCTION fail_review_approval() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN \
           IF NEW.status = 'APPROVED' THEN RAISE EXCEPTION 'injected approval failure'; END IF; \
           RETURN NEW; \
         END; $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    // The backing UUID comes from a generated typed ID, so this audited DDL is safe.
    sqlx::query(AssertSqlSafe(format!(
        "CREATE TRIGGER fail_review_approval BEFORE UPDATE ON crawler_reviews \
         FOR EACH ROW WHEN (NEW.review_id = '{}'::uuid) \
         EXECUTE FUNCTION fail_review_approval()",
        review_id.as_uuid()
    )))
    .execute(&pool)
    .await
    .unwrap();

    let result = repository.approve_review(review_id, None).await;

    assert!(result.is_err());
    let pattern: (Option<String>, String) = sqlx::query_as(
        "SELECT url_pattern, url_pattern_state FROM listing_source_domains WHERE domain_id = $1",
    )
    .bind(domain_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pattern, (Some("/old/".to_owned()), "MATCHED".to_owned()));
    let status: String =
        sqlx::query_scalar("SELECT status FROM crawler_reviews WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, STATUS_PENDING_REVIEW);
    sqlx::query("DROP TRIGGER fail_review_approval ON crawler_reviews")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION fail_review_approval()")
        .execute(&pool)
        .await
        .unwrap();
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_allow_only_one_concurrent_review_approval() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id =
        insert_domain(&pool, listing_source_id, "concurrent-approval.example.com").await;
    let review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "url_pattern_generation",
            Some(&Regex::new("/product/").unwrap()),
            &["https://concurrent-approval.example.com/product/1".to_owned()],
            None,
        )
        .await
        .unwrap();
    let first = repository.clone();
    let second = repository.clone();

    let (first, second) = tokio::join!(
        async move { first.approve_review(review_id, None).await },
        async move { second.approve_review(review_id, None).await }
    );

    assert!(first.is_ok() ^ second.is_ok());
    assert!(matches!(
        first.err().or(second.err()),
        Some(ReviewRepositoryError::NotPending(id)) if id == review_id
    ));
    let status: String =
        sqlx::query_scalar("SELECT status FROM crawler_reviews WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, STATUS_APPROVED);
}

#[aura_integration_test(services = [POSTGRES])]
async fn approved_no_pattern_clears_stale_live_pattern_and_sets_no_pattern_state() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "no-pattern.example.com").await;
    sqlx::query(
        "UPDATE listing_source_domains SET url_pattern = '/stale/', url_pattern_state = 'MATCHED' WHERE domain_id = $1",
    )
    .bind(domain_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "url_pattern_generation",
            None,
            &["https://no-pattern.example.com/about".to_owned()],
            Some(&Regex::new("/stale/").unwrap()),
        )
        .await
        .unwrap();

    repository.approve_review(review_id, None).await.unwrap();

    let row: (Option<String>, String) = sqlx::query_as(
        "SELECT url_pattern, url_pattern_state FROM listing_source_domains WHERE domain_id = $1",
    )
    .bind(domain_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (None, "NO_PATTERN".to_owned()));
}
