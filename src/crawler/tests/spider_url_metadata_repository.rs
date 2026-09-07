use crawler::CrawlerDomainId;
use crawler::spider::classification::url_metadata::{
    CrawlerDisposition, CrawlerUrlWriteOutcome, UrlClass,
};
use crawler::spider::classification::url_metadata_repository::{
    UrlMetadataRepository, UrlMetadataRepositoryError, UrlMetadataRepositoryImpl,
};
use listing_source_core::ListingSourceId;
use test_api::*;
use url::Url;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

async fn insert_source(pool: &sqlx::PgPool, listing_source_id: ListingSourceId) {
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled) \
         VALUES ($1, 'Test source', 'test-source', TRUE)",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_domain(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
    domain: &str,
) -> CrawlerDomainId {
    sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO listing_source_domains (listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, $2) RETURNING domain_id",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(domain)
    .fetch_one(pool)
    .await
    .unwrap()
    .into()
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_insert_and_update_url_for_its_same_owner() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "same-owner.example.com").await;
    let url = Url::parse("https://same-owner.example.com/products/1").unwrap();

    let inserted = repository
        .upsert_link(
            &listing_source_id,
            &domain_id,
            &url,
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();
    assert_eq!(inserted.listing_source_id, listing_source_id);
    assert_eq!(inserted.domain_id, domain_id);
    assert_eq!(inserted.url, url);
    assert_eq!(inserted.url_class, UrlClass::ProductListing);
    assert_eq!(inserted.disposition, CrawlerDisposition::Active);
    assert!(inserted.last_scraped.is_none());
    assert!(inserted.last_scraped_hash.is_none());

    let updated = repository
        .upsert_link(&listing_source_id, &domain_id, &url, &UrlClass::Other)
        .await
        .unwrap();
    assert_eq!(updated.listing_source_id, listing_source_id);
    assert_eq!(updated.domain_id, domain_id);
    assert_eq!(updated.url, url);
    assert_eq!(updated.url_class, UrlClass::Other);
    assert_eq!(updated.disposition, CrawlerDisposition::Active);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_mark_owned_url_as_scraped() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "scraped.example.com").await;
    let url = Url::parse("https://scraped.example.com/products/1").unwrap();
    repository
        .upsert_link(
            &listing_source_id,
            &domain_id,
            &url,
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();

    assert_eq!(
        repository
            .mark_as_scraped(&listing_source_id, &url, "content-hash")
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    let scraped: (Option<String>, Option<String>, String) = sqlx::query_as(
        "SELECT last_scraped_hash, last_scraped::text, crawler_disposition \
         FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(scraped.0.as_deref(), Some("content-hash"));
    assert!(scraped.1.is_some());
    assert_eq!(scraped.2, CrawlerDisposition::Active.as_str());
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_transition_owned_url_to_dormant_without_reactivation() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "presence.example.com").await;
    let url = Url::parse("https://presence.example.com/products/1").unwrap();
    repository
        .upsert_link(
            &listing_source_id,
            &domain_id,
            &url,
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();

    assert_eq!(
        repository
            .set_disposition(&listing_source_id, &url, CrawlerDisposition::DormantRemoved)
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    assert_eq!(
        repository
            .set_disposition(&listing_source_id, &url, CrawlerDisposition::Active)
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale
    );
    let row: (String, uuid::Uuid) = sqlx::query_as(
        "SELECT crawler_disposition, domain_id FROM listing_source_urls \
         WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, CrawlerDisposition::DormantRemoved.as_str());
    assert_eq!(CrawlerDomainId::from(row.1), domain_id);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_keep_dormant_removed_url_absorbing_during_rediscovery_and_active_update() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "absorbing.example.com").await;
    let url = Url::parse("https://absorbing.example.com/products/1").unwrap();
    repository
        .upsert_link(
            &listing_source_id,
            &domain_id,
            &url,
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();

    assert_eq!(
        repository
            .set_disposition(&listing_source_id, &url, CrawlerDisposition::DormantRemoved)
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    let rediscovered = repository
        .upsert_link(&listing_source_id, &domain_id, &url, &UrlClass::Other)
        .await
        .unwrap();
    assert_eq!(rediscovered.disposition, CrawlerDisposition::DormantRemoved);
    assert_eq!(rediscovered.url_class, UrlClass::ProductListing);

    assert_eq!(
        repository
            .mark_as_scraped(&listing_source_id, &url, "delayed-active-hash")
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale
    );
    assert_eq!(
        repository
            .set_disposition(&listing_source_id, &url, CrawlerDisposition::Active)
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale
    );
    let row: (String, Option<String>) = sqlx::query_as(
        "SELECT crawler_disposition, last_scraped_hash FROM listing_source_urls \
         WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, CrawlerDisposition::DormantRemoved.as_str());
    assert!(row.1.is_none());
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_upsert_valid_url_batch_for_one_owned_domain() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "batch.example.com").await;
    let first = Url::parse("https://batch.example.com/products/1").unwrap();
    let second = Url::parse("https://batch.example.com/products/2").unwrap();

    let records = repository
        .upsert_links_batch(
            &listing_source_id,
            &domain_id,
            &[first.clone(), second.clone()],
            &[UrlClass::ProductListing, UrlClass::Category],
        )
        .await
        .unwrap();

    assert_eq!(records.len(), 2);
    assert!(records.iter().any(|record| {
        record.listing_source_id == listing_source_id
            && record.domain_id == domain_id
            && record.url == first
            && record.url_class == UrlClass::ProductListing
            && record.disposition == CrawlerDisposition::Active
    }));
    assert!(records.iter().any(|record| {
        record.listing_source_id == listing_source_id
            && record.domain_id == domain_id
            && record.url == second
            && record.url_class == UrlClass::Category
            && record.disposition == CrawlerDisposition::Active
    }));
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_batch_when_url_and_class_lengths_differ() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "lengths.example.com").await;
    let url = Url::parse("https://lengths.example.com/products/1").unwrap();

    let result = repository
        .upsert_links_batch(
            &listing_source_id,
            &domain_id,
            std::slice::from_ref(&url),
            &[UrlClass::ProductListing, UrlClass::Other],
        )
        .await;

    assert!(matches!(
        result,
        Err(UrlMetadataRepositoryError::Database {
            source: sqlx::Error::Protocol(message)
        }) if message == "URL and URL-class batch lengths differ"
    ));
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_duplicate_urls_in_batch_without_persisting_any_row() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "duplicates.example.com").await;
    let url = Url::parse("https://duplicates.example.com/products/1").unwrap();

    let result = repository
        .upsert_links_batch(
            &listing_source_id,
            &domain_id,
            &[url.clone(), url.clone()],
            &[UrlClass::ProductListing, UrlClass::Other],
        )
        .await;

    assert!(matches!(
        result,
        Err(UrlMetadataRepositoryError::Database {
            source: sqlx::Error::Database(_)
        })
    ));
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_delete_urls_when_their_domain_is_deleted() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "cascade.example.com").await;
    let url = Url::parse("https://cascade.example.com/products/1").unwrap();
    repository
        .upsert_link(
            &listing_source_id,
            &domain_id,
            &url,
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();

    sqlx::query(
        "DELETE FROM listing_source_domains WHERE listing_source_id = $1 AND domain_id = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(uuid::Uuid::from(domain_id))
    .execute(&pool)
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM listing_source_urls WHERE url = $1")
        .bind(url.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_new_url_when_domain_belongs_to_another_listing_source() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let source_a = ListingSourceId::new();
    let source_b = ListingSourceId::new();
    insert_source(&pool, source_a).await;
    insert_source(&pool, source_b).await;
    let domain_b = insert_domain(&pool, source_b, "b.example.com").await;
    let url = Url::parse("https://b.example.com/product/1").unwrap();

    let result = repository
        .upsert_link(&source_a, &domain_b, &url, &UrlClass::ProductListing)
        .await;
    assert!(matches!(
        result,
        Err(UrlMetadataRepositoryError::DomainNotOwnedByListingSource { .. })
    ));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM listing_source_urls")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_cross_source_url_claim_when_requested_domain_does_not_own_the_url_host() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let source_a = ListingSourceId::new();
    let source_b = ListingSourceId::new();
    insert_source(&pool, source_a).await;
    insert_source(&pool, source_b).await;
    let domain_a = insert_domain(&pool, source_a, "shared.example.com").await;
    let domain_b = insert_domain(&pool, source_b, "b.example.com").await;
    let url = Url::parse("https://shared.example.com/product/1").unwrap();

    repository
        .upsert_link(&source_a, &domain_a, &url, &UrlClass::ProductListing)
        .await
        .unwrap();
    let conflict = repository
        .upsert_link(&source_b, &domain_b, &url, &UrlClass::Other)
        .await;
    assert!(matches!(
        conflict,
        Err(UrlMetadataRepositoryError::UrlHostDoesNotMatchDomain { .. })
    ));

    let row: (uuid::Uuid, uuid::Uuid, String) = sqlx::query_as(
        "SELECT listing_source_id, domain_id, url_class FROM listing_source_urls WHERE url = $1",
    )
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, uuid::Uuid::from(source_a));
    assert_eq!(row.1, uuid::Uuid::from(domain_a));
    assert_eq!(row.2, "product");
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_duplicate_canonical_domain_for_same_source() {
    let pool = get_postgres_client().await;
    let source = ListingSourceId::new();
    insert_source(&pool, source).await;
    insert_domain(&pool, source, "example.com").await;

    let duplicate = sqlx::query(
        "INSERT INTO listing_source_domains \
         (listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, $3)",
    )
    .bind(uuid::Uuid::from(source))
    .bind("example.com")
    .bind("www.example.com")
    .execute(&pool)
    .await;

    assert!(duplicate.is_err());
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_roll_back_batch_when_one_url_is_owned_by_another_listing_source() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let source_a = ListingSourceId::new();
    let source_b = ListingSourceId::new();
    insert_source(&pool, source_a).await;
    insert_source(&pool, source_b).await;
    let domain_a = insert_domain(&pool, source_a, "shared.example.com").await;
    let domain_b = insert_domain(&pool, source_b, "batch-b.example.com").await;
    let conflicting = Url::parse("https://shared.example.com/product/1").unwrap();
    let fresh = Url::parse("https://batch-b.example.com/product/2").unwrap();
    repository
        .upsert_link(
            &source_a,
            &domain_a,
            &conflicting,
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();

    let result = repository
        .upsert_links_batch(
            &source_b,
            &domain_b,
            &[conflicting.clone(), fresh.clone()],
            &[UrlClass::Other, UrlClass::ProductListing],
        )
        .await;
    assert!(matches!(
        result,
        Err(UrlMetadataRepositoryError::UrlHostDoesNotMatchDomain { .. })
    ));
    let fresh_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM listing_source_urls WHERE url = $1")
            .bind(fresh.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(fresh_count, 0);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_accept_bare_and_www_domain_equivalence_but_reject_other_subdomains() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let source = ListingSourceId::new();
    insert_source(&pool, source).await;
    let domain = insert_domain(&pool, source, "example.com").await;

    repository
        .upsert_link(
            &source,
            &domain,
            &Url::parse("https://www.example.com/products/1").unwrap(),
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();
    let result = repository
        .upsert_link(
            &source,
            &domain,
            &Url::parse("https://catalog.example.com/products/2").unwrap(),
            &UrlClass::ProductListing,
        )
        .await;

    assert!(matches!(
        result,
        Err(UrlMetadataRepositoryError::UrlHostDoesNotMatchDomain { .. })
    ));
}
