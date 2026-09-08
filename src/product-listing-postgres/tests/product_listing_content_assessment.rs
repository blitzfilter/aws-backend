mod support;

use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    content_policy::ContentPolicyDecision, product_listing_id::ProductListingId,
};
use product_listing_postgres::{
    SqlxProductListingContentAssessmentReader, SqlxProductListingContentAssessmentWriterFactory,
};
use product_listing_service::ports::{
    ProductListingContentAssessmentReader, ProductListingContentAssessmentWrite,
    ProductListingContentAssessmentWriteOutcome, ProductListingContentAssessmentWriter,
    ProductListingContentAssessmentWriterFactory,
};
use std::time::Duration;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_keep_assessment_current_after_price_and_enrichment_events() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_listing_id, content_source_event_id) =
            insert_product_with_created_event(&pool).await?;

        assert_eq!(
            ProductListingContentAssessmentWriteOutcome::Applied,
            apply_assessment(&pool, product_listing_id, content_source_event_id).await?
        );

        for (event_type, event_group) in [
            ("PRODUCT_LISTING_CHANGED", "DOMAIN"),
            ("ENRICHMENT_EMBEDDED", "ENRICHMENT"),
        ] {
            advance_current_event(&pool, product_listing_id, event_type, event_group).await?;
            let assessments = SqlxProductListingContentAssessmentReader::new(pool.clone())
                .find_current_assessments(&[product_listing_id])
                .await?;
            assert_eq!(
                Some(content_source_event_id),
                assessments
                    .get(&product_listing_id)
                    .map(|assessment| assessment.source_event_id),
                "{event_type} must not invalidate the content assessment"
            );
        }

        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "content assessment source-event acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_hide_assessment_when_content_source_event_changes() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_listing_id, initial_content_source_event_id) =
            insert_product_with_created_event(&pool).await?;

        assert_eq!(
            ProductListingContentAssessmentWriteOutcome::Applied,
            apply_assessment(&pool, product_listing_id, initial_content_source_event_id).await?
        );

        advance_content_source_event(&pool, product_listing_id).await?;

        let assessments = SqlxProductListingContentAssessmentReader::new(pool.clone())
            .find_current_assessments(&[product_listing_id])
            .await?;
        assert!(!assessments.contains_key(&product_listing_id));
        assert_eq!(
            ProductListingContentAssessmentWriteOutcome::Stale,
            apply_assessment(&pool, product_listing_id, initial_content_source_event_id).await?
        );

        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "content-source assessment invalidation acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_persist_one_assessment_when_duplicate_completions_overlap() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_id, source) = insert_product_with_created_event(&pool).await?;
        let mut first = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(first.connection()).await?;
        assert_eq!(ProductListingContentAssessmentWriteOutcome::Applied,
            SqlxProductListingContentAssessmentWriterFactory::new().in_transaction(&mut first)
                .apply(&ProductListingContentAssessmentWrite {
                    product_listing_id: product_id, source_event_id: source,
                    decision: Some(ContentPolicyDecision::Allowed),
                }).await?);
        let duplicate = apply_assessment(&pool, product_id, source);
        tokio::pin!(duplicate);
        support::assert_blocked(&pool, blocker_pid, 1, duplicate.as_mut()).await?;
        first.commit().await?;
        assert_eq!(ProductListingContentAssessmentWriteOutcome::Duplicate,
            tokio::time::timeout(Duration::from_secs(10), duplicate).await??);
        let stored: Vec<(uuid::Uuid, String)> = sqlx::query_as(
            "SELECT source_event_id, decision FROM product_listing_content_assessments WHERE product_listing_id = $1"
        ).bind(uuid::Uuid::from(product_id)).fetch_all(&pool).await?;
        assert_eq!(vec![(uuid::Uuid::from(source), "ALLOWED".to_owned())], stored);
        let state: (i64, i64, i64) = sqlx::query_as(
            "SELECT version, projection_version, (SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1) FROM product_listings WHERE product_listing_id = $1"
        ).bind(uuid::Uuid::from(product_id)).fetch_one(&pool).await?;
        assert_eq!((1, 1, 1), state);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "concurrent assessment completion: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_preserve_new_assessment_when_old_apply_and_clear_arrive_concurrently() {
    let result: Result<(), Box<dyn std::error::Error>> =
        async {
            let pool = get_postgres_client().await;
            let (product_id, old_source) = insert_product_with_created_event(&pool).await?;
            advance_content_source_event(&pool, product_id).await?;
            let new_source: uuid::Uuid = sqlx::query_scalar(
            "SELECT content_source_event_id FROM product_listings WHERE product_listing_id = $1"
        ).bind(uuid::Uuid::from(product_id)).fetch_one(&pool).await?;
            let mut newer = SqlxUnitOfWork::new(pool.clone()).begin().await?;
            let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(newer.connection())
                .await?;
            SqlxProductListingContentAssessmentWriterFactory::new()
                .in_transaction(&mut newer)
                .apply(&ProductListingContentAssessmentWrite {
                    product_listing_id: product_id,
                    source_event_id: new_source.into(),
                    decision: Some(ContentPolicyDecision::Allowed),
                })
                .await?;
            let late_completions = async {
                let clear = async {
                    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
                    let outcome = SqlxProductListingContentAssessmentWriterFactory::new()
                        .in_transaction(&mut tx)
                        .apply(&ProductListingContentAssessmentWrite {
                            product_listing_id: product_id,
                            source_event_id: old_source,
                            decision: None,
                        })
                        .await?;
                    tx.commit().await?;
                    Ok::<_, Box<dyn std::error::Error>>(outcome)
                };
                tokio::join!(apply_assessment(&pool, product_id, old_source), clear)
            };
            tokio::pin!(late_completions);
            support::assert_blocked(&pool, blocker_pid, 2, late_completions.as_mut()).await?;
            newer.commit().await?;
            let (apply, clear) =
                tokio::time::timeout(Duration::from_secs(10), late_completions).await?;
            assert_eq!(ProductListingContentAssessmentWriteOutcome::Stale, apply?);
            assert_eq!(ProductListingContentAssessmentWriteOutcome::Stale, clear?);
            let stored = SqlxProductListingContentAssessmentReader::new(pool.clone())
                .find_current_assessments(&[product_id])
                .await?;
            assert_eq!(
                Some(EventId::from(new_source)),
                stored.get(&product_id).map(|value| value.source_event_id)
            );
            Ok(())
        }
        .await;
    assert!(result.is_ok(), "reversed assessment completion: {result:?}");
}

async fn apply_assessment(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    source_event_id: EventId,
) -> Result<ProductListingContentAssessmentWriteOutcome, Box<dyn std::error::Error>> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let outcome = SqlxProductListingContentAssessmentWriterFactory::new()
        .in_transaction(&mut tx)
        .apply(&ProductListingContentAssessmentWrite {
            product_listing_id,
            source_event_id,
            decision: Some(ContentPolicyDecision::Allowed),
        })
        .await?;
    tx.commit().await?;
    Ok(outcome)
}

async fn insert_product_with_created_event(
    pool: &sqlx::PgPool,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let content_source_event_id = EventId::new();
    let party_id = uuid::Uuid::new_v4();
    let listing_source_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Content assessment party')")
        .bind(party_id)
        .bind(format!("content-assessment-party-{party_id}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Content assessment source', $3)")
        .bind(listing_source_id)
        .bind(format!("content-assessment-source-{listing_source_id}"))
        .bind(party_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Assessment chair', 'en', 'Assessment description', 'en', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(format!(
                    "content-assessment-{}",
                    &product_listing_id.to_string()[..6]
                ))
        .bind(uuid::Uuid::from(content_source_event_id))
        .bind(listing_source_id)
        .bind(product_listing_id.to_string())
        .execute(&mut *tx)
        .await?;
    insert_event(
        &mut tx,
        product_listing_id,
        content_source_event_id,
        "PRODUCT_LISTING_DISCOVERED",
        "DOMAIN",
        serde_json::json!({
            "listingSourceId": listing_source_id.to_string(),
            "sourceListingId": product_listing_id.to_string(),
            "title": {"language": "en", "text": "Assessment chair"},
            "description": {"language": "en", "text": "Assessment description"},
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": {"start": null, "end": null}
        }),
    )
    .await?;
    tx.commit().await?;
    Ok((product_listing_id, content_source_event_id))
}

async fn advance_content_source_event(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<(), sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    insert_event(
        &mut tx,
        product_listing_id,
        event_id,
        "PRODUCT_LISTING_CHANGED",
        "DOMAIN",
        serde_json::json!({
            "availability": {"previous": "AVAILABLE", "current": "SOLD_OUT"}
        }),
    )
    .await?;
    sqlx::query(
        "UPDATE product_listings SET current_event_id = $1, content_source_event_id = $1, availability = 'SOLD_OUT', version = version + 1, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $2",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

async fn advance_current_event(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    event_type: &str,
    event_group: &str,
) -> Result<(), sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    insert_event(
        &mut tx,
        product_listing_id,
        event_id,
        event_type,
        event_group,
        match event_type {
            "ENRICHMENT_EMBEDDED" => serde_json::json!({
                "sourceEventId": event_id.to_string()
            }),
            "PRODUCT_LISTING_CHANGED" => serde_json::json!({
                "images": {"previousCount": 0, "currentCount": 0}
            }),
            _ => serde_json::json!({}),
        },
    )
    .await?;
    sqlx::query(
        "UPDATE product_listings SET current_event_id = $1, version = version + 1, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $2",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

async fn insert_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    product_listing_id: ProductListingId,
    event_id: EventId,
    event_type: &str,
    event_group: &str,
    payload: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, $3, $4, 1, $5, now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(event_type)
        .bind(event_group)
        .bind(payload)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}
