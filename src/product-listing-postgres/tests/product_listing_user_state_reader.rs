use domain_primitives::event_id::EventId;
use notification_core::notification_id::NotificationId;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_postgres::SqlxProductListingUserStateReader;
use product_listing_service::ports::{
    ProductListingUserStateLookup, ProductListingUserStateReadError, ProductListingUserStateReader,
};
use product_listing_service::user_state::ProductListingUserState;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use user_core::user_id::UserId;

use std::collections::HashMap;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_complete_state_with_safe_and_unsafe_content_and_free_tier_limit() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "FREE", false).await;
    let first_filter_id = deterministic_filter_id(1);
    let later_filter_id = deterministic_filter_id(2);
    let month_start = OffsetDateTime::UNIX_EPOCH + Duration::days(31);
    let unsafe_product = seed_product(&pool).await;
    let safe_product = seed_product(&pool).await;
    set_product_images(
        &pool,
        unsafe_product,
        serde_json::json!([{"url": "https://example.test/unsafe.jpg"}]),
    )
    .await;

    insert_search_filter(&pool, user_id, first_filter_id, "Early filter").await;
    insert_search_filter(&pool, user_id, later_filter_id, "Later filter").await;

    for hour in 0_i64..10 {
        let product = seed_product(&pool).await;
        insert_search_filter_match(
            &pool,
            user_id,
            first_filter_id,
            product,
            "Early filter",
            None,
            None,
            month_start + Duration::hours(hour),
        )
        .await;
    }

    insert_watchlist(&pool, user_id, unsafe_product, false).await;
    insert_search_filter_match(
        &pool,
        user_id,
        first_filter_id,
        unsafe_product,
        "Early filter",
        Some("Matches the early filter."),
        Some(true),
        month_start + Duration::hours(10),
    )
    .await;
    insert_search_filter_match(
        &pool,
        user_id,
        later_filter_id,
        unsafe_product,
        "Later filter",
        Some("Must not be selected."),
        Some(false),
        month_start + Duration::hours(10),
    )
    .await;

    let states = find_for_user(
        &pool,
        ProductListingUserStateLookup {
            user_id,
            product_listing_ids: vec![unsafe_product, safe_product],
        },
    )
    .await;

    let unsafe_state = state(&states, unsafe_product);
    assert!(unsafe_state.watchlist.watching);
    assert!(!unsafe_state.watchlist.notifications);
    assert!(
        !unsafe_state
            .content_visibility
            .show_unassessed_or_sensitive_content
    );
    assert!(unsafe_state.notification.unseen_notification_ids.is_empty());
    assert!(unsafe_state.search_filter.matched);
    assert!(unsafe_state.search_filter.hidden);
    assert_eq!(
        unsafe_state.search_filter.user_search_filter_id,
        Some(first_filter_id)
    );
    assert_eq!(
        unsafe_state
            .search_filter
            .user_search_filter_name
            .as_ref()
            .map(AsRef::as_ref),
        Some("Early filter")
    );
    assert_eq!(
        unsafe_state
            .search_filter
            .match_reason
            .as_ref()
            .map(AsRef::as_ref),
        Some("Matches the early filter.")
    );
    assert_eq!(unsafe_state.search_filter.match_feedback, Some(true));

    let safe_state = state(&states, safe_product);
    assert!(!safe_state.watchlist.watching);
    assert!(!safe_state.watchlist.notifications);
    assert!(
        !safe_state
            .content_visibility
            .show_unassessed_or_sensitive_content
    );
    assert!(!safe_state.search_filter.matched);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_default_state_when_user_has_no_watchlist_or_matches() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "FREE", false).await;
    let product_listing_id = seed_product(&pool).await;
    set_product_images(
        &pool,
        product_listing_id,
        serde_json::json!([{"url": "https://example.test/unsafe.jpg"}]),
    )
    .await;

    let states = find_for_user(
        &pool,
        ProductListingUserStateLookup {
            user_id,
            product_listing_ids: vec![product_listing_id],
        },
    )
    .await;

    assert_eq!(
        state(&states, product_listing_id),
        &ProductListingUserState::default()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_unknown_user_instead_of_defaulting_state() {
    let pool = get_postgres_client().await;

    let result = find_for_user_result(
        &pool,
        ProductListingUserStateLookup {
            user_id: UserId::new(),
            product_listing_ids: vec![ProductListingId::new()],
        },
    )
    .await;

    assert!(matches!(
        result,
        Err(ProductListingUserStateReadError::InvalidReadModel { .. })
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_all_unseen_notification_ids_newest_first_without_cross_product_leakage() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "PRO", false).await;
    let product_listing_id = seed_product(&pool).await;
    let other_product_listing_id = seed_product(&pool).await;
    let watchlist_notification_id = NotificationId::new();
    let filter_notification_id = NotificationId::new();
    let seen_notification_id = NotificationId::new();
    let other_notification_id = NotificationId::new();
    let filter_id = UserSearchFilterId::new();

    insert_watchlist_notification(
        &pool,
        user_id,
        product_listing_id,
        watchlist_notification_id,
        OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
        false,
    )
    .await;
    insert_search_filter_notification(
        &pool,
        user_id,
        product_listing_id,
        filter_id,
        filter_notification_id,
        OffsetDateTime::UNIX_EPOCH + Duration::seconds(2),
        false,
    )
    .await;
    insert_watchlist_notification(
        &pool,
        user_id,
        product_listing_id,
        seen_notification_id,
        OffsetDateTime::UNIX_EPOCH + Duration::seconds(3),
        true,
    )
    .await;
    insert_watchlist_notification(
        &pool,
        user_id,
        other_product_listing_id,
        other_notification_id,
        OffsetDateTime::UNIX_EPOCH + Duration::seconds(4),
        false,
    )
    .await;

    let states = find_for_user(
        &pool,
        ProductListingUserStateLookup {
            user_id,
            product_listing_ids: vec![product_listing_id, other_product_listing_id],
        },
    )
    .await;

    assert_eq!(
        vec![filter_notification_id, watchlist_notification_id],
        state(&states, product_listing_id)
            .notification
            .unseen_notification_ids
    );
    assert_eq!(
        vec![other_notification_id],
        state(&states, other_product_listing_id)
            .notification
            .unseen_notification_ids
    );

    let update = sqlx::query(
        "UPDATE notifications SET seen = true WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(user_id.into_uuid())
    .bind(product_listing_id.into_uuid())
    .execute(&pool)
    .await;
    if let Err(error) = update {
        panic!("failed to mark product notifications seen: {error}");
    }

    let states = find_for_user(
        &pool,
        ProductListingUserStateLookup {
            user_id,
            product_listing_ids: vec![product_listing_id],
        },
    )
    .await;
    assert!(
        state(&states, product_listing_id)
            .notification
            .unseen_notification_ids
            .is_empty()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_user_state_for_many_products_in_one_batched_lookup() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "PRO", false).await;
    let watched_product = seed_product(&pool).await;
    let safe_product = seed_product(&pool).await;
    let unsafe_product = seed_product(&pool).await;
    insert_watchlist(&pool, user_id, watched_product, true).await;

    set_product_images(
        &pool,
        unsafe_product,
        serde_json::json!([{"url": "https://example.test/unsafe.jpg"}]),
    )
    .await;

    let states = find_for_user(
        &pool,
        ProductListingUserStateLookup {
            user_id,
            product_listing_ids: vec![watched_product, safe_product, unsafe_product],
        },
    )
    .await;

    assert_eq!(states.len(), 3);
    assert!(state(&states, watched_product).watchlist.watching);
    assert!(state(&states, watched_product).watchlist.notifications);
}

async fn find_for_user(
    pool: &sqlx::PgPool,
    lookup: ProductListingUserStateLookup,
) -> HashMap<ProductListingId, ProductListingUserState> {
    match find_for_user_result(pool, lookup).await {
        Ok(states) => states,
        Err(error) => panic!("failed to read product user state: {error}"),
    }
}

async fn find_for_user_result(
    pool: &sqlx::PgPool,
    lookup: ProductListingUserStateLookup,
) -> Result<HashMap<ProductListingId, ProductListingUserState>, ProductListingUserStateReadError> {
    SqlxProductListingUserStateReader::new(pool.clone())
        .find_for_user(&lookup)
        .await
}

fn state(
    states: &HashMap<ProductListingId, ProductListingUserState>,
    product_listing_id: ProductListingId,
) -> &ProductListingUserState {
    match states.get(&product_listing_id) {
        Some(state) => state,
        None => panic!("missing user state for product {product_listing_id}"),
    }
}

async fn seed_user(
    pool: &sqlx::PgPool,
    tier: &str,
    show_unassessed_or_sensitive_content: bool,
) -> UserId {
    let user_id = UserId::new();
    let result = sqlx::query(
        r#"
        INSERT INTO users (user_id, email, show_unassessed_or_sensitive_content, tier, role)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(user_id.into_uuid())
    .bind(format!("{user_id}@example.test"))
    .bind(show_unassessed_or_sensitive_content)
    .bind(tier)
    .bind("USER")
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed user: {error}");
    }

    user_id
}

async fn seed_product(pool: &sqlx::PgPool) -> ProductListingId {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = uuid::Uuid::now_v7();
    let raw_product_listing_id = product_listing_id.into_uuid();
    let mut transaction = match pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => panic!("failed to begin product seed transaction: {error}"),
    };
    let source_label = format!("product-user-state-{listing_source_id}");

    let party_result =
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(party_id)
            .bind(format!("{source_label}-party"))
            .bind(format!("{source_label} party"))
            .execute(&mut *transaction)
            .await;
    if let Err(error) = party_result {
        panic!("failed to seed product party: {error}");
    }

    let source_result = sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(listing_source_id)
    .bind(&source_label)
    .bind(&source_label)
    .bind(party_id)
    .execute(&mut *transaction)
    .await;
    if let Err(error) = source_result {
        panic!("failed to seed product listing source: {error}");
    }

    let product_result = sqlx::query(
        r#"
        INSERT INTO product_listings (
            product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id,
            availability, lifecycle, url
        ) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(raw_product_listing_id)
    .bind(format!(
        "product-user-state-{}",
        &raw_product_listing_id.simple().to_string()[26..]
    ))
    .bind(event_id.into_uuid())
    .bind(listing_source_id)
    .bind(raw_product_listing_id.to_string())
    .bind("AVAILABLE")
    .bind("ACTIVE")
    .bind("https://example.test/product")
        .execute(&mut *transaction)
    .await;
    if let Err(error) = product_result {
        panic!("failed to seed product: {error}");
    }

    let event_result = sqlx::query(
        r#"
        INSERT INTO product_listing_events (
            event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time
        ) VALUES ($1, $2, $3, $4, 1, $5, $6)
        "#,
    )
    .bind(event_id.into_uuid())
    .bind(raw_product_listing_id)
    .bind("PRODUCT_LISTING_DISCOVERED")
    .bind("DOMAIN")
    .bind(serde_json::json!({
        "listingSourceId": listing_source_id.to_string(),
        "sourceListingId": raw_product_listing_id.to_string(),
        "title": null,
        "description": null,
        "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
        "availability": "AVAILABLE",
        "url": "https://example.test/product",
        "imageCount": 0,
        "auction": null
    }))
    .bind(OffsetDateTime::UNIX_EPOCH)
    .execute(&mut *transaction)
    .await;
    if let Err(error) = event_result {
        panic!("failed to seed product event: {error}");
    }

    if let Err(error) = transaction.commit().await {
        panic!("failed to commit product seed transaction: {error}");
    }

    product_listing_id
}

async fn set_product_images(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    images: serde_json::Value,
) {
    let result = sqlx::query(
        "UPDATE product_listings SET product_images = $1 WHERE product_listing_id = $2",
    )
    .bind(images)
    .bind(product_listing_id.into_uuid())
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to set product images: {error}");
    }
}

async fn insert_watchlist_notification(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_listing_id: ProductListingId,
    notification_id: NotificationId,
    created: OffsetDateTime,
    seen: bool,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO notifications (
            notification_id, user_id, kind, origin_event_id, product_listing_id, payload, seen, created
        ) VALUES ($1, $2, 'WATCHLIST_AVAILABILITY_CHANGED', $3, $4, $5, $6, $7)
        "#,
    )
    .bind(notification_id.into_uuid())
    .bind(user_id.into_uuid())
    .bind(uuid::Uuid::now_v7())
    .bind(product_listing_id.into_uuid())
    .bind(serde_json::json!({}))
    .bind(seen)
    .bind(created)
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed watchlist notification: {error}");
    }
}

async fn insert_search_filter_notification(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_listing_id: ProductListingId,
    filter_id: UserSearchFilterId,
    notification_id: NotificationId,
    created: OffsetDateTime,
    seen: bool,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO notifications (
            notification_id, user_id, kind, origin_event_id, product_listing_id,
            user_search_filter_id, payload, seen, created
        ) VALUES ($1, $2, 'SEARCH_FILTER_MATCH', $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(notification_id.into_uuid())
    .bind(user_id.into_uuid())
    .bind(uuid::Uuid::now_v7())
    .bind(product_listing_id.into_uuid())
    .bind(filter_id.into_uuid())
    .bind(serde_json::json!({}))
    .bind(seen)
    .bind(created)
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed search-filter notification: {error}");
    }
}

async fn insert_watchlist(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_listing_id: ProductListingId,
    notifications: bool,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO product_listing_watchlist (user_id, product_listing_id, notifications, state, active_since, notifications_enabled_since)
        VALUES ($1, $2, $3, $4, CASE WHEN $4 = 'ACTIVE' THEN now() ELSE NULL END, CASE WHEN $3 THEN now() ELSE NULL END)
        "#,
    )
    .bind(user_id.into_uuid())
    .bind(product_listing_id.into_uuid())
    .bind(notifications)
    .bind("ACTIVE")
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed product watchlist: {error}");
    }
}

async fn insert_search_filter(
    pool: &sqlx::PgPool,
    user_id: UserId,
    filter_id: UserSearchFilterId,
    name: &str,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO search_filters (
            user_search_filter_id, user_id, name, notifications, state, search, language, currency
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(filter_id.into_uuid())
    .bind(user_id.into_uuid())
    .bind(name)
    .bind(true)
    .bind("ACTIVE")
    .bind(serde_json::json!({}))
    .bind("en")
    .bind("EUR")
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed search filter: {error}");
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_search_filter_match(
    pool: &sqlx::PgPool,
    user_id: UserId,
    filter_id: UserSearchFilterId,
    product_listing_id: ProductListingId,
    name: &str,
    reason: Option<&str>,
    feedback: Option<bool>,
    created: OffsetDateTime,
) {
    let origin_event_id = event_id_for_product(pool, product_listing_id).await;
    let result = sqlx::query(
        r#"
        INSERT INTO search_filter_matches (
            user_id, user_search_filter_id, product_listing_id, origin_event_id,
            user_search_filter_name, enhanced_match_reason, feedback, created
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(user_id.into_uuid())
    .bind(filter_id.into_uuid())
    .bind(product_listing_id.into_uuid())
    .bind(origin_event_id.into_uuid())
    .bind(name)
    .bind(reason)
    .bind(feedback)
    .bind(created)
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed search filter match: {error}");
    }
}

async fn event_id_for_product(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> EventId {
    let result = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT current_event_id FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(product_listing_id.into_uuid())
    .fetch_one(pool)
    .await;

    match result {
        Ok(event_id) => EventId::try_from(event_id)
            .unwrap_or_else(|error| panic!("invalid persisted ProductListing event ID: {error}")),
        Err(error) => panic!("failed to read product event ID: {error}"),
    }
}

fn deterministic_filter_id(sequence: u16) -> UserSearchFilterId {
    let value = format!("01900000-0000-7000-8000-{sequence:012x}");
    let uuid = value
        .parse::<uuid::Uuid>()
        .unwrap_or_else(|error| panic!("valid deterministic UUIDv7 fixture: {error}"));
    UserSearchFilterId::try_from(uuid)
        .unwrap_or_else(|error| panic!("valid deterministic search-filter ID: {error}"))
}
