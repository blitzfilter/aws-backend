use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::ListingSourceId;
use partnership_core::{
    partnership_application::{
        NewPartnershipApplication, PartnershipApplication, PartnershipApplicationApprovalResult,
        PartnershipProposal,
    },
    partnership_application_id::PartnershipApplicationId,
    partnership_id::PartnershipId,
};
use partnership_postgres::SqlxPartnershipApplicationRepositoryFactory;
use partnership_service::ports::{
    PartnershipApplicationRepository, PartnershipApplicationRepositoryError,
    PartnershipApplicationRepositoryFactory, VersionedPartnershipApplication,
};
use party_core::party_id::PartyId;
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

fn submitted_application(
    applicant_user_id: UserId,
    listing_source_id: ListingSourceId,
) -> PartnershipApplication {
    PartnershipApplication::submit(NewPartnershipApplication {
        id: PartnershipApplicationId::new(),
        applicant_user_id,
        proposal: PartnershipProposal::ExistingListingSource { listing_source_id },
    })
}

async fn begin(pool: &PgPool) -> SqlxTransaction {
    match SqlxUnitOfWork::new(pool.clone()).begin().await {
        Ok(transaction) => transaction,
        Err(error) => panic!("begin application repository transaction: {error}"),
    }
}

async fn commit(transaction: SqlxTransaction) {
    if let Err(error) = transaction.commit().await {
        panic!("commit application repository transaction: {error}");
    }
}

async fn seed_user(pool: &PgPool) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(uuid::Uuid::from(user_id))
        .bind(format!("{user_id}@application-repository.test"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed application repository user: {error}"));
    user_id
}

async fn seed_approval_targets(pool: &PgPool) -> (PartnershipId, ListingSourceId) {
    let party_id = PartyId::new();
    let listing_source_id = ListingSourceId::new();
    let partnership_id = PartnershipId::new();

    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(uuid::Uuid::from(party_id))
        .bind(format!("application-party-{party_id}"))
        .bind("Application Repository Party")
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed application repository party: {error}"));
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(format!("application-source-{listing_source_id}"))
    .bind("Application Repository Source")
    .bind(uuid::Uuid::from(party_id))
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed application repository listing source: {error}"));
    sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
        .bind(uuid::Uuid::from(partnership_id))
        .bind(uuid::Uuid::from(party_id))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed application repository partnership: {error}"));

    (partnership_id, listing_source_id)
}

async fn insert_committed(
    pool: &PgPool,
    application: &PartnershipApplication,
) -> VersionedPartnershipApplication {
    let mut transaction = begin(pool).await;
    let inserted = SqlxPartnershipApplicationRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .insert(application)
        .await
        .unwrap_or_else(|error| panic!("insert partnership application: {error}"));
    commit(transaction).await;
    inserted
}

async fn count_applications(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM partnership_applications")
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("count partnership applications: {error}"))
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_and_find_application_by_id_and_applicant() {
    let pool = get_postgres_client().await;
    let applicant_user_id = seed_user(&pool).await;
    let other_user_id = seed_user(&pool).await;
    let application = submitted_application(applicant_user_id, ListingSourceId::new());
    let mut transaction = begin(&pool).await;
    let factory = SqlxPartnershipApplicationRepositoryFactory::new();

    let inserted = factory
        .in_transaction(&mut transaction)
        .insert(&application)
        .await
        .unwrap_or_else(|error| panic!("insert partnership application: {error}"));
    assert_eq!(application, inserted.value);
    assert_eq!(1, inserted.version.into_inner());

    let by_id = factory
        .in_transaction(&mut transaction)
        .find_by_id(application.id())
        .await
        .unwrap_or_else(|error| panic!("find partnership application by id: {error}"));
    assert!(matches!(
        by_id,
        Some(found) if found.value == application && found.version.into_inner() == 1
    ));

    let by_applicant = factory
        .in_transaction(&mut transaction)
        .find_by_user_and_id(applicant_user_id, application.id())
        .await
        .unwrap_or_else(|error| panic!("find partnership application by applicant: {error}"));
    assert!(matches!(
        by_applicant,
        Some(found)
            if found.value == application
                && found.value.applicant_user_id() == applicant_user_id
    ));

    let wrong_applicant = factory
        .in_transaction(&mut transaction)
        .find_by_user_and_id(other_user_id, application.id())
        .await
        .unwrap_or_else(|error| {
            panic!("find partnership application for other applicant: {error}")
        });
    assert!(wrong_applicant.is_none());

    commit(transaction).await;
    assert_eq!(1, count_applications(&pool).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_find_application_for_update() {
    let pool = get_postgres_client().await;
    let applicant_user_id = seed_user(&pool).await;
    let application = submitted_application(applicant_user_id, ListingSourceId::new());
    let inserted = insert_committed(&pool, &application).await;
    let mut transaction = begin(&pool).await;

    let found = SqlxPartnershipApplicationRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .find_by_id_for_update(application.id())
        .await
        .unwrap_or_else(|error| panic!("find partnership application for update: {error}"));
    assert!(matches!(
        found,
        Some(found)
            if found.value == application
                && found.version == inserted.version
    ));

    commit(transaction).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_update_application_increment_version_and_persist_approval_result() {
    let pool = get_postgres_client().await;
    let applicant_user_id = seed_user(&pool).await;
    let (partnership_id, listing_source_id) = seed_approval_targets(&pool).await;
    let application = submitted_application(applicant_user_id, listing_source_id);
    let inserted = insert_committed(&pool, &application).await;
    let approval_result =
        PartnershipApplicationApprovalResult::new(partnership_id, listing_source_id);
    let mut approved_application = application.clone();
    approved_application
        .mark_in_review()
        .unwrap_or_else(|error| panic!("mark application in review: {error}"));
    approved_application
        .approve(approval_result)
        .unwrap_or_else(|error| panic!("approve partnership application: {error}"));

    let mut transaction = begin(&pool).await;
    let updated = SqlxPartnershipApplicationRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .update(&approved_application, inserted.version)
        .await
        .unwrap_or_else(|error| panic!("update partnership application: {error}"));
    assert_eq!(approved_application, updated.value);
    assert_eq!(2, updated.version.into_inner());
    assert_eq!(Some(approval_result), updated.value.approval_result());
    commit(transaction).await;

    let row = sqlx::query_as::<_, (String, i64, Option<uuid::Uuid>, Option<uuid::Uuid>)>(
        "SELECT business_state, version, approved_partnership_id, approved_listing_source_id FROM partnership_applications WHERE partnership_application_id=$1",
    )
    .bind(uuid::Uuid::from(application.id()))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("read persisted partnership application: {error}"));
    assert_eq!("APPROVED", row.0);
    assert_eq!(2, row.1);
    assert_eq!(Some(uuid::Uuid::from(partnership_id)), row.2);
    assert_eq!(Some(uuid::Uuid::from(listing_source_id)), row.3);
    assert_eq!(1, count_applications(&pool).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_stale_application_version() {
    let pool = get_postgres_client().await;
    let applicant_user_id = seed_user(&pool).await;
    let application = submitted_application(applicant_user_id, ListingSourceId::new());
    let inserted = insert_committed(&pool, &application).await;
    let mut changed_application = application.clone();
    changed_application
        .mark_in_review()
        .unwrap_or_else(|error| panic!("mark application in review: {error}"));

    let mut transaction = begin(&pool).await;
    let updated = SqlxPartnershipApplicationRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .update(&changed_application, inserted.version)
        .await
        .unwrap_or_else(|error| panic!("update partnership application: {error}"));
    assert_eq!(2, updated.version.into_inner());
    commit(transaction).await;

    let mut stale_transaction = begin(&pool).await;
    let stale_update = SqlxPartnershipApplicationRepositoryFactory::new()
        .in_transaction(&mut stale_transaction)
        .update(&application, inserted.version)
        .await;
    assert!(matches!(
        stale_update,
        Err(PartnershipApplicationRepositoryError::ConcurrencyConflict)
    ));
    commit(stale_transaction).await;

    let row = sqlx::query_as::<_, (String, i64)>(
        "SELECT business_state, version FROM partnership_applications WHERE partnership_application_id=$1",
    )
    .bind(uuid::Uuid::from(application.id()))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("read application after stale update: {error}"));
    assert_eq!("IN_REVIEW", row.0);
    assert_eq!(2, row.1);
    assert_eq!(1, count_applications(&pool).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_hide_application_insert_and_update_after_transaction_rollback() {
    let pool = get_postgres_client().await;
    let applicant_user_id = seed_user(&pool).await;
    let application = submitted_application(applicant_user_id, ListingSourceId::new());

    let mut insert_transaction = begin(&pool).await;
    let inserted = SqlxPartnershipApplicationRepositoryFactory::new()
        .in_transaction(&mut insert_transaction)
        .insert(&application)
        .await
        .unwrap_or_else(|error| panic!("insert partnership application for rollback: {error}"));
    assert_eq!(application, inserted.value);
    drop(insert_transaction);
    assert_eq!(0, count_applications(&pool).await);

    let committed = insert_committed(&pool, &application).await;
    let mut changed_application = application.clone();
    changed_application
        .mark_in_review()
        .unwrap_or_else(|error| panic!("mark application in review: {error}"));
    let mut update_transaction = begin(&pool).await;
    let updated = SqlxPartnershipApplicationRepositoryFactory::new()
        .in_transaction(&mut update_transaction)
        .update(&changed_application, committed.version)
        .await
        .unwrap_or_else(|error| panic!("update partnership application for rollback: {error}"));
    assert_eq!(2, updated.version.into_inner());
    drop(update_transaction);

    let row = sqlx::query_as::<_, (String, i64)>(
        "SELECT business_state, version FROM partnership_applications WHERE partnership_application_id=$1",
    )
    .bind(uuid::Uuid::from(application.id()))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("read application after rollback: {error}"));
    assert_eq!("SUBMITTED", row.0);
    assert_eq!(1, row.1);
    assert_eq!(1, count_applications(&pool).await);
}
