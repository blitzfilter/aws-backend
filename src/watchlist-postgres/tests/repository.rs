use application::transaction::{Transaction, UnitOfWork};
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use user_core::user_id::UserId;
use watchlist_core::{WatchlistProductListing, WatchlistState};
use watchlist_postgres::{
    SqlxWatchlistQuotaReaderFactory, SqlxWatchlistReaderFactory, SqlxWatchlistRepositoryFactory,
};
use watchlist_service::ports::{
    WatchlistQuotaReader, WatchlistQuotaReaderFactory, WatchlistReader, WatchlistReaderFactory,
    WatchlistRepository, WatchlistRepositoryError, WatchlistRepositoryFactory,
    WatchlistStorageVersion,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_find_update_read_and_delete_watchlist_entry() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxWatchlistRepositoryFactory;
    let reader = SqlxWatchlistReaderFactory;
    let user_id = seed_user(&pool, "watchlist-postgres-user@example.com").await;
    let product_listing_id = seed_product(&pool, "watchlist-postgres-product").await;
    let mut entry = WatchlistProductListing::rehydrate(
        user_id,
        product_listing_id,
        true,
        WatchlistState::Active,
    );

    let mut tx = begin(&unit).await;
    let inserted = repo
        .in_transaction(&mut tx)
        .insert(&entry)
        .await
        .unwrap_or_else(|error| panic!("insert failed: {error:?}"));
    assert_eq!(WatchlistStorageVersion::INITIAL, inserted.version);
    commit(tx).await;

    let mut tx = begin(&unit).await;
    let loaded = repo
        .in_transaction(&mut tx)
        .find_by_user_and_product(user_id, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("find failed: {error:?}"))
        .unwrap_or_else(|| panic!("missing watchlist entry"));
    assert!(loaded.value.notifications());

    entry.change_notifications(false);
    let updated = repo
        .in_transaction(&mut tx)
        .update(&entry, loaded.version)
        .await
        .unwrap_or_else(|error| panic!("update failed: {error:?}"));
    assert_eq!(loaded.version.next(), updated.version);
    commit(tx).await;

    let mut tx = begin(&unit).await;
    let views = reader
        .in_transaction(&mut tx)
        .find_for_user(user_id)
        .await
        .unwrap_or_else(|error| panic!("read failed: {error:?}"));
    assert_eq!(1, views.len());
    assert!(!views[0].notifications);
    assert!(views[0].updated >= views[0].created);

    let user_ids = reader
        .in_transaction(&mut tx)
        .find_user_ids_for_product(product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("read users failed: {error:?}"));
    assert_eq!(vec![user_id], user_ids);
    commit(tx).await;

    let mut tx = begin(&unit).await;
    repo.in_transaction(&mut tx)
        .delete(user_id, product_listing_id, updated.version)
        .await
        .unwrap_or_else(|error| panic!("delete failed: {error:?}"));
    commit(tx).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_order_user_watchlist_entries_by_created_then_product_listing_id() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repository = SqlxWatchlistRepositoryFactory;
    let reader = SqlxWatchlistReaderFactory;
    let user_id = seed_user(&pool, "watchlist-postgres-order@example.com").await;
    let first_product_listing_id = seed_product(&pool, "watchlist-postgres-order-first").await;
    let second_product_listing_id = seed_product(&pool, "watchlist-postgres-order-second").await;
    let created = OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .unwrap_or_else(|error| panic!("failed to normalize timestamp: {error}"));

    let mut tx = begin(&unit).await;
    for product_listing_id in [first_product_listing_id, second_product_listing_id] {
        repository
            .in_transaction(&mut tx)
            .insert(&WatchlistProductListing::rehydrate(
                user_id,
                product_listing_id,
                true,
                WatchlistState::Active,
            ))
            .await
            .unwrap_or_else(|error| panic!("insert watchlist entry failed: {error:?}"));
    }
    commit(tx).await;

    sqlx::query("UPDATE product_listing_watchlist SET created = $1 WHERE user_id = $2")
        .bind(created)
        .bind(user_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to align watchlist timestamps: {error:?}"));

    let mut tx = begin(&unit).await;
    let product_listing_ids = reader
        .in_transaction(&mut tx)
        .find_for_user(user_id)
        .await
        .unwrap_or_else(|error| panic!("read watchlist entries failed: {error:?}"))
        .into_iter()
        .map(|entry| entry.product_listing_id)
        .collect::<Vec<_>>();
    commit(tx).await;

    let mut expected = [first_product_listing_id, second_product_listing_id];
    expected.sort();
    assert_eq!(product_listing_ids, expected);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_preserve_and_reset_current_interval_timestamps() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repository = SqlxWatchlistRepositoryFactory;
    let user_id = seed_user(&pool, "watchlist-postgres-intervals@example.com").await;
    let active_product_listing_id = seed_product(&pool, "watchlist-postgres-interval-active").await;
    let inactive_product_listing_id =
        seed_product(&pool, "watchlist-postgres-interval-inactive").await;
    let mut active = WatchlistProductListing::rehydrate(
        user_id,
        active_product_listing_id,
        true,
        WatchlistState::Active,
    );
    let inactive = WatchlistProductListing::rehydrate(
        user_id,
        inactive_product_listing_id,
        false,
        WatchlistState::InactiveByUser,
    );

    let mut tx = begin(&unit).await;
    repository
        .in_transaction(&mut tx)
        .insert(&active)
        .await
        .unwrap_or_else(|error| panic!("insert active entry failed: {error:?}"));
    repository
        .in_transaction(&mut tx)
        .insert(&inactive)
        .await
        .unwrap_or_else(|error| panic!("insert inactive entry failed: {error:?}"));
    commit(tx).await;

    let (initial_active_since, initial_email_since) =
        intervals(&pool, user_id, active_product_listing_id).await;
    assert!(initial_active_since.is_some());
    assert!(initial_email_since.is_some());

    let old = (OffsetDateTime::now_utc() - Duration::hours(1))
        .replace_nanosecond(0)
        .unwrap_or_else(|error| panic!("failed to normalize baseline timestamp: {error}"));
    sqlx::query(
        "UPDATE product_listing_watchlist SET active_since = $3, notifications_enabled_since = $3, created = $3, updated = $3 WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(user_id.into_uuid())
    .bind(active_product_listing_id.into_uuid())
    .bind(old)
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to set interval baseline: {error:?}"));

    let mut tx = begin(&unit).await;
    let loaded = repository
        .in_transaction(&mut tx)
        .find_by_user_and_product(user_id, active_product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("active lookup failed: {error:?}"))
        .unwrap_or_else(|| panic!("missing active entry"));
    repository
        .in_transaction(&mut tx)
        .update(&active, loaded.version)
        .await
        .unwrap_or_else(|error| panic!("active-to-active update failed: {error:?}"));
    commit(tx).await;
    assert_eq!(
        (Some(old), Some(old)),
        intervals(&pool, user_id, active_product_listing_id).await
    );

    active.change_state(WatchlistState::InactiveByUser);
    repository_update(&unit, &repository, &active).await;
    assert_eq!(
        (None, Some(old)),
        intervals(&pool, user_id, active_product_listing_id).await
    );

    active.change_state(WatchlistState::Active);
    repository_update(&unit, &repository, &active).await;
    let (reactivated_since, email_since) =
        intervals(&pool, user_id, active_product_listing_id).await;
    assert!(reactivated_since.is_some_and(|value| value > old));
    assert_eq!(Some(old), email_since);

    active.change_notifications(false);
    repository_update(&unit, &repository, &active).await;
    assert_eq!(
        (reactivated_since, None),
        intervals(&pool, user_id, active_product_listing_id).await
    );

    active.change_notifications(true);
    repository_update(&unit, &repository, &active).await;
    let (active_since, reenabled_email_since) =
        intervals(&pool, user_id, active_product_listing_id).await;
    assert_eq!(
        Some(reactivated_since.expect("active interval must remain")),
        active_since
    );
    assert!(reenabled_email_since.is_some_and(|value| value > old));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_count_only_active_watchlist_entries_in_transaction() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repository = SqlxWatchlistRepositoryFactory;
    let quotas = SqlxWatchlistQuotaReaderFactory;
    let user_id = seed_user(&pool, "watchlist-postgres-quota@example.com").await;
    let active_product_listing_id = seed_product(&pool, "watchlist-postgres-active-product").await;
    let inactive_product_listing_id =
        seed_product(&pool, "watchlist-postgres-inactive-product").await;
    let active = WatchlistProductListing::rehydrate(
        user_id,
        active_product_listing_id,
        true,
        WatchlistState::Active,
    );
    let inactive = WatchlistProductListing::rehydrate(
        user_id,
        inactive_product_listing_id,
        true,
        WatchlistState::InactiveByUser,
    );

    let mut tx = begin(&unit).await;
    repository
        .in_transaction(&mut tx)
        .insert(&active)
        .await
        .unwrap_or_else(|error| panic!("insert active entry failed: {error:?}"));
    repository
        .in_transaction(&mut tx)
        .insert(&inactive)
        .await
        .unwrap_or_else(|error| panic!("insert inactive entry failed: {error:?}"));
    let active_count = quotas
        .in_transaction(&mut tx)
        .count_active_for_user(user_id)
        .await
        .unwrap_or_else(|error| panic!("count active entries failed: {error:?}"));

    assert_eq!(1, active_count);
    commit(tx).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_keep_active_watch_count_and_intervals_when_product_listing_is_withdrawn() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repository = SqlxWatchlistRepositoryFactory;
    let quotas = SqlxWatchlistQuotaReaderFactory;
    let user_id = seed_user(&pool, "watchlist-postgres-withdrawn-count@example.com").await;
    let product_listing_id =
        seed_product(&pool, "watchlist-postgres-withdrawn-count-product").await;
    let entry = WatchlistProductListing::rehydrate(
        user_id,
        product_listing_id,
        true,
        WatchlistState::Active,
    );
    let interval_start = (OffsetDateTime::now_utc() - Duration::hours(1))
        .replace_nanosecond(0)
        .unwrap_or_else(|error| panic!("failed to normalize interval baseline: {error}"));

    let mut tx = begin(&unit).await;
    repository
        .in_transaction(&mut tx)
        .insert(&entry)
        .await
        .unwrap_or_else(|error| panic!("insert active entry failed: {error:?}"));
    commit(tx).await;
    sqlx::query(
        "UPDATE product_listing_watchlist SET active_since = $3, notifications_enabled_since = $3 WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(user_id.into_uuid())
    .bind(product_listing_id.into_uuid())
    .bind(interval_start)
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to set watchlist interval baseline: {error:?}"));

    sqlx::query(
        "UPDATE product_listings SET lifecycle = 'WITHDRAWN', availability = NULL WHERE product_listing_id = $1",
    )
    .bind(product_listing_id.into_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to withdraw product listing: {error:?}"));

    let mut tx = begin(&unit).await;
    let active_count = quotas
        .in_transaction(&mut tx)
        .count_active_for_user(user_id)
        .await
        .unwrap_or_else(|error| panic!("count active entries failed: {error:?}"));
    commit(tx).await;

    assert_eq!(1, active_count);
    assert_eq!(
        (
            "ACTIVE".to_owned(),
            Some(interval_start),
            Some(interval_start)
        ),
        watchlist_state_and_intervals(&pool, user_id, product_listing_id).await
    );

    sqlx::query("UPDATE product_listings SET lifecycle = 'ACTIVE' WHERE product_listing_id = $1")
        .bind(product_listing_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to restore product listing fixture: {error:?}"));

    assert_eq!(
        (
            "ACTIVE".to_owned(),
            Some(interval_start),
            Some(interval_start)
        ),
        watchlist_state_and_intervals(&pool, user_id, product_listing_id).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_watchlist_update_and_delete_concurrency_conflicts() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repository = SqlxWatchlistRepositoryFactory;
    let user_id = seed_user(&pool, "watchlist-postgres-conflict@example.com").await;
    let product_listing_id = seed_product(&pool, "watchlist-postgres-conflict-product").await;
    let entry = WatchlistProductListing::rehydrate(
        user_id,
        product_listing_id,
        true,
        WatchlistState::Active,
    );

    let mut tx = begin(&unit).await;
    repository
        .in_transaction(&mut tx)
        .insert(&entry)
        .await
        .unwrap_or_else(|error| panic!("insert failed: {error:?}"));
    let loaded = repository
        .in_transaction(&mut tx)
        .find_by_user_and_product(user_id, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("find failed: {error:?}"))
        .unwrap_or_else(|| panic!("missing watchlist entry"));

    let mut changed = entry.clone();
    changed.change_notifications(false);
    let updated = repository
        .in_transaction(&mut tx)
        .update(&changed, loaded.version)
        .await
        .unwrap_or_else(|error| panic!("update failed: {error:?}"));
    let stale_update = repository
        .in_transaction(&mut tx)
        .update(&entry, loaded.version)
        .await;
    let stale_delete = repository
        .in_transaction(&mut tx)
        .delete(user_id, product_listing_id, loaded.version)
        .await;

    assert!(matches!(
        stale_update,
        Err(watchlist_service::ports::WatchlistRepositoryError::ConcurrencyConflict)
    ));
    assert!(matches!(
        stale_delete,
        Err(watchlist_service::ports::WatchlistRepositoryError::ConcurrencyConflict)
    ));

    repository
        .in_transaction(&mut tx)
        .delete(user_id, product_listing_id, updated.version)
        .await
        .unwrap_or_else(|error| panic!("current delete failed: {error:?}"));
    commit(tx).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_stale_delete_and_keep_newer_watchlist_entry() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repository = SqlxWatchlistRepositoryFactory;
    let user_id = seed_user(&pool, "watchlist-postgres-stale-delete@example.com").await;
    let product_listing_id = seed_product(&pool, "watchlist-postgres-stale-delete-product").await;
    let entry = WatchlistProductListing::rehydrate(
        user_id,
        product_listing_id,
        true,
        WatchlistState::Active,
    );

    let mut insert_tx = begin(&unit).await;
    repository
        .in_transaction(&mut insert_tx)
        .insert(&entry)
        .await
        .unwrap_or_else(|error| panic!("insert failed: {error:?}"));
    commit(insert_tx).await;

    let mut delete_tx = begin(&unit).await;
    let stale = repository
        .in_transaction(&mut delete_tx)
        .find_by_user_and_product(user_id, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("delete lookup failed: {error:?}"))
        .unwrap_or_else(|| panic!("missing watchlist entry"));

    let mut update_tx = begin(&unit).await;
    let current = repository
        .in_transaction(&mut update_tx)
        .find_by_user_and_product(user_id, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("update lookup failed: {error:?}"))
        .unwrap_or_else(|| panic!("missing watchlist entry"));
    let mut changed = current.value;
    changed.change_notifications(false);
    repository
        .in_transaction(&mut update_tx)
        .update(&changed, current.version)
        .await
        .unwrap_or_else(|error| panic!("newer update failed: {error:?}"));
    commit(update_tx).await;

    let stale_delete = repository
        .in_transaction(&mut delete_tx)
        .delete(user_id, product_listing_id, stale.version)
        .await;
    assert!(matches!(
        stale_delete,
        Err(WatchlistRepositoryError::ConcurrencyConflict)
    ));
    drop(delete_tx);

    let persisted = persisted_fields(&pool, user_id, product_listing_id).await;
    assert!(!persisted.1);
    assert_eq!(2, persisted.5);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_stale_partial_updates_without_changing_persisted_fields() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repository = SqlxWatchlistRepositoryFactory;
    let user_id = seed_user(&pool, "watchlist-postgres-stale-fields@example.com").await;
    let product_listing_id = seed_product(&pool, "watchlist-postgres-stale-fields-product").await;
    let initial = WatchlistProductListing::rehydrate(
        user_id,
        product_listing_id,
        true,
        WatchlistState::Active,
    );

    let mut insert_tx = begin(&unit).await;
    repository
        .in_transaction(&mut insert_tx)
        .insert(&initial)
        .await
        .unwrap_or_else(|error| panic!("insert failed: {error:?}"));
    commit(insert_tx).await;

    let mut winner_tx = begin(&unit).await;
    let loaded_by_winner = repository
        .in_transaction(&mut winner_tx)
        .find_by_user_and_product(user_id, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("winner lookup failed: {error:?}"))
        .unwrap_or_else(|| panic!("missing watchlist entry"));
    let mut winner = loaded_by_winner.value.clone();
    winner.change_state(WatchlistState::InactiveByUser);

    let mut stale_tx = begin(&unit).await;
    let loaded_by_stale = repository
        .in_transaction(&mut stale_tx)
        .find_by_user_and_product(user_id, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("stale lookup failed: {error:?}"))
        .unwrap_or_else(|| panic!("missing watchlist entry"));
    let mut stale = loaded_by_stale.value;
    stale.change_notifications(false);

    repository
        .in_transaction(&mut winner_tx)
        .update(&winner, loaded_by_winner.version)
        .await
        .unwrap_or_else(|error| panic!("winner update failed: {error:?}"));
    commit(winner_tx).await;
    let expected = persisted_fields(&pool, user_id, product_listing_id).await;

    let stale_result = repository
        .in_transaction(&mut stale_tx)
        .update(&stale, loaded_by_stale.version)
        .await;
    assert!(matches!(
        stale_result,
        Err(WatchlistRepositoryError::ConcurrencyConflict)
    ));
    drop(stale_tx);

    assert_eq!(
        expected,
        persisted_fields(&pool, user_id, product_listing_id).await
    );
    assert_eq!("INACTIVE_BY_USER", expected.0);
    assert!(expected.1);
    assert_eq!(None, expected.2);
    assert!(expected.3.is_some());
    assert_eq!(2, expected.5);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_apply_retried_notifications_patch_after_state_patch_wins() {
    run_partial_patch_race(true).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_apply_retried_state_patch_after_notifications_patch_wins() {
    run_partial_patch_race(false).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_already_exists_when_watchlist_entry_exists() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repo = SqlxWatchlistRepositoryFactory;
    let user_id = seed_user(&pool, "watchlist-postgres-duplicate@example.com").await;
    let product_listing_id = seed_product(&pool, "watchlist-postgres-duplicate-product").await;
    let entry = WatchlistProductListing::rehydrate(
        user_id,
        product_listing_id,
        true,
        WatchlistState::Active,
    );

    let mut tx = begin(&unit).await;
    repo.in_transaction(&mut tx)
        .insert(&entry)
        .await
        .unwrap_or_else(|error| panic!("insert failed: {error:?}"));
    let second = repo.in_transaction(&mut tx).insert(&entry).await;

    assert!(matches!(
        second,
        Err(watchlist_service::ports::WatchlistRepositoryError::AlreadyExists)
    ));
}

async fn run_partial_patch_race(state_patch_wins: bool) {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let repository = SqlxWatchlistRepositoryFactory;
    let user_id = seed_user(
        &pool,
        if state_patch_wins {
            "watchlist-postgres-state-wins@example.com"
        } else {
            "watchlist-postgres-notifications-wins@example.com"
        },
    )
    .await;
    let product_listing_id = seed_product(
        &pool,
        if state_patch_wins {
            "watchlist-postgres-state-wins-product"
        } else {
            "watchlist-postgres-notifications-wins-product"
        },
    )
    .await;
    let initial = WatchlistProductListing::rehydrate(
        user_id,
        product_listing_id,
        true,
        WatchlistState::Active,
    );

    let mut insert_tx = begin(&unit).await;
    repository
        .in_transaction(&mut insert_tx)
        .insert(&initial)
        .await
        .unwrap_or_else(|error| panic!("insert failed: {error:?}"));
    commit(insert_tx).await;

    let mut state_tx = begin(&unit).await;
    let loaded_by_state = repository
        .in_transaction(&mut state_tx)
        .find_by_user_and_product(user_id, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("state lookup failed: {error:?}"))
        .unwrap_or_else(|| panic!("missing watchlist entry"));
    let mut state_entry = loaded_by_state.value.clone();
    state_entry.change_state(WatchlistState::InactiveByUser);

    let mut notifications_tx = begin(&unit).await;
    let loaded_by_notifications = repository
        .in_transaction(&mut notifications_tx)
        .find_by_user_and_product(user_id, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("notifications lookup failed: {error:?}"))
        .unwrap_or_else(|| panic!("missing watchlist entry"));
    let mut notifications_entry = loaded_by_notifications.value.clone();
    notifications_entry.change_notifications(false);

    if state_patch_wins {
        repository
            .in_transaction(&mut state_tx)
            .update(&state_entry, loaded_by_state.version)
            .await
            .unwrap_or_else(|error| panic!("state update failed: {error:?}"));
        commit(state_tx).await;
        let stale = repository
            .in_transaction(&mut notifications_tx)
            .update(&notifications_entry, loaded_by_notifications.version)
            .await;
        assert!(matches!(
            stale,
            Err(WatchlistRepositoryError::ConcurrencyConflict)
        ));
        drop(notifications_tx);

        let mut retry_tx = begin(&unit).await;
        let current = repository
            .in_transaction(&mut retry_tx)
            .find_by_user_and_product(user_id, product_listing_id)
            .await
            .unwrap_or_else(|error| panic!("retry lookup failed: {error:?}"))
            .unwrap_or_else(|| panic!("missing watchlist entry"));
        let mut retried = current.value;
        retried.change_notifications(false);
        repository
            .in_transaction(&mut retry_tx)
            .update(&retried, current.version)
            .await
            .unwrap_or_else(|error| panic!("retry update failed: {error:?}"));
        commit(retry_tx).await;
    } else {
        repository
            .in_transaction(&mut notifications_tx)
            .update(&notifications_entry, loaded_by_notifications.version)
            .await
            .unwrap_or_else(|error| panic!("notifications update failed: {error:?}"));
        commit(notifications_tx).await;
        let stale = repository
            .in_transaction(&mut state_tx)
            .update(&state_entry, loaded_by_state.version)
            .await;
        assert!(matches!(
            stale,
            Err(WatchlistRepositoryError::ConcurrencyConflict)
        ));
        drop(state_tx);

        let mut retry_tx = begin(&unit).await;
        let current = repository
            .in_transaction(&mut retry_tx)
            .find_by_user_and_product(user_id, product_listing_id)
            .await
            .unwrap_or_else(|error| panic!("retry lookup failed: {error:?}"))
            .unwrap_or_else(|| panic!("missing watchlist entry"));
        let mut retried = current.value;
        retried.change_state(WatchlistState::InactiveByUser);
        repository
            .in_transaction(&mut retry_tx)
            .update(&retried, current.version)
            .await
            .unwrap_or_else(|error| panic!("retry update failed: {error:?}"));
        commit(retry_tx).await;
    }

    let persisted = persisted_fields(&pool, user_id, product_listing_id).await;
    assert_eq!("INACTIVE_BY_USER", persisted.0);
    assert!(!persisted.1);
    assert_eq!(None, persisted.2);
    assert_eq!(None, persisted.3);
    assert_eq!(3, persisted.5);
}

async fn persisted_fields(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_listing_id: ProductListingId,
) -> (
    String,
    bool,
    Option<OffsetDateTime>,
    Option<OffsetDateTime>,
    OffsetDateTime,
    i64,
) {
    sqlx::query_as(
        "SELECT state, notifications, active_since, notifications_enabled_since, updated, version \
         FROM product_listing_watchlist WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(user_id.into_uuid())
    .bind(product_listing_id.into_uuid())
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("failed to read persisted watchlist fields: {error:?}"))
}

async fn repository_update(
    unit: &SqlxUnitOfWork,
    repository: &SqlxWatchlistRepositoryFactory,
    entry: &WatchlistProductListing,
) {
    let mut tx = begin(unit).await;
    let loaded = repository
        .in_transaction(&mut tx)
        .find_by_user_and_product(entry.user_id(), entry.product_listing_id())
        .await
        .unwrap_or_else(|error| panic!("watchlist lookup failed: {error:?}"))
        .unwrap_or_else(|| panic!("missing watchlist entry"));
    repository
        .in_transaction(&mut tx)
        .update(entry, loaded.version)
        .await
        .unwrap_or_else(|error| panic!("watchlist update failed: {error:?}"));
    commit(tx).await;
}

async fn watchlist_state_and_intervals(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_listing_id: ProductListingId,
) -> (String, Option<OffsetDateTime>, Option<OffsetDateTime>) {
    sqlx::query_as(
        "SELECT state, active_since, notifications_enabled_since FROM product_listing_watchlist WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(user_id.into_uuid())
    .bind(product_listing_id.into_uuid())
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("failed to read watchlist state and intervals: {error:?}"))
}

async fn intervals(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_listing_id: ProductListingId,
) -> (Option<OffsetDateTime>, Option<OffsetDateTime>) {
    sqlx::query_as(
        "SELECT active_since, notifications_enabled_since FROM product_listing_watchlist WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(user_id.into_uuid())
    .bind(product_listing_id.into_uuid())
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("failed to read watchlist intervals: {error:?}"))
}

async fn begin(unit: &SqlxUnitOfWork) -> SqlxTransaction {
    unit.begin()
        .await
        .unwrap_or_else(|error| panic!("begin failed: {error:?}"))
}

async fn commit(tx: SqlxTransaction) {
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit failed: {error:?}"));
}

async fn seed_user(pool: &sqlx::PgPool, email: &str) -> UserId {
    let id = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(id.into_uuid())
    .bind(email)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed user failed: {error:?}"));
    id
}

async fn seed_product(pool: &sqlx::PgPool, label: &str) -> ProductListingId {
    let product_listing_id = ProductListingId::new();
    let title_slug_id = ProductListingSlugId::from_title_and_suffix(
        label,
        &product_listing_id.into_uuid().simple().to_string()[..6],
    )
    .unwrap_or_else(|error| panic!("valid fixture title slug: {error}"));
    let source_listing_id = format!("{label}-source-listing-{product_listing_id}");
    let listing_source_id = uuid::Uuid::now_v7();
    let event_id = uuid::Uuid::now_v7();
    let mut tx = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("seed tx failed: {error:?}"));
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), concat($3, ' operator')) RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $1, $2, $3, party_id FROM operator")
        .bind(listing_source_id).bind(format!("{label}-source")).bind(format!("{label} source")).execute(&mut *tx).await.unwrap_or_else(|error| panic!("seed source failed: {error:?}"));
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(event_id)
        .bind(product_listing_id.into_uuid())
        .bind(serde_json::json!({
            "listingSourceId": listing_source_id.to_string(),
            "sourceListingId": source_listing_id.clone(),
            "title": null,
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": null,
            "url": "https://example.com/product",
            "imageCount": 0,
            "auction": {"start": null, "end": null}
        }))
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|error| panic!("seed event failed: {error:?}"));
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, availability, lifecycle, url) VALUES ($1, $2, $3, $3, $3, $4, $5, NULL, 'ACTIVE', 'https://example.com/product')")
        .bind(product_listing_id.into_uuid()).bind(title_slug_id.as_ref()).bind(event_id).bind(listing_source_id).bind(source_listing_id)
        .execute(&mut *tx).await.unwrap_or_else(|error| panic!("seed product failed: {error:?}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("seed commit failed: {error:?}"));
    product_listing_id
}
