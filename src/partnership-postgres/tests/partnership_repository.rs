use application::transaction::{Transaction, UnitOfWork};
use partnership_core::partnership_id::PartnershipId;
use partnership_postgres::SqlxPartnershipRepositoryFactory;
use partnership_service::ports::{
    PartnershipMembershipAddOutcome, PartnershipMembershipRemoveOutcome,
    PartnershipMembershipRepository, PartnershipMembershipRepositoryFactory, PartnershipRepository,
    PartnershipRepositoryFactory,
};
use party_core::party_id::PartyId;
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

async fn begin(pool: &PgPool) -> SqlxTransaction {
    match SqlxUnitOfWork::new(pool.clone()).begin().await {
        Ok(transaction) => transaction,
        Err(error) => panic!("begin partnership repository transaction: {error}"),
    }
}

async fn commit(transaction: SqlxTransaction) {
    if let Err(error) = transaction.commit().await {
        panic!("commit partnership repository transaction: {error}");
    }
}

async fn seed_party(pool: &PgPool) -> PartyId {
    let party_id = PartyId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(uuid::Uuid::from(party_id))
        .bind(format!("partnership-repository-party-{party_id}"))
        .bind("Partnership Repository Party")
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed partnership repository party: {error}"));
    party_id
}

async fn seed_user(pool: &PgPool) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(uuid::Uuid::from(user_id))
        .bind(format!("{user_id}@partnership-repository.test"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed partnership repository user: {error}"));
    user_id
}

async fn count_partnerships(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM partnerships")
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("count partnerships: {error}"))
}

async fn count_members(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM partnership_members")
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("count partnership members: {error}"))
}

async fn create_committed_partnership(pool: &PgPool, party_id: PartyId) -> PartnershipId {
    let partnership_id = PartnershipId::new();
    let mut transaction = begin(pool).await;
    let factory = SqlxPartnershipRepositoryFactory::new();
    let stored = PartnershipRepositoryFactory::in_transaction(&factory, &mut transaction)
        .find_or_create_for_party(party_id, partnership_id)
        .await
        .unwrap_or_else(|error| panic!("create partnership: {error}"));
    assert_eq!(partnership_id, stored.value.id());
    commit(transaction).await;
    stored.value.id()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_find_partnership_by_id_and_create_only_once_for_party() {
    let pool = get_postgres_client().await;
    let party_id = seed_party(&pool).await;
    let first_partnership_id = PartnershipId::new();
    let conflicting_partnership_id = PartnershipId::new();
    let mut transaction = begin(&pool).await;
    let factory = SqlxPartnershipRepositoryFactory::new();

    let first = PartnershipRepositoryFactory::in_transaction(&factory, &mut transaction)
        .find_or_create_for_party(party_id, first_partnership_id)
        .await
        .unwrap_or_else(|error| panic!("create first partnership: {error}"));
    assert_eq!(first_partnership_id, first.value.id());
    assert_eq!(party_id, first.value.party_id());
    assert_eq!(1, first.version.into_inner());

    let second = PartnershipRepositoryFactory::in_transaction(&factory, &mut transaction)
        .find_or_create_for_party(party_id, conflicting_partnership_id)
        .await
        .unwrap_or_else(|error| panic!("find existing partnership for party: {error}"));
    assert_eq!(first_partnership_id, second.value.id());
    assert_eq!(party_id, second.value.party_id());
    assert_ne!(conflicting_partnership_id, second.value.id());

    let by_id = PartnershipRepositoryFactory::in_transaction(&factory, &mut transaction)
        .find_by_id(first_partnership_id)
        .await
        .unwrap_or_else(|error| panic!("find partnership by id: {error}"));
    assert!(matches!(
        by_id,
        Some(found)
            if found.value.id() == first_partnership_id
                && found.value.party_id() == party_id
                && found.version.into_inner() == 1
    ));
    let conflicting_by_id =
        PartnershipRepositoryFactory::in_transaction(&factory, &mut transaction)
            .find_by_id(conflicting_partnership_id)
            .await
            .unwrap_or_else(|error| panic!("find conflicting partnership by id: {error}"));
    assert!(conflicting_by_id.is_none());

    commit(transaction).await;
    assert_eq!(1, count_partnerships(&pool).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_add_and_remove_members_idempotently() {
    let pool = get_postgres_client().await;
    let party_id = seed_party(&pool).await;
    let user_id = seed_user(&pool).await;
    let partnership_id = create_committed_partnership(&pool, party_id).await;

    let mut add_transaction = begin(&pool).await;
    let factory = SqlxPartnershipRepositoryFactory::new();
    assert_eq!(
        PartnershipMembershipAddOutcome::Added,
        PartnershipMembershipRepositoryFactory::in_transaction(&factory, &mut add_transaction)
            .add_member(user_id, partnership_id)
            .await
            .unwrap_or_else(|error| panic!("add partnership member: {error}"))
    );
    assert_eq!(
        PartnershipMembershipAddOutcome::AlreadyMember,
        PartnershipMembershipRepositoryFactory::in_transaction(&factory, &mut add_transaction)
            .add_member(user_id, partnership_id)
            .await
            .unwrap_or_else(|error| panic!("add existing partnership member: {error}"))
    );
    commit(add_transaction).await;
    assert_eq!(1, count_members(&pool).await);

    let mut remove_transaction = begin(&pool).await;
    assert_eq!(
        PartnershipMembershipRemoveOutcome::Removed,
        PartnershipMembershipRepositoryFactory::in_transaction(&factory, &mut remove_transaction,)
            .remove_member(user_id, partnership_id)
            .await
            .unwrap_or_else(|error| panic!("remove partnership member: {error}"))
    );
    assert_eq!(
        PartnershipMembershipRemoveOutcome::AlreadyAbsent,
        PartnershipMembershipRepositoryFactory::in_transaction(&factory, &mut remove_transaction,)
            .remove_member(user_id, partnership_id)
            .await
            .unwrap_or_else(|error| panic!("remove absent partnership member: {error}"))
    );
    commit(remove_transaction).await;
    assert_eq!(0, count_members(&pool).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_rollback_partnership_and_membership_writes() {
    let pool = get_postgres_client().await;
    let party_id = seed_party(&pool).await;
    let user_id = seed_user(&pool).await;
    let partnership_id = PartnershipId::new();
    let mut transaction = begin(&pool).await;
    let factory = SqlxPartnershipRepositoryFactory::new();

    let created = PartnershipRepositoryFactory::in_transaction(&factory, &mut transaction)
        .find_or_create_for_party(party_id, partnership_id)
        .await
        .unwrap_or_else(|error| panic!("create partnership for rollback: {error}"));
    assert_eq!(partnership_id, created.value.id());
    assert_eq!(
        PartnershipMembershipAddOutcome::Added,
        PartnershipMembershipRepositoryFactory::in_transaction(&factory, &mut transaction)
            .add_member(user_id, partnership_id)
            .await
            .unwrap_or_else(|error| panic!("add partnership member for rollback: {error}"))
    );

    drop(transaction);
    assert_eq!(0, count_partnerships(&pool).await);
    assert_eq!(0, count_members(&pool).await);
}
