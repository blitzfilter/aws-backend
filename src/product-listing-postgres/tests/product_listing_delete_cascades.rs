use serde_json::json;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_cascade_product_listing_owned_rows_and_retain_notification_snapshot() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let seeded = seed_product_listing_owned_rows(&pool).await?;

        let mut delete_transaction = pool.begin().await?;
        sqlx::query("DELETE FROM product_listings WHERE product_listing_id = $1")
            .bind(seeded.product_listing_id)
            .execute(&mut *delete_transaction)
            .await?;
        delete_transaction.commit().await?;

        for (table, query) in [
            (
                "product_listing_translations",
                "SELECT count(*) FROM product_listing_translations WHERE product_listing_id = $1",
            ),
            (
                "product_listing_events",
                "SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1",
            ),
            (
                "product_listing_content_assessments",
                "SELECT count(*) FROM product_listing_content_assessments WHERE product_listing_id = $1",
            ),
            (
                "product_listing_watchlist",
                "SELECT count(*) FROM product_listing_watchlist WHERE product_listing_id = $1",
            ),
            (
                "search_filter_matches",
                "SELECT count(*) FROM search_filter_matches WHERE product_listing_id = $1",
            ),
        ] {
            assert_no_product_listing_rows(&pool, table, query, seeded.product_listing_id).await?;
        }

        let notification: (Uuid, serde_json::Value) = sqlx::query_as(
            "SELECT product_listing_id, payload FROM notifications WHERE notification_id = $1",
        )
        .bind(seeded.notification_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(seeded.product_listing_id, notification.0);
        assert_eq!(seeded.notification_payload, notification.1);

        let delivery: (Uuid, String, String, String) = sqlx::query_as(
            "SELECT notification_id, channel, target_key, status FROM notification_deliveries WHERE notification_delivery_id = $1",
        )
        .bind(seeded.notification_delivery_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(seeded.notification_id, delivery.0);
        assert_eq!("EMAIL", delivery.1);
        assert_eq!("user@delete-cascade.test", delivery.2);
        assert_eq!("PENDING", delivery.3);

        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "ProductListing physical-delete cascade contract failed: {result:?}"
    );
}

async fn assert_no_product_listing_rows(
    pool: &sqlx::PgPool,
    table: &str,
    query: &'static str,
    product_listing_id: Uuid,
) -> Result<(), sqlx::Error> {
    let count: i64 = sqlx::query_scalar(query)
        .bind(product_listing_id)
        .fetch_one(pool)
        .await?;
    assert_eq!(
        0, count,
        "{table} must cascade from ProductListing deletion"
    );
    Ok(())
}

struct SeededRows {
    product_listing_id: Uuid,
    notification_id: Uuid,
    notification_delivery_id: Uuid,
    notification_payload: serde_json::Value,
}

async fn seed_product_listing_owned_rows(pool: &sqlx::PgPool) -> Result<SeededRows, sqlx::Error> {
    let product_listing_id = Uuid::now_v7();
    let event_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();
    let search_filter_id = Uuid::now_v7();
    let notification_id = Uuid::now_v7();
    let notification_delivery_id = Uuid::now_v7();
    let party_id = Uuid::now_v7();
    let listing_source_id = Uuid::now_v7();
    let listing_source_slug_id = format!("delete-cascade-source-{listing_source_id}");
    let product_listing_title_slug_id = format!(
        "delete-cascade-{}",
        &product_listing_id.simple().to_string()[26..]
    );
    let notification_payload = json!({
        "type": "WATCHLIST",
        "snapshot": {
            "listing_source_id": listing_source_id.to_string(),
            "source_listing_id": product_listing_id.to_string(),
            "listing_source_slug_id": listing_source_slug_id,
            "product_listing_title_slug_id": product_listing_title_slug_id,
            "listing_source_name": "Delete cascade source",
            "title": [{"language": "en", "title": "Delete cascade product"}],
            "image": null,
            "content_policy": null,
            "url": "https://example.test/product",
            "view_url": "https://example.test/product"
        },
        "change": {
            "type": "PRICE_CHANGE",
            "old_price": {"currency": "EUR", "amount": 1200},
            "new_price": {"currency": "EUR", "amount": 1000}
        }
    });
    let mut transaction = pool.begin().await?;

    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(user_id)
        .bind(format!("{user_id}@delete-cascade.test"))
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Delete cascade party')")
        .bind(party_id)
        .bind(format!("delete-cascade-party-{party_id}"))
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Delete cascade source', $3)")
        .bind(listing_source_id)
        .bind(&listing_source_slug_id)
        .bind(party_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Delete cascade product', 'en', 'Delete cascade description', 'en', 'AVAILABLE', 'ACTIVE', 'https://example.test/product')")
        .bind(product_listing_id)
        .bind(&product_listing_title_slug_id)
        .bind(event_id)
        .bind(listing_source_id)
        .bind(product_listing_id.to_string())
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(event_id)
        .bind(product_listing_id)
        .bind(json!({}))
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO product_listing_translations (product_listing_id, source_event_id, language, title) VALUES ($1, $2, 'de', 'Produkt')")
        .bind(product_listing_id)
        .bind(event_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO product_listing_content_assessments (product_listing_id, source_event_id, decision) VALUES ($1, $2, 'ALLOWED')")
        .bind(product_listing_id)
        .bind(event_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO product_listing_watchlist (user_id, product_listing_id, notifications, state, active_since, notifications_enabled_since) VALUES ($1, $2, true, 'ACTIVE', now(), now())")
        .bind(user_id)
        .bind(product_listing_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO search_filters (user_search_filter_id, user_id, name, state, search, language, currency) VALUES ($1, $2, 'Delete cascade filter', 'ACTIVE', '{}', 'en', 'EUR')")
        .bind(search_filter_id)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO search_filter_matches (user_id, user_search_filter_id, product_listing_id, origin_event_id) VALUES ($1, $2, $3, $4)")
        .bind(user_id)
        .bind(search_filter_id)
        .bind(product_listing_id)
        .bind(event_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO notifications (notification_id, user_id, kind, origin_event_id, product_listing_id, payload) VALUES ($1, $2, 'WATCHLIST_PRICE_CHANGED', $3, $4, $5)")
        .bind(notification_id)
        .bind(user_id)
        .bind(event_id)
        .bind(product_listing_id)
        .bind(&notification_payload)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO notification_deliveries (notification_delivery_id, notification_id, channel, target_key, status) VALUES ($1, $2, 'EMAIL', 'user@delete-cascade.test', 'PENDING')")
        .bind(notification_delivery_id)
        .bind(notification_id)
        .execute(&mut *transaction)
        .await?;

    transaction.commit().await?;

    Ok(SeededRows {
        product_listing_id,
        notification_id,
        notification_delivery_id,
        notification_payload,
    })
}
