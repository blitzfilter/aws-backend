use application::transaction::{Transaction, UnitOfWork};
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use product_listing_core::product_listing_id::ProductListingId;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use user_core::tier::UserTier;
use user_core::user_id::UserId;
use user_postgres::SqlxUserTierEntitlementsFactory;
use user_service::ports::{UserTierEntitlements, UserTierEntitlementsFactory};
use watchlist_core::WatchlistState;
use watchlist_postgres::SqlxWatchlistRepositoryFactory;
use watchlist_service::ports::{
    WatchlistRepository, WatchlistRepositoryError, WatchlistRepositoryFactory,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_another_transaction_lock_while_tier_entitlements_are_locked() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let entitlements = SqlxUserTierEntitlementsFactory::new();
    let user_id = seed_user(&pool, "tier-entitlements-lock@example.com", "FREE").await;

    let mut tx = begin(&unit).await;
    entitlements
        .in_transaction(&mut tx)
        .lock_user_tier(user_id)
        .await
        .unwrap_or_else(|error| panic!("failed to lock user tier: {error:?}"));

    let concurrent_lock = sqlx::query("SELECT 1 FROM users WHERE user_id = $1 FOR UPDATE NOWAIT")
        .bind(user_id.into_uuid())
        .execute(&pool)
        .await;

    assert!(concurrent_lock.is_err());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reconcile_legacy_newest_first_quotas() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let entitlements = SqlxUserTierEntitlementsFactory::new();
    let user_id = seed_user(&pool, "tier-entitlements-free@example.com", "FREE").await;
    let now = OffsetDateTime::now_utc();

    let old_filter = seed_search_filter(
        &pool,
        user_id,
        "old eligible",
        "ACTIVE",
        None,
        now - Duration::seconds(30),
    )
    .await;
    let middle_filter = seed_search_filter(
        &pool,
        user_id,
        "middle eligible",
        "ACTIVE",
        None,
        now - Duration::seconds(20),
    )
    .await;
    let newest_eligible_filter = seed_search_filter(
        &pool,
        user_id,
        "newest eligible",
        "ACTIVE",
        None,
        now - Duration::seconds(10),
    )
    .await;
    let newest_restricted_filter = seed_search_filter(
        &pool,
        user_id,
        "newest feature restricted",
        "ACTIVE",
        Some("gold ring"),
        now,
    )
    .await;
    let watchlist_ids = seed_watchlist_entries(&pool, user_id, 21, now).await;

    let mut tx = begin(&unit).await;
    let locked_tier = entitlements
        .in_transaction(&mut tx)
        .lock_user_tier(user_id)
        .await
        .unwrap_or_else(|error| panic!("failed to lock user tier: {error:?}"));
    assert_eq!(Some(UserTier::Free), locked_tier);
    entitlements
        .in_transaction(&mut tx)
        .reconcile_for_tier(user_id, UserTier::Free)
        .await
        .unwrap_or_else(|error| panic!("failed to reconcile tier entitlements: {error:?}"));
    commit(tx).await;

    assert_eq!(
        "INACTIVE_BY_RESTRICTED_PLAN",
        state_for_search_filter(&pool, old_filter).await
    );
    assert_eq!(
        "INACTIVE_BY_RESTRICTED_PLAN",
        state_for_search_filter(&pool, middle_filter).await
    );
    assert_eq!(
        "ACTIVE",
        state_for_search_filter(&pool, newest_eligible_filter).await
    );
    assert_eq!(
        "INACTIVE_BY_RESTRICTED_PLAN",
        state_for_search_filter(&pool, newest_restricted_filter).await
    );
    assert_eq!(
        1,
        count_state(&pool, ResourceTable::SearchFilters, user_id, "ACTIVE").await
    );
    assert_eq!(
        "INACTIVE_BY_RESTRICTED_PLAN",
        state_for_watchlist_entry(&pool, user_id, watchlist_ids[0]).await
    );
    assert_eq!(
        "ACTIVE",
        state_for_watchlist_entry(&pool, user_id, watchlist_ids[20]).await
    );
    assert_eq!(
        20,
        count_state(
            &pool,
            ResourceTable::ProductListingWatchlist,
            user_id,
            "ACTIVE",
        )
        .await
    );
    assert_eq!(
        2,
        version_for_watchlist_entry(&pool, user_id, watchlist_ids[0]).await
    );
    assert_eq!(
        1,
        version_for_watchlist_entry(&pool, user_id, watchlist_ids[20]).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_keep_user_deactivation_when_tier_reconciliation_runs_afterward() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let entitlements = SqlxUserTierEntitlementsFactory::new();
    let watchlist = SqlxWatchlistRepositoryFactory;
    let user_id = seed_user(&pool, "tier-reconciliation-user-wins@example.com", "FREE").await;
    let product_listing_ids =
        seed_watchlist_entries(&pool, user_id, 21, OffsetDateTime::now_utc()).await;
    let product_listing_id = ProductListingId::try_from(product_listing_ids[0])
        .unwrap_or_else(|error| panic!("invalid product-listing fixture: {error}"));

    let mut user_tx = begin(&unit).await;
    let loaded = watchlist
        .in_transaction(&mut user_tx)
        .find_by_user_and_product(user_id, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("user write lookup failed: {error:?}"))
        .unwrap_or_else(|| panic!("missing watchlist entry"));
    let mut deactivated = loaded.value;
    deactivated.change_state(WatchlistState::InactiveByUser);
    watchlist
        .in_transaction(&mut user_tx)
        .update(&deactivated, loaded.version)
        .await
        .unwrap_or_else(|error| panic!("user deactivation failed: {error:?}"));
    commit(user_tx).await;

    let mut reconciliation_tx = begin(&unit).await;
    entitlements
        .in_transaction(&mut reconciliation_tx)
        .lock_user_tier(user_id)
        .await
        .unwrap_or_else(|error| panic!("tier lock failed: {error:?}"));
    entitlements
        .in_transaction(&mut reconciliation_tx)
        .reconcile_for_tier(user_id, UserTier::Free)
        .await
        .unwrap_or_else(|error| panic!("reconciliation failed: {error:?}"));
    commit(reconciliation_tx).await;

    assert_eq!(
        "INACTIVE_BY_USER",
        state_for_watchlist_entry(&pool, user_id, product_listing_ids[0]).await
    );
    assert_eq!(
        2,
        version_for_watchlist_entry(&pool, user_id, product_listing_ids[0]).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_stale_user_update_after_tier_reconciliation_wins() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let entitlements = SqlxUserTierEntitlementsFactory::new();
    let watchlist = SqlxWatchlistRepositoryFactory;
    let user_id = seed_user(&pool, "tier-reconciliation-wins@example.com", "FREE").await;
    let product_listing_ids =
        seed_watchlist_entries(&pool, user_id, 21, OffsetDateTime::now_utc()).await;
    let product_listing_id = ProductListingId::try_from(product_listing_ids[0])
        .unwrap_or_else(|error| panic!("invalid product-listing fixture: {error}"));

    let mut user_tx = begin(&unit).await;
    let loaded = watchlist
        .in_transaction(&mut user_tx)
        .find_by_user_and_product(user_id, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("user write lookup failed: {error:?}"))
        .unwrap_or_else(|| panic!("missing watchlist entry"));
    let mut stale_user_entry = loaded.value;
    stale_user_entry.change_notifications(false);

    let mut reconciliation_tx = begin(&unit).await;
    entitlements
        .in_transaction(&mut reconciliation_tx)
        .lock_user_tier(user_id)
        .await
        .unwrap_or_else(|error| panic!("tier lock failed: {error:?}"));
    entitlements
        .in_transaction(&mut reconciliation_tx)
        .reconcile_for_tier(user_id, UserTier::Free)
        .await
        .unwrap_or_else(|error| panic!("reconciliation failed: {error:?}"));
    commit(reconciliation_tx).await;

    let stale_update = watchlist
        .in_transaction(&mut user_tx)
        .update(&stale_user_entry, loaded.version)
        .await;
    assert!(matches!(
        stale_update,
        Err(WatchlistRepositoryError::ConcurrencyConflict)
    ));
    drop(user_tx);

    assert_eq!(
        "INACTIVE_BY_RESTRICTED_PLAN",
        state_for_watchlist_entry(&pool, user_id, product_listing_ids[0]).await
    );
    assert!(notifications_for_watchlist_entry(&pool, user_id, product_listing_ids[0]).await);
    assert_eq!(
        2,
        version_for_watchlist_entry(&pool, user_id, product_listing_ids[0]).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_keep_legacy_free_tier_product_exclusions_and_lifecycle_filters_active() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let entitlements = SqlxUserTierEntitlementsFactory::new();
    let user_id = seed_user(&pool, "tier-entitlements-legacy-free@example.com", "FREE").await;
    let filter_id = uuid::Uuid::now_v7();

    sqlx::query(
        "INSERT INTO search_filters (user_search_filter_id, user_id, name, state, search, language, currency) VALUES ($1, $2, 'legacy-compatible', 'ACTIVE', $3, 'en', 'EUR')",
    )
    .bind(filter_id)
    .bind(user_id.into_uuid())
    .bind(serde_json::json!({
        "exclude_product_listing_id_query": [uuid::Uuid::now_v7()],
        "lifecycle_query": ["Deleted"],
    }))
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed legacy-compatible filter: {error:?}"));

    let mut tx = begin(&unit).await;
    entitlements
        .in_transaction(&mut tx)
        .lock_user_tier(user_id)
        .await
        .unwrap_or_else(|error| panic!("failed to lock user tier: {error:?}"));
    entitlements
        .in_transaction(&mut tx)
        .reconcile_for_tier(user_id, UserTier::Free)
        .await
        .unwrap_or_else(|error| panic!("failed to reconcile tier entitlements: {error:?}"));
    commit(tx).await;

    assert_eq!("ACTIVE", state_for_search_filter(&pool, filter_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reactivate_only_plan_restricted_resources_on_upgrade() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let entitlements = SqlxUserTierEntitlementsFactory::new();
    let user_id = seed_user(&pool, "tier-entitlements-ultimate@example.com", "ULTIMATE").await;
    let now = OffsetDateTime::now_utc();

    let plan_restricted_filter = seed_search_filter(
        &pool,
        user_id,
        "plan restricted",
        "INACTIVE_BY_RESTRICTED_PLAN",
        None,
        now,
    )
    .await;
    let user_inactive_filter = seed_search_filter(
        &pool,
        user_id,
        "user inactive",
        "INACTIVE_BY_USER",
        None,
        now - Duration::seconds(1),
    )
    .await;
    let watchlist_ids = seed_watchlist_entries(&pool, user_id, 2, now).await;
    sqlx::query(
        "UPDATE product_listing_watchlist SET state = 'INACTIVE_BY_RESTRICTED_PLAN', active_since = NULL WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(user_id.into_uuid())
    .bind(watchlist_ids[0])
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to plan-restrict watchlist entry: {error:?}"));
    sqlx::query(
        "UPDATE product_listing_watchlist SET state = 'INACTIVE_BY_USER', active_since = NULL WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(user_id.into_uuid())
    .bind(watchlist_ids[1])
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to inactivate watchlist entry by user: {error:?}"));
    let plan_restricted_version =
        version_for_watchlist_entry(&pool, user_id, watchlist_ids[0]).await;
    let user_inactive_version = version_for_watchlist_entry(&pool, user_id, watchlist_ids[1]).await;

    let mut tx = begin(&unit).await;
    entitlements
        .in_transaction(&mut tx)
        .lock_user_tier(user_id)
        .await
        .unwrap_or_else(|error| panic!("failed to lock user tier: {error:?}"));
    entitlements
        .in_transaction(&mut tx)
        .reconcile_for_tier(user_id, UserTier::Ultimate)
        .await
        .unwrap_or_else(|error| panic!("failed to reconcile tier entitlements: {error:?}"));
    commit(tx).await;

    assert_eq!(
        "ACTIVE",
        state_for_search_filter(&pool, plan_restricted_filter).await
    );
    assert_eq!(
        "INACTIVE_BY_USER",
        state_for_search_filter(&pool, user_inactive_filter).await
    );
    assert_eq!(
        "ACTIVE",
        state_for_watchlist_entry(&pool, user_id, watchlist_ids[0]).await
    );
    assert_eq!(
        "INACTIVE_BY_USER",
        state_for_watchlist_entry(&pool, user_id, watchlist_ids[1]).await
    );
    assert_eq!(
        plan_restricted_version + 1,
        version_for_watchlist_entry(&pool, user_id, watchlist_ids[0]).await
    );
    assert_eq!(
        user_inactive_version,
        version_for_watchlist_entry(&pool, user_id, watchlist_ids[1]).await
    );
}

async fn begin(unit: &SqlxUnitOfWork) -> SqlxTransaction {
    unit.begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin transaction: {error:?}"))
}

async fn commit(tx: SqlxTransaction) {
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("failed to commit transaction: {error:?}"));
}

async fn seed_user(pool: &sqlx::PgPool, email: &str, tier: &str) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, $3, 'USER')")
        .bind(user_id.into_uuid())
        .bind(email)
        .bind(tier)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed user: {error:?}"));
    user_id
}

async fn seed_search_filter(
    pool: &sqlx::PgPool,
    user_id: UserId,
    name: &str,
    state: &str,
    enhanced_search_description: Option<&str>,
    created: OffsetDateTime,
) -> uuid::Uuid {
    let filter_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO search_filters (user_search_filter_id, user_id, name, state, search, enhanced_search_description, language, currency, created, updated) VALUES ($1, $2, $3, $4, '{}'::jsonb, $5, 'en', 'EUR', $6, $6)",
    )
    .bind(filter_id)
    .bind(user_id.into_uuid())
    .bind(name)
    .bind(state)
    .bind(enhanced_search_description)
    .bind(created)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed search filter: {error:?}"));
    filter_id
}

async fn seed_watchlist_entries(
    pool: &sqlx::PgPool,
    user_id: UserId,
    count: usize,
    start: OffsetDateTime,
) -> Vec<uuid::Uuid> {
    let listing_source_id = uuid::Uuid::now_v7();
    let mut tx = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin product seed transaction: {error:?}"));
    sqlx::query(
        "WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), concat($3, ' operator')) RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $1, $2, $3, party_id FROM operator",
    )
    .bind(listing_source_id)
    .bind(format!("tier-entitlements-source-{listing_source_id}"))
    .bind("Tier entitlement test source")
    .execute(&mut *tx)
    .await
    .unwrap_or_else(|error| panic!("failed to seed source: {error:?}"));

    let mut product_listing_ids = Vec::with_capacity(count);
    for index in 0..count {
        let product_listing_id = uuid::Uuid::now_v7();
        let event_id = uuid::Uuid::now_v7();
        sqlx::query(
            "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())",
        )
        .bind(event_id)
        .bind(product_listing_id)
        .bind(serde_json::json!({
            "listingSourceId": listing_source_id.to_string(),
            "sourceListingId": format!("tier-entitlements-{index}"),
            "title": null,
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.com/product-listings",
            "imageCount": 0,
            "auction": {"start": null, "end": null}
        }))
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|error| panic!("failed to seed product-listing event: {error:?}"));
        sqlx::query(
            "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, availability, lifecycle, url) VALUES ($1, $2, $3, $3, $3, $4, $5, 'AVAILABLE', 'ACTIVE', 'https://example.com/product-listings')",
        )
        .bind(product_listing_id)
        .bind(product_listing_title_slug_id(
            "tier-entitlements-product",
            product_listing_id,
        ))
        .bind(event_id)
        .bind(listing_source_id)
        .bind(format!("tier-entitlements-{index}"))
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|error| panic!("failed to seed product listing: {error:?}"));
        product_listing_ids.push(product_listing_id);
    }
    tx.commit().await.unwrap_or_else(|error| {
        panic!("failed to commit product-listing seed transaction: {error:?}")
    });

    for (index, product_listing_id) in product_listing_ids.iter().enumerate() {
        let created = start - Duration::seconds((count - index) as i64);
        sqlx::query(
            "INSERT INTO product_listing_watchlist (user_id, product_listing_id, state, active_since, notifications_enabled_since, created, updated) VALUES ($1, $2, 'ACTIVE', $3, $3, $3, $3)",
        )
        .bind(user_id.into_uuid())
        .bind(product_listing_id)
        .bind(created)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed watchlist entry: {error:?}"));
    }

    product_listing_ids
}

async fn state_for_search_filter(pool: &sqlx::PgPool, filter_id: uuid::Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM search_filters WHERE user_search_filter_id = $1")
        .bind(filter_id)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to read search filter state: {error:?}"))
}

async fn state_for_watchlist_entry(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_listing_id: uuid::Uuid,
) -> String {
    sqlx::query_scalar("SELECT state FROM product_listing_watchlist WHERE user_id = $1 AND product_listing_id = $2")
        .bind(user_id.into_uuid())
        .bind(product_listing_id)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to read watchlist state: {error:?}"))
}

async fn notifications_for_watchlist_entry(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_listing_id: uuid::Uuid,
) -> bool {
    sqlx::query_scalar(
        "SELECT notifications FROM product_listing_watchlist WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(user_id.into_uuid())
    .bind(product_listing_id)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("failed to read watchlist notifications: {error:?}"))
}

async fn version_for_watchlist_entry(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_listing_id: uuid::Uuid,
) -> i64 {
    sqlx::query_scalar(
        "SELECT version FROM product_listing_watchlist WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(user_id.into_uuid())
    .bind(product_listing_id)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("failed to read watchlist version: {error:?}"))
}

enum ResourceTable {
    SearchFilters,
    ProductListingWatchlist,
}

async fn count_state(
    pool: &sqlx::PgPool,
    table: ResourceTable,
    user_id: UserId,
    state: &str,
) -> i64 {
    let sql = match table {
        ResourceTable::SearchFilters => {
            "SELECT count(*) FROM search_filters WHERE user_id = $1 AND state = $2"
        }
        ResourceTable::ProductListingWatchlist => {
            "SELECT count(*) FROM product_listing_watchlist WHERE user_id = $1 AND state = $2"
        }
    };
    sqlx::query_scalar(sql)
        .bind(user_id.into_uuid())
        .bind(state)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to count resource state: {error:?}"))
}

fn product_listing_title_slug_id(prefix: &str, product_listing_id: uuid::Uuid) -> String {
    let uuid = product_listing_id.simple().to_string();
    format!("{prefix}-{}", &uuid[uuid.len() - 6..])
}
