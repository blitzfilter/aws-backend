use ::application::transaction::{Transaction, UnitOfWork};
use ::platform_postgres::SqlxUnitOfWork;
use ::user_core::user_id::UserId;
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
use user_postgres::{
    SqlxAccessTokenAuthenticationReader, SqlxAccessTokenDetailsReader, SqlxAccessTokenListReader,
    SqlxAccessTokenRepositoryFactory, SqlxUserRepositoryFactory,
};
use user_service::ports::{
    AccessTokenAuthenticationReader, AccessTokenDetailsReader, AccessTokenListReader,
    AccessTokenRepository, AccessTokenRepositoryError, AccessTokenRepositoryFactory,
    UserRepository, UserRepositoryFactory,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_find_by_id_find_by_hash_and_update_access_token_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let tokens = SqlxAccessTokenRepositoryFactory::new();
    let user = sample_user("access-token-repository");
    let raw = RawAccessToken::new();
    let mut token = sample_access_token(
        user.id(),
        raw.clone(),
        "repository token",
        HashSet::from([Scope::UsersRead]),
        None,
    );

    let mut tx = begin(&unit_of_work).await;
    insert_user(&users, &mut tx, &user).await;
    let inserted = insert_token(&tokens, &mut tx, &token).await;
    let loaded_by_id = find_token_by_id(&tokens, &mut tx, user.id(), token.id()).await;
    let loaded_by_hash = find_token_by_hash(&tokens, &mut tx, token.hashed_token()).await;

    assert_access_token_state(&token, &inserted.value);
    assert_access_token_state(&token, &loaded_by_id.value);
    assert_access_token_state(&token, &loaded_by_hash.value);
    assert!(raw.check(loaded_by_hash.value.hashed_token()));

    assert!(token.change_name(AccessTokenName::from("renamed repository token")));
    assert!(token.replace_scopes(HashSet::from([Scope::UsersRead, Scope::AccessTokensWrite,])));
    assert!(token.change_expires(Some(OffsetDateTime::UNIX_EPOCH + Duration::days(20_000))));
    let updated = match tokens
        .in_transaction(&mut tx)
        .update(&token, loaded_by_id.version)
        .await
    {
        Ok(updated) => updated,
        Err(error) => panic!("failed to update access token: {error}"),
    };
    commit(tx).await;

    assert_access_token_state(&token, &updated.value);
    assert!(updated.version.into_inner() > loaded_by_id.version.into_inner());
}

fn assert_access_token_state(expected: &AccessToken, actual: &AccessToken) {
    assert_eq!(expected.id(), actual.id());
    assert_eq!(expected.user_id(), actual.user_id());
    assert_eq!(expected.name(), actual.name());
    assert_eq!(expected.scopes(), actual.scopes());
    assert_eq!(expected.origin(), actual.origin());
    assert_eq!(expected.expires(), actual.expires());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_stale_access_token_update() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let tokens = SqlxAccessTokenRepositoryFactory::new();
    let user = sample_user("access-token-stale");
    let token = sample_access_token(
        user.id(),
        RawAccessToken::new(),
        "stale token",
        HashSet::from([Scope::UsersRead]),
        None,
    );

    let mut tx = begin(&unit_of_work).await;
    insert_user(&users, &mut tx, &user).await;
    insert_token(&tokens, &mut tx, &token).await;
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let loaded = find_token_by_id(&tokens, &mut tx, user.id(), token.id()).await;
    commit(tx).await;

    let mut updated = loaded.value.clone();
    assert!(updated.change_name(AccessTokenName::from("fresh token name")));
    let mut tx = begin(&unit_of_work).await;
    let persisted = match tokens
        .in_transaction(&mut tx)
        .update(&updated, loaded.version)
        .await
    {
        Ok(updated) => updated,
        Err(error) => panic!("failed to apply first access token update: {error}"),
    };
    commit(tx).await;

    let mut stale_tx = begin(&unit_of_work).await;
    let stale = tokens
        .in_transaction(&mut stale_tx)
        .update(&updated, loaded.version)
        .await;
    drop(stale_tx);

    assert!(matches!(
        stale,
        Err(AccessTokenRepositoryError::ConcurrencyConflict)
    ));
    assert!(persisted.version.into_inner() > loaded.version.into_inner());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_delete_access_token_and_report_missing_token() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let tokens = SqlxAccessTokenRepositoryFactory::new();
    let user = sample_user("access-token-delete");
    let token = sample_access_token(
        user.id(),
        RawAccessToken::new(),
        "delete token",
        HashSet::from([Scope::AccessTokensWrite]),
        None,
    );

    let mut tx = begin(&unit_of_work).await;
    insert_user(&users, &mut tx, &user).await;
    insert_token(&tokens, &mut tx, &token).await;
    let deleted = match tokens
        .in_transaction(&mut tx)
        .delete_by_id(user.id(), token.id())
        .await
    {
        Ok(deleted) => deleted,
        Err(error) => panic!("failed to delete access token: {error}"),
    };
    let missing = tokens
        .in_transaction(&mut tx)
        .find_by_id(user.id(), token.id())
        .await;
    let missing_delete = tokens
        .in_transaction(&mut tx)
        .delete_by_id(user.id(), AccessTokenId::new())
        .await;
    commit(tx).await;

    assert!(deleted);
    assert!(matches!(missing, Ok(None)));
    assert!(matches!(missing_delete, Ok(false)));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_delete_all_access_tokens_for_target_user_without_touching_other_users() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let tokens = SqlxAccessTokenRepositoryFactory::new();
    let target = sample_user("access-token-bulk-target");
    let unrelated = sample_user("access-token-bulk-unrelated");
    let first_target_token = sample_access_token(
        target.id(),
        RawAccessToken::new(),
        "first bulk token",
        HashSet::from([Scope::UsersRead]),
        None,
    );
    let second_target_token = sample_access_token(
        target.id(),
        RawAccessToken::new(),
        "second bulk token",
        HashSet::from([Scope::AccessTokensRead]),
        None,
    );
    let unrelated_token = sample_access_token(
        unrelated.id(),
        RawAccessToken::new(),
        "unrelated bulk token",
        HashSet::from([Scope::UsersRead]),
        None,
    );

    let mut tx = begin(&unit_of_work).await;
    insert_user(&users, &mut tx, &target).await;
    insert_user(&users, &mut tx, &unrelated).await;
    insert_token(&tokens, &mut tx, &first_target_token).await;
    insert_token(&tokens, &mut tx, &second_target_token).await;
    insert_token(&tokens, &mut tx, &unrelated_token).await;
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let deleted = match tokens
        .in_transaction(&mut tx)
        .delete_by_user_id(target.id())
        .await
    {
        Ok(deleted) => deleted,
        Err(error) => panic!("failed to bulk delete access tokens: {error}"),
    };
    let first_target_remaining = tokens
        .in_transaction(&mut tx)
        .find_by_id(target.id(), first_target_token.id())
        .await;
    let second_target_remaining = tokens
        .in_transaction(&mut tx)
        .find_by_id(target.id(), second_target_token.id())
        .await;
    let unrelated_remaining = tokens
        .in_transaction(&mut tx)
        .find_by_id(unrelated.id(), unrelated_token.id())
        .await;
    commit(tx).await;

    assert_eq!(2, deleted);
    assert!(matches!(first_target_remaining, Ok(None)));
    assert!(matches!(second_target_remaining, Ok(None)));
    assert!(matches!(unrelated_remaining, Ok(Some(_))));

    let mut retry_tx = begin(&unit_of_work).await;
    let retried = match tokens
        .in_transaction(&mut retry_tx)
        .delete_by_user_id(target.id())
        .await
    {
        Ok(deleted) => deleted,
        Err(error) => panic!("failed to retry bulk delete of access tokens: {error}"),
    };
    commit(retry_tx).await;

    assert_eq!(0, retried);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_rollback_access_token_insert_when_transaction_drops() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let tokens = SqlxAccessTokenRepositoryFactory::new();
    let user = sample_user("access-token-rollback");
    let token = sample_access_token(
        user.id(),
        RawAccessToken::new(),
        "rollback token",
        HashSet::from([Scope::UsersRead]),
        None,
    );

    let mut tx = begin(&unit_of_work).await;
    insert_user(&users, &mut tx, &user).await;
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    insert_token(&tokens, &mut tx, &token).await;
    drop(tx);

    let mut tx = begin(&unit_of_work).await;
    let missing = tokens
        .in_transaction(&mut tx)
        .find_by_id(user.id(), token.id())
        .await;
    commit(tx).await;

    assert!(matches!(missing, Ok(None)));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_access_token_details_list_and_authentication_deterministically() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let users = SqlxUserRepositoryFactory::new();
    let tokens = SqlxAccessTokenRepositoryFactory::new();
    let details = SqlxAccessTokenDetailsReader::new(pool.clone());
    let list = SqlxAccessTokenListReader::new(pool.clone());
    let authentication = SqlxAccessTokenAuthenticationReader::new(pool);
    let user = sample_user("access-token-readers");
    let first_raw = RawAccessToken::new();
    let first = sample_access_token(
        user.id(),
        first_raw.clone(),
        "first reader token",
        HashSet::from([Scope::UsersRead]),
        None,
    );
    let second = sample_access_token(
        user.id(),
        RawAccessToken::new(),
        "second reader token",
        HashSet::from([Scope::AccessTokensRead]),
        Some(OffsetDateTime::now_utc() + Duration::hours(2)),
    );

    let mut tx = begin(&unit_of_work).await;
    insert_user(&users, &mut tx, &user).await;
    insert_token(&tokens, &mut tx, &first).await;
    insert_token(&tokens, &mut tx, &second).await;
    commit(tx).await;

    let detail = match details.find_by_id(user.id(), first.id()).await {
        Ok(Some(detail)) => detail,
        Ok(None) => panic!("missing access token details"),
        Err(error) => panic!("failed to read access token details: {error}"),
    };
    let missing_detail = details.find_by_id(UserId::new(), first.id()).await;
    let first_list = match list.list_for_user(user.id()).await {
        Ok(tokens) => tokens,
        Err(error) => panic!("failed to list access tokens: {error}"),
    };
    let second_list = match list.list_for_user(user.id()).await {
        Ok(tokens) => tokens,
        Err(error) => panic!("failed to list access tokens again: {error}"),
    };
    let authenticated = match authentication
        .find_authentication_by_hashed_token(first.hashed_token())
        .await
    {
        Ok(Some(authentication)) => authentication,
        Ok(None) => panic!("missing access token authentication"),
        Err(error) => panic!("failed to read access token authentication: {error}"),
    };
    let missing_authentication = authentication
        .find_authentication_by_hashed_token(&RawAccessToken::new().into())
        .await;

    assert_eq!(user.id(), detail.user_id);
    assert_eq!(first.id(), detail.access_token_id);
    assert_eq!(first.name(), &detail.name);
    assert_eq!(first.scopes(), &detail.scopes);
    assert_eq!(first.origin(), &detail.origin);
    assert_eq!(first.expires(), detail.expires);
    assert!(matches!(missing_detail, Ok(None)));
    let mut expected_ids = vec![first.id(), second.id()];
    expected_ids.sort();
    assert_eq!(
        expected_ids,
        first_list
            .iter()
            .map(|token| token.access_token_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(first_list, second_list);
    assert_eq!(first.id(), authenticated.access_token_id);
    assert_eq!(user.id(), authenticated.user_id);
    assert_eq!(first.scopes(), &authenticated.scopes);
    assert_eq!(first.origin(), &authenticated.origin);
    assert_eq!(first.expires(), authenticated.expires);
    assert!(matches!(missing_authentication, Ok(None)));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_not_authenticate_access_token_for_suspended_user() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let users = SqlxUserRepositoryFactory::new();
    let tokens = SqlxAccessTokenRepositoryFactory::new();
    let authentication = SqlxAccessTokenAuthenticationReader::new(pool.clone());
    let user = sample_user("access-token-suspended-user");
    let token = sample_access_token(
        user.id(),
        RawAccessToken::new(),
        "suspended user token",
        HashSet::from([Scope::UsersRead]),
        None,
    );

    let mut tx = begin(&unit_of_work).await;
    insert_user(&users, &mut tx, &user).await;
    insert_token(&tokens, &mut tx, &token).await;
    commit(tx).await;

    let active = authentication
        .find_authentication_by_hashed_token(token.hashed_token())
        .await;
    if let Err(error) = sqlx::query("UPDATE users SET suspended = true WHERE user_id = $1")
        .bind(uuid::Uuid::from(user.id()))
        .execute(&pool)
        .await
    {
        panic!("failed to suspend token user: {error}");
    }
    let suspended = authentication
        .find_authentication_by_hashed_token(token.hashed_token())
        .await;

    assert!(matches!(active, Ok(Some(_))));
    assert!(matches!(suspended, Ok(None)));
}

fn sample_access_token(
    user_id: UserId,
    raw: RawAccessToken,
    name: &str,
    scopes: HashSet<Scope>,
    expires: Option<OffsetDateTime>,
) -> AccessToken {
    AccessToken::create(NewAccessToken {
        id: AccessTokenId::new(),
        hashed_token: raw.into(),
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
        Err(error) => panic!("failed to create user: {error}"),
    }
}

async fn insert_user(
    users: &SqlxUserRepositoryFactory,
    tx: &mut ::platform_postgres::SqlxTransaction,
    user: &User,
) {
    if let Err(error) = users.in_transaction(tx).insert(user).await {
        panic!("failed to seed user through repository: {error}");
    }
}

async fn insert_token(
    tokens: &SqlxAccessTokenRepositoryFactory,
    tx: &mut ::platform_postgres::SqlxTransaction,
    token: &AccessToken,
) -> user_service::ports::VersionedAccessToken {
    match tokens.in_transaction(tx).insert(token).await {
        Ok(token) => token,
        Err(error) => panic!("failed to insert access token: {error}"),
    }
}

async fn find_token_by_id(
    tokens: &SqlxAccessTokenRepositoryFactory,
    tx: &mut ::platform_postgres::SqlxTransaction,
    user_id: UserId,
    access_token_id: AccessTokenId,
) -> user_service::ports::VersionedAccessToken {
    match tokens
        .in_transaction(tx)
        .find_by_id(user_id, access_token_id)
        .await
    {
        Ok(Some(token)) => token,
        Ok(None) => panic!("missing access token by id"),
        Err(error) => panic!("failed to find access token by id: {error}"),
    }
}

async fn find_token_by_hash(
    tokens: &SqlxAccessTokenRepositoryFactory,
    tx: &mut ::platform_postgres::SqlxTransaction,
    hashed_token: &user_core::access_token::HashedRawAccessToken,
) -> user_service::ports::VersionedAccessToken {
    match tokens
        .in_transaction(tx)
        .find_by_hashed_token(hashed_token)
        .await
    {
        Ok(Some(token)) => token,
        Ok(None) => panic!("missing access token by hash"),
        Err(error) => panic!("failed to find access token by hash: {error}"),
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
        Err(error) => panic!("failed to begin transaction: {error}"),
    }
}

async fn commit(tx: ::platform_postgres::SqlxTransaction) {
    if let Err(error) = tx.commit().await {
        panic!("failed to commit transaction: {error}");
    }
}
