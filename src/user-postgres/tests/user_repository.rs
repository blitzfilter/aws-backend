use ::application::transaction::{Transaction, UnitOfWork};
use ::platform_postgres::SqlxUnitOfWork;
use ::user_core::stripe_customer_id::StripeCustomerId;
use ::user_core::user_id::UserId;
use localization::Language;
use money::Currency;
use serde_email::Email;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::first_name::FirstName;
use user_core::last_name::LastName;
use user_core::measurement_unit::MeasurementUnit;
use user_core::role::UserRole;
use user_core::tier::UserTier;
use user_core::user::{
    NewUser, RehydratedUserState, User, UserAccount, UserPreferences, UserProfile,
};
use user_postgres::SqlxUserRepositoryFactory;
use user_service::ports::{
    UserInsertOutcome, UserRepository, UserRepositoryError, UserRepositoryFactory,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_find_update_user_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let mut user = sample_user("postgres-main", UserRole::Admin, Some("cus_postgres_main"));

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }
    let loaded_by_id = match users.in_transaction(&mut tx).find_by_id(user.id()).await {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing user by id"),
        Err(error) => panic!("failed to find user by id: {error:?}"),
    };
    let loaded_by_email = match users
        .in_transaction(&mut tx)
        .find_by_email(user.email())
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing user by email"),
        Err(error) => panic!("failed to find user by email: {error:?}"),
    };
    let loaded_by_stripe = match users
        .in_transaction(&mut tx)
        .find_by_stripe_customer_id(&StripeCustomerId::from("cus_postgres_main"))
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing user by stripe customer id"),
        Err(error) => panic!("failed to find user by stripe customer id: {error:?}"),
    };

    assert_eq!(user.id(), loaded_by_id.value.id());
    assert_eq!(user.id(), loaded_by_email.value.id());
    assert_eq!(user.id(), loaded_by_stripe.value.id());

    user.change_email(email("postgres-main-updated@example.com"));
    user.change_role(UserRole::User);
    user.change_tier(UserTier::Ultimate);
    user.change_stripe_customer_id(None);
    match users
        .in_transaction(&mut tx)
        .update(&user, loaded_by_id.version)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to update user: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let updated = match users.in_transaction(&mut tx).find_by_id(user.id()).await {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing updated user"),
        Err(error) => panic!("failed to find updated user: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(user.email(), updated.value.email());
    assert_eq!(UserRole::User, updated.value.account().role);
    assert_eq!(UserTier::Ultimate, updated.value.account().tier);
    assert_eq!(None, updated.value.account().stripe_customer_id);
    assert!(updated.version.into_inner() > loaded_by_id.version.into_inner());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_durably_update_and_rehydrate_user_suspension() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let user = sample_user("postgres-suspension", UserRole::User, None);

    let mut tx = begin(&unit_of_work).await;
    let inserted = match users.in_transaction(&mut tx).insert(&user).await {
        Ok(user) => user,
        Err(error) => panic!("failed to insert user: {error:?}"),
    };
    assert!(!inserted.value.is_suspended());

    let suspended_user = match User::rehydrate(RehydratedUserState {
        id: user.id(),
        email: user.email().clone(),
        profile: user.profile().clone(),
        preferences: user.preferences().clone(),
        account: user.account().clone(),
        suspended: true,
    }) {
        Ok(user) => user,
        Err(error) => panic!("failed to rehydrate suspended user: {error}"),
    };
    let updated = match users
        .in_transaction(&mut tx)
        .update(&suspended_user, inserted.version)
        .await
    {
        Ok(user) => user,
        Err(error) => panic!("failed to update suspended user: {error:?}"),
    };
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let rehydrated = match users.in_transaction(&mut tx).find_by_id(user.id()).await {
        Ok(Some(user)) => user,
        Ok(None) => panic!("missing suspended user"),
        Err(error) => panic!("failed to rehydrate suspended user: {error:?}"),
    };
    commit(tx).await;

    assert!(updated.value.is_suspended());
    assert!(rehydrated.value.is_suspended());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_user_repository_conflicts_and_missing_rows() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let user = sample_user(
        "postgres-conflict",
        UserRole::User,
        Some("cus_postgres_conflict"),
    );
    let duplicate_email = sample_user("postgres-conflict", UserRole::User, Some("cus_other"));
    let duplicate_stripe = sample_user(
        "postgres-conflict-other",
        UserRole::User,
        Some("cus_postgres_conflict"),
    );

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }
    let missing = match users
        .in_transaction(&mut tx)
        .find_by_id(UserId::new())
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed missing lookup: {error:?}"),
    };
    assert!(missing.is_none());
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let email_conflict = users.in_transaction(&mut tx).insert(&duplicate_email).await;
    assert!(matches!(
        email_conflict,
        Err(UserRepositoryError::EmailConflict { source }) if !source.to_string().is_empty()
    ));

    let mut tx = begin(&unit_of_work).await;
    let stripe_conflict = users
        .in_transaction(&mut tx)
        .insert(&duplicate_stripe)
        .await;
    assert!(matches!(
        stripe_conflict,
        Err(UserRepositoryError::StripeCustomerConflict { source }) if !source.to_string().is_empty()
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_existing_user_when_insert_if_absent_replays_user_id() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let user = sample_user("postgres-idempotent", UserRole::User, None);
    let mut changed_email = user.clone();
    changed_email.change_email(email("postgres-idempotent-changed@example.com"));

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert_if_absent(&user).await {
        Ok(UserInsertOutcome::Created(_)) => {}
        Ok(UserInsertOutcome::Existing(_)) => panic!("first insert unexpectedly found a user"),
        Err(error) => panic!("failed to create user: {error:?}"),
    }
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let same_user = users.in_transaction(&mut tx).insert_if_absent(&user).await;
    let changed_user = users
        .in_transaction(&mut tx)
        .insert_if_absent(&changed_email)
        .await;
    commit(tx).await;

    assert!(matches!(
        same_user,
        Ok(UserInsertOutcome::Existing(existing)) if existing.value.email() == user.email()
    ));
    assert!(matches!(
        changed_user,
        Ok(UserInsertOutcome::Existing(existing)) if existing.value.email() == user.email()
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_user_update_concurrency_conflict() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let mut user = sample_user("postgres-stale", UserRole::User, None);

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }
    let loaded = match users.in_transaction(&mut tx).find_by_id(user.id()).await {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing user"),
        Err(error) => panic!("failed to load user: {error:?}"),
    };
    user.change_tier(UserTier::Pro);
    match users
        .in_transaction(&mut tx)
        .update(&user, loaded.version)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed first update: {error:?}"),
    }
    let stale = users
        .in_transaction(&mut tx)
        .update(&user, loaded.version)
        .await;

    assert!(matches!(
        stale,
        Err(UserRepositoryError::ConcurrencyConflict)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_rollback_insert_when_transaction_drops() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let user = sample_user("postgres-rollback", UserRole::User, None);

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert rollback user: {error:?}"),
    }
    drop(tx);

    let mut tx = begin(&unit_of_work).await;
    let missing = match users.in_transaction(&mut tx).find_by_id(user.id()).await {
        Ok(value) => value,
        Err(error) => panic!("failed rollback lookup: {error:?}"),
    };
    commit(tx).await;

    assert!(missing.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_report_concurrency_conflict_when_updating_missing_user() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let user = sample_user("postgres-missing-update", UserRole::User, None);

    let mut tx = begin(&unit_of_work).await;
    let result = users
        .in_transaction(&mut tx)
        .update(&user, user_service::ports::UserStorageVersion::INITIAL)
        .await;

    assert!(matches!(
        result,
        Err(UserRepositoryError::ConcurrencyConflict)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_delete_user_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let user = sample_user("postgres-delete", UserRole::User, None);

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }
    match users.in_transaction(&mut tx).delete_by_id(user.id()).await {
        Ok(deleted) => assert!(deleted),
        Err(error) => panic!("failed to delete user: {error:?}"),
    }
    let missing = match users.in_transaction(&mut tx).find_by_id(user.id()).await {
        Ok(value) => value,
        Err(error) => panic!("failed deleted lookup: {error:?}"),
    };
    let missing_delete = match users
        .in_transaction(&mut tx)
        .delete_by_id(UserId::new())
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("failed missing delete: {error:?}"),
    };

    assert!(missing.is_none());
    assert!(!missing_delete);
}

fn sample_user(slug: &str, role: UserRole, stripe_customer_id: Option<&str>) -> User {
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
            role,
            stripe_customer_id: stripe_customer_id.map(StripeCustomerId::from),
        },
    }) {
        Ok(user) => user,
        Err(error) => panic!("failed to create user: {error}"),
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
