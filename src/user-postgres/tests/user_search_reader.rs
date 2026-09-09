use ::application::pagination::Cursor;
use ::application::transaction::{Transaction, UnitOfWork};
use ::domain_primitives::query::range_query::RangeQuery;
use ::domain_primitives::query::text_query::TextQuery;
use ::domain_primitives::sort::{Sort, SortOrder};
use ::platform_postgres::SqlxUnitOfWork;
use ::user_core::stripe_customer_id::StripeCustomerId;
use ::user_core::user_id::UserId;
use localization::Language;
use money::Currency;
use serde_email::Email;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use user_core::first_name::FirstName;
use user_core::last_name::LastName;
use user_core::measurement_unit::MeasurementUnit;
use user_core::role::UserRole;
use user_core::sort_user_field::SortUserField;
use user_core::tier::UserTier;
use user_core::user::{NewUser, User, UserAccount, UserPreferences, UserProfile};
use user_core::user_search::UserSearch;
use user_postgres::{SqlxUserRepositoryFactory, SqlxUserSearchReaderFactory};
use user_service::ports::{
    UserRepository, UserRepositoryFactory, UserSearchReader, UserSearchReaderFactory,
};
use user_service::use_cases::queries::search_users::{SearchUsersRequest, UserSummary};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_search_users_with_filters_and_sorting_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let search = SqlxUserSearchReaderFactory::new();
    let matching = sample_user(
        "postgres-search-match",
        UserRole::Admin,
        Some("cus_postgres_search"),
    );
    let other = sample_user("postgres-search-other", UserRole::User, None);

    let mut tx = begin(&unit_of_work).await;
    for user in [&matching, &other] {
        match users.in_transaction(&mut tx).insert(user).await {
            Ok(_) => {}
            Err(error) => panic!("failed to insert search user: {error:?}"),
        }
    }
    let result = search_users(
        &search,
        &mut tx,
        SearchUsersRequest {
            search: UserSearch {
                query: Some(text_query("match")),
                email_query: Some(text_query("postgres-search-match")),
                first_name_query: Some(text_query("Ada")),
                last_name_query: Some(text_query("Lovelace")),
                tier_query: std::collections::HashSet::from([UserTier::Pro]).into(),
                role_query: std::collections::HashSet::from([UserRole::Admin]).into(),
                ..Default::default()
            },
            sort: Some(Sort {
                sort: SortUserField::Email,
                order: SortOrder::Asc,
            }),
            cursor: Some(Cursor {
                search_after: None,
                size: 1000,
            }),
        },
    )
    .await;
    commit(tx).await;

    assert_eq!(1, result.items.len());
    assert_eq!(matching.id(), result.items[0].user_id);
    assert_eq!(100, result.cursor.size);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_search_users_by_remaining_filters_dates_sorts_and_cursor_size() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let users = SqlxUserRepositoryFactory::new();
    let search = SqlxUserSearchReaderFactory::new();
    let admin = sample_user(
        "postgres-search-admin",
        UserRole::Admin,
        Some("cus_search_admin"),
    );
    let ultimate = sample_user_with_profile(
        "postgres-search-ultimate",
        UserRole::User,
        UserTier::Ultimate,
        None,
        Some("Ada"),
        Some("Lovelace"),
    );
    let free = sample_user_with_profile(
        "postgres-search-free",
        UserRole::User,
        UserTier::Free,
        None,
        None,
        None,
    );
    let created = datetime("2024-01-02T03:04:05Z");
    let updated = datetime("2024-01-03T03:04:05Z");

    let mut tx = begin(&unit_of_work).await;
    for user in [&admin, &ultimate, &free] {
        match users.in_transaction(&mut tx).insert(user).await {
            Ok(_) => {}
            Err(error) => panic!("failed to insert search user: {error:?}"),
        }
    }
    commit(tx).await;
    set_user_dates(&pool, admin.id(), created, updated).await;

    let mut tx = begin(&unit_of_work).await;
    let continent_result = search_users(
        &search,
        &mut tx,
        SearchUsersRequest {
            search: UserSearch {
                created: Some(RangeQuery {
                    min: Some(datetime("2024-01-01T00:00:00Z")),
                    max: Some(datetime("2024-01-02T23:59:59Z")),
                }),
                updated: Some(RangeQuery {
                    min: Some(datetime("2024-01-03T00:00:00Z")),
                    max: Some(datetime("2024-01-03T23:59:59Z")),
                }),
                ..Default::default()
            },
            sort: Some(Sort {
                sort: SortUserField::Name,
                order: SortOrder::Desc,
            }),
            cursor: Some(Cursor {
                search_after: None,
                size: 250,
            }),
        },
    )
    .await;
    let tier_role_result = search_users(
        &search,
        &mut tx,
        SearchUsersRequest {
            search: UserSearch {
                tier_query: std::collections::HashSet::from([UserTier::Ultimate]).into(),
                role_query: std::collections::HashSet::from([UserRole::User]).into(),
                ..Default::default()
            },
            sort: Some(Sort {
                sort: SortUserField::Tier,
                order: SortOrder::Asc,
            }),
            cursor: Some(Cursor {
                search_after: None,
                size: 5,
            }),
        },
    )
    .await;
    let empty_result = search_users(
        &search,
        &mut tx,
        SearchUsersRequest {
            search: UserSearch {
                first_name_query: Some(text_query("Nobody")),
                ..Default::default()
            },
            sort: Some(Sort {
                sort: SortUserField::Updated,
                order: SortOrder::Asc,
            }),
            cursor: None,
        },
    )
    .await;
    commit(tx).await;

    assert_eq!(vec![admin.id()], user_ids(&continent_result.items));
    assert_eq!(100, continent_result.cursor.size);
    assert!(continent_result.cursor.search_after.is_none());
    assert_eq!(vec![ultimate.id()], user_ids(&tier_role_result.items));
    assert!(empty_result.items.is_empty());
    assert_eq!(21, empty_result.cursor.size);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_sort_users_by_each_user_sort_field() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let users = SqlxUserRepositoryFactory::new();
    let search = SqlxUserSearchReaderFactory::new();
    let a = sample_user_named(
        "postgres-sort-a",
        "Ann",
        "Able",
        UserRole::Admin,
        UserTier::Free,
    );
    let b = sample_user_named(
        "postgres-sort-b",
        "Bob",
        "Baker",
        UserRole::User,
        UserTier::Pro,
    );
    let c = sample_user_named(
        "postgres-sort-c",
        "Cal",
        "Clark",
        UserRole::User,
        UserTier::Ultimate,
    );

    let mut tx = begin(&unit_of_work).await;
    for user in [&b, &c, &a] {
        match users.in_transaction(&mut tx).insert(user).await {
            Ok(_) => {}
            Err(error) => panic!("failed to insert sort user: {error:?}"),
        }
    }
    commit(tx).await;
    set_user_dates(
        &pool,
        a.id(),
        datetime("2024-02-01T00:00:00Z"),
        datetime("2024-02-03T00:00:00Z"),
    )
    .await;
    set_user_dates(
        &pool,
        b.id(),
        datetime("2024-02-02T00:00:00Z"),
        datetime("2024-02-02T00:00:00Z"),
    )
    .await;
    set_user_dates(
        &pool,
        c.id(),
        datetime("2024-02-03T00:00:00Z"),
        datetime("2024-02-01T00:00:00Z"),
    )
    .await;

    let mut tx = begin(&unit_of_work).await;
    let default_result = search_users(
        &search,
        &mut tx,
        SearchUsersRequest {
            search: UserSearch {
                query: Some(text_query("postgres-sort")),
                ..Default::default()
            },
            sort: None,
            cursor: Some(Cursor {
                search_after: None,
                size: 10,
            }),
        },
    )
    .await;
    assert_eq!(
        vec![a.id(), b.id(), c.id()],
        user_ids(&default_result.items)
    );

    let mut user_role_tie = [b.id(), c.id()];
    user_role_tie.sort();
    for (field, order, expected) in [
        (
            SortUserField::Name,
            SortOrder::Desc,
            vec![c.id(), b.id(), a.id()],
        ),
        (
            SortUserField::Email,
            SortOrder::Asc,
            vec![a.id(), b.id(), c.id()],
        ),
        (
            SortUserField::FirstName,
            SortOrder::Desc,
            vec![c.id(), b.id(), a.id()],
        ),
        (
            SortUserField::LastName,
            SortOrder::Asc,
            vec![a.id(), b.id(), c.id()],
        ),
        (
            SortUserField::Role,
            SortOrder::Asc,
            vec![a.id(), user_role_tie[0], user_role_tie[1]],
        ),
        (
            SortUserField::Created,
            SortOrder::Desc,
            vec![c.id(), b.id(), a.id()],
        ),
        (
            SortUserField::Updated,
            SortOrder::Asc,
            vec![c.id(), b.id(), a.id()],
        ),
    ] {
        let result = search_users(
            &search,
            &mut tx,
            SearchUsersRequest {
                search: UserSearch {
                    query: Some(text_query("postgres-sort")),
                    ..Default::default()
                },
                sort: Some(Sort { sort: field, order }),
                cursor: Some(Cursor {
                    search_after: None,
                    size: 10,
                }),
            },
        )
        .await;
        assert_eq!(expected, user_ids(&result.items));
    }
    commit(tx).await;
}

fn sample_user(slug: &str, role: UserRole, stripe_customer_id: Option<&str>) -> User {
    sample_user_with_profile(
        slug,
        role,
        UserTier::Pro,
        stripe_customer_id,
        Some("Ada"),
        Some("Lovelace"),
    )
}

fn sample_user_named(
    slug: &str,
    first_name: &str,
    last_name: &str,
    role: UserRole,
    tier: UserTier,
) -> User {
    sample_user_with_profile(slug, role, tier, None, Some(first_name), Some(last_name))
}

fn sample_user_with_profile(
    slug: &str,
    role: UserRole,
    tier: UserTier,
    stripe_customer_id: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
) -> User {
    match User::create(NewUser {
        id: UserId::new(),
        email: email(&format!("{slug}@example.com")),
        profile: UserProfile {
            first_name: first_name.map(FirstName::from),
            last_name: last_name.map(LastName::from),
        },
        preferences: UserPreferences {
            language: Some(Language::En),
            currency: Some(Currency::Gbp),
            measurement_unit: Some(MeasurementUnit::Imperial),
            show_unassessed_or_sensitive_content: true,
        },
        account: UserAccount {
            tier,
            role,
            stripe_customer_id: stripe_customer_id.map(StripeCustomerId::from),
        },
    }) {
        Ok(user) => user,
        Err(error) => panic!("failed to create user: {error}"),
    }
}

async fn search_users(
    search: &SqlxUserSearchReaderFactory,
    tx: &mut ::platform_postgres::SqlxTransaction,
    request: SearchUsersRequest,
) -> user_service::use_cases::queries::search_users::SearchUsersResult {
    match search.in_transaction(tx).search(&request).await {
        Ok(result) => result,
        Err(error) => panic!("failed to search users: {error:?}"),
    }
}

fn user_ids(items: &[UserSummary]) -> Vec<UserId> {
    items.iter().map(|item| item.user_id).collect()
}

async fn set_user_dates(
    pool: &sqlx::PgPool,
    user_id: UserId,
    created: OffsetDateTime,
    updated: OffsetDateTime,
) {
    let result = sqlx::query("UPDATE users SET created = $2, updated = $3 WHERE user_id = $1")
        .bind(user_id.into_uuid())
        .bind(created)
        .bind(updated)
        .execute(pool)
        .await;

    if let Err(error) = result {
        panic!("failed to set user dates: {error}");
    }
}

fn email(value: &str) -> Email {
    match Email::try_from(value) {
        Ok(email) => email,
        Err(error) => panic!("invalid test email: {error}"),
    }
}

fn text_query(value: &str) -> TextQuery<0> {
    match TextQuery::try_from(value) {
        Ok(query) => query,
        Err(error) => panic!("invalid test query: {error}"),
    }
}

fn datetime(value: &str) -> OffsetDateTime {
    match OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339) {
        Ok(value) => value,
        Err(error) => panic!("invalid test datetime: {error}"),
    }
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> ::platform_postgres::SqlxTransaction {
    match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin transaction: {error}"),
    }
}

async fn commit(tx: ::platform_postgres::SqlxTransaction) {
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
    }
}
