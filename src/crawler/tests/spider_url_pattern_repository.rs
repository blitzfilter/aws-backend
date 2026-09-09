use crawler::CrawlerDomainId;
use crawler::spider::classification::url_pattern_repository::{
    ListingSourceUrlPatternRepository, ListingSourceUrlPatternRepositoryImpl, UrlPatternState,
};
use listing_source_core::ListingSourceId;
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

async fn insert_source(pool: &sqlx::PgPool, listing_source_id: ListingSourceId) {
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled) \
         VALUES ($1, 'Test source', 'test-source', TRUE)",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_domain(
    pool: &sqlx::PgPool,
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

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_return_none_when_domain_is_missing_for_listing_source() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;

    let found = repository
        .find_pattern(&listing_source_id, &CrawlerDomainId::new())
        .await
        .unwrap();

    assert!(found.is_none());
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_save_replace_and_clear_pattern_for_owned_domain() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "patterns.example.com").await;

    let initial = repository
        .find_pattern(&listing_source_id, &domain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial.listing_source_id, listing_source_id);
    assert_eq!(initial.domain_id, domain_id);
    assert!(initial.url_pattern.is_none());
    assert_eq!(initial.url_pattern_state, UrlPatternState::Unknown);

    repository
        .save_pattern(&listing_source_id, &domain_id, Some(r"/products/"))
        .await
        .unwrap();
    let saved = repository
        .find_pattern(&listing_source_id, &domain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.url_pattern.as_deref(), Some(r"/products/"));
    assert_eq!(saved.url_pattern_state, UrlPatternState::Matched);

    repository
        .save_pattern(&listing_source_id, &domain_id, Some(r"/objects/"))
        .await
        .unwrap();
    let replaced = repository
        .find_pattern(&listing_source_id, &domain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replaced.url_pattern.as_deref(), Some(r"/objects/"));
    assert_eq!(replaced.url_pattern_state, UrlPatternState::Matched);

    repository
        .save_pattern(&listing_source_id, &domain_id, None)
        .await
        .unwrap();
    let cleared = repository
        .find_pattern(&listing_source_id, &domain_id)
        .await
        .unwrap()
        .unwrap();
    assert!(cleared.url_pattern.is_none());
    assert_eq!(cleared.url_pattern_state, UrlPatternState::Unknown);

    repository
        .save_no_pattern(&listing_source_id, &domain_id)
        .await
        .unwrap();
    let no_pattern = repository
        .find_pattern(&listing_source_id, &domain_id)
        .await
        .unwrap()
        .unwrap();
    assert!(no_pattern.url_pattern.is_none());
    assert_eq!(no_pattern.url_pattern_state, UrlPatternState::NoPattern);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_mark_owned_domain_as_crawled() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "crawled.example.com").await;

    let before = repository
        .find_pattern(&listing_source_id, &domain_id)
        .await
        .unwrap()
        .unwrap();
    assert!(before.last_crawled.is_none());

    repository
        .mark_as_crawled(&listing_source_id, &domain_id)
        .await
        .unwrap();

    let crawled = repository
        .find_pattern(&listing_source_id, &domain_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(crawled.listing_source_id, listing_source_id);
    assert_eq!(crawled.domain_id, domain_id);
    assert!(crawled.last_crawled.is_some());
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_store_patterns_independently_for_domains_of_one_listing_source() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_a = insert_domain(&pool, listing_source_id, "a.example.com").await;
    let domain_b = insert_domain(&pool, listing_source_id, "b.example.com").await;

    repository
        .save_pattern(&listing_source_id, &domain_a, Some(r"/products/"))
        .await
        .unwrap();
    repository
        .save_pattern(&listing_source_id, &domain_b, Some(r"/objects/"))
        .await
        .unwrap();

    let pattern_a = repository
        .find_pattern(&listing_source_id, &domain_a)
        .await
        .unwrap()
        .unwrap();
    let pattern_b = repository
        .find_pattern(&listing_source_id, &domain_b)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pattern_a.url_pattern.as_deref(), Some(r"/products/"));
    assert_eq!(pattern_a.url_pattern_state, UrlPatternState::Matched);
    assert!(pattern_a.last_crawled.is_none());
    assert_eq!(pattern_b.url_pattern.as_deref(), Some(r"/objects/"));
    assert_eq!(pattern_b.url_pattern_state, UrlPatternState::Matched);
    assert!(pattern_b.last_crawled.is_none());

    repository
        .mark_as_crawled(&listing_source_id, &domain_a)
        .await
        .unwrap();
    let crawled_a = repository
        .find_pattern(&listing_source_id, &domain_a)
        .await
        .unwrap()
        .unwrap();
    let untouched_b = repository
        .find_pattern(&listing_source_id, &domain_b)
        .await
        .unwrap()
        .unwrap();
    assert!(crawled_a.last_crawled.is_some());
    assert_eq!(crawled_a.url_pattern.as_deref(), Some(r"/products/"));
    assert!(untouched_b.last_crawled.is_none());
    assert_eq!(untouched_b.url_pattern.as_deref(), Some(r"/objects/"));
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_pattern_write_for_domain_owned_by_another_listing_source() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let owner = ListingSourceId::new();
    let other = ListingSourceId::new();
    insert_source(&pool, owner).await;
    insert_source(&pool, other).await;
    let domain_id = insert_domain(&pool, owner, "owned.example.com").await;

    let result = repository
        .save_pattern(&other, &domain_id, Some(r"/items/"))
        .await;
    assert!(matches!(result, Err(sqlx::Error::RowNotFound)));
    let stored: Option<String> =
        sqlx::query_scalar("SELECT url_pattern FROM listing_source_domains WHERE domain_id = $1")
            .bind(domain_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(stored.is_none());
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_crawl_mark_for_domain_owned_by_another_listing_source() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let owner = ListingSourceId::new();
    let other = ListingSourceId::new();
    insert_source(&pool, owner).await;
    insert_source(&pool, other).await;
    let domain_id = insert_domain(&pool, owner, "marked.example.com").await;

    let result = repository.mark_as_crawled(&other, &domain_id).await;
    assert!(matches!(result, Err(sqlx::Error::RowNotFound)));
    let crawled: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT last_crawled FROM listing_source_domains WHERE domain_id = $1")
            .bind(domain_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(crawled.is_none());
}
