use ::application::transaction::{Transaction, UnitOfWork};
use ::platform_postgres::SqlxUnitOfWork;
use ::user_core::user_id::UserId;
use localization::Language;
use money::Currency;
use serde_email::Email;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::first_name::FirstName;
use user_core::last_name::LastName;
use user_core::measurement_unit::MeasurementUnit;
use user_core::role::UserRole;
use user_core::tier::UserTier;
use user_core::user::{NewUser, User, UserAccount, UserPreferences, UserProfile};
use user_postgres::{SqlxUserAdminReaderFactory, SqlxUserRepositoryFactory};
use user_service::ports::{
    UserAdminMutationGuard, UserAdminMutationGuardFactory, UserAdminReader, UserAdminReaderFactory,
    UserAdminRemovalDecision, UserRepository, UserRepositoryFactory,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_last_admin_when_target_is_sole_admin() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let admins = SqlxUserAdminReaderFactory::new();
    let user = sample_user("postgres-sole-admin", UserRole::Admin);

    let mut tx = begin(&unit_of_work).await;
    insert_user(&users, &mut tx, &user).await;
    let decision = match UserAdminMutationGuardFactory::in_transaction(&admins, &mut tx)
        .check_removal(user.id())
        .await
    {
        Ok(decision) => decision,
        Err(error) => panic!("failed to check sole admin removal: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(UserAdminRemovalDecision::LastAdmin, decision);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_protect_last_active_admin_when_suspended_admin_remains() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let users = SqlxUserRepositoryFactory::new();
    let admins = SqlxUserAdminReaderFactory::new();
    let active = sample_user("postgres-active-admin", UserRole::Admin);
    let suspended = sample_user("postgres-suspended-admin", UserRole::Admin);

    let mut tx = begin(&unit_of_work).await;
    insert_user(&users, &mut tx, &active).await;
    insert_user(&users, &mut tx, &suspended).await;
    commit(tx).await;
    set_suspension(&pool, suspended.id(), true).await;

    let mut tx = begin(&unit_of_work).await;
    let decision = match UserAdminMutationGuardFactory::in_transaction(&admins, &mut tx)
        .check_removal(active.id())
        .await
    {
        Ok(decision) => decision,
        Err(error) => panic!("failed to check final active admin removal: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(UserAdminRemovalDecision::LastAdmin, decision);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_make_suspended_admin_unavailable_for_authorization_and_removal() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let users = SqlxUserRepositoryFactory::new();
    let admins = SqlxUserAdminReaderFactory::new();
    let user = sample_user("postgres-suspended-admin-target", UserRole::Admin);

    let mut tx = begin(&unit_of_work).await;
    insert_user(&users, &mut tx, &user).await;
    commit(tx).await;
    set_suspension(&pool, user.id(), true).await;

    let mut tx = begin(&unit_of_work).await;
    let actor = UserAdminReaderFactory::in_transaction(&admins, &mut tx)
        .find_admin_actor(user.id())
        .await;
    let decision = match UserAdminMutationGuardFactory::in_transaction(&admins, &mut tx)
        .check_removal(user.id())
        .await
    {
        Ok(decision) => decision,
        Err(error) => panic!("failed to check suspended admin removal: {error:?}"),
    };
    commit(tx).await;

    assert!(matches!(actor, Ok(None)));
    assert_eq!(UserAdminRemovalDecision::TargetNotAdmin, decision);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_allow_admin_removal_when_multiple_admins_exist() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let admins = SqlxUserAdminReaderFactory::new();
    let target = sample_user("postgres-multiple-admin-target", UserRole::Admin);
    let remaining = sample_user("postgres-multiple-admin-remaining", UserRole::Admin);

    let mut tx = begin(&unit_of_work).await;
    insert_user(&users, &mut tx, &target).await;
    insert_user(&users, &mut tx, &remaining).await;
    let decision = match UserAdminMutationGuardFactory::in_transaction(&admins, &mut tx)
        .check_removal(target.id())
        .await
    {
        Ok(decision) => decision,
        Err(error) => panic!("failed to check multiple admin removal: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(UserAdminRemovalDecision::Allowed, decision);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_serialize_concurrent_admin_removals_in_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let admins = SqlxUserAdminReaderFactory::new();
    let first = sample_user("postgres-concurrent-first-admin", UserRole::Admin);
    let second = sample_user("postgres-concurrent-second-admin", UserRole::Admin);

    let mut seed_tx = begin(&unit_of_work).await;
    insert_user(&users, &mut seed_tx, &first).await;
    insert_user(&users, &mut seed_tx, &second).await;
    commit(seed_tx).await;

    let mut first_tx = begin(&unit_of_work).await;
    let first_loaded = match users
        .in_transaction(&mut first_tx)
        .find_by_id(first.id())
        .await
    {
        Ok(Some(loaded)) => loaded,
        Ok(None) => panic!("missing first admin"),
        Err(error) => panic!("failed to load first admin: {error:?}"),
    };
    let mut first_user = first_loaded.value;
    assert!(first_user.change_role(UserRole::User).changed());
    let first_decision = match UserAdminMutationGuardFactory::in_transaction(&admins, &mut first_tx)
        .check_removal(first.id())
        .await
    {
        Ok(decision) => decision,
        Err(error) => panic!("failed to check first concurrent removal: {error:?}"),
    };
    assert_eq!(UserAdminRemovalDecision::Allowed, first_decision);

    let second_started = Arc::new(AtomicBool::new(false));
    let second_started_signal = Arc::clone(&second_started);
    let second_unit_of_work = unit_of_work.clone();
    let second_admins = admins;
    let second_id = second.id();
    let second_task = tokio::spawn(async move {
        let mut second_tx = begin(&second_unit_of_work).await;
        second_started_signal.store(true, Ordering::SeqCst);
        let decision =
            match UserAdminMutationGuardFactory::in_transaction(&second_admins, &mut second_tx)
                .check_removal(second_id)
                .await
            {
                Ok(decision) => decision,
                Err(error) => panic!("failed to check second concurrent removal: {error:?}"),
            };
        commit(second_tx).await;
        decision
    });

    while !second_started.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }

    match users
        .in_transaction(&mut first_tx)
        .update(&first_user, first_loaded.version)
        .await
    {
        Ok(_) => {}
        Err(error) => panic!("failed to demote first admin: {error:?}"),
    }
    commit(first_tx).await;

    let second_decision = match second_task.await {
        Ok(decision) => decision,
        Err(error) => panic!("concurrent admin removal task failed: {error}"),
    };
    assert_eq!(UserAdminRemovalDecision::LastAdmin, second_decision);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_target_not_admin_for_non_admin_user() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let admins = SqlxUserAdminReaderFactory::new();
    let user = sample_user("postgres-non-admin-target", UserRole::User);

    let mut tx = begin(&unit_of_work).await;
    insert_user(&users, &mut tx, &user).await;
    let decision = match UserAdminMutationGuardFactory::in_transaction(&admins, &mut tx)
        .check_removal(user.id())
        .await
    {
        Ok(decision) => decision,
        Err(error) => panic!("failed to check non-admin removal: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(UserAdminRemovalDecision::TargetNotAdmin, decision);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_target_not_found_for_missing_user() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let admins = SqlxUserAdminReaderFactory::new();
    let missing_user_id = UserId::new();

    let mut tx = begin(&unit_of_work).await;
    let decision = match UserAdminMutationGuardFactory::in_transaction(&admins, &mut tx)
        .check_removal(missing_user_id)
        .await
    {
        Ok(decision) => decision,
        Err(error) => panic!("failed to check missing user removal: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(UserAdminRemovalDecision::TargetNotFound, decision);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_user_admin_view_from_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let users = SqlxUserRepositoryFactory::new();
    let admins = SqlxUserAdminReaderFactory::new();
    let user = sample_user("postgres-admin-reader", UserRole::Admin);

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }

    let admin_view = match UserAdminReaderFactory::in_transaction(&admins, &mut tx)
        .find_admin_actor(user.id())
        .await
    {
        Ok(Some(view)) => view,
        Ok(None) => panic!("missing admin view"),
        Err(error) => panic!("failed to read admin view: {error:?}"),
    };
    commit(tx).await;

    assert_eq!(UserRole::Admin, admin_view.role);
}

async fn set_suspension(pool: &sqlx::PgPool, user_id: UserId, suspended: bool) {
    if let Err(error) = sqlx::query("UPDATE users SET suspended = $1 WHERE user_id = $2")
        .bind(suspended)
        .bind(uuid::Uuid::from(user_id))
        .execute(pool)
        .await
    {
        panic!("failed to set user suspension: {error}");
    }
}

async fn insert_user(
    users: &SqlxUserRepositoryFactory,
    tx: &mut ::platform_postgres::SqlxTransaction,
    user: &User,
) {
    match users.in_transaction(tx).insert(user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error:?}"),
    }
}

fn sample_user(slug: &str, role: UserRole) -> User {
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
            stripe_customer_id: None,
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
