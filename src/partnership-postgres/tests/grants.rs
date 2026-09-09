use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::ListingSourceId;
use partnership_core::partnership_id::PartnershipId;
use partnership_postgres::SqlxListingSourceGrantRepositoryFactory;
use partnership_service::ports::{
    ListingSourceGrantOutcome, ListingSourceGrantRemoveOutcome, ListingSourceGrantRepository,
    ListingSourceGrantRepositoryFactory,
};
use party_core::party_id::PartyId;
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

async fn begin(pool: &PgPool) -> SqlxTransaction {
    match SqlxUnitOfWork::new(pool.clone()).begin().await {
        Ok(transaction) => transaction,
        Err(error) => panic!("begin listing source grant transaction: {error}"),
    }
}

async fn commit(transaction: SqlxTransaction) {
    if let Err(error) = transaction.commit().await {
        panic!("commit listing source grant transaction: {error}");
    }
}

async fn seed_grant_targets(pool: &PgPool) -> (PartnershipId, ListingSourceId) {
    let party_id = PartyId::new();
    let listing_source_id = ListingSourceId::new();
    let partnership_id = PartnershipId::new();

    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id.into_uuid())
        .bind(format!(
            "grant-repository-party-{}",
            party_id.as_uuid().simple()
        ))
        .bind("Grant Repository Party")
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed grant repository party: {error}"));
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(listing_source_id.into_uuid())
    .bind(format!(
            "grant-repository-source-{}",
            listing_source_id.as_uuid().simple()
        ))
    .bind("Grant Repository Source")
    .bind(party_id.into_uuid())
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed grant repository listing source: {error}"));
    sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
        .bind(partnership_id.into_uuid())
        .bind(party_id.into_uuid())
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed grant repository partnership: {error}"));

    (partnership_id, listing_source_id)
}

async fn count_grants(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM partnership_listing_source_grants")
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("count listing source grants: {error}"))
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_add_and_remove_listing_source_grants_idempotently() {
    let pool = get_postgres_client().await;
    let (partnership_id, listing_source_id) = seed_grant_targets(&pool).await;
    let factory = SqlxListingSourceGrantRepositoryFactory::new();

    let mut grant_transaction = begin(&pool).await;
    assert_eq!(
        ListingSourceGrantOutcome::Granted,
        factory
            .in_transaction(&mut grant_transaction)
            .grant_source_access(partnership_id, listing_source_id)
            .await
            .unwrap_or_else(|error| panic!("grant listing source access: {error}"))
    );
    assert_eq!(
        ListingSourceGrantOutcome::AlreadyGranted,
        factory
            .in_transaction(&mut grant_transaction)
            .grant_source_access(partnership_id, listing_source_id)
            .await
            .unwrap_or_else(|error| panic!("grant existing listing source access: {error}"))
    );
    commit(grant_transaction).await;
    assert_eq!(1, count_grants(&pool).await);

    let mut remove_transaction = begin(&pool).await;
    assert_eq!(
        ListingSourceGrantRemoveOutcome::Removed,
        factory
            .in_transaction(&mut remove_transaction)
            .remove_source_access(partnership_id, listing_source_id)
            .await
            .unwrap_or_else(|error| panic!("remove listing source access: {error}"))
    );
    assert_eq!(
        ListingSourceGrantRemoveOutcome::AlreadyAbsent,
        factory
            .in_transaction(&mut remove_transaction)
            .remove_source_access(partnership_id, listing_source_id)
            .await
            .unwrap_or_else(|error| panic!("remove absent listing source access: {error}"))
    );
    commit(remove_transaction).await;
    assert_eq!(0, count_grants(&pool).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_rollback_listing_source_grant_add_and_remove() {
    let pool = get_postgres_client().await;
    let (partnership_id, listing_source_id) = seed_grant_targets(&pool).await;
    let factory = SqlxListingSourceGrantRepositoryFactory::new();

    let mut grant_transaction = begin(&pool).await;
    assert_eq!(
        ListingSourceGrantOutcome::Granted,
        factory
            .in_transaction(&mut grant_transaction)
            .grant_source_access(partnership_id, listing_source_id)
            .await
            .unwrap_or_else(|error| panic!("grant listing source access for rollback: {error}"))
    );
    drop(grant_transaction);
    assert_eq!(0, count_grants(&pool).await);

    let mut committed_grant_transaction = begin(&pool).await;
    assert_eq!(
        ListingSourceGrantOutcome::Granted,
        factory
            .in_transaction(&mut committed_grant_transaction)
            .grant_source_access(partnership_id, listing_source_id)
            .await
            .unwrap_or_else(|error| panic!(
                "grant listing source access before removal rollback: {error}"
            ))
    );
    commit(committed_grant_transaction).await;
    assert_eq!(1, count_grants(&pool).await);

    let mut remove_transaction = begin(&pool).await;
    assert_eq!(
        ListingSourceGrantRemoveOutcome::Removed,
        factory
            .in_transaction(&mut remove_transaction)
            .remove_source_access(partnership_id, listing_source_id)
            .await
            .unwrap_or_else(|error| panic!("remove listing source access for rollback: {error}"))
    );
    drop(remove_transaction);
    assert_eq!(1, count_grants(&pool).await);
}
