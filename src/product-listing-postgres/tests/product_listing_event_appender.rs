use application::transaction::{Transaction, UnitOfWork};
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    product_listing::{
        NewProductListing, ProductListing, ProductListingAuction, ProductListingPricing,
    },
    product_listing_id::ProductListingId,
    product_listing_image::ProductListingImage,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_postgres::{
    SqlxProductListingEventAppenderFactory, SqlxProductListingRepositoryFactory,
};
use product_listing_service::ports::{
    ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
    ProductListingRepositoryFactory, stamp_product_listing_event,
};
use serde_json::Value;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_append_domain_event_with_type_version_group_and_semantic_payload() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let listing_source_id = seed_listing_source(&pool, "event-appender-source").await;
    let mut product = sample_product(listing_source_id);
    let payload = match product.take_pending_event_payload() {
        Some(payload) => payload,
        None => panic!("new product is missing discovered event"),
    };
    let event = stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload);

    let mut transaction = begin(&unit_of_work).await;
    SqlxProductListingRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .insert(&product, event.event_id)
        .await
        .unwrap_or_else(|error| panic!("insert product: {error:?}"));
    SqlxProductListingEventAppenderFactory::new()
        .in_transaction(&mut transaction)
        .append(&event)
        .await
        .unwrap_or_else(|error| panic!("append event: {error:?}"));
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("commit: {error}"));

    let row: (String, String, i16, Value) = sqlx::query_as(
        "SELECT event_type, event_group, event_type_schema_version, payload FROM product_listing_events WHERE event_id = $1",
    )
    .bind(event.event_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("read event: {error}"));

    assert_eq!("PRODUCT_LISTING_DISCOVERED", row.0);
    assert_eq!("DOMAIN", row.1);
    assert_eq!(1, row.2);
    assert!(row.3.get("kind").is_none());
    assert_eq!(Some(1), row.3.get("imageCount").and_then(Value::as_u64));
    assert!(!row.3.to_string().contains("event-appender-image"));
}

fn sample_product(listing_source_id: ListingSourceId) -> ProductListing {
    let mut images = IndexSet::new();
    images.insert(ProductListingImage::new(url(
        "https://example.com/event-appender-image.jpg",
    )));
    ProductListing::create(NewProductListing {
        id: ProductListingId::new(),
        title_slug_id: ProductListingSlugId::raw("event-appender-a1b2c3")
            .unwrap_or_else(|error| panic!("valid product slug: {error}")),
        listing_source_id,
        source_listing_id: SourceListingId::try_from("event-appender-source-listing")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
        title: Some(Localized::new(Language::En, Title::from("Event appender"))),
        description: Some(Localized::new(
            Language::En,
            Description::from("Semantic event payload"),
        )),
        pricing: ProductListingPricing {
            price: Some(Price::new(MonetaryAmount::from(12_u64), Currency::Eur)),
            price_estimate_min: None,
            price_estimate_max: None,
        },
        availability: Some(ListingAvailability::Available),
        url: url("https://example.com/event-appender"),
        images,
        auction: ProductListingAuction::default(),
    })
    .unwrap_or_else(|error| panic!("create product: {error}"))
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
        .unwrap_or_else(|error| panic!("seed party: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(listing_source_id.into_uuid())
        .bind(slug)
        .bind(slug)
        .bind(party_id)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed listing source: {error}"));
    listing_source_id
}

fn url(value: &str) -> Url {
    Url::parse(value).unwrap_or_else(|error| panic!("valid URL: {error}"))
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> platform_postgres::SqlxTransaction {
    unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin transaction: {error}"))
}
