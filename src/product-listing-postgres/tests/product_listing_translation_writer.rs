use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use indexmap::IndexMap;
use localization::Language;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{product_listing_id::ProductListingId, title::Title};
use product_listing_postgres::SqlxProductListingTranslationWriterFactory;
use product_listing_service::ports::{
    ProductListingTranslationWrite, ProductListingTranslationWriteOutcome,
    ProductListingTranslationWriter, ProductListingTranslationWriterFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_store_all_translations_append_enrichment_event_and_advance_current_event_without_aggregate_version()
 {
    let result: Result<(), Box<dyn std::error::Error>> = async {
    let pool = get_postgres_client().await;
    let (product_listing_id, source_event_id) = insert_product_with_discovered_event(&pool).await?;
    let enrichment_event_id = EventId::new();
    let write = write(product_listing_id, source_event_id, enrichment_event_id);
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;

    let outcome = SqlxProductListingTranslationWriterFactory::new()
        .in_transaction(&mut tx)
        .apply(&write)
        .await?;
    tx.commit().await?;

    assert_eq!(ProductListingTranslationWriteOutcome::Applied, outcome);
    let translations = sqlx::query_as::<_, (String, String, uuid::Uuid)>(
        "SELECT language, title, source_event_id FROM product_listing_translations WHERE product_listing_id = $1 ORDER BY language",
    )
    .bind(product_listing_id.into_uuid())
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        vec![
            (
                "en".to_owned(),
                "Antique chair".to_owned(),
                source_event_id.into_uuid()
            ),
            (
                "fr".to_owned(),
                "Chaise ancienne".to_owned(),
                source_event_id.into_uuid()
            ),
        ],
        translations
    );
    let event = sqlx::query_as::<_, (String, String, serde_json::Value)>(
        "SELECT event_type, event_group, payload FROM product_listing_events WHERE event_id = $1",
    )
    .bind(enrichment_event_id.into_uuid())
    .fetch_one(&pool)
    .await?;
    assert_eq!("ENRICHMENT_TRANSLATED_TITLES", event.0);
    assert_eq!("ENRICHMENT", event.1);
    assert_eq!(
        serde_json::json!({
            "sourceEventId": source_event_id.as_uuid().to_string(),
            "sourceLanguage": "de",
            "targetLanguages": ["en", "fr"],
        }),
        event.2
    );
    let (current_event, version, projection_version): (uuid::Uuid, i64, i64) =
        sqlx::query_as("SELECT current_event_id, version, projection_version FROM product_listings WHERE product_listing_id = $1")
            .bind(product_listing_id.into_uuid())
            .fetch_one(&pool)
            .await?;
    assert_eq!(enrichment_event_id.into_uuid(), current_event);
    assert_eq!(1, version);
    assert_eq!(2, projection_version);
    Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "translation write acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_duplicate_without_second_event_when_same_source_is_redelivered() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_listing_id, source_event_id) = insert_product_with_discovered_event(&pool).await?;
        let write = write(product_listing_id, source_event_id, EventId::new());
        apply(&pool, &write).await?;

        let outcome = apply(
            &pool,
            &ProductListingTranslationWrite {
                enrichment_event_id: EventId::new(),
                ..write
            },
        )
        .await?;

        assert_eq!(ProductListingTranslationWriteOutcome::Duplicate, outcome);
        let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_group = 'ENRICHMENT'",
    )
    .bind(product_listing_id.into_uuid())
    .fetch_one(&pool)
    .await?;
        assert_eq!(1, count, "one translated event for the discovered source");
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "translation duplicate acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_keep_first_translation_when_same_source_redelivery_has_different_text() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_listing_id, source_event_id) = insert_product_with_discovered_event(&pool).await?;
        let first = write(product_listing_id, source_event_id, EventId::new());
        apply(&pool, &first).await?;
        let before: (uuid::Uuid, i64) = sqlx::query_as(
            "SELECT current_event_id, projection_version FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(product_listing_id.into_uuid())
        .fetch_one(&pool)
        .await?;

        let second = ProductListingTranslationWrite {
            enrichment_event_id: EventId::new(),
            titles: IndexMap::from([
                (Language::En, Title::from("Different English title")),
                (Language::Fr, Title::from("Titre français différent")),
            ]),
            ..first
        };
        assert_eq!(
            ProductListingTranslationWriteOutcome::Duplicate,
            apply(&pool, &second).await?
        );

        let translations = sqlx::query_as::<_, (String, String)>(
            "SELECT language, title FROM product_listing_translations WHERE product_listing_id = $1 ORDER BY language",
        )
        .bind(product_listing_id.into_uuid())
        .fetch_all(&pool)
        .await?;
        assert_eq!(
            vec![
                ("en".to_owned(), "Antique chair".to_owned()),
                ("fr".to_owned(), "Chaise ancienne".to_owned()),
            ],
            translations
        );
        let after: (uuid::Uuid, i64) = sqlx::query_as(
            "SELECT current_event_id, projection_version FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(product_listing_id.into_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!(before, after);
        assert_eq!(
            1,
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_TRANSLATED_TITLES' AND event_group = 'ENRICHMENT' AND event_type_schema_version = 1",
            )
            .bind(product_listing_id.into_uuid())
            .fetch_one(&pool)
            .await?
        );
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "translation output-idempotency acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_serialize_concurrent_same_source_translation_attempts() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_listing_id, source_event_id) =
            insert_product_with_discovered_event(&pool).await?;
        let first = write(product_listing_id, source_event_id, EventId::new());
        let second = ProductListingTranslationWrite {
            enrichment_event_id: EventId::new(),
            titles: IndexMap::from([
                (Language::En, Title::from("Concurrent English title")),
                (Language::Fr, Title::from("Titre français concurrent")),
            ]),
            ..first.clone()
        };

        let (first_result, second_result) = tokio::join!(apply(&pool, &first), apply(&pool, &second));
        let outcomes = [first_result?, second_result?];
        assert!(outcomes.contains(&ProductListingTranslationWriteOutcome::Applied));
        assert!(outcomes.contains(&ProductListingTranslationWriteOutcome::Duplicate));

        assert_eq!(
            1,
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_TRANSLATED_TITLES' AND event_group = 'ENRICHMENT' AND event_type_schema_version = 1",
            )
            .bind(product_listing_id.into_uuid())
            .fetch_one(&pool)
            .await?
        );
        assert_eq!(
            2,
            sqlx::query_scalar::<_, i64>(
                "SELECT projection_version FROM product_listings WHERE product_listing_id = $1",
            )
            .bind(product_listing_id.into_uuid())
            .fetch_one(&pool)
            .await?
        );
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "concurrent translation write acceptance failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_stale_without_writing_when_content_source_event_advanced() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let (product_listing_id, source_event_id) =
            insert_product_with_discovered_event(&pool).await?;
        let newer_event_id = insert_event_and_advance_product(
            &pool,
            product_listing_id,
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
        )
        .await?;

        let outcome = apply(
            &pool,
            &write(product_listing_id, source_event_id, EventId::new()),
        )
        .await?;

        assert_eq!(ProductListingTranslationWriteOutcome::Stale, outcome);
        let translation_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM product_listing_translations WHERE product_listing_id = $1",
        )
        .bind(product_listing_id.into_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!(0, translation_count);
        let current_event = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT current_event_id FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(product_listing_id.into_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!(newer_event_id.into_uuid(), current_event);
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "translation stale acceptance failed: {result:?}"
    );
}

async fn apply(
    pool: &sqlx::PgPool,
    write: &ProductListingTranslationWrite,
) -> Result<ProductListingTranslationWriteOutcome, Box<dyn std::error::Error>> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let outcome = SqlxProductListingTranslationWriterFactory::new()
        .in_transaction(&mut tx)
        .apply(write)
        .await?;
    tx.commit().await?;
    Ok(outcome)
}

fn write(
    product_listing_id: ProductListingId,
    source_event_id: EventId,
    enrichment_event_id: EventId,
) -> ProductListingTranslationWrite {
    ProductListingTranslationWrite {
        product_listing_id,
        source_event_id,
        enrichment_event_id,
        source_language: Language::De,
        titles: IndexMap::from([
            (Language::En, Title::from("Antique chair")),
            (Language::Fr, Title::from("Chaise ancienne")),
        ]),
    }
}

async fn insert_product_with_discovered_event(
    pool: &sqlx::PgPool,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Translation party')",
    )
    .bind(party_id)
    .bind(format!("translation-party-{party_id}"))
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Translation source', $3)")
        .bind(listing_source_id)
        .bind(format!("translation-source-{listing_source_id}"))
        .bind(party_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Stuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_listing_id.into_uuid())
        .bind(title_slug("translation-product", product_listing_id))
        .bind(event_id.into_uuid())
        .bind(listing_source_id)
        .bind(product_listing_id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(event_id.into_uuid())
        .bind(product_listing_id.into_uuid())
        .bind(serde_json::json!({
            "listingSourceId": listing_source_id.to_string(),
            "sourceListingId": product_listing_id.to_string(),
            "title": {"language": "de", "text": "Antiker Stuhl"},
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": {"start": null, "end": null}
        }))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((product_listing_id, event_id))
}

async fn insert_event_and_advance_product(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    event_type: &str,
    event_group: &str,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, $3, $4, 1, $5, now())")
        .bind(event_id.into_uuid())
        .bind(product_listing_id.into_uuid())
        .bind(event_type)
        .bind(event_group)
        .bind(serde_json::json!({
            "images": {"previousCount": 0, "currentCount": 0}
        }))
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE product_listings SET current_event_id = $1, content_source_event_id = $1, version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2")
        .bind(event_id.into_uuid())
        .bind(product_listing_id.into_uuid())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(event_id)
}

fn title_slug(prefix: &str, product_listing_id: ProductListingId) -> String {
    format!(
        "{prefix}-{}",
        &product_listing_id.as_uuid().simple().to_string()[26..]
    )
}
