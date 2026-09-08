mod support;

use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use localization::Language;
use money::Currency;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::{
    product_listing::ProductListingPriceValuationBasis,
    product_listing_search::ProductListingSearch,
};
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use search_filter_core::{
    NewSearchFilter, PriceMatchValuation, SearchFilter, SearchFilterProductListingMatch,
};
use search_filter_postgres::{
    SqlxActiveSearchFilterMatchCandidateReaderFactory,
    SqlxSearchFilterMatchNotificationSourceReaderFactory, SqlxSearchFilterMatchWriterFactory,
};
use search_filter_postgres::{
    SqlxSearchFilterIndexReader, SqlxSearchFilterMatchRepositoryFactory,
    SqlxSearchFilterQuotaReaderFactory, SqlxSearchFilterReader, SqlxSearchFilterRepositoryFactory,
};
use search_filter_service::ports::{
    ActiveSearchFilterMatchCandidateReader, ActiveSearchFilterMatchCandidateReaderFactory,
    SearchFilterMatchCandidate, SearchFilterMatchNotificationSourceReader,
    SearchFilterMatchNotificationSourceReaderFactory, SearchFilterMatchPersistOutcome,
    SearchFilterMatchWriter, SearchFilterMatchWriterFactory,
};
use search_filter_service::ports::{
    SearchFilterIndexReader, SearchFilterMatchRepository, SearchFilterMatchRepositoryFactory,
    SearchFilterQuotaReader, SearchFilterQuotaReaderFactory, SearchFilterReader,
    SearchFilterRepository, SearchFilterRepositoryFactory,
};
use std::time::Duration;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_find_update_read_and_delete_search_filter() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxSearchFilterRepositoryFactory;
    let reader = SqlxSearchFilterReader::new(pool.clone());
    let user_id = seed_user(&pool, "search-filter-postgres-user@example.com").await;
    let mut filter = sample_filter(user_id, "daily finds");

    let mut tx = begin(&unit).await;
    repo.in_transaction(&mut tx)
        .insert(&filter)
        .await
        .unwrap_or_else(|error| panic!("insert failed: {error:?}"));
    let loaded = repo
        .in_transaction(&mut tx)
        .find_by_id(filter.id())
        .await
        .unwrap_or_else(|error| panic!("find failed: {error:?}"));
    assert!(matches!(loaded, Some(ref value) if value.filter.notifications()));
    let expected_version = match loaded {
        Some(value) => value.version,
        None => panic!("inserted filter was not found"),
    };
    filter.change_notifications(false);
    repo.in_transaction(&mut tx)
        .update(&filter, expected_version)
        .await
        .unwrap_or_else(|error| panic!("update failed: {error:?}"));
    commit(tx).await;

    let filters = reader
        .find_for_user(user_id)
        .await
        .unwrap_or_else(|error| panic!("list failed: {error:?}"));
    assert_eq!(1, filters.len());
    assert!(!filters[0].notifications);
    assert!(filters[0].updated >= filters[0].created);

    let mut tx = begin(&unit).await;
    repo.in_transaction(&mut tx)
        .delete(filter.id())
        .await
        .unwrap_or_else(|error| panic!("delete failed: {error:?}"));
    commit(tx).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_complete_versioned_projection_pages_from_postgres() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repository = SqlxSearchFilterRepositoryFactory;
    let reader = SqlxSearchFilterIndexReader::new(pool.clone());
    let user_id = seed_user(&pool, "search-filter-projection-reader@example.com").await;
    let first = sample_filter(user_id, "first projection");
    let second = sample_filter(user_id, "second projection");

    let mut tx = begin(&unit).await;
    repository
        .in_transaction(&mut tx)
        .insert(&first)
        .await
        .unwrap_or_else(|error| panic!("first insert failed: {error:?}"));
    repository
        .in_transaction(&mut tx)
        .insert(&second)
        .await
        .unwrap_or_else(|error| panic!("second insert failed: {error:?}"));
    commit(tx).await;

    let projection = reader
        .find_by_id(first.id())
        .await
        .unwrap_or_else(|error| panic!("projection read failed: {error:?}"));
    assert!(matches!(
        projection,
        Some(ref projection)
            if projection.view.search_filter_id == first.id() && projection.source_version == 1
    ));

    let first_page = reader
        .list_after(None, 1)
        .await
        .unwrap_or_else(|error| panic!("first projection page failed: {error:?}"));
    assert_eq!(1, first_page.len());
    let second_page = reader
        .list_after(Some(first_page[0].view.search_filter_id), 10)
        .await
        .unwrap_or_else(|error| panic!("second projection page failed: {error:?}"));
    assert_eq!(1, second_page.len());
    assert_ne!(
        first_page[0].view.search_filter_id,
        second_page[0].view.search_filter_id
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_already_exists_when_search_filter_exists() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxSearchFilterRepositoryFactory;
    let user_id = seed_user(&pool, "search-filter-postgres-duplicate@example.com").await;
    let filter = sample_filter(user_id, "daily duplicate");

    let mut tx = begin(&unit).await;
    repo.in_transaction(&mut tx)
        .insert(&filter)
        .await
        .unwrap_or_else(|error| panic!("insert failed: {error:?}"));
    let second = repo.in_transaction(&mut tx).insert(&filter).await;

    assert!(matches!(
        second,
        Err(search_filter_service::ports::SearchFilterRepositoryError::AlreadyExists)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_count_only_active_search_filters_in_transaction() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let filters = SqlxSearchFilterRepositoryFactory;
    let quotas = SqlxSearchFilterQuotaReaderFactory;
    let user_id = seed_user(&pool, "search-filter-postgres-quota@example.com").await;
    let active = sample_filter(user_id, "active filter");
    let mut inactive = sample_filter(user_id, "inactive filter");
    let _ = inactive.change_state(SearchFilterState::InactiveByUser);

    let mut tx = begin(&unit).await;
    filters
        .in_transaction(&mut tx)
        .insert(&active)
        .await
        .unwrap_or_else(|error| panic!("insert active filter failed: {error:?}"));
    filters
        .in_transaction(&mut tx)
        .insert(&inactive)
        .await
        .unwrap_or_else(|error| panic!("insert inactive filter failed: {error:?}"));

    let active_count = quotas
        .in_transaction(&mut tx)
        .count_active_for_user(user_id)
        .await
        .unwrap_or_else(|error| panic!("count active filters failed: {error:?}"));
    assert_eq!(1, active_count);
    commit(tx).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_find_and_update_search_filter_match() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let filters = SqlxSearchFilterRepositoryFactory;
    let matches = SqlxSearchFilterMatchRepositoryFactory;
    let user_id = seed_user(&pool, "search-filter-postgres-match@example.com").await;
    let filter = sample_filter(user_id, "match filter");
    let product_listing_id = seed_product(&pool, "search-filter-match-product").await;
    let event_id = seed_product_event(&pool, product_listing_id).await;
    let fx_rate_id = seed_fx_rate(&pool).await;
    let mut product_match = SearchFilterProductListingMatch {
        user_id,
        user_search_filter_id: filter.id(),
        user_search_filter_name: Some(filter.name().clone()),
        product_listing_id,
        origin_event_id: event_id,
        price_match_valuation: Some(PriceMatchValuation {
            basis: ProductListingPriceValuationBasis::Event,
            fx_rate_id,
        }),
        enhanced_match_reason: None,
        feedback: None,
    };

    let mut tx = begin(&unit).await;
    filters
        .in_transaction(&mut tx)
        .insert(&filter)
        .await
        .unwrap_or_else(|error| panic!("insert filter failed: {error:?}"));
    let inserted = matches
        .in_transaction(&mut tx)
        .insert(&product_match)
        .await
        .unwrap_or_else(|error| panic!("insert match failed: {error:?}"));
    assert!(inserted.updated >= inserted.created);
    let loaded = matches
        .in_transaction(&mut tx)
        .find_by_filter_and_product(filter.id(), product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("find match failed: {error:?}"));
    assert!(matches!(loaded, Some(ref value) if value.product_match == product_match));
    product_match.change_feedback(Some(true));
    let updated = matches
        .in_transaction(&mut tx)
        .update(&product_match)
        .await
        .unwrap_or_else(|error| panic!("update match failed: {error:?}"));
    assert_eq!(inserted.created, updated.created);
    assert!(updated.updated >= inserted.updated);
    commit(tx).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_evaluated_candidates_when_filter_changes_before_final_lock() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let unit = SqlxUnitOfWork::new(pool.clone());
        let product_id = seed_product(&pool, "candidate-race-product").await;
        let event_id = seed_product_event(&pool, product_id).await;
        for change in ["search", "embedding", "inactive"] {
            let user_id = seed_user(&pool, &format!("candidate-{change}@example.test")).await;
            let mut filter = sample_filter(user_id, change);
            let mut tx = unit.begin().await?;
            let persisted = SqlxSearchFilterRepositoryFactory
                .in_transaction(&mut tx)
                .insert(&filter)
                .await?;
            tx.commit().await?;
            let evaluated = match_candidate(&filter);

            // External work used the old inputs; an API write is now in flight.
            let mut editing = unit.begin().await?;
            let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(editing.connection())
                .await?;
            match change {
                "search" => {
                    filter.replace_search(
                        ProductListingSearch::new(Language::De, Currency::Eur),
                        None,
                    );
                }
                "embedding" => {
                    filter.replace_search(filter.search().clone(), Some(vec![0.5; 768]));
                }
                _ => {
                    filter.change_state(SearchFilterState::InactiveByUser);
                }
            }
            SqlxSearchFilterRepositoryFactory
                .in_transaction(&mut editing)
                .update(&filter, persisted.version)
                .await?;
            let final_write = persist_candidate(&pool, evaluated, product_id, event_id);
            tokio::pin!(final_write);
            support::assert_blocked(&pool, blocker_pid, 1, final_write.as_mut()).await?;
            editing.commit().await?;
            assert_eq!(
                None,
                tokio::time::timeout(Duration::from_secs(10), final_write).await??,
                "stale {change} evaluation claimed the permanent match row"
            );
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM search_filter_matches WHERE user_search_filter_id = $1",
            )
            .bind(uuid::Uuid::try_from(filter.id().to_string())?)
            .fetch_one(&pool)
            .await?;
            assert_eq!(0, count);
            if change != "inactive" {
                assert_eq!(
                    Some(SearchFilterMatchPersistOutcome::Inserted),
                    persist_candidate(&pool, match_candidate(&filter), product_id, event_id)
                        .await?
                );
            }
        }
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "candidate revalidation race: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_hold_filter_lock_through_match_commit_and_allow_unrelated_filter_edits() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let unit = SqlxUnitOfWork::new(pool.clone());
        let user_id = seed_user(&pool, "candidate-lock@example.test").await;
        let mut filter = sample_filter(user_id, "before rename");
        let product_id = seed_product(&pool, "candidate-lock-product").await;
        let event_id = seed_product_event(&pool, product_id).await;
        let evaluated = match_candidate(&filter);
        let mut tx = unit.begin().await?;
        let stored = SqlxSearchFilterRepositoryFactory.in_transaction(&mut tx).insert(&filter).await?;
        filter.rename(UserSearchFilterName::from("after rename"));
        filter.change_notifications(false);
        SqlxSearchFilterRepositoryFactory.in_transaction(&mut tx).update(&filter, stored.version).await?;
        tx.commit().await?;

        let mut final_tx = unit.begin().await?;
        let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(final_tx.connection()).await?;
        let candidates = SqlxActiveSearchFilterMatchCandidateReaderFactory.in_transaction(&mut final_tx)
            .find_active(&[evaluated]).await?;
        assert_eq!(1, candidates.len());
        assert_eq!(filter.name(), &candidates[0].search_filter_name);
        let deactivate = async {
            sqlx::query("UPDATE search_filters SET state = 'INACTIVE_BY_USER', version = version + 1 WHERE user_search_filter_id = $1")
                .bind(uuid::Uuid::try_from(filter.id().to_string())?).execute(&pool).await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        };
        tokio::pin!(deactivate);
        support::assert_blocked(&pool, blocker_pid, 1, deactivate.as_mut()).await?;
        assert_eq!(SearchFilterMatchPersistOutcome::Inserted,
            SqlxSearchFilterMatchWriterFactory.in_transaction(&mut final_tx)
                .insert_if_absent(&product_match(&filter, product_id, event_id)).await?);
        final_tx.commit().await?;
        tokio::time::timeout(Duration::from_secs(10), deactivate).await??;
        Ok(())
    }.await;
    assert!(result.is_ok(), "candidate lock through commit: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_persist_one_match_when_single_and_batch_completions_overlap() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let unit = SqlxUnitOfWork::new(pool.clone());
        let user_id = seed_user(&pool, "match-duplicate@example.test").await;
        let filter = sample_filter(user_id, "duplicate match");
        let product_id = seed_product(&pool, "duplicate-match-product").await;
        let event_id = seed_product_event(&pool, product_id).await;
        let matched = product_match(&filter, product_id, event_id);
        let mut setup = unit.begin().await?;
        SqlxSearchFilterRepositoryFactory
            .in_transaction(&mut setup)
            .insert(&filter)
            .await?;
        setup.commit().await?;
        let mut first = unit.begin().await?;
        let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(first.connection())
            .await?;
        assert_eq!(
            SearchFilterMatchPersistOutcome::Inserted,
            SqlxSearchFilterMatchWriterFactory
                .in_transaction(&mut first)
                .insert_if_absent(&matched)
                .await?
        );
        let duplicate = async {
            let mut tx = unit.begin().await?;
            let mut different_result = matched.clone();
            different_result.enhanced_match_reason = Some("later evaluation".into());
            let outcome = SqlxSearchFilterMatchWriterFactory
                .in_transaction(&mut tx)
                .insert_all_if_absent(&[different_result])
                .await?;
            tx.commit().await?;
            Ok::<_, Box<dyn std::error::Error>>(outcome)
        };
        tokio::pin!(duplicate);
        support::assert_blocked(&pool, blocker_pid, 1, duplicate.as_mut()).await?;
        first.commit().await?;
        let outcome = tokio::time::timeout(Duration::from_secs(10), duplicate).await??;
        assert_eq!((0, 1), (outcome.inserted, outcome.already_exists));
        let mut tx = unit.begin().await?;
        let stored = SqlxSearchFilterMatchRepositoryFactory
            .in_transaction(&mut tx)
            .find_by_filter_and_product(filter.id(), product_id)
            .await?;
        assert_eq!(
            Some(matched.clone()),
            stored.map(|stored| stored.product_match)
        );
        tx.commit().await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM search_filter_matches")
            .fetch_one(&pool)
            .await?;
        assert_eq!(1, count);
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "concurrent match completion: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_keep_exact_historical_match_source_after_unrelated_newer_product_event() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let unit = SqlxUnitOfWork::new(pool.clone());
        let user_id = seed_user(&pool, "historical-match@example.test").await;
        let filter = sample_filter(user_id, "historical match");
        let product_id = seed_product(&pool, "historical-match-product").await;
        let original = seed_product_event(&pool, product_id).await;
        let mut tx = unit.begin().await?;
        SqlxSearchFilterRepositoryFactory
            .in_transaction(&mut tx)
            .insert(&filter)
            .await?;
        SqlxSearchFilterMatchWriterFactory
            .in_transaction(&mut tx)
            .insert_if_absent(&product_match(&filter, product_id, original))
            .await?;
        tx.commit().await?;
        let newer = seed_product_event(&pool, product_id).await;
        let mut tx = unit.begin().await?;
        let source = SqlxSearchFilterMatchNotificationSourceReaderFactory
            .in_transaction(&mut tx)
            .find_source(user_id, filter.id(), product_id, original)
            .await?;
        assert_eq!(Some(original), source.map(|source| source.origin_event_id));
        assert!(
            SqlxSearchFilterMatchNotificationSourceReaderFactory
                .in_transaction(&mut tx)
                .find_source(user_id, filter.id(), product_id, newer)
                .await?
                .is_none()
        );
        assert_eq!(
            SearchFilterMatchPersistOutcome::AlreadyExists,
            SqlxSearchFilterMatchWriterFactory
                .in_transaction(&mut tx)
                .insert_if_absent(&product_match(&filter, product_id, newer))
                .await?
        );
        tx.commit().await?;
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "historical match source: {result:?}");
}

fn match_candidate(filter: &SearchFilter) -> SearchFilterMatchCandidate {
    SearchFilterMatchCandidate {
        user_id: filter.user_id(),
        search_filter_id: filter.id(),
        expected_search: filter.search().clone(),
        expected_embedding: filter.embedding().cloned(),
        price_match_valuation: None,
        enhanced_match_reason: None,
    }
}

fn product_match(
    filter: &SearchFilter,
    product_listing_id: ProductListingId,
    origin_event_id: EventId,
) -> SearchFilterProductListingMatch {
    SearchFilterProductListingMatch {
        user_id: filter.user_id(),
        user_search_filter_id: filter.id(),
        user_search_filter_name: Some(filter.name().clone()),
        product_listing_id,
        origin_event_id,
        price_match_valuation: None,
        enhanced_match_reason: None,
        feedback: None,
    }
}

async fn persist_candidate(
    pool: &sqlx::PgPool,
    candidate: SearchFilterMatchCandidate,
    product_listing_id: ProductListingId,
    origin_event_id: EventId,
) -> Result<Option<SearchFilterMatchPersistOutcome>, Box<dyn std::error::Error>> {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let active = SqlxActiveSearchFilterMatchCandidateReaderFactory
        .in_transaction(&mut tx)
        .find_active(&[candidate])
        .await?;
    let mut outcome = None;
    for candidate in active {
        outcome = Some(
            SqlxSearchFilterMatchWriterFactory
                .in_transaction(&mut tx)
                .insert_if_absent(&SearchFilterProductListingMatch {
                    user_id: candidate.user_id,
                    user_search_filter_id: candidate.search_filter_id,
                    user_search_filter_name: Some(candidate.search_filter_name),
                    product_listing_id,
                    origin_event_id,
                    price_match_valuation: candidate.price_match_valuation,
                    enhanced_match_reason: candidate.enhanced_match_reason,
                    feedback: None,
                })
                .await?,
        );
    }
    tx.commit().await?;
    Ok(outcome)
}

fn sample_filter(user_id: UserId, name: &str) -> SearchFilter {
    SearchFilter::create(NewSearchFilter {
        user_search_filter_id: UserSearchFilterId::new(),
        user_id,
        name: UserSearchFilterName::from(name),
        notifications: true,
        state: SearchFilterState::Active,
        search: ProductListingSearch::new(Language::En, Currency::Eur),
        embedding: None,
    })
}

async fn begin(unit: &SqlxUnitOfWork) -> platform_postgres::SqlxTransaction {
    unit.begin()
        .await
        .unwrap_or_else(|error| panic!("begin failed: {error:?}"))
}

async fn commit(tx: platform_postgres::SqlxTransaction) {
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit failed: {error:?}"));
}

async fn seed_user(pool: &sqlx::PgPool, email: &str) -> UserId {
    let id = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(uuid::Uuid::from(id))
    .bind(email)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed user failed: {error:?}"));
    id
}

async fn seed_product(pool: &sqlx::PgPool, source_listing_id: &str) -> ProductListingId {
    let product_listing_id = ProductListingId::new();
    let title_slug_id = ProductListingSlugId::raw("search-filter-match-product-a1b2c3")
        .unwrap_or_else(|error| panic!("valid fixture title slug: {error}"));
    let slug = source_listing_id;
    let listing_source_id = uuid::Uuid::new_v4();
    let event_id = uuid::Uuid::new_v4();
    let discovery_payload = serde_json::json!({
        "listingSourceId": listing_source_id,
        "sourceListingId": source_listing_id,
        "title": null,
        "description": null,
        "pricing": {
            "price": null,
            "priceEstimateMin": null,
            "priceEstimateMax": null
        },
        "availability": null,
        "url": "https://example.com/product",
        "imageCount": 0,
        "auction": { "start": null, "end": null }
    });
    let mut tx = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("seed tx failed: {error:?}"));
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), concat($3, ' operator')) RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $1, $2, $3, party_id FROM operator")
        .bind(listing_source_id).bind(format!("{slug}-source")).bind(format!("{slug} source")).execute(&mut *tx).await.unwrap_or_else(|error| panic!("seed source failed: {error:?}"));
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(event_id).bind(uuid::Uuid::from(product_listing_id)).bind(discovery_payload).execute(&mut *tx).await.unwrap_or_else(|error| panic!("seed event failed: {error:?}"));
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, availability, lifecycle, url) VALUES ($1, $2, $3, $3, $3, $4, $5, NULL, 'ACTIVE', 'https://example.com/product')")
        .bind(uuid::Uuid::from(product_listing_id)).bind(title_slug_id.as_ref()).bind(event_id).bind(listing_source_id).bind(slug)
        .execute(&mut *tx).await.unwrap_or_else(|error| panic!("seed product failed: {error:?}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("seed commit failed: {error:?}"));
    product_listing_id
}

async fn seed_fx_rate(pool: &sqlx::PgPool) -> FxRateId {
    let fx_rate_id = FxRateId::new();
    sqlx::query(
        "INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id) VALUES ($1, now(), 'fxratesapi', $2)",
    )
    .bind(uuid::Uuid::from(fx_rate_id))
    .bind(uuid::Uuid::new_v4().to_string())
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed FX rate failed: {error:?}"));
    fx_rate_id
}

async fn seed_product_event(pool: &sqlx::PgPool, product_listing_id: ProductListingId) -> EventId {
    let event_id = EventId::new();
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, '{\"availability\": {\"previous\": null, \"current\": \"AVAILABLE\"}}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed product event failed: {error:?}"));
    sqlx::query("UPDATE product_listings SET current_event_id = $1, availability = 'AVAILABLE', version = version + 1, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $2")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("advance product event fixture failed: {error:?}"));
    event_id
}
