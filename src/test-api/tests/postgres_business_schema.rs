use sqlx::{AssertSqlSafe, Executor};
use test_api::*;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_apply_business_schema_migrations() {
    let pool = get_postgres_client().await;

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name \
         FROM information_schema.tables \
         WHERE table_schema = 'public' \
           AND table_type = 'BASE TABLE' \
         ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    for expected in [
        "users",
        "parties",
        "listing_sources",
        "partnerships",
        "partnership_members",
        "partnership_listing_source_grants",
        "fx_rate_quotes",
        "fx_rates",
        "product_listings",
        "product_listing_events",
        "product_listing_translations",
        "product_listing_watchlist",
        "search_filters",
        "search_filter_matches",
    ] {
        assert!(tables.contains(&expected.to_string()));
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_clear_business_rows_without_removing_pg_ttl_configuration() {
    let pool = get_postgres_client().await;
    pool.execute(
        sqlx::query(
            "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
        )
        .bind(uuid::Uuid::new_v4())
        .bind("teardown-check@example.com"),
    )
    .await
    .unwrap();

    BUSINESS_SCHEMA.tear_down().await;

    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    let ttl_registrations: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT schema_name, table_name, column_name \
         FROM ttl_summary() \
         ORDER BY schema_name, table_name, column_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(0, user_count);
    assert_eq!(
        vec![
            (
                "public".to_owned(),
                "access_tokens".to_owned(),
                "expires_at".to_owned()
            ),
            (
                "public".to_owned(),
                "oauth_authorization_codes".to_owned(),
                "expires_at".to_owned(),
            ),
            (
                "public".to_owned(),
                "oauth_third_party_exchange_codes".to_owned(),
                "expires_at".to_owned(),
            ),
            (
                "public".to_owned(),
                "product_listing_raw_provider_observation_receipts".to_owned(),
                "expires_at".to_owned(),
            ),
        ],
        ttl_registrations
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_apply_intentional_secondary_index_definitions() {
    let pool = get_postgres_client().await;

    for (index_name, definition_fragment) in [
        (
            "product_listing_watchlist_product_user_idx",
            "(product_listing_id, user_id)",
        ),
        (
            "search_filter_matches_filter_created_desc_product_listing_idx",
            "(user_search_filter_id, created DESC, product_listing_id)",
        ),
        (
            "listing_sources_operator_party_id_idx",
            "(operator_party_id)",
        ),
        (
            "product_listings_listing_source_id_idx",
            "(listing_source_id)",
        ),
        (
            "product_listings_lifecycle_updated_idx",
            "(lifecycle, updated DESC)",
        ),
        (
            "product_listing_events_product_listing_time_event_idx",
            "(product_listing_id, event_time, event_id)",
        ),
    ] {
        let definition: Option<String> = sqlx::query_scalar(
            "SELECT pg_get_indexdef(indexrelid) FROM pg_stat_user_indexes WHERE indexrelname = $1",
        )
        .bind(index_name)
        .fetch_optional(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to inspect index {index_name}: {error:?}"));

        assert!(
            definition.is_some_and(|definition| definition.contains(definition_fragment)),
            "missing or unexpected definition for {index_name}"
        );
    }

    let removed_indexes: Vec<String> = sqlx::query_scalar(
        "SELECT indexrelname FROM pg_stat_user_indexes WHERE indexrelname = ANY($1) ORDER BY indexrelname",
    )
    .bind(vec![
        "product_listing_events_product_time_idx",
        "product_listing_watchlist_product_listing_id_idx",
        "product_listing_watchlist_user_created_idx",
    ])
    .fetch_all(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to inspect removed indexes: {error:?}"));

    assert!(removed_indexes.is_empty());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_support_core_business_relations() {
    let pool = get_postgres_client().await;
    let product_listing_title_slug_id = product_listing_title_slug_id("A vase");
    let business_fixture_sql = r#"
        INSERT INTO users (user_id, email, tier, role)
        VALUES ('10000000-0000-0000-0000-000000000001', 'user@example.com', 'FREE', 'USER');

        INSERT INTO parties (party_id, party_slug_id, name)
        VALUES (
            '20000000-0000-0000-0000-000000000001',
            'source-operator',
            'Source Operator'
        );

        INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id)
        VALUES (
            '20000000-0000-0000-0000-000000000002',
            'source-one',
            'Source One',
            '20000000-0000-0000-0000-000000000001'
        );

        INSERT INTO partnerships (partnership_id, party_id)
        VALUES (
            '60000000-0000-0000-0000-000000000001',
            '20000000-0000-0000-0000-000000000001'
        );

        INSERT INTO partnership_members (user_id, partnership_id)
        VALUES (
            '10000000-0000-0000-0000-000000000001',
            '60000000-0000-0000-0000-000000000001'
        );

        INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id)
        VALUES (
            '60000000-0000-0000-0000-000000000001',
            '20000000-0000-0000-0000-000000000002'
        );

        BEGIN;

        INSERT INTO product_listings (
            product_listing_id,
            product_listing_title_slug_id,
            current_event_id,
            content_source_event_id,
            embedding_source_event_id,
            listing_source_id,
            source_listing_id,
            title_text,
            title_language,
            availability,
            lifecycle,
            url,
            product_images
        )
        VALUES (
            '30000000-0000-0000-0000-000000000001',
            '__PRODUCT_LISTING_TITLE_SLUG_ID__',
            '40000000-0000-0000-0000-000000000001',
            '40000000-0000-0000-0000-000000000001',
            '40000000-0000-0000-0000-000000000001',
            '20000000-0000-0000-0000-000000000002',
            'external-1',
            'A vase',
            'en',
            NULL,
            'ACTIVE',
            'https://shop.example.com/product_listings/external-1',
            '["https://cdn.example.com/image.jpg"]'
        );

        INSERT INTO product_listing_events (
            event_id,
            product_listing_id,
            event_type,
            event_group,
            event_type_schema_version,
            payload,
            event_time
        )
        VALUES (
            '40000000-0000-0000-0000-000000000001',
            '30000000-0000-0000-0000-000000000001',
            'PRODUCT_LISTING_DISCOVERED',
            'DOMAIN',
            1,
            '{"listingSourceId":"20000000-0000-0000-0000-000000000002","sourceListingId":"external-1","title":{"language":"en","text":"A vase"},"description":null,"pricing":{"price":null,"priceEstimateMin":null,"priceEstimateMax":null},"availability":null,"url":"https://shop.example.com/product_listings/external-1","imageCount":1,"auction":{"start":null,"end":null}}',
            now()
        );

        COMMIT;

        INSERT INTO product_listing_watchlist (user_id, product_listing_id, state, active_since, notifications_enabled_since)
        VALUES (
            '10000000-0000-0000-0000-000000000001',
            '30000000-0000-0000-0000-000000000001',
            'ACTIVE',
            now(),
            now()
        );

        INSERT INTO search_filters (
            user_search_filter_id,
            user_id,
            name,
            state,
            search,
            language,
            currency
        )
        VALUES (
            '50000000-0000-0000-0000-000000000001',
            '10000000-0000-0000-0000-000000000001',
            'Vases',
            'ACTIVE',
            '{"product_listing_query": ["vase"]}',
            'en',
            'EUR'
        );

        INSERT INTO search_filter_matches (
            user_id,
            user_search_filter_id,
            product_listing_id,
            origin_event_id
        )
        VALUES (
            '10000000-0000-0000-0000-000000000001',
            '50000000-0000-0000-0000-000000000001',
            '30000000-0000-0000-0000-000000000001',
            '40000000-0000-0000-0000-000000000001'
        );
        "#
    .replace(
        "__PRODUCT_LISTING_TITLE_SLUG_ID__",
        &product_listing_title_slug_id,
    );
    pool.execute(sqlx::raw_sql(AssertSqlSafe(business_fixture_sql)))
        .await
        .unwrap();

    let match_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM search_filter_matches")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(1, match_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_noncanonical_persisted_enum_values() {
    let pool = get_postgres_client().await;

    let result =
        sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, $3, $4)")
            .bind(uuid::Uuid::new_v4())
            .bind("invalid-state@example.com")
            .bind("FREE")
            .bind("User")
            .execute(&pool)
            .await;

    assert!(result.is_err(), "noncanonical user role must be rejected");
}

fn product_listing_title_slug_id(title: &str) -> String {
    product_listing_core::product_listing_slug_id::ProductListingSlugId::from_title_and_suffix(
        title, "a1b2c3",
    )
    .unwrap_or_else(|error| panic!("valid product listing title slug: {error}"))
    .to_string()
}
