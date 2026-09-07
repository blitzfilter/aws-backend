use ::application::transaction::{Transaction, UnitOfWork};
use ::platform_postgres::SqlxUnitOfWork;
use localization::Language;
use money::Currency;
use serde_email::Email;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::first_name::FirstName;
use user_core::last_name::LastName;
use user_core::measurement_unit::MeasurementUnit;
use user_core::role::UserRole;
use user_core::tier::UserTier;
use user_core::user::{NewUser, User, UserAccount, UserPreferences, UserProfile};
use user_postgres::{SqlxUserAuthenticationReader, SqlxUserRepositoryFactory};
use user_service::ports::{UserAuthenticationReader, UserRepository, UserRepositoryFactory};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_user_suspension_from_postgres() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let users = SqlxUserRepositoryFactory::new();
    let authentication = SqlxUserAuthenticationReader::new(pool.clone());
    let user = sample_user("user-authentication-reader");

    let mut tx = begin(&unit_of_work).await;
    match users.in_transaction(&mut tx).insert(&user).await {
        Ok(_) => {}
        Err(error) => panic!("failed to insert user: {error}"),
    }
    commit(tx).await;

    let active = authentication.find_suspension(user.id()).await;
    set_suspension(&pool, user.id(), true).await;
    let suspended = authentication.find_suspension(user.id()).await;
    let missing = authentication
        .find_suspension(user_core::user_id::UserId::new())
        .await;

    assert!(matches!(active, Ok(Some(false))));
    assert!(matches!(suspended, Ok(Some(true))));
    assert!(matches!(missing, Ok(None)));
}

async fn set_suspension(pool: &sqlx::PgPool, user_id: user_core::user_id::UserId, suspended: bool) {
    if let Err(error) = sqlx::query("UPDATE users SET suspended = $1 WHERE user_id = $2")
        .bind(suspended)
        .bind(uuid::Uuid::from(user_id))
        .execute(pool)
        .await
    {
        panic!("failed to set user suspension: {error}");
    }
}

fn sample_user(slug: &str) -> User {
    match User::create(NewUser {
        id: user_core::user_id::UserId::new(),
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
