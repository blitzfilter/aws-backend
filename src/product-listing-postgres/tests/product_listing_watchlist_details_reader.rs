use application::pagination::{Cursor, CursoredResult};
use application::transaction::{Transaction, UnitOfWork};
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use money::Currency;
use money::{MonetaryAmount, Price};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing::{
    NewProductListing, ProductListing, ProductListingAuction, ProductListingPricing,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;

use listing_source_core::ListingSourceId;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::title::Title;
use product_listing_postgres::{
    SqlxProductListingEventAppenderFactory, SqlxProductListingRepositoryFactory,
    SqlxProductListingWatchlistDetailsReaderFactory,
};
use product_listing_service::ports::PersonalizedProductListingDetailsReadModel;
use product_listing_service::ports::{
    ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
    ProductListingRepositoryFactory, ProductListingWatchlistDetailsCursor,
    ProductListingWatchlistDetailsReadError, ProductListingWatchlistDetailsReader,
    ProductListingWatchlistDetailsReaderFactory, ProductListingWatchlistDetailsRequest,
    stamp_product_listing_event,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use url::Url;
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_join_watchlisted_product_localization_and_user_state() {
    let pool = get_postgres_client().await;
    let product = persist_product(
        &pool,
        "watchlist-joined",
        Some(Localized::new(Language::En, Title::from("Original title"))),
        Some(Localized::new(
            Language::En,
            Description::from("Original description"),
        )),
    )
    .await;
    let user_id = seed_user(&pool, "FREE", false).await;

    insert_translation(
        &pool,
        product.id(),
        "de",
        Some("Deutscher Titel"),
        Some("Deutsche Beschreibung"),
    )
    .await;
    insert_watchlist(
        &pool,
        user_id,
        product.id(),
        false,
        OffsetDateTime::UNIX_EPOCH,
    )
    .await;

    let product_listings = find_for_user(&pool, user_id, Language::De).await;
    let [view] = product_listings.as_slice() else {
        panic!("expected one watchlisted product");
    };

    assert_eq!(view.item.product_listing_id, product.id());
    assert_eq!(view.item.source_listing_id.as_ref(), "watchlist-joined");
    assert_eq!(view.item.source.name.as_ref(), "watchlist-joined-source");
    assert!(view.item.sale_observation.is_none());
    assert_localized_title(
        view.item.product_title.as_ref(),
        Language::En,
        "Original title",
    );
    assert_localized_description(
        view.item.product_description.as_ref(),
        Language::En,
        "Original description",
    );
    assert_localized_title(view.item.title.as_ref(), Language::De, "Deutscher Titel");
    assert_localized_description(
        view.item.description.as_ref(),
        Language::De,
        "Deutsche Beschreibung",
    );

    let Some(user_state) = view.user_state.as_ref() else {
        panic!("missing product user state");
    };
    assert!(user_state.watchlist.watching);
    assert!(!user_state.watchlist.notifications);
    assert!(
        !user_state
            .content_visibility
            .show_unassessed_or_sensitive_content
    );
    assert!(!user_state.search_filter.matched);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_retain_withdrawn_product_in_watchlist_collection() {
    let pool = get_postgres_client().await;
    let product = persist_product(&pool, "watchlist-withdrawn", None, None).await;
    let user_id = seed_user(&pool, "FREE", false).await;
    insert_watchlist(
        &pool,
        user_id,
        product.id(),
        true,
        OffsetDateTime::UNIX_EPOCH,
    )
    .await;
    sqlx::query(
        "UPDATE product_listings SET lifecycle = 'WITHDRAWN', availability = NULL WHERE product_listing_id = $1",
    )
    .bind(product.id().into_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("withdraw product fixture: {error}"));

    let product_listings = find_for_user(&pool, user_id, Language::En).await;
    let [view] = product_listings.as_slice() else {
        panic!("expected retained withdrawn watchlist product");
    };

    assert_eq!(product.id(), view.item.product_listing_id);
    assert_eq!(ListingLifecycle::Withdrawn, view.item.lifecycle);
    assert!(view.item.availability.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_order_watchlisted_products_by_created_desc_then_product_listing_id_asc() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "FREE", false).await;
    let first_tied = persist_product(&pool, "watchlist-order-first", None, None).await;
    let second_tied = persist_product(&pool, "watchlist-order-second", None, None).await;
    let newest = persist_product(&pool, "watchlist-order-newest", None, None).await;
    let tied_created = OffsetDateTime::UNIX_EPOCH + Duration::days(1);

    insert_watchlist(&pool, user_id, first_tied.id(), true, tied_created).await;
    insert_watchlist(&pool, user_id, second_tied.id(), true, tied_created).await;
    insert_watchlist(
        &pool,
        user_id,
        newest.id(),
        true,
        tied_created + Duration::seconds(1),
    )
    .await;

    let product_listings = find_for_user(&pool, user_id, Language::En).await;
    let mut tied_ids = [first_tied.id(), second_tied.id()];
    tied_ids.sort_by_key(|product_listing_id| (*product_listing_id).into_uuid());
    let product_listing_ids = product_listings
        .into_iter()
        .map(|product| product.item.product_listing_id)
        .collect::<Vec<_>>();

    assert_eq!(
        product_listing_ids,
        vec![newest.id(), tied_ids[0], tied_ids[1]]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_page_watchlisted_products_by_created_desc_then_product_listing_id_asc() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "FREE", false).await;
    let first_tied = persist_product(&pool, "watchlist-page-first", None, None).await;
    let second_tied = persist_product(&pool, "watchlist-page-second", None, None).await;
    let third_tied = persist_product(&pool, "watchlist-page-third", None, None).await;
    let newest = persist_product(&pool, "watchlist-page-newest", None, None).await;
    let tied_created = OffsetDateTime::UNIX_EPOCH + Duration::days(1);

    for product_listing_id in [first_tied.id(), second_tied.id(), third_tied.id()] {
        insert_watchlist(&pool, user_id, product_listing_id, true, tied_created).await;
    }
    insert_watchlist(
        &pool,
        user_id,
        newest.id(),
        true,
        tied_created + Duration::seconds(1),
    )
    .await;

    let mut tied_ids = [first_tied.id(), second_tied.id(), third_tied.id()];
    tied_ids.sort_by_key(|product_listing_id| (*product_listing_id).into_uuid());
    let first_page = find_for_user_page(
        &pool,
        &ProductListingWatchlistDetailsRequest {
            user_id,
            language: Language::En,
            cursor: Cursor {
                size: 2,
                search_after: None,
            },
        },
    )
    .await;
    let expected_cursor = ProductListingWatchlistDetailsCursor {
        watchlist_created: tied_created,
        product_listing_id: tied_ids[0],
    };

    assert_eq!(
        first_page
            .items
            .iter()
            .map(|product| product.item.product_listing_id)
            .collect::<Vec<_>>(),
        vec![newest.id(), tied_ids[0]]
    );
    assert_eq!(first_page.cursor.search_after, Some(expected_cursor));

    let second_page = find_for_user_page(
        &pool,
        &ProductListingWatchlistDetailsRequest {
            user_id,
            language: Language::En,
            cursor: Cursor {
                size: 2,
                search_after: first_page.cursor.search_after,
            },
        },
    )
    .await;

    assert_eq!(
        second_page
            .items
            .iter()
            .map(|product| product.item.product_listing_id)
            .collect::<Vec<_>>(),
        vec![tied_ids[1], tied_ids[2]]
    );
    assert_eq!(second_page.cursor.search_after, None);
}

async fn find_for_user(
    pool: &sqlx::PgPool,
    user_id: UserId,
    language: Language,
) -> Vec<PersonalizedProductListingDetailsReadModel> {
    match find_for_user_result(pool, user_id, language).await {
        Ok(product_listings) => product_listings,
        Err(error) => panic!("failed to read product watchlist details: {error:?}"),
    }
}

async fn find_for_user_result(
    pool: &sqlx::PgPool,
    user_id: UserId,
    language: Language,
) -> Result<Vec<PersonalizedProductListingDetailsReadModel>, ProductListingWatchlistDetailsReadError>
{
    find_for_user_page_result(
        pool,
        &ProductListingWatchlistDetailsRequest {
            user_id,
            language,
            cursor: Cursor {
                size: 100,
                search_after: None,
            },
        },
    )
    .await
    .map(|page| page.items)
}

async fn find_for_user_page(
    pool: &sqlx::PgPool,
    request: &ProductListingWatchlistDetailsRequest,
) -> CursoredResult<PersonalizedProductListingDetailsReadModel, ProductListingWatchlistDetailsCursor>
{
    match find_for_user_page_result(pool, request).await {
        Ok(page) => page,
        Err(error) => panic!("failed to read product watchlist details: {error:?}"),
    }
}

async fn find_for_user_page_result(
    pool: &sqlx::PgPool,
    request: &ProductListingWatchlistDetailsRequest,
) -> Result<
    CursoredResult<
        PersonalizedProductListingDetailsReadModel,
        ProductListingWatchlistDetailsCursor,
    >,
    ProductListingWatchlistDetailsReadError,
> {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let details = SqlxProductListingWatchlistDetailsReaderFactory::new();
    let mut tx = begin(&unit_of_work).await;
    let result = details.in_transaction(&mut tx).find_for_user(request).await;
    commit(tx).await;
    result
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

async fn persist_product(
    pool: &sqlx::PgPool,
    slug: &str,
    title: Option<Localized<Language, Title>>,
    description: Option<Localized<Language, Description>>,
) -> ProductListing {
    let listing_source_id = seed_listing_source(pool, &format!("{slug}-source")).await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let product_listings = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventAppenderFactory::new();
    let product = sample_product(slug, listing_source_id, title, description);
    let event = first_stamped_event(&product);

    let mut tx = begin(&unit_of_work).await;
    match product_listings
        .in_transaction(&mut tx)
        .insert(&product, event.event_id)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to persist product: {error:?}"),
    }
    match events.in_transaction(&mut tx).append(&event).await {
        Ok(_) => {}
        Err(error) => panic!("failed to persist product event: {error:?}"),
    }
    commit(tx).await;

    product
}

async fn insert_translation(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    language: &str,
    title: Option<&str>,
    description: Option<&str>,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO product_listing_translations (product_listing_id, source_event_id, language, title, description)
        SELECT product_listing_id, current_event_id, $2, $3, $4
        FROM product_listings
        WHERE product_listing_id = $1
        "#,
    )
    .bind(product_listing_id.into_uuid())
    .bind(language)
    .bind(title)
    .bind(description)
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed product translation: {error}");
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

async fn insert_watchlist(
    pool: &sqlx::PgPool,
    user_id: UserId,
    product_listing_id: ProductListingId,
    notifications: bool,
    created: OffsetDateTime,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO product_listing_watchlist (user_id, product_listing_id, notifications, state, active_since, notifications_enabled_since, created, updated)
        VALUES ($1, $2, $3, $4, CASE WHEN $4 = 'ACTIVE' THEN $5 ELSE NULL END, CASE WHEN $3 THEN $5 ELSE NULL END, $5, $5)
        "#,
    )
    .bind(user_id.into_uuid())
    .bind(product_listing_id.into_uuid())
    .bind(notifications)
    .bind("ACTIVE")
    .bind(created)
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed product watchlist: {error}");
    }
}

fn sample_product(
    slug: &str,
    listing_source_id: ListingSourceId,
    title: Option<Localized<Language, Title>>,
    description: Option<Localized<Language, Description>>,
) -> ProductListing {
    let mut images = IndexSet::new();
    images.insert(ProductListingImage::new(url(&format!(
        "https://example.com/{slug}.jpg"
    ))));

    match ProductListing::create(NewProductListing {
        id: ProductListingId::new(),
        title_slug_id: ProductListingSlugId::from_title_and_suffix(slug, "a1b2c3")
            .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
        listing_source_id,
        source_listing_id: SourceListingId::try_from(slug)
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
        title,
        description,
        pricing: ProductListingPricing {
            price: Some(Price::new(MonetaryAmount::from(1_200_u64), Currency::Eur)),
            price_estimate_min: None,
            price_estimate_max: None,
        },
        availability: Some(ListingAvailability::Available),
        url: url(&format!("https://example.com/{slug}")),
        images,
        auction: ProductListingAuction::default(),
    }) {
        Ok(product) => product,
        Err(error) => panic!("failed to create product: {error}"),
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

fn assert_localized_title(
    value: Option<&Localized<Language, Title>>,
    language: Language,
    text: &str,
) {
    match value {
        Some(value) => {
            assert_eq!(value.localization, language);
            assert_eq!(value.payload.as_ref(), text);
        }
        None => panic!("missing title"),
    }
}

fn assert_localized_description(
    value: Option<&Localized<Language, Description>>,
    language: Language,
    text: &str,
) {
    match value {
        Some(value) => {
            assert_eq!(value.localization, language);
            assert_eq!(value.payload.as_ref(), text);
        }
        None => panic!("missing description"),
    }
}

fn url(value: &str) -> Url {
    match Url::parse(value) {
        Ok(url) => url,
        Err(error) => panic!("invalid test URL: {error}"),
    }
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
