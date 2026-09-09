use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use domain_primitives::versioned::Versioned;
use fxrate_core::FxRateId;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::Language;
use localization::Localized;
use money::Currency;
use money::{MonetaryAmount, Price};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing::{
    ListingSaleObservation, NewProductListing, ProductListing, ProductListingAuction,
    ProductListingPricing,
};
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use product_listing_core::product_listing_image::ProductListingImage;

use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::title::Title;
use product_listing_postgres::{
    SqlxProductListingEventAppenderFactory, SqlxProductListingRepositoryFactory,
};
use product_listing_service::ports::{
    ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, ProductListingStorageVersion,
    ProductListingWriteEffects, stamp_product_listing_event,
};
use strum::IntoEnumIterator;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_append_find_and_update_product_by_id_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventAppenderFactory::new();
    let listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-main-source").await;
    let product = sample_product("postgres-product-main", listing_source_id);
    let created_event = first_stamped_event(&product);

    let mut tx = begin(&unit_of_work).await;
    match product_listings
        .in_transaction(&mut tx)
        .insert(&product, created_event.event_id)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to insert product: {error:?}"),
    }
    match events.in_transaction(&mut tx).append(&created_event).await {
        Ok(_) => {}
        Err(error) => panic!("failed to append product event: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let Versioned {
        value: loaded_by_id,
        version,
    } = match product_listings
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing product by id"),
        Err(error) => panic!("failed to find product by id: {error:?}"),
    };

    let loaded_by_key = product_listings
        .in_transaction(&mut tx)
        .find_by_key(&ProductListingKey::new(
            product.listing_source_id(),
            product.source_listing_id().clone(),
        ))
        .await;
    assert!(matches!(
        loaded_by_key,
        Ok(Some(Versioned { ref value, .. })) if value.id() == product.id()
    ));

    commit(tx).await;

    assert_eq!(product.id(), loaded_by_id.id());
    assert_eq!(ProductListingStorageVersion::INITIAL, version);

    assert_eq!(ListingLifecycle::Active, loaded_by_id.lifecycle());

    let mut updated = loaded_by_id;
    let outcome = updated.clear_price();
    assert!(matches!(
        outcome,
        Ok(domain_primitives::change_outcome::ChangeOutcome::Changed)
    ));
    let update_event = first_stamped_event(&updated);
    let mut tx = begin(&unit_of_work).await;
    match product_listings
        .in_transaction(&mut tx)
        .update(
            &updated,
            version,
            update_event.event_id,
            ProductListingWriteEffects::default(),
        )
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to update product: {error:?}"),
    }
    match events.in_transaction(&mut tx).append(&update_event).await {
        Ok(_) => {}
        Err(error) => panic!("failed to append update event: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let loaded = match product_listings
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing updated product"),
        Err(error) => panic!("failed to load updated product: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(None, loaded.value.pricing().price);
    assert_eq!(2, loaded.version.into_inner());

    let persisted_identity: (String, uuid::Uuid, String, uuid::Uuid, i64) = sqlx::query_as(
        "SELECT product_listing_title_slug_id, listing_source_id, source_listing_id, current_event_id, projection_version FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(product.id().into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to load persisted product identity: {error}"));
    assert_eq!(product.title_slug_id().as_ref(), persisted_identity.0);
    assert_eq!(
        product.listing_source_id().into_uuid(),
        persisted_identity.1
    );
    assert_eq!(product.source_listing_id().as_ref(), persisted_identity.2);
    assert_eq!(update_event.event_id.into_uuid(), persisted_identity.3);
    assert_eq!(2, persisted_identity.4);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_persist_valid_long_incompressible_url_through_canonical_repository() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventAppenderFactory::new();
    let listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-long-url-source").await;
    let long_url = long_incompressible_url();
    assert!(long_url.as_str().len() > 2_704);
    let product = sample_product_with_url(
        "postgres-product-long-url",
        listing_source_id,
        long_url.clone(),
    );

    insert_product_with_event(&unit_of_work, &product_listings, &events, &product).await;

    let persisted_url: String =
        sqlx::query_scalar("SELECT url FROM product_listings WHERE product_listing_id = $1")
            .bind(product.id().into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("load long canonical URL: {error}"));
    assert_eq!(long_url.as_str(), persisted_url);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_install_required_indexes_without_wide_or_duplicate_btrees() {
    let pool = get_postgres_client().await;
    let (source_url_index, duplicate_raw_revision_index): (Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT to_regclass('product_listings_listing_source_url_idx')::text, to_regclass('product_listing_raw_revisions_stream_revision_idx')::text",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("inspect removed indexes: {error}"));
    assert_eq!(None, source_url_index);
    assert_eq!(None, duplicate_raw_revision_index);

    let raw_revision_constraint: Option<String> = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conrelid = 'product_listing_raw_revisions'::regclass AND conname = 'product_listing_raw_revisions_stream_revision_unique' AND contype = 'u'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap_or_else(|error| panic!("inspect raw revision unique constraint: {error}"));
    assert_eq!(
        Some("UNIQUE (product_listing_raw_stream_id, revision)".to_owned()),
        raw_revision_constraint
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_preserve_embedding_for_price_and_clear_it_when_images_change() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventAppenderFactory::new();
    let listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-embedding-source").await;
    let product = sample_product("postgres-product-embedding-source", listing_source_id);

    insert_product_with_event(&unit_of_work, &product_listings, &events, &product).await;
    sqlx::query(
        "UPDATE product_listings SET embedding = array_fill(1::real, ARRAY[768]) WHERE product_listing_id = $1",
    )
    .bind(product.id().into_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed product embedding: {error}"));
    let initial_embedding_source_event_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT embedding_source_event_id FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(product.id().into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to load initial embedding source: {error}"));

    let mut tx = begin(&unit_of_work).await;
    let Versioned {
        value: mut price_changed,
        version,
    } = match product_listings
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(Some(product)) => product,
        Ok(None) => panic!("missing persisted product"),
        Err(error) => panic!("failed to load product: {error:?}"),
    };
    assert!(
        price_changed
            .set_price(Price::new(MonetaryAmount::from(1_100_u64), Currency::Eur))
            .is_ok()
    );
    let price_payload = match price_changed.take_pending_event_payload() {
        Some(payload) => payload,
        None => panic!("price change is missing a pending event"),
    };
    let price_event = stamp_product_listing_event(
        price_changed.id(),
        OffsetDateTime::now_utc(),
        price_payload.clone(),
    );
    let persisted = match product_listings
        .in_transaction(&mut tx)
        .update(
            &price_changed,
            version,
            price_event.event_id,
            ProductListingWriteEffects::from(&price_payload),
        )
        .await
    {
        Ok(product) => product,
        Err(error) => panic!("failed to update product price: {error:?}"),
    };
    assert_eq!(2, persisted.version.into_inner());
    match events.in_transaction(&mut tx).append(&price_event).await {
        Ok(()) => {}
        Err(error) => panic!("failed to append price event: {error:?}"),
    }
    commit(tx).await;

    let (embedding_source_event_id, has_embedding, version, projection_version):
        (uuid::Uuid, bool, i64, i64) = sqlx::query_as(
        "SELECT embedding_source_event_id, embedding IS NOT NULL, version, projection_version FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(product.id().into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to load price-updated revisions: {error}"));
    assert_eq!(initial_embedding_source_event_id, embedding_source_event_id);
    assert!(has_embedding);
    assert_eq!(2, version);
    assert_eq!(2, projection_version);

    let mut tx = begin(&unit_of_work).await;
    let Versioned {
        value: mut image_changed,
        version,
    } = match product_listings
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(Some(product)) => product,
        Ok(None) => panic!("missing price-updated product"),
        Err(error) => panic!("failed to load price-updated product: {error:?}"),
    };
    let mut replacement_images = IndexSet::new();
    replacement_images.insert(ProductListingImage::new(url(
        "https://example.com/postgres-product-embedding-source-replacement.jpg",
    )));
    assert!(image_changed.replace_images(replacement_images).is_ok());
    let image_payload = match image_changed.take_pending_event_payload() {
        Some(payload) => payload,
        None => panic!("image change is missing a pending event"),
    };
    let image_event = stamp_product_listing_event(
        image_changed.id(),
        OffsetDateTime::now_utc(),
        image_payload.clone(),
    );
    let persisted = match product_listings
        .in_transaction(&mut tx)
        .update(
            &image_changed,
            version,
            image_event.event_id,
            ProductListingWriteEffects::from(&image_payload),
        )
        .await
    {
        Ok(product) => product,
        Err(error) => panic!("failed to update product images: {error:?}"),
    };
    assert_eq!(3, persisted.version.into_inner());
    match events.in_transaction(&mut tx).append(&image_event).await {
        Ok(()) => {}
        Err(error) => panic!("failed to append image event: {error:?}"),
    }
    commit(tx).await;

    let (embedding_source_event_id, has_embedding, version, projection_version):
        (uuid::Uuid, bool, i64, i64) = sqlx::query_as(
        "SELECT embedding_source_event_id, embedding IS NOT NULL, version, projection_version FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(product.id().into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to load image-updated revisions: {error}"));
    assert_eq!(image_event.event_id.into_uuid(), embedding_source_event_id);
    assert!(!has_embedding);
    assert_eq!(3, version);
    assert_eq!(3, projection_version);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_round_trip_immutable_sale_observation_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventAppenderFactory::new();
    let listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-sale-source").await;
    let fx_rate_id = FxRateId::new();
    seed_complete_fx_snapshot(&pool, fx_rate_id).await;
    let observation = ListingSaleObservation::new(OffsetDateTime::UNIX_EPOCH, fx_rate_id);
    let mut product = sample_product("postgres-product-sale", listing_source_id);
    let _discovery = product.take_pending_event_payload();
    let transition = product.record_sale_observation(observation);
    assert!(matches!(
        transition,
        Ok(domain_primitives::change_outcome::ChangeOutcome::Changed)
    ));
    let payload = match product.take_pending_event_payload() {
        Some(payload) => payload,
        None => panic!("listing with a sale observation is missing an event"),
    };
    let observation_event =
        stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload);
    let current_event_id = observation_event.event_id;

    let mut tx = begin(&unit_of_work).await;
    match product_listings
        .in_transaction(&mut tx)
        .insert(&product, current_event_id)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to insert listing with sale observation: {error:?}"),
    }
    match events
        .in_transaction(&mut tx)
        .append(&observation_event)
        .await
    {
        Ok(()) => {}
        Err(error) => panic!("failed to append sold product event: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let loaded = match product_listings
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(Some(product)) => product,
        Ok(None) => panic!("persisted sold product is missing"),
        Err(error) => panic!("failed to load sold product: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(Some(observation), loaded.value.sale_observation());
    assert_eq!(product.pricing(), loaded.value.pricing());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_enforce_paired_sale_observation_columns_in_postgres() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(
        &pool,
        "product-listing-postgres-sale-observation-constraint-source",
    )
    .await;
    let fx_rate_id = FxRateId::new();
    seed_complete_fx_snapshot(&pool, fx_rate_id).await;

    let observation_without_fx_rate = insert_product_row(
        &pool,
        listing_source_id,
        "product-listing-postgres-observation-without-fx-rate",
        None,
        Some(OffsetDateTime::UNIX_EPOCH),
    )
    .await;
    assert_check_violation(observation_without_fx_rate);

    let fx_rate_without_observation = insert_product_row(
        &pool,
        listing_source_id,
        "product-listing-postgres-fx-rate-without-observation",
        Some(fx_rate_id),
        None,
    )
    .await;
    assert_check_violation(fx_rate_without_observation);

    let complete_observation = insert_product_row(
        &pool,
        listing_source_id,
        "product-listing-postgres-complete-observation",
        Some(fx_rate_id),
        Some(OffsetDateTime::UNIX_EPOCH),
    )
    .await;
    if let Err(error) = complete_observation {
        panic!("complete sale observation must be valid: {error}");
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_none_when_product_is_missing_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let product_listing_id = ProductListingId::new();

    let mut tx = begin(&unit_of_work).await;
    let by_id = match product_listings
        .in_transaction(&mut tx)
        .find_by_id(product_listing_id)
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to find missing product by id: {error:?}"),
    };

    commit(tx).await;

    assert!(by_id.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_persist_canonical_source_listing_id_after_unicode_whitespace_input() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventAppenderFactory::new();
    let listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-unicode-source-id").await;
    let source_listing_id = SourceListingId::try_from("\u{2003} SKU  #42/Blue \u{2002}")
        .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));
    let product = sample_product_with_source_listing_id(
        "postgres-product-unicode-source-id",
        listing_source_id,
        source_listing_id.clone(),
    );

    insert_product_with_event(&unit_of_work, &product_listings, &events, &product).await;

    let persisted_source_listing_id: String = sqlx::query_scalar(
        "SELECT source_listing_id FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(product.id().into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to load persisted source listing ID: {error}"));
    assert_eq!("SKU  #42/Blue", persisted_source_listing_id);

    let mut tx = begin(&unit_of_work).await;
    let loaded = match product_listings
        .in_transaction(&mut tx)
        .find_by_key(&ProductListingKey::new(
            listing_source_id,
            source_listing_id,
        ))
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing product by canonical source listing ID"),
        Err(error) => panic!("failed to find product by canonical source listing ID: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(product.id(), loaded.value.id());
    assert_eq!("SKU  #42/Blue", loaded.value.source_listing_id().as_ref());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_map_source_listing_key_conflict_to_source_listing_already_exists() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventAppenderFactory::new();
    let listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-source-key-conflict").await;
    let first = sample_product("postgres-product-source-key-first", listing_source_id);
    let second = sample_product_with_source_listing_id(
        "postgres-product-source-key-second",
        listing_source_id,
        first.source_listing_id().clone(),
    );

    insert_product_with_event(&unit_of_work, &product_listings, &events, &first).await;

    let mut tx = begin(&unit_of_work).await;
    let result = product_listings
        .in_transaction(&mut tx)
        .insert(&second, EventId::new())
        .await;

    assert!(matches!(
        result,
        Err(ProductListingRepositoryError::SourceListingAlreadyExists)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_map_title_slug_conflict_to_product_listing_title_slug_already_exists() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventAppenderFactory::new();
    let first_listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-title-slug-first").await;
    let second_listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-title-slug-second").await;
    let first = sample_product("postgres-product-title-slug-first", first_listing_source_id);
    let second = sample_product_with_title_slug_id(
        "postgres-product-title-slug-second",
        second_listing_source_id,
        SourceListingId::try_from("postgres-product-title-slug-second")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
        ProductListingId::new(),
        first.title_slug_id().clone(),
    );

    insert_product_with_event(&unit_of_work, &product_listings, &events, &first).await;

    let mut tx = begin(&unit_of_work).await;
    let result = product_listings
        .in_transaction(&mut tx)
        .insert(&second, EventId::new())
        .await;

    assert!(matches!(
        result,
        Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_map_unclassified_insert_conflict_to_generic_fallback() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventAppenderFactory::new();
    let first_listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-fallback-first").await;
    let second_listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-fallback-second").await;
    let first = sample_product("postgres-product-fallback-first", first_listing_source_id);
    let second = sample_product_with_id_and_source_listing_id(
        "postgres-product-fallback-second",
        second_listing_source_id,
        SourceListingId::try_from("postgres-product-fallback-second")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
        first.id(),
    );

    insert_product_with_event(&unit_of_work, &product_listings, &events, &first).await;

    let mut tx = begin(&unit_of_work).await;
    let result = product_listings
        .in_transaction(&mut tx)
        .insert(&second, EventId::new())
        .await;

    assert!(matches!(
        result,
        Err(ProductListingRepositoryError::ProductListingInsertFailed)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_product_update_conflict_when_product_row_is_missing() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-missing-update-source").await;
    let product = sample_product("postgres-product-missing-update", listing_source_id);

    let result = {
        let mut tx = begin(&unit_of_work).await;
        product_listings
            .in_transaction(&mut tx)
            .update(
                &product,
                ProductListingStorageVersion::INITIAL,
                EventId::new(),
                ProductListingWriteEffects::default(),
            )
            .await
    };

    assert!(matches!(
        result,
        Err(ProductListingRepositoryError::ConcurrencyConflict)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_product_update_conflict_when_storage_version_is_stale() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventAppenderFactory::new();
    let listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-stale-source").await;
    let mut product = sample_product("postgres-product-stale", listing_source_id);

    insert_product_with_event(&unit_of_work, &product_listings, &events, &product).await;

    let outcome = product.set_availability(ListingAvailability::Reserved);
    assert!(outcome.is_ok());
    let result = {
        let mut tx = begin(&unit_of_work).await;
        product_listings
            .in_transaction(&mut tx)
            .update(
                &product,
                ProductListingStorageVersion::try_from(2_i64)
                    .unwrap_or_else(|error| panic!("valid stale storage version: {error}")),
                EventId::new(),
                ProductListingWriteEffects::default(),
            )
            .await
    };

    assert!(matches!(
        result,
        Err(ProductListingRepositoryError::ConcurrencyConflict)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_roll_back_product_and_event_when_transaction_is_not_committed() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventAppenderFactory::new();
    let listing_source_id =
        seed_listing_source(&pool, "product-listing-postgres-rollback-source").await;
    let product = sample_product("postgres-product-rollback", listing_source_id);
    let event = first_stamped_event(&product);

    {
        let mut tx = begin(&unit_of_work).await;
        match product_listings
            .in_transaction(&mut tx)
            .insert(&product, event.event_id)
            .await
        {
            Ok(_) => {}
            Err(error) => panic!("failed to insert product before rollback: {error:?}"),
        }
        match events.in_transaction(&mut tx).append(&event).await {
            Ok(_) => {}
            Err(error) => panic!("failed to append event before rollback: {error:?}"),
        }
    }

    let mut tx = begin(&unit_of_work).await;
    let product_after_rollback = match product_listings
        .in_transaction(&mut tx)
        .find_by_id(product.id())
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to find rolled-back product: {error:?}"),
    };

    let persisted_event_count: i64 = match sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1",
    )
    .bind(product.id().into_uuid())
    .fetch_one(&pool)
    .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed to count rolled-back events: {error}"),
    };
    commit(tx).await;

    assert!(product_after_rollback.is_none());

    assert_eq!(0, persisted_event_count);
}

async fn insert_product_row(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
    slug: &str,
    sale_observation_fx_rate_id: Option<FxRateId>,
    sale_observed_at: Option<OffsetDateTime>,
) -> Result<(), sqlx::Error> {
    let product_listing_id = uuid::Uuid::now_v7();
    let event_id = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await?;
    let source_listing_id = SourceListingId::try_from(format!("{slug}-source-listing"))
        .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, lifecycle, url, sale_observation_fx_rate_id, sale_observed_at) VALUES ($1, $2, $3, $3, $3, $4, $5, 'ACTIVE', 'https://example.test/product', $6, $7)",
    )
    .bind(product_listing_id)
    .bind(title_slug(slug, product_listing_id))
    .bind(event_id)
    .bind(listing_source_id.into_uuid())
    .bind(source_listing_id.as_ref())
    .bind(sale_observation_fx_rate_id.map(|id| id.into_uuid()))
    .bind(sale_observed_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())",
    )
    .bind(event_id)
    .bind(product_listing_id)
    .bind(serde_json::json!({
        "listingSourceId": listing_source_id.to_string(),
        "sourceListingId": source_listing_id.as_ref(),
        "title": null,
        "description": null,
        "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
        "availability": null,
        "url": "https://example.test/product",
        "imageCount": 0,
        "auction": {"start": null, "end": null}
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

fn assert_check_violation(result: Result<(), sqlx::Error>) {
    assert!(matches!(
        result,
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("23514")
    ));
}

async fn insert_product_with_event(
    unit_of_work: &SqlxUnitOfWork,
    product_listings: &SqlxProductListingRepositoryFactory,
    events: &SqlxProductListingEventAppenderFactory,
    product: &ProductListing,
) {
    let event = first_stamped_event(product);
    let mut tx = begin(unit_of_work).await;
    match product_listings
        .in_transaction(&mut tx)
        .insert(product, event.event_id)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to insert product: {error:?}"),
    }
    match events.in_transaction(&mut tx).append(&event).await {
        Ok(_) => {}
        Err(error) => panic!("failed to append product event: {error:?}"),
    }
    commit(tx).await;
}

fn first_stamped_event(
    product: &ProductListing,
) -> product_listing_service::ports::product_listing_event_appender::ProductListingEvent {
    let mut product = product.clone();
    let payload = match product.take_pending_event_payload() {
        Some(payload) => payload,
        None => panic!("product is missing a pending event payload"),
    };
    stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload)
}

fn title_slug(prefix: &str, product_listing_id: uuid::Uuid) -> String {
    format!(
        "{prefix}-{}",
        &product_listing_id.simple().to_string()[26..]
    )
}

fn sample_product(slug: &str, listing_source_id: ListingSourceId) -> ProductListing {
    let source_listing_id = SourceListingId::try_from(slug)
        .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));
    sample_product_with_source_listing_id(slug, listing_source_id, source_listing_id)
}

fn sample_product_with_source_listing_id(
    slug: &str,
    listing_source_id: ListingSourceId,
    source_listing_id: SourceListingId,
) -> ProductListing {
    sample_product_with_id_and_source_listing_id(
        slug,
        listing_source_id,
        source_listing_id,
        ProductListingId::new(),
    )
}

fn sample_product_with_url(
    slug: &str,
    listing_source_id: ListingSourceId,
    listing_url: Url,
) -> ProductListing {
    let source_listing_id = SourceListingId::try_from(slug)
        .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));
    let mut input = sample_new_product_listing(
        slug,
        listing_source_id,
        source_listing_id,
        ProductListingId::new(),
    );
    input.url = listing_url;
    ProductListing::create(input).unwrap_or_else(|error| panic!("create product: {error}"))
}

fn sample_product_with_id_and_source_listing_id(
    slug: &str,
    listing_source_id: ListingSourceId,
    source_listing_id: SourceListingId,
    id: ProductListingId,
) -> ProductListing {
    match ProductListing::create(sample_new_product_listing(
        slug,
        listing_source_id,
        source_listing_id,
        id,
    )) {
        Ok(product) => product,
        Err(error) => panic!("failed to create product: {error}"),
    }
}

fn sample_product_with_title_slug_id(
    slug: &str,
    listing_source_id: ListingSourceId,
    source_listing_id: SourceListingId,
    id: ProductListingId,
    title_slug_id: product_listing_core::product_listing_slug_id::ProductListingSlugId,
) -> ProductListing {
    let mut input = sample_new_product_listing(slug, listing_source_id, source_listing_id, id);
    input.title_slug_id = title_slug_id;
    match ProductListing::create(input) {
        Ok(product) => product,
        Err(error) => panic!("failed to create product: {error}"),
    }
}

fn sample_new_product_listing(
    slug: &str,
    listing_source_id: ListingSourceId,
    source_listing_id: SourceListingId,
    id: ProductListingId,
) -> NewProductListing {
    let mut images = IndexSet::new();
    images.insert(ProductListingImage::new(url(&format!(
        "https://example.com/{slug}.jpg"
    ))));
    NewProductListing {
        id,
        title_slug_id: product_listing_core::product_listing_slug_id::ProductListingSlugId::from_title_and_suffix(
            slug,
            "a1b2c3",
        )
        .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
        listing_source_id,
        source_listing_id,
        title: Some(Localized::new(Language::En, Title::from(slug))),
        description: Some(Localized::new(
            Language::En,
            Description::from("Nice product"),
        )),
        pricing: ProductListingPricing {
            price: Some(Price::new(MonetaryAmount::from(1_200_u64), Currency::Eur)),
            price_estimate_min: None,
            price_estimate_max: None,
        },
        availability: None,
        url: url(&format!("https://example.com/{slug}")),
        images,
        auction: ProductListingAuction::default(),
    }
}

async fn seed_complete_fx_snapshot(pool: &sqlx::PgPool, fx_rate_id: FxRateId) {
    let rate = sqlx::query(
        "INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(fx_rate_id.into_uuid())
    .bind(OffsetDateTime::UNIX_EPOCH)
    .bind("fxratesapi")
    .bind(fx_rate_id.to_string())
    .execute(pool)
    .await;
    if let Err(error) = rate {
        panic!("failed to seed FX snapshot: {error}");
    }

    for currency in Currency::iter() {
        let quote = sqlx::query(
            "INSERT INTO fx_rate_quotes (fx_rate_id, currency, units_per_eur) VALUES ($1, $2, $3)",
        )
        .bind(fx_rate_id.into_uuid())
        .bind(currency.as_str())
        .bind(if currency == Currency::Eur {
            1_000_000_i64
        } else {
            1_250_000_i64
        })
        .execute(pool)
        .await;
        if let Err(error) = quote {
            panic!("failed to seed FX quote: {error}");
        }
    }
}

async fn seed_listing_source(pool: &sqlx::PgPool, slug: &str) -> ListingSourceId {
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed listing-source party: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(listing_source_id.into_uuid())
        .bind(slug)
        .bind(slug)
        .bind(party_id)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed listing source: {error}"));
    listing_source_id
}

fn url(value: &str) -> Url {
    match Url::parse(value) {
        Ok(url) => url,
        Err(error) => panic!("invalid test URL: {error}"),
    }
}

fn long_incompressible_url() -> Url {
    const PATH_LENGTH: usize = 4_096;
    const URL_SAFE_ASCII: &[u8] =
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";

    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut path = String::with_capacity(PATH_LENGTH);
    for _ in 0..PATH_LENGTH {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let index = ((state >> 32) as usize) % URL_SAFE_ASCII.len();
        path.push(URL_SAFE_ASCII[index] as char);
    }

    url(&format!("https://example.test/{path}"))
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> platform_postgres::SqlxTransaction {
    match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    }
}

async fn commit(tx: platform_postgres::SqlxTransaction) {
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
    }
}
