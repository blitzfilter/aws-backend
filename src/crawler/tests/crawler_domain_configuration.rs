use crawler::local_db::crawler_domain_configuration_repository::CrawlerDomainConfigurationRepositoryImpl;
use crawler::service::crawler_domain_configuration::{
    CrawlerDomainConfigurationError, CrawlerDomainConfigurationRepository,
};
use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
    RegisteredListingSource,
};
use crawler::spider::candidate_service::{SpiderCandidateService, SpiderCandidateServiceImpl};
use crawler::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::{Domain, ListingSourceId, ListingSourceName, ListingSourceSlugId};
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

fn source(id: ListingSourceId, enabled: bool) -> RegisteredListingSource {
    RegisteredListingSource {
        listing_source_id: id,
        listing_source_name: ListingSourceName::try_from("Test source").unwrap(),
        listing_source_slug: ListingSourceSlugId::raw("test-source").unwrap(),
        crawl_enabled: enabled,
        fallback_currency: None,
    }
}

async fn insert_pending_review(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
    domain_id: Option<CrawlerDomainId>,
    artifact_type: &str,
) -> Result<CrawlerReviewId, sqlx::Error> {
    let review_id = CrawlerReviewId::new();
    sqlx::query(
        "INSERT INTO crawler_reviews ( \
            review_id, listing_source_id, domain_id, artifact_type, status, reason, candidate_payload, validation_summary \
         ) VALUES ($1, $2, $3, $4, 'PENDING_REVIEW', 'test', '{}'::jsonb, '{}'::jsonb)",
    )
    .bind(review_id.as_uuid())
    .bind(listing_source_id.as_uuid())
    .bind(domain_id.map(|id| *id.as_uuid()))
    .bind(artifact_type)
    .execute(pool)
    .await?;
    Ok(review_id)
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_onboard_enabled_source_then_preserve_domain_while_disabled_and_reenable_it() {
    let pool = get_postgres_client().await;
    let registration = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let domains = CrawlerDomainConfigurationRepositoryImpl::new(pool.clone());
    let candidates = SpiderCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();

    registration
        .apply_snapshot(&[source(listing_source_id, true)])
        .await
        .unwrap();
    assert!(candidates.get_candidates(10, &[]).await.unwrap().is_empty());

    let configured = domains
        .register(listing_source_id, Domain::try_from("example.com").unwrap())
        .await
        .unwrap();
    let repeated = domains
        .register(listing_source_id, Domain::try_from("example.com").unwrap())
        .await
        .unwrap();
    assert_eq!(configured.domain_id, repeated.domain_id);
    assert_eq!(7, configured.domain_id.as_uuid().get_version_num());
    assert_eq!(candidates.get_candidates(10, &[]).await.unwrap().len(), 1);

    registration
        .apply_snapshot(&[source(listing_source_id, false)])
        .await
        .unwrap();
    assert!(candidates.get_candidates(10, &[]).await.unwrap().is_empty());
    assert_eq!(
        domains
            .list_for_source(listing_source_id)
            .await
            .unwrap()
            .len(),
        1
    );

    registration
        .apply_snapshot(&[source(listing_source_id, true)])
        .await
        .unwrap();
    let candidates = candidates.get_candidates(10, &[]).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].domain_id, configured.domain_id);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_surface_invalid_persisted_v4_domain_id() {
    let pool = get_postgres_client().await;
    let registration = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let domains = CrawlerDomainConfigurationRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    registration
        .apply_snapshot(&[source(listing_source_id, true)])
        .await
        .unwrap();
    let invalid_persisted_domain_id =
        uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000004").unwrap();
    sqlx::query(
        "INSERT INTO listing_source_domains \
         (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, 'corrupt-domain.example.com', 'corrupt-domain.example.com')",
    )
    .bind(invalid_persisted_domain_id)
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let result = domains.list_for_source(listing_source_id).await;

    assert!(matches!(
        result,
        Err(CrawlerDomainConfigurationError::Database { .. })
    ));
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_domain_claimed_by_another_listing_source() {
    let pool = get_postgres_client().await;
    let registration = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let domains = CrawlerDomainConfigurationRepositoryImpl::new(pool);
    let owner = ListingSourceId::new();
    let claimant = ListingSourceId::new();
    registration
        .apply_snapshot(&[source(owner, true), source(claimant, true)])
        .await
        .unwrap();
    domains
        .register(owner, Domain::try_from("owned.example.com").unwrap())
        .await
        .unwrap();

    let result = domains
        .register(claimant, Domain::try_from("owned.example.com").unwrap())
        .await;
    assert!(matches!(
        result,
        Err(CrawlerDomainConfigurationError::DomainOwnedByAnotherListingSource { .. })
    ));
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_register_a_concurrent_same_source_domain_once() {
    let pool = get_postgres_client().await;
    let registration = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let domains = CrawlerDomainConfigurationRepositoryImpl::new(pool);
    let listing_source_id = ListingSourceId::new();
    registration
        .apply_snapshot(&[source(listing_source_id, true)])
        .await
        .unwrap();

    let (first, second) = tokio::join!(
        domains.register(
            listing_source_id,
            Domain::try_from("concurrent-domain.example.com").unwrap(),
        ),
        domains.register(
            listing_source_id,
            Domain::try_from("concurrent-domain.example.com").unwrap(),
        ),
    );
    let first = first.unwrap();
    let second = second.unwrap();

    assert_eq!(first.domain_id, second.domain_id);
    assert_ne!(first.created, second.created);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_treat_bare_www_and_trailing_dot_as_one_crawler_domain_identity() {
    let pool = get_postgres_client().await;
    let registration = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let domains = CrawlerDomainConfigurationRepositoryImpl::new(pool);
    let owner = ListingSourceId::new();
    let claimant = ListingSourceId::new();
    registration
        .apply_snapshot(&[source(owner, true), source(claimant, true)])
        .await
        .unwrap();

    let bare = domains
        .register(owner, Domain::try_from("example.com").unwrap())
        .await
        .unwrap();
    let equivalent = domains
        .register(owner, Domain::try_from("www.example.com.").unwrap())
        .await
        .unwrap();
    let conflict = domains
        .register(claimant, Domain::try_from("www.example.com").unwrap())
        .await;

    assert_eq!(bare.domain_id, equivalent.domain_id);
    assert_eq!(equivalent.domain.as_str(), "example.com");
    assert!(matches!(
        conflict,
        Err(CrawlerDomainConfigurationError::DomainOwnedByAnotherListingSource { .. })
    ));
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_ip_literal_crawler_domain() {
    let pool = get_postgres_client().await;
    let registration = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let domains = CrawlerDomainConfigurationRepositoryImpl::new(pool);
    let listing_source_id = ListingSourceId::new();
    registration
        .apply_snapshot(&[source(listing_source_id, true)])
        .await
        .unwrap();

    let result = domains
        .register(
            listing_source_id,
            Domain::try_from("169.254.169.254").unwrap(),
        )
        .await;

    assert!(matches!(
        result,
        Err(CrawlerDomainConfigurationError::UnsafeDomain { .. })
    ));
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_repeated_www_prefix_and_mismatched_raw_sql_crawl_root() {
    let pool = get_postgres_client().await;
    let registration = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let domains = CrawlerDomainConfigurationRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    registration
        .apply_snapshot(&[source(listing_source_id, true)])
        .await
        .unwrap();

    let repeated_www = domains
        .register(
            listing_source_id,
            Domain::try_from("www.www.example.com").unwrap(),
        )
        .await;
    assert!(matches!(
        repeated_www,
        Err(CrawlerDomainConfigurationError::RepeatedWwwPrefix { .. })
    ));

    for (owner, root) in [
        ("example.com", "unrelated.example"),
        ("example.com", "www.www.example.com"),
    ] {
        let domain_id = CrawlerDomainId::new();
        assert!(
            sqlx::query(
                "INSERT INTO listing_source_domains \
             (domain_id, listing_source_id, listing_source_domain, crawl_root_host) VALUES ($1, $2, $3, $4)",
            )
            .bind(domain_id.as_uuid())
            .bind(listing_source_id.as_uuid())
            .bind(owner)
            .bind(root)
            .execute(&pool)
            .await
            .is_err()
        );
    }
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_reviews_with_an_invalid_domain_shape_or_owner() {
    let pool = get_postgres_client().await;
    let registration = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let domains = CrawlerDomainConfigurationRepositoryImpl::new(pool.clone());
    let first_source_id = ListingSourceId::new();
    let second_source_id = ListingSourceId::new();
    registration
        .apply_snapshot(&[
            source(first_source_id, true),
            source(second_source_id, true),
        ])
        .await
        .unwrap();
    let first_domain = domains
        .register(
            first_source_id,
            Domain::try_from("first.example.com").unwrap(),
        )
        .await
        .unwrap();
    let second_domain = domains
        .register(
            second_source_id,
            Domain::try_from("second.example.com").unwrap(),
        )
        .await
        .unwrap();

    assert!(
        insert_pending_review(&pool, first_source_id, None, "URL_PATTERN")
            .await
            .is_err()
    );
    assert!(
        insert_pending_review(
            &pool,
            first_source_id,
            Some(second_domain.domain_id),
            "URL_PATTERN",
        )
        .await
        .is_err()
    );
    assert!(
        insert_pending_review(
            &pool,
            first_source_id,
            Some(first_domain.domain_id),
            "PRODUCT_SCHEMA",
        )
        .await
        .is_err()
    );
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_remove_pending_url_pattern_review_and_preserve_product_schema_review_when_domain_removed()
 {
    let pool = get_postgres_client().await;
    let registration = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let domains = CrawlerDomainConfigurationRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    registration
        .apply_snapshot(&[source(listing_source_id, true)])
        .await
        .unwrap();
    let domain = domains
        .register(
            listing_source_id,
            Domain::try_from("reviews.example.com").unwrap(),
        )
        .await
        .unwrap();
    let url_pattern_review_id = insert_pending_review(
        &pool,
        listing_source_id,
        Some(domain.domain_id),
        "URL_PATTERN",
    )
    .await
    .unwrap();
    let product_schema_review_id =
        insert_pending_review(&pool, listing_source_id, None, "PRODUCT_SCHEMA")
            .await
            .unwrap();

    let removal = domains
        .remove(listing_source_id, domain.domain_id)
        .await
        .unwrap();

    assert_eq!(removal.domain_id, domain.domain_id);
    assert_eq!(removal.removed_url_count, 0);
    assert_eq!(removal.removed_url_pattern_review_count, 1);
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM crawler_reviews WHERE review_id = $1)",
        )
        .bind(url_pattern_review_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM crawler_reviews WHERE review_id = $1)",
        )
        .bind(product_schema_review_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
    );
}
