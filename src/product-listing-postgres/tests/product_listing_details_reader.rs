use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use notification_core::notification_id::NotificationId;
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use product_listing_core::description::Description;
use product_listing_core::product_listing::{
    NewProductListing, ProductListing, ProductListingAuction, ProductListingPricing,
};
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::title::Title;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing_id::ProductListingId,
    product_listing_slug_id::ProductListingSlugId, source_listing_id::SourceListingId,
};
use product_listing_postgres::{
    SqlxProductListingDetailsReaderFactory, SqlxProductListingEventAppenderFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::ports::{
    PersonalizedProductListingDetailsReadModel, ProductListingDetailsReadModel,
    ProductListingDetailsReadRequest,
};
use product_listing_service::ports::{
    ProductListingDetailsReader, ProductListingDetailsReaderFactory, ProductListingEventAppender,
    ProductListingEventAppenderFactory, ProductListingRepository, ProductListingRepositoryFactory,
    stamp_product_listing_event,
};
use product_listing_service::use_cases::queries::get_product_listing::ProductListingLookup;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use url::Url;
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_select_requested_translations_independently_and_preserve_original_text_for_id_lookup()
 {
    let pool = get_postgres_client().await;
    let product = persist_product(
        &pool,
        "details-requested",
        Some(Localized::new(Language::En, Title::from("Original title"))),
        Some(Localized::new(
            Language::En,
            Description::from("Original description"),
        )),
    )
    .await;

    insert_translation(&pool, product.id(), "de", Some("Deutscher Titel"), None).await;
    insert_translation(
        &pool,
        product.id(),
        "en",
        None,
        Some("Translated description"),
    )
    .await;

    let view = find_details(
        &pool,
        ProductListingDetailsReadRequest {
            lookup: ProductListingLookup::ById(product.id()),
            language: Language::De,
            user_id: None,
        },
    )
    .await;

    assert_localized_title(view.product_title.as_ref(), Language::En, "Original title");
    assert_localized_description(
        view.product_description.as_ref(),
        Language::En,
        "Original description",
    );
    assert!(view.sale_observation.is_none());
    assert_localized_title(view.title.as_ref(), Language::De, "Deutscher Titel");
    assert_localized_description(
        view.description.as_ref(),
        Language::En,
        "Translated description",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_derive_partnerize_view_url_without_changing_raw_product_url() {
    let pool = get_postgres_client().await;
    let product = persist_product(&pool, "details-referral", None, None).await;
    let config = serde_json::json!({"kind": "PARTNERIZE", "camref": "campaign"});
    let result = sqlx::query(
        "UPDATE listing_sources SET referral_configuration = $1 WHERE listing_source_id = $2",
    )
    .bind(config)
    .bind(uuid::Uuid::from(product.listing_source_id()))
    .execute(&pool)
    .await;
    if let Err(error) = result {
        panic!("failed to configure listing-source referral: {error}");
    }

    let view = find_details(
        &pool,
        ProductListingDetailsReadRequest {
            lookup: ProductListingLookup::ById(product.id()),
            language: Language::En,
            user_id: None,
        },
    )
    .await;

    assert_eq!("https://example.com/details-referral", view.url.as_str());
    assert_eq!(
        "https://prf.hn/click/camref:campaign/pubref:aurahistoria/destination:https%3A%2F%2Fexample.com%2Fdetails-referral",
        view.view_url.as_str(),
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_fall_back_to_de_then_deterministic_remaining_translation_for_slug_lookup() {
    let pool = get_postgres_client().await;
    let listing_source_slug = "details-fallback-source";
    let product =
        persist_product_with_listing_source_slug(&pool, "details-fallback", listing_source_slug)
            .await;

    insert_translation(&pool, product.id(), "de", Some("Deutscher Titel"), None).await;
    insert_translation(
        &pool,
        product.id(),
        "fr",
        Some("Titre français"),
        Some("Description française"),
    )
    .await;
    insert_translation(
        &pool,
        product.id(),
        "es",
        Some("Título español"),
        Some("Descripción española"),
    )
    .await;

    let view = find_details(
        &pool,
        ProductListingDetailsReadRequest {
            lookup: ProductListingLookup::ByTitleSlug(product.title_slug_id().clone()),
            language: Language::It,
            user_id: None,
        },
    )
    .await;

    assert!(view.product_title.is_none());
    assert!(view.product_description.is_none());
    assert_localized_title(view.title.as_ref(), Language::De, "Deutscher Titel");
    assert_localized_description(
        view.description.as_ref(),
        Language::Es,
        "Descripción española",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_original_english_text_when_no_translation_has_selected_text() {
    let pool = get_postgres_client().await;
    let product = persist_product(
        &pool,
        "details-original-fallback",
        Some(Localized::new(Language::En, Title::from("Original title"))),
        Some(Localized::new(
            Language::En,
            Description::from("Original description"),
        )),
    )
    .await;

    insert_translation(&pool, product.id(), "de", Some("Deutscher Titel"), None).await;

    let view = find_details(
        &pool,
        ProductListingDetailsReadRequest {
            lookup: ProductListingLookup::ById(product.id()),
            language: Language::Fr,
            user_id: None,
        },
    )
    .await;

    assert_localized_title(view.title.as_ref(), Language::En, "Original title");
    assert_localized_description(
        view.description.as_ref(),
        Language::En,
        "Original description",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_no_selected_text_when_product_has_no_stored_or_translated_text() {
    let pool = get_postgres_client().await;
    let product = persist_product(&pool, "details-no-text", None, None).await;

    let view = find_details(
        &pool,
        ProductListingDetailsReadRequest {
            lookup: ProductListingLookup::ById(product.id()),
            language: Language::En,
            user_id: None,
        },
    )
    .await;

    assert!(view.product_title.is_none());
    assert!(view.product_description.is_none());
    assert!(view.title.is_none());
    assert!(view.description.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_none_when_product_is_missing() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let details = SqlxProductListingDetailsReaderFactory::new();
    let mut tx = begin(&unit_of_work).await;
    let result = details
        .in_transaction(&mut tx)
        .find_details(&ProductListingDetailsReadRequest {
            lookup: ProductListingLookup::ById(ProductListingId::new()),
            language: Language::En,
            user_id: None,
        })
        .await;
    commit(tx).await;

    match result {
        Ok(None) => {}
        Ok(Some(_)) => panic!("found missing product details"),
        Err(error) => panic!("failed to query missing product details: {error:?}"),
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_join_all_postgres_user_state_sections_for_authenticated_user() {
    let pool = get_postgres_client().await;
    let product = persist_product(&pool, "details-user-state", None, None).await;
    let user_id = seed_user(&pool, "FREE", false).await;
    let filter_id = UserSearchFilterId::new();
    let match_created = OffsetDateTime::UNIX_EPOCH + Duration::days(10);

    insert_watchlist(&pool, user_id, product.id(), false, "INACTIVE_BY_USER").await;
    insert_search_filter(&pool, user_id, filter_id, "Vintage furniture").await;
    insert_search_filter_match(
        &pool,
        user_id,
        filter_id,
        product.id(),
        event_id_for_product(&pool, product.id()).await,
        "Vintage furniture",
        Some("Matches the requested vintage furniture."),
        Some(true),
        match_created,
    )
    .await;
    let notification_id = NotificationId::new();
    let notification = sqlx::query(
        r#"
        INSERT INTO notifications (
            notification_id, user_id, kind, origin_event_id, product_listing_id, payload, seen
        ) VALUES ($1, $2, 'WATCHLIST_AVAILABILITY_CHANGED', $3, $4, $5, false)
        "#,
    )
    .bind(uuid::Uuid::from(notification_id))
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::new_v4())
    .bind(uuid::Uuid::from(product.id()))
    .bind(serde_json::json!({}))
    .execute(&pool)
    .await;
    if let Err(error) = notification {
        panic!("failed to seed notification: {error}");
    }

    let view = find_personalized_details(&pool, details_request(product.id(), Some(user_id))).await;
    let user_state = view.user_state.unwrap_or_default();

    assert!(user_state.watchlist.watching);
    assert!(!user_state.watchlist.notifications);
    assert!(
        !user_state
            .content_visibility
            .show_unassessed_or_sensitive_content
    );
    assert_eq!(
        vec![notification_id],
        user_state.notification.unseen_notification_ids
    );
    assert!(user_state.search_filter.matched);
    assert!(!user_state.search_filter.hidden);
    assert_eq!(
        Some(filter_id),
        user_state.search_filter.user_search_filter_id
    );
    assert_eq!(
        Some("Vintage furniture"),
        user_state
            .search_filter
            .user_search_filter_name
            .as_ref()
            .map(AsRef::as_ref)
    );
    assert_eq!(
        Some("Matches the requested vintage furniture."),
        user_state
            .search_filter
            .match_reason
            .as_ref()
            .map(AsRef::as_ref)
    );
    assert_eq!(Some(true), user_state.search_filter.match_feedback);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_default_postgres_user_state_when_no_watchlist_or_match_exists() {
    let pool = get_postgres_client().await;
    let product = persist_product(&pool, "details-user-state-default", None, None).await;
    let user_id = seed_user(&pool, "FREE", false).await;

    let view = find_personalized_details(&pool, details_request(product.id(), Some(user_id))).await;
    let user_state = view.user_state.unwrap_or_default();

    assert!(!user_state.watchlist.watching);
    assert!(!user_state.watchlist.notifications);
    assert!(
        !user_state
            .content_visibility
            .show_unassessed_or_sensitive_content
    );
    assert!(!user_state.search_filter.matched);
    assert!(!user_state.search_filter.hidden);
    assert_eq!(None, user_state.search_filter.user_search_filter_id);
    assert_eq!(None, user_state.search_filter.user_search_filter_name);
    assert_eq!(None, user_state.search_filter.match_reason);
    assert_eq!(None, user_state.search_filter.match_feedback);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_product_personalization_when_authenticated_user_is_missing() {
    let pool = get_postgres_client().await;
    let product = persist_product(&pool, "details-missing-user", None, None).await;

    let result =
        find_details_result(&pool, details_request(product.id(), Some(UserId::new()))).await;

    assert!(matches!(
        result,
        Err(
            product_listing_service::ports::ProductListingDetailsReadError::ProductListingDetailsReadModelInvalid
        )
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_hide_the_eleventh_free_tier_match_in_its_month() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "FREE", false).await;
    let filter_id = UserSearchFilterId::new();
    let month_start = OffsetDateTime::UNIX_EPOCH + Duration::days(31);
    insert_search_filter(&pool, user_id, filter_id, "Free quota").await;

    for index in 0..10 {
        let product =
            persist_product(&pool, &format!("details-free-before-{index}"), None, None).await;
        insert_search_filter_match(
            &pool,
            user_id,
            filter_id,
            product.id(),
            event_id_for_product(&pool, product.id()).await,
            "Free quota",
            None,
            None,
            month_start + Duration::hours(i64::from(index)),
        )
        .await;
    }

    let product = persist_product(&pool, "details-free-hidden", None, None).await;
    insert_search_filter_match(
        &pool,
        user_id,
        filter_id,
        product.id(),
        event_id_for_product(&pool, product.id()).await,
        "Free quota",
        None,
        None,
        month_start + Duration::hours(10),
    )
    .await;

    let view = find_personalized_details(&pool, details_request(product.id(), Some(user_id))).await;
    let user_state = view.user_state.unwrap_or_default();

    assert!(user_state.search_filter.matched);
    assert!(user_state.search_filter.hidden);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_keep_first_tied_free_tier_match_visible() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "FREE", false).await;
    let timestamp = OffsetDateTime::UNIX_EPOCH + Duration::days(61);
    let target = persist_product(&pool, "details-free-tied-target", None, None).await;
    let target_filter_id = UserSearchFilterId::from(uuid::Uuid::from_u128(1));
    insert_search_filter(&pool, user_id, target_filter_id, "First tied filter").await;
    insert_search_filter_match(
        &pool,
        user_id,
        target_filter_id,
        target.id(),
        event_id_for_product(&pool, target.id()).await,
        "First tied filter",
        None,
        None,
        timestamp,
    )
    .await;

    for index in 2_u128..=11 {
        let product =
            persist_product(&pool, &format!("details-free-tied-{index}"), None, None).await;
        let filter_id = UserSearchFilterId::from(uuid::Uuid::from_u128(index));
        insert_search_filter(&pool, user_id, filter_id, &format!("Tied filter {index}")).await;
        insert_search_filter_match(
            &pool,
            user_id,
            filter_id,
            product.id(),
            event_id_for_product(&pool, product.id()).await,
            &format!("Tied filter {index}"),
            None,
            None,
            timestamp,
        )
        .await;
    }

    let view = find_personalized_details(&pool, details_request(target.id(), Some(user_id))).await;
    let search_filter = view.user_state.unwrap_or_default().search_filter;

    assert!(search_filter.matched);
    assert!(!search_filter.hidden);
    assert_eq!(Some(target_filter_id), search_filter.user_search_filter_id);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_not_hide_matched_product_for_unlimited_tier() {
    let pool = get_postgres_client().await;
    let product = persist_product(&pool, "details-pro-visible", None, None).await;
    let user_id = seed_user(&pool, "PRO", false).await;
    let filter_id = UserSearchFilterId::new();
    insert_search_filter(&pool, user_id, filter_id, "Pro quota").await;
    insert_search_filter_match(
        &pool,
        user_id,
        filter_id,
        product.id(),
        event_id_for_product(&pool, product.id()).await,
        "Pro quota",
        None,
        None,
        OffsetDateTime::UNIX_EPOCH,
    )
    .await;

    let view = find_personalized_details(&pool, details_request(product.id(), Some(user_id))).await;

    assert!(!view.user_state.unwrap_or_default().search_filter.hidden);

    let ultimate_user_id = seed_user(&pool, "ULTIMATE", false).await;
    let ultimate_filter_id = UserSearchFilterId::new();
    insert_search_filter(
        &pool,
        ultimate_user_id,
        ultimate_filter_id,
        "Ultimate quota",
    )
    .await;
    insert_search_filter_match(
        &pool,
        ultimate_user_id,
        ultimate_filter_id,
        product.id(),
        event_id_for_product(&pool, product.id()).await,
        "Ultimate quota",
        None,
        None,
        OffsetDateTime::UNIX_EPOCH,
    )
    .await;

    let ultimate_view =
        find_personalized_details(&pool, details_request(product.id(), Some(ultimate_user_id)))
            .await;

    assert!(
        !ultimate_view
            .user_state
            .unwrap_or_default()
            .search_filter
            .hidden
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_select_earliest_search_filter_match_deterministically() {
    let pool = get_postgres_client().await;
    let product = persist_product(&pool, "details-match-selection", None, None).await;
    let user_id = seed_user(&pool, "FREE", false).await;
    let earlier_filter_id = UserSearchFilterId::from(uuid::Uuid::from_u128(1));
    let later_filter_id = UserSearchFilterId::from(uuid::Uuid::from_u128(2));
    let event_id = event_id_for_product(&pool, product.id()).await;
    insert_search_filter(&pool, user_id, later_filter_id, "Later filter").await;
    insert_search_filter(&pool, user_id, earlier_filter_id, "Earlier filter").await;
    insert_search_filter_match(
        &pool,
        user_id,
        later_filter_id,
        product.id(),
        event_id,
        "Later filter",
        None,
        None,
        OffsetDateTime::UNIX_EPOCH,
    )
    .await;
    insert_search_filter_match(
        &pool,
        user_id,
        earlier_filter_id,
        product.id(),
        event_id,
        "Earlier filter",
        None,
        None,
        OffsetDateTime::UNIX_EPOCH,
    )
    .await;

    let view = find_personalized_details(&pool, details_request(product.id(), Some(user_id))).await;
    let search_filter = view.user_state.unwrap_or_default().search_filter;

    assert_eq!(Some(earlier_filter_id), search_filter.user_search_filter_id);
    assert_eq!(
        Some("Earlier filter"),
        search_filter
            .user_search_filter_name
            .as_ref()
            .map(AsRef::as_ref)
    );
}

fn details_request(
    product_listing_id: ProductListingId,
    user_id: Option<UserId>,
) -> ProductListingDetailsReadRequest {
    ProductListingDetailsReadRequest {
        lookup: ProductListingLookup::ById(product_listing_id),
        language: Language::En,
        user_id,
    }
}

async fn find_details(
    pool: &sqlx::PgPool,
    request: ProductListingDetailsReadRequest,
) -> ProductListingDetailsReadModel {
    find_personalized_details(pool, request).await.item
}

async fn find_personalized_details(
    pool: &sqlx::PgPool,
    request: ProductListingDetailsReadRequest,
) -> PersonalizedProductListingDetailsReadModel {
    match find_details_result(pool, request).await {
        Ok(Some(view)) => view,
        Ok(None) => panic!("missing product details"),
        Err(error) => panic!("failed to read product details: {error:?}"),
    }
}

async fn find_details_result(
    pool: &sqlx::PgPool,
    request: ProductListingDetailsReadRequest,
) -> Result<
    Option<PersonalizedProductListingDetailsReadModel>,
    product_listing_service::ports::ProductListingDetailsReadError,
> {
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let details = SqlxProductListingDetailsReaderFactory::new();
    let mut tx = begin(&unit_of_work).await;
    let result = details.in_transaction(&mut tx).find_details(&request).await;
    commit(tx).await;
    result
}

async fn persist_product(
    pool: &sqlx::PgPool,
    slug: &str,
    title: Option<Localized<Language, Title>>,
    description: Option<Localized<Language, Description>>,
) -> ProductListing {
    let listing_source_id = seed_listing_source(pool, &format!("{slug}-source")).await;
    persist_product_for_listing_source(pool, slug, listing_source_id, title, description).await
}

async fn persist_product_with_listing_source_slug(
    pool: &sqlx::PgPool,
    slug: &str,
    listing_source_slug: &str,
) -> ProductListing {
    let listing_source_id = seed_listing_source(pool, listing_source_slug).await;
    persist_product_for_listing_source(pool, slug, listing_source_id, None, None).await
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

async fn persist_product_for_listing_source(
    pool: &sqlx::PgPool,
    slug: &str,
    listing_source_id: ListingSourceId,
    title: Option<Localized<Language, Title>>,
    description: Option<Localized<Language, Description>>,
) -> ProductListing {
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
    .bind(uuid::Uuid::from(product_listing_id))
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
    .bind(uuid::Uuid::from(user_id))
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
    state: &str,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO product_listing_watchlist (user_id, product_listing_id, notifications, state, active_since, notifications_enabled_since)
        VALUES ($1, $2, $3, $4, CASE WHEN $4 = 'ACTIVE' THEN now() ELSE NULL END, CASE WHEN $3 THEN now() ELSE NULL END)
        "#,
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(notifications)
    .bind(state)
    .execute(pool)
    .await;

    if let Err(error) = result {
        panic!("failed to seed product watchlist: {error}");
    }
}

fn uuid_from_filter_id(filter_id: UserSearchFilterId) -> uuid::Uuid {
    match uuid::Uuid::parse_str(&filter_id.to_string()) {
        Ok(value) => value,
        Err(error) => panic!("invalid user search filter ID: {error}"),
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
    .bind(uuid_from_filter_id(filter_id))
    .bind(uuid::Uuid::from(user_id))
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
    origin_event_id: EventId,
    name: &str,
    reason: Option<&str>,
    feedback: Option<bool>,
    created: OffsetDateTime,
) {
    let result = sqlx::query(
        r#"
        INSERT INTO search_filter_matches (
            user_id, user_search_filter_id, product_listing_id, origin_event_id,
            user_search_filter_name, enhanced_match_reason, feedback, created
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid_from_filter_id(filter_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(uuid::Uuid::from(origin_event_id))
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
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_one(pool)
    .await;

    match result {
        Ok(event_id) => EventId::from(event_id),
        Err(error) => panic!("failed to read product event ID: {error}"),
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
            price: Some(Price::new(MonetaryAmount::from(1_200_u64), Currency::Eur).into()),
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
    let party_id = uuid::Uuid::new_v4();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed listing-source party: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(uuid::Uuid::from(listing_source_id))
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

async fn begin(unit_of_work: &SqlxUnitOfWork) -> SqlxTransaction {
    match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    }
}

async fn commit(tx: SqlxTransaction) {
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
    }
}
