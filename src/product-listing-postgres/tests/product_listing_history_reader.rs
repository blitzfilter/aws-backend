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
    SqlxProductListingEventAppenderFactory, SqlxProductListingHistoryReaderFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::{
    ports::{
        ProductListingEventAppender, ProductListingEventAppenderFactory,
        ProductListingHistoryReader, ProductListingHistoryReaderFactory, ProductListingRepository,
        ProductListingRepositoryFactory, ProductListingWriteEffects, stamp_product_listing_event,
    },
    use_cases::{
        ProductListingHistoryChange, ProductListingHistoryEntryKind, ProductListingHistoryLookup,
    },
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_service_owned_domain_history_by_id_and_title_slug() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let listing_source_id = seed_listing_source(&pool, "history-reader-source").await;
    let mut product = sample_product(listing_source_id);
    let payload = product
        .take_pending_event_payload()
        .unwrap_or_else(|| panic!("new product is missing discovered event"));
    let event = stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload);
    let product_listing_id = product.id();
    let product_listing_title_slug_id = product.title_slug_id().clone();

    let mut transaction = begin(&unit_of_work).await;
    let created = SqlxProductListingRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .insert(&product, event.event_id)
        .await
        .unwrap_or_else(|error| panic!("insert product: {error:?}"));
    SqlxProductListingEventAppenderFactory::new()
        .in_transaction(&mut transaction)
        .append(&event)
        .await
        .unwrap_or_else(|error| panic!("append event: {error:?}"));

    product
        .replace_pricing(ProductListingPricing {
            price: Some(Price::new(MonetaryAmount::from(13_u64), Currency::Eur).into()),
            price_estimate_min: None,
            price_estimate_max: None,
        })
        .unwrap_or_else(|error| panic!("replace price: {error}"));
    product
        .set_availability(ListingAvailability::SoldOut)
        .unwrap_or_else(|error| panic!("set availability: {error}"));
    let changed_event = stamp_product_listing_event(
        product.id(),
        OffsetDateTime::now_utc(),
        product
            .take_pending_event_payload()
            .unwrap_or_else(|| panic!("changed product event is missing")),
    );
    SqlxProductListingRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .update(
            &product,
            created.version,
            changed_event.event_id,
            ProductListingWriteEffects::default(),
        )
        .await
        .unwrap_or_else(|error| panic!("update product: {error:?}"));
    SqlxProductListingEventAppenderFactory::new()
        .in_transaction(&mut transaction)
        .append(&changed_event)
        .await
        .unwrap_or_else(|error| panic!("append changed event: {error:?}"));
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("commit: {error}"));

    let by_id = read_history(
        &unit_of_work,
        ProductListingHistoryLookup::ById(product_listing_id),
    )
    .await;
    let by_title_slug = read_history(
        &unit_of_work,
        ProductListingHistoryLookup::ByTitleSlug(product_listing_title_slug_id),
    )
    .await;

    assert_eq!(by_id, by_title_slug);
    assert_eq!(2, by_id.len());
    assert_eq!(product_listing_id, by_id[0].product_listing_id);
    assert_eq!(event.event_id, by_id[0].event_id);
    assert!(matches!(
        &by_id[0].kind,
        ProductListingHistoryEntryKind::Discovered(_)
    ));
    assert_eq!(changed_event.event_id, by_id[1].event_id);
    assert!(matches!(
        &by_id[1].kind,
        ProductListingHistoryEntryKind::Changed(changes)
            if matches!(changes.as_slice(), [
                ProductListingHistoryChange::MainPriceChanged { .. },
                ProductListingHistoryChange::AvailabilityChanged { .. },
            ])
    ));
}

async fn read_history(
    unit_of_work: &SqlxUnitOfWork,
    lookup: ProductListingHistoryLookup,
) -> Vec<product_listing_service::use_cases::ProductListingHistoryEntry> {
    let mut transaction = begin(unit_of_work).await;
    let history = SqlxProductListingHistoryReaderFactory::new()
        .in_transaction(&mut transaction)
        .find_history(&lookup)
        .await
        .unwrap_or_else(|error| panic!("read history: {error:?}"))
        .unwrap_or_else(|| panic!("history source is missing"));
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("commit history read: {error}"));
    history
}

fn sample_product(listing_source_id: ListingSourceId) -> ProductListing {
    let mut images = IndexSet::new();
    images.insert(ProductListingImage::new(url(
        "https://example.com/history-reader-image.jpg",
    )));
    ProductListing::create(NewProductListing {
        id: ProductListingId::new(),
        title_slug_id: ProductListingSlugId::raw("history-reader-a1b2c3")
            .unwrap_or_else(|error| panic!("valid product slug: {error}")),
        listing_source_id,
        source_listing_id: SourceListingId::try_from("history-reader-source-listing")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
        title: Some(Localized::new(Language::En, Title::from("History reader"))),
        description: Some(Localized::new(
            Language::En,
            Description::from("Service-owned history model"),
        )),
        pricing: ProductListingPricing {
            price: Some(Price::new(MonetaryAmount::from(12_u64), Currency::Eur).into()),
            price_estimate_min: None,
            price_estimate_max: None,
        },
        availability: Some(ListingAvailability::Available),
        url: url("https://example.com/history-reader"),
        images,
        auction: ProductListingAuction::default(),
    })
    .unwrap_or_else(|error| panic!("create product: {error}"))
}

async fn seed_listing_source(pool: &sqlx::PgPool, slug: &str) -> ListingSourceId {
    let party_id = uuid::Uuid::new_v4();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed party: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(uuid::Uuid::from(listing_source_id))
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
