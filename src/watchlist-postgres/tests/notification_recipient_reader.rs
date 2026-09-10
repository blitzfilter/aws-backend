use std::collections::HashMap;

use application::transaction::{Transaction, UnitOfWork};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_service::ports::{
    WatchlistNotificationRecipientReader, WatchlistNotificationRecipientReaderFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::Duration;
use user_core::user_id::UserId;
use watchlist_postgres::SqlxWatchlistNotificationRecipientReaderFactory;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_select_watchlist_recipients_using_current_intervals_at_event_time() {
    let pool = get_postgres_client().await;
    let product_listing_id = seed_product(&pool).await;
    let event_time = time::OffsetDateTime::now_utc();
    let before = event_time - Duration::hours(1);
    let after = event_time + Duration::hours(1);
    let users = [
        ("before-email", true, "ACTIVE", Some(before), Some(before)),
        (
            "at-email",
            true,
            "ACTIVE",
            Some(event_time),
            Some(event_time),
        ),
        ("late-active", true, "ACTIVE", Some(after), Some(before)),
        ("inactive", true, "INACTIVE_BY_USER", None, Some(before)),
        ("in-app-only", false, "ACTIVE", Some(before), None),
        ("late-email", true, "ACTIVE", Some(before), Some(after)),
        ("reactivated", true, "ACTIVE", Some(after), Some(after)),
        ("email-reenabled", true, "ACTIVE", Some(before), Some(after)),
    ];

    let mut expected = HashMap::new();
    for (label, notifications, state, active_since, email_since) in users {
        let user_id = seed_user(&pool, label).await;
        expected.insert(label, user_id);
        seed_watchlist(
            &pool,
            user_id,
            product_listing_id,
            notifications,
            state,
            (active_since, email_since, before),
        )
        .await;
    }

    let unit = SqlxUnitOfWork::new(pool);
    let mut tx = unit
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin failed: {error:?}"));
    let recipients = SqlxWatchlistNotificationRecipientReaderFactory
        .in_transaction(&mut tx)
        .find_eligible_for_product_at(product_listing_id, event_time)
        .await
        .unwrap_or_else(|error| panic!("recipient read failed: {error:?}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit failed: {error:?}"));

    assert_eq!(5, recipients.len());
    let recipients_by_user = recipients
        .into_iter()
        .map(|recipient| (recipient.user_id, recipient.external_delivery_requested))
        .collect::<HashMap<_, _>>();
    assert_eq!(
        Some(&true),
        expected
            .get("before-email")
            .and_then(|id| recipients_by_user.get(id))
    );
    assert_eq!(
        Some(&true),
        expected
            .get("at-email")
            .and_then(|id| recipients_by_user.get(id))
    );
    assert_eq!(
        Some(&false),
        expected
            .get("in-app-only")
            .and_then(|id| recipients_by_user.get(id))
    );
    assert_eq!(
        Some(&false),
        expected
            .get("late-email")
            .and_then(|id| recipients_by_user.get(id))
    );
    assert_eq!(
        Some(&false),
        expected
            .get("email-reenabled")
            .and_then(|id| recipients_by_user.get(id))
    );
    assert!(
        !expected
            .get("reactivated")
            .is_some_and(|id| recipients_by_user.contains_key(id))
    );
    assert!(
        !expected
            .get("late-active")
            .is_some_and(|id| recipients_by_user.contains_key(id))
    );
    assert!(
        !expected
            .get("inactive")
            .is_some_and(|id| recipients_by_user.contains_key(id))
    );
}

async fn seed_user(pool: &sqlx::PgPool, label: &str) -> UserId {
    let user_id = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(user_id.into_uuid())
    .bind(format!(
        "watchlist-recipient-{label}-{user_id}@example.test"
    ))
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed user failed: {error:?}"));
    user_id
}

async fn seed_product(pool: &sqlx::PgPool) -> ProductListingId {
    let product_listing_id = ProductListingId::new();
    let product_uuid = product_listing_id.into_uuid();
    let title_slug_id = ProductListingSlugId::from_title_and_suffix(
        "recipient product",
        &product_uuid.simple().to_string()[..6],
    )
    .unwrap_or_else(|error| panic!("valid fixture title slug: {error}"));
    let event_id = uuid::Uuid::now_v7();
    let listing_source_id = uuid::Uuid::now_v7();
    let mut tx = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("seed product transaction failed: {error:?}"));
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $1, $2, 'Recipient source', party_id FROM operator")
        .bind(listing_source_id)
        .bind(format!("recipient-source-{listing_source_id}"))
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|error| panic!("seed source failed: {error:?}"));
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, availability, lifecycle, url) VALUES ($1, $2, $3, $3, $3, $4, $5, NULL, 'ACTIVE', 'https://example.test/product')")
        .bind(product_uuid)
        .bind(title_slug_id.as_ref())
        .bind(event_id)
        .bind(listing_source_id)
        .bind(product_uuid.to_string())

        .execute(&mut *tx)
        .await
        .unwrap_or_else(|error| panic!("seed product failed: {error:?}"));
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(event_id)
        .bind(product_uuid)
        .bind(serde_json::json!({
            "listingSourceId": listing_source_id.to_string(),
            "sourceListingId": product_uuid.to_string(),
            "title": null,
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": null,
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": {"start": null, "end": null}
        }))
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|error| panic!("seed product event failed: {error:?}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("seed product commit failed: {error:?}"));
    product_listing_id
}

async fn seed_watchlist(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_listing_id: ProductListingId,
    notifications: bool,
    state: &str,
    timestamps: (
        Option<time::OffsetDateTime>,
        Option<time::OffsetDateTime>,
        time::OffsetDateTime,
    ),
) {
    let (active_since, email_since, updated) = timestamps;
    sqlx::query("INSERT INTO product_listing_watchlist (user_id, product_listing_id, notifications, state, active_since, notifications_enabled_since, created, updated) VALUES ($1, $2, $3, $4, $5, $6, $7, $7)")
        .bind(user_id.into_uuid())
        .bind(product_listing_id.into_uuid())
        .bind(notifications)
        .bind(state)
        .bind(active_since)
        .bind(email_since)
        .bind(updated)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed watchlist failed: {error:?}"));
}
