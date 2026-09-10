use application::transaction::{Transaction, UnitOfWork};
use localization::Language;
use money::Currency;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_search::{
    EnhancedSearchDescription, ProductListingSearch,
};
use search_filter_core::{
    NewSearchFilter, SearchFilter, search_filter_state::SearchFilterState,
    user_search_filter_id::UserSearchFilterId, user_search_filter_name::UserSearchFilterName,
};
use search_filter_postgres::{
    SqlxPeriodicSearchFilterCandidateReader, SqlxSearchFilterRepositoryFactory,
};
use search_filter_service::ports::{
    PeriodicSearchFilterCandidatePageRequest, PeriodicSearchFilterCandidateReader,
    SearchFilterRepository, SearchFilterRepositoryFactory,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_select_only_filters_eligible_for_closed_window_and_use_creation_fallback() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let user_id = seed_user(&pool, "periodic-candidate-eligibility@example.com").await;
    let eligible = seed_enhanced_filter(&unit, user_id, "eligible").await;
    let future = seed_enhanced_filter(&unit, user_id, "future").await;
    let window_end = OffsetDateTime::UNIX_EPOCH + Duration::hours(10);
    let eligible_created = window_end - Duration::hours(1);

    sqlx::query("UPDATE search_filters SET created = $2 WHERE user_search_filter_id = $1")
        .bind(eligible.id().into_uuid())
        .bind(eligible_created)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("set eligible creation failed: {error:?}"));
    sqlx::query("UPDATE search_filters SET created = $2 WHERE user_search_filter_id = $1")
        .bind(future.id().into_uuid())
        .bind(window_end + Duration::seconds(1))
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("set future creation failed: {error:?}"));

    let candidates = SqlxPeriodicSearchFilterCandidateReader::new(pool)
        .find_active_page(PeriodicSearchFilterCandidatePageRequest {
            after: None,
            page_size: 10,
            eligible_at_or_before: window_end,
        })
        .await
        .unwrap_or_else(|error| panic!("candidate read failed: {error:?}"));

    assert_eq!(1, candidates.len());
    assert_eq!(eligible.id(), candidates[0].search_filter_id);
    assert_eq!(eligible_created, candidates[0].matched_through);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_blank_dedicated_description_and_blank_persisted_json() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let user_id = seed_user(&pool, "periodic-candidate-blank@example.com").await;
    let filter = seed_enhanced_filter(&unit, user_id, "blank").await;
    let window_end = OffsetDateTime::now_utc() + Duration::hours(1);

    let blank_column = sqlx::query(
        "UPDATE search_filters SET enhanced_search_description = '   ' WHERE user_search_filter_id = $1",
    )
    .bind(filter.id().into_uuid())
    .execute(&pool)
    .await;
    assert!(blank_column.is_err());

    sqlx::query(
        "UPDATE search_filters SET search = jsonb_set(search, '{enhanced_search_description}', '\" \"'::jsonb) WHERE user_search_filter_id = $1",
    )
    .bind(filter.id().into_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("set malformed JSON failed: {error:?}"));

    let result = SqlxPeriodicSearchFilterCandidateReader::new(pool)
        .find_active_page(PeriodicSearchFilterCandidatePageRequest {
            after: None,
            page_size: 10,
            eligible_at_or_before: window_end,
        })
        .await;

    assert!(matches!(
        result,
        Err(search_filter_service::ports::PeriodicSearchFilterCandidateReadError::InvalidPersistedState { .. })
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_exclude_filter_whose_checkpoint_already_covers_window() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let user_id = seed_user(&pool, "periodic-candidate-covered@example.com").await;
    let filter = seed_enhanced_filter(&unit, user_id, "covered").await;
    let window_end = OffsetDateTime::UNIX_EPOCH + Duration::hours(10);

    sqlx::query("UPDATE search_filters SET created = $2 WHERE user_search_filter_id = $1")
        .bind(filter.id().into_uuid())
        .bind(window_end - Duration::hours(1))
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("set creation failed: {error:?}"));
    sqlx::query(
        "INSERT INTO search_filter_periodic_match_state (user_search_filter_id, matched_through) VALUES ($1, $2)",
    )
    .bind(filter.id().into_uuid())
    .bind(window_end)
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("seed checkpoint failed: {error:?}"));

    let candidates = SqlxPeriodicSearchFilterCandidateReader::new(pool)
        .find_active_page(PeriodicSearchFilterCandidatePageRequest {
            after: None,
            page_size: 10,
            eligible_at_or_before: window_end,
        })
        .await
        .unwrap_or_else(|error| panic!("candidate read failed: {error:?}"));

    assert!(candidates.is_empty());
}

async fn seed_enhanced_filter(unit: &SqlxUnitOfWork, user_id: UserId, name: &str) -> SearchFilter {
    let search = ProductListingSearch::new(Language::En, Currency::Eur)
        .with_enhanced_search_description(
            EnhancedSearchDescription::try_from("Find a red bicycle")
                .unwrap_or_else(|error| panic!("description invalid: {error:?}")),
        );
    let filter = SearchFilter::create(NewSearchFilter {
        user_search_filter_id: UserSearchFilterId::new(),
        user_id,
        name: UserSearchFilterName::from(name),
        notifications: true,
        state: SearchFilterState::Active,
        search,
        embedding: None,
    });
    let repository = SqlxSearchFilterRepositoryFactory;
    let mut tx = unit
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin failed: {error:?}"));
    repository
        .in_transaction(&mut tx)
        .insert(&filter)
        .await
        .unwrap_or_else(|error| panic!("insert filter failed: {error:?}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit failed: {error:?}"));
    filter
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
