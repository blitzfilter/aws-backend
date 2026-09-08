use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
    RegisteredListingSource,
};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use money::Currency;
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

type ListingSourceRow = (
    uuid::Uuid,
    String,
    String,
    bool,
    time::OffsetDateTime,
    time::OffsetDateTime,
);
type ListingSourceDomainRow = (
    uuid::Uuid,
    String,
    Option<String>,
    String,
    Option<time::OffsetDateTime>,
    i32,
    Option<String>,
    Option<time::OffsetDateTime>,
);

fn listing_source(
    listing_source_id: ListingSourceId,
    crawl_enabled: bool,
) -> RegisteredListingSource {
    RegisteredListingSource {
        listing_source_id,
        listing_source_name: ListingSourceName::try_from("Test source").unwrap(),
        listing_source_slug: ListingSourceSlugId::raw("test-source").unwrap(),
        crawl_enabled,
        fallback_currency: None,
    }
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_persist_and_clear_fallback_currency_from_listing_source_snapshot() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    let mut source = listing_source(listing_source_id, true);
    source.fallback_currency = Some(Currency::Zar);

    repository.apply_snapshot(&[source.clone()]).await.unwrap();
    let persisted: Option<String> = sqlx::query_scalar(
        "SELECT fallback_currency FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(Some("ZAR".to_owned()), persisted);

    source.fallback_currency = None;
    repository.apply_snapshot(&[source]).await.unwrap();
    let cleared: Option<String> = sqlx::query_scalar(
        "SELECT fallback_currency FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(None, cleared);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_disable_absent_sources_without_deleting_local_domain_configuration() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let kept = ListingSourceId::new();
    let removed = ListingSourceId::new();
    repository
        .apply_snapshot(&[listing_source(kept, true), listing_source(removed, true)])
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO listing_source_domains (listing_source_id, listing_source_domain, crawl_root_host) VALUES ($1, $2, $2)",
    )
    .bind(uuid::Uuid::from(removed))
    .bind("removed.example.com")
    .execute(&pool)
    .await
    .unwrap();

    let result = repository
        .apply_snapshot(&[listing_source(kept, true)])
        .await
        .unwrap();
    assert_eq!(result.disabled, 1);
    let enabled: bool = sqlx::query_scalar(
        "SELECT crawl_enabled FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(uuid::Uuid::from(removed))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!enabled);
    let domain_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM listing_source_domains WHERE listing_source_id = $1",
    )
    .bind(uuid::Uuid::from(removed))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(domain_count, 1);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_default_ad_hoc_listing_sources_to_crawl_disabled() {
    let pool = get_postgres_client().await;
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug) \
         VALUES ($1, 'Ad hoc', 'ad-hoc')",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .execute(&pool)
    .await
    .unwrap();
    let enabled: bool = sqlx::query_scalar(
        "SELECT crawl_enabled FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!enabled);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_roll_back_snapshot_when_final_disable_update_fails_and_retry_after_trigger_removal()
{
    let pool = get_postgres_client().await;
    let repository = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let kept = ListingSourceId::new();
    let removed = ListingSourceId::new();

    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
         VALUES \
         ($1, 'Old kept source', 'old-kept-source', TRUE, NOW(), NOW()), \
         ($2, 'Removed source', 'removed-source', TRUE, NOW(), NOW())",
    )
    .bind(uuid::Uuid::from(kept))
    .bind(uuid::Uuid::from(removed))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO listing_source_domains \
         (listing_source_id, listing_source_domain, crawl_root_host, url_pattern, url_pattern_state, last_crawled, \
          crawl_failure_count, last_crawl_error_kind, next_crawl_at) \
         VALUES \
         ($1, 'kept.example.com', 'kept.example.com', '/products/', 'MATCHED', NOW() - INTERVAL '1 day', \
          3, 'HTTP_500', NOW() + INTERVAL '1 hour'), \
         ($2, 'removed.example.com', 'removed.example.com', '/stock/', 'MATCHED', NOW() - INTERVAL '2 days', \
          4, 'TIMEOUT', NOW() + INTERVAL '2 hours')",
    )
    .bind(uuid::Uuid::from(kept))
    .bind(uuid::Uuid::from(removed))
    .execute(&pool)
    .await
    .unwrap();

    let source_rows_before: Vec<ListingSourceRow> = sqlx::query_as(
        "SELECT listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated \
         FROM listing_sources ORDER BY listing_source_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let domain_rows_before: Vec<ListingSourceDomainRow> = sqlx::query_as(
        "SELECT listing_source_id, listing_source_domain, url_pattern, url_pattern_state, last_crawled, \
         crawl_failure_count, last_crawl_error_kind, next_crawl_at \
         FROM listing_source_domains ORDER BY listing_source_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    sqlx::query(
        "CREATE FUNCTION fail_listing_source_snapshot_disable() RETURNS trigger \
         LANGUAGE plpgsql AS $$ \
         BEGIN \
             RAISE EXCEPTION 'injected final listing source disable failure'; \
         END; \
         $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_listing_source_snapshot_disable \
         BEFORE UPDATE OF crawl_enabled ON listing_sources \
         FOR EACH ROW \
         WHEN (OLD.crawl_enabled IS TRUE AND NEW.crawl_enabled IS FALSE) \
         EXECUTE FUNCTION fail_listing_source_snapshot_disable()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let failed_snapshot = repository
        .apply_snapshot(&[listing_source(kept, true)])
        .await;

    sqlx::query("DROP TRIGGER fail_listing_source_snapshot_disable ON listing_sources")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION fail_listing_source_snapshot_disable()")
        .execute(&pool)
        .await
        .unwrap();

    assert!(failed_snapshot.is_err());
    let source_rows_after_failure: Vec<ListingSourceRow> = sqlx::query_as(
        "SELECT listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated \
         FROM listing_sources ORDER BY listing_source_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let domain_rows_after_failure: Vec<ListingSourceDomainRow> = sqlx::query_as(
        "SELECT listing_source_id, listing_source_domain, url_pattern, url_pattern_state, last_crawled, \
         crawl_failure_count, last_crawl_error_kind, next_crawl_at \
         FROM listing_source_domains ORDER BY listing_source_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(source_rows_after_failure, source_rows_before);
    assert_eq!(domain_rows_after_failure, domain_rows_before);

    let retry = repository
        .apply_snapshot(&[listing_source(kept, true)])
        .await;
    assert!(matches!(retry, Ok(result) if result.disabled == 1));
    let source_rows_after_retry: Vec<(uuid::Uuid, String, String, bool)> = sqlx::query_as(
        "SELECT listing_source_id, listing_source_name, listing_source_slug, crawl_enabled \
         FROM listing_sources ORDER BY listing_source_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let kept_uuid = uuid::Uuid::from(kept);
    let removed_uuid = uuid::Uuid::from(removed);
    assert!(source_rows_after_retry.contains(&(
        kept_uuid,
        "Test source".to_owned(),
        "test-source".to_owned(),
        true,
    )));
    assert!(source_rows_after_retry.contains(&(
        removed_uuid,
        "Removed source".to_owned(),
        "removed-source".to_owned(),
        false,
    )));
    let domain_rows_after_retry: Vec<ListingSourceDomainRow> = sqlx::query_as(
        "SELECT listing_source_id, listing_source_domain, url_pattern, url_pattern_state, last_crawled, \
         crawl_failure_count, last_crawl_error_kind, next_crawl_at \
         FROM listing_source_domains ORDER BY listing_source_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(domain_rows_after_retry, domain_rows_before);
}
