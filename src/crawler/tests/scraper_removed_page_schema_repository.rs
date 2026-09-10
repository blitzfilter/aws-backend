use crawler::scraper::css_selector::removed_page_schema::{
    ListingSourceRemovedPageSchema, RemovedPageSchema,
};
use crawler::scraper::css_selector::removed_page_schema_repository::{
    RemovedPageSchemaRepository, RemovedPageSchemaRepositoryImpl,
};
use crawler::scraper::css_selector::rule::CssSelector;
use listing_source_core::ListingSourceId;
use sqlx::PgPool;
use test_api::*;
use time::OffsetDateTime;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

fn removed_schema(selector: &str, text: &str) -> RemovedPageSchema {
    RemovedPageSchema {
        selector: CssSelector::from(selector),
        text: Some(text.to_string()),
        regex: None,
    }
}

fn make_listing_source_removed_page_schemas(
    listing_source_id: ListingSourceId,
) -> ListingSourceRemovedPageSchema {
    let now = OffsetDateTime::now_utc();
    ListingSourceRemovedPageSchema {
        listing_source_id,
        removed_page_schemas: vec![removed_schema("#main h1", "ProductListing removed")],
        created: now,
        updated: now,
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

#[aura_integration_test(services = [POSTGRES])]
async fn should_insert_and_find_removed_schema_for_listing_source() {
    let pool = get_postgres_client().await;
    let repository = RemovedPageSchemaRepositoryImpl::new(&pool);
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;

    let row = make_listing_source_removed_page_schemas(listing_source_id);
    repository
        .insert_removed_page_schema(&listing_source_id, &row)
        .await
        .unwrap();

    let found = repository
        .find_removed_page_schema(&listing_source_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found.listing_source_id, listing_source_id);
    assert_eq!(
        found.removed_page_schemas,
        vec![removed_schema("#main h1", "ProductListing removed")]
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_update_removed_schema_for_only_target_listing_source() {
    let pool = get_postgres_client().await;
    let repository = RemovedPageSchemaRepositoryImpl::new(&pool);
    let first_listing_source_id = ListingSourceId::new();
    let second_listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, first_listing_source_id).await;
    insert_listing_source(&pool, second_listing_source_id).await;

    repository
        .insert_removed_page_schema(
            &first_listing_source_id,
            &make_listing_source_removed_page_schemas(first_listing_source_id),
        )
        .await
        .unwrap();
    repository
        .insert_removed_page_schema(
            &second_listing_source_id,
            &make_listing_source_removed_page_schemas(second_listing_source_id),
        )
        .await
        .unwrap();

    let updated = vec![removed_schema(".missing", "No longer here")];
    repository
        .update_removed_page_schema(&first_listing_source_id, &updated)
        .await
        .unwrap();

    let found_a = repository
        .find_removed_page_schema(&first_listing_source_id)
        .await
        .unwrap()
        .unwrap();
    let found_b = repository
        .find_removed_page_schema(&second_listing_source_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found_a.removed_page_schemas, updated);
    assert_eq!(
        found_b.removed_page_schemas,
        vec![removed_schema("#main h1", "ProductListing removed")]
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_delete_removed_schema_when_parent_listing_source_is_deleted() {
    let pool = get_postgres_client().await;
    let repository = RemovedPageSchemaRepositoryImpl::new(&pool);
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;

    repository
        .insert_removed_page_schema(
            &listing_source_id,
            &make_listing_source_removed_page_schemas(listing_source_id),
        )
        .await
        .unwrap();

    sqlx::query("DELETE FROM listing_sources WHERE listing_source_id = $1")
        .bind(listing_source_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    let found = repository
        .find_removed_page_schema(&listing_source_id)
        .await
        .unwrap();
    assert!(found.is_none());
}
