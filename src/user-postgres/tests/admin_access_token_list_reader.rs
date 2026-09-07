use ::application::pagination::Cursor;
use ::application::transaction::{Transaction, UnitOfWork};
use ::platform_postgres::SqlxUnitOfWork;
use localization::Language;
use money::Currency;
use serde_email::Email;
use std::collections::HashSet;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, NewAccessToken, RawAccessToken,
    Scope,
};
use user_core::first_name::FirstName;
use user_core::last_name::LastName;
use user_core::measurement_unit::MeasurementUnit;
use user_core::role::UserRole;

use user_core::tier::UserTier;
use user_core::user::{NewUser, User, UserAccount, UserPreferences, UserProfile};
use user_core::user_id::UserId;
use user_postgres::{
    SqlxAccessTokenRepositoryFactory, SqlxAdminAccessTokenListReaderFactory,
    SqlxUserRepositoryFactory,
};
use user_service::ports::{
    AccessTokenRepository, AccessTokenRepositoryFactory, AdminAccessTokenListReader,
    AdminAccessTokenListReaderFactory, UserRepository, UserRepositoryFactory,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_bounded_target_tokens_with_expired_and_current_metadata() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let tokens = SqlxAccessTokenRepositoryFactory::new();
    let reader = SqlxAdminAccessTokenListReaderFactory::new();
    let target = sample_user("admin-token-list-target");
    let unrelated = sample_user("admin-token-list-unrelated");
    let expired = sample_access_token(
        target.id(),
        "expired admin token",
        HashSet::from([Scope::UsersRead]),
        Some(OffsetDateTime::UNIX_EPOCH),
    );
    let current = sample_access_token(
        target.id(),
        "current admin token",
        HashSet::from([Scope::AccessTokensRead]),
        Some(OffsetDateTime::now_utc() + Duration::hours(1)),
    );
    let unrelated_token = sample_access_token(
        unrelated.id(),
        "unrelated admin token",
        HashSet::from([Scope::UsersRead]),
        None,
    );

    let mut tx = begin(&unit_of_work).await;
    insert_user(&users, &mut tx, &target).await;
    insert_user(&users, &mut tx, &unrelated).await;
    insert_token(&tokens, &mut tx, &expired).await;
    insert_token(&tokens, &mut tx, &current).await;
    insert_token(&tokens, &mut tx, &unrelated_token).await;

    let first_page = match reader
        .in_transaction(&mut tx)
        .list_for_user(
            target.id(),
            Cursor {
                size: 1,
                search_after: None,
            },
        )
        .await
    {
        Ok(page) => page,
        Err(error) => panic!("failed to read first admin token page: {error:?}"),
    };
    let next_cursor = first_page.cursor.search_after;
    commit(tx).await;

    assert_eq!(1, first_page.items.len());
    assert!(next_cursor.is_some());

    let mut tx = begin(&unit_of_work).await;
    let second_page = match reader
        .in_transaction(&mut tx)
        .list_for_user(
            target.id(),
            Cursor {
                size: 1,
                search_after: next_cursor,
            },
        )
        .await
    {
        Ok(page) => page,
        Err(error) => panic!("failed to read second admin token page: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(1, second_page.items.len());
    assert!(second_page.cursor.search_after.is_none());
    let listed = first_page
        .items
        .into_iter()
        .chain(second_page.items)
        .collect::<Vec<_>>();
    assert_eq!(2, listed.len());
    assert!(listed.iter().any(|token| {
        token.name == AccessTokenName::from("expired admin token")
            && token.expires == Some(OffsetDateTime::UNIX_EPOCH)
    }));
    assert!(listed.iter().any(|token| {
        token.name == AccessTokenName::from("current admin token")
            && token
                .expires
                .is_some_and(|expires| expires > OffsetDateTime::now_utc())
    }));
}

fn sample_access_token(
    user_id: UserId,
    name: &str,
    scopes: HashSet<Scope>,
    expires: Option<OffsetDateTime>,
) -> AccessToken {
    AccessToken::create(NewAccessToken {
        id: AccessTokenId::new(),
        hashed_token: RawAccessToken::new().into(),
        user_id,
        name: AccessTokenName::from(name),
        scopes,
        origin: AccessTokenOrigin::User,
        expires,
    })
}

fn sample_user(slug: &str) -> User {
    match User::create(NewUser {
        id: UserId::new(),
        email: email(&format!("{slug}@example.com")),
        profile: UserProfile {
            first_name: Some(FirstName::from("Ada")),
            last_name: Some(LastName::from("Lovelace")),
        },
        preferences: UserPreferences {
            language: Some(Language::En),
            currency: Some(Currency::Gbp),
            measurement_unit: Some(MeasurementUnit::Imperial),
            show_unassessed_or_sensitive_content: true,
        },
        account: UserAccount {
            tier: UserTier::Pro,
            role: UserRole::User,
            stripe_customer_id: None,
        },
    }) {
        Ok(user) => user,
        Err(error) => panic!("failed to create test user: {error}"),
    }
}

async fn insert_user(
    users: &SqlxUserRepositoryFactory,
    tx: &mut ::platform_postgres::SqlxTransaction,
    user: &User,
) {
    if let Err(error) = users.in_transaction(tx).insert(user).await {
        panic!("failed to insert test user: {error}");
    }
}

async fn insert_token(
    tokens: &SqlxAccessTokenRepositoryFactory,
    tx: &mut ::platform_postgres::SqlxTransaction,
    token: &AccessToken,
) {
    if let Err(error) = tokens.in_transaction(tx).insert(token).await {
        panic!("failed to insert test token: {error}");
    }
}

fn email(value: &str) -> Email {
    match Email::try_from(value) {
        Ok(email) => email,
        Err(error) => panic!("invalid test email: {error}"),
    }
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> ::platform_postgres::SqlxTransaction {
    match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin test transaction: {error}"),
    }
}

async fn commit(tx: ::platform_postgres::SqlxTransaction) {
    if let Err(error) = tx.commit().await {
        panic!("failed to commit test transaction: {error}");
    }
}
