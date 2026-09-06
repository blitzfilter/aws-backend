use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::ListingSourceId;
use partnership_core::{
    partnership_id::PartnershipId, partnership_lifecycle::PartnershipLifecycle,
};
use partnership_postgres::{
    SqlxListingSourceGrantRepositoryFactory, SqlxPartnershipRepositoryFactory,
};
use partnership_service::ports::{
    ListingSourceGrantRepository, ListingSourceGrantRepositoryFactory,
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

async fn seed_listing_source(pool: &PgPool, party_id: PartyId) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(format!("partnership-repository-source-{listing_source_id}"))
    .bind("Partnership Repository Source")
    .bind(uuid::Uuid::from(party_id))
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("seed partnership repository source: {error}"));
    listing_source_id
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

async fn count_source_grants(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM partnership_listing_source_grants")
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("count partnership source grants: {error}"))
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
async fn should_reactivate_dissolved_partnership_for_party_with_same_id_and_incremented_version() {
    let pool = get_postgres_client().await;
    let party_id = seed_party(&pool).await;
    let partnership_id = PartnershipId::new();
    let requested_reactivation_id = PartnershipId::new();
    let factory = SqlxPartnershipRepositoryFactory::new();

    let mut dissolve_transaction = begin(&pool).await;
    let created = PartnershipRepositoryFactory::in_transaction(&factory, &mut dissolve_transaction)
        .find_or_create_for_party(party_id, partnership_id)
        .await
        .unwrap_or_else(|error| panic!("create partnership before reactivation: {error}"));
    let mut dissolved_partnership = created.value;
    assert!(dissolved_partnership.dissolve());
    let dissolved =
        PartnershipRepositoryFactory::in_transaction(&factory, &mut dissolve_transaction)
            .dissolve(&dissolved_partnership, created.version)
            .await
            .unwrap_or_else(|error| panic!("dissolve partnership before reactivation: {error}"));
    assert_eq!(PartnershipLifecycle::Dissolved, dissolved.value.lifecycle());
    assert_eq!(2, dissolved.version.into_inner());
    commit(dissolve_transaction).await;

    let mut reactivate_transaction = begin(&pool).await;
    let reactivated =
        PartnershipRepositoryFactory::in_transaction(&factory, &mut reactivate_transaction)
            .find_or_create_for_party(party_id, requested_reactivation_id)
            .await
            .unwrap_or_else(|error| panic!("reactivate dissolved partnership: {error}"));
    assert_eq!(partnership_id, reactivated.value.id());
    assert_ne!(requested_reactivation_id, reactivated.value.id());
    assert_eq!(PartnershipLifecycle::Active, reactivated.value.lifecycle());
    assert_eq!(3, reactivated.version.into_inner());
    commit(reactivate_transaction).await;

    let persisted = sqlx::query_as::<_, (uuid::Uuid, String, i64)>(
        "SELECT partnership_id, business_state, version FROM partnerships WHERE party_id = $1",
    )
    .bind(uuid::Uuid::from(party_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("read reactivated partnership: {error}"));
    assert_eq!(uuid::Uuid::from(partnership_id), persisted.0);
    assert_eq!("ACTIVE", persisted.1);
    assert_eq!(3, persisted.2);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_dissolve_active_partnership_remove_associations_and_replay_without_version_bump() {
    let pool = get_postgres_client().await;
    let party_id = seed_party(&pool).await;
    let user_id = seed_user(&pool).await;
    let listing_source_id = seed_listing_source(&pool, party_id).await;
    let partnership_id = create_committed_partnership(&pool, party_id).await;
    let partnership_factory = SqlxPartnershipRepositoryFactory::new();

    let mut grant_transaction = begin(&pool).await;
    PartnershipMembershipRepositoryFactory::in_transaction(
        &partnership_factory,
        &mut grant_transaction,
    )
    .add_member(user_id, partnership_id)
    .await
    .unwrap_or_else(|error| panic!("add partnership member before dissolution: {error}"));
    SqlxListingSourceGrantRepositoryFactory::new()
        .in_transaction(&mut grant_transaction)
        .grant_source_access(partnership_id, listing_source_id)
        .await
        .unwrap_or_else(|error| panic!("grant source before dissolution: {error}"));
    commit(grant_transaction).await;

    let mut dissolve_transaction = begin(&pool).await;
    let loaded = PartnershipRepositoryFactory::in_transaction(
        &partnership_factory,
        &mut dissolve_transaction,
    )
    .find_by_id(partnership_id)
    .await
    .unwrap_or_else(|error| panic!("load partnership before dissolution: {error}"))
    .unwrap_or_else(|| panic!("partnership should exist before dissolution"));
    let mut dissolved_partnership = loaded.value;
    dissolved_partnership.dissolve();
    let dissolved = PartnershipRepositoryFactory::in_transaction(
        &partnership_factory,
        &mut dissolve_transaction,
    )
    .dissolve(&dissolved_partnership, loaded.version)
    .await
    .unwrap_or_else(|error| panic!("dissolve partnership: {error}"));
    assert_eq!(PartnershipLifecycle::Dissolved, dissolved.value.lifecycle());
    assert_eq!(2, dissolved.version.into_inner());
    commit(dissolve_transaction).await;

    assert_eq!(0, count_members(&pool).await);
    assert_eq!(0, count_source_grants(&pool).await);
    let persisted = sqlx::query_as::<_, (String, i64)>(
        "SELECT business_state, version FROM partnerships WHERE partnership_id = $1",
    )
    .bind(uuid::Uuid::from(partnership_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("read dissolved partnership: {error}"));
    assert_eq!("DISSOLVED", persisted.0);
    assert_eq!(2, persisted.1);

    let mut stale_transaction = begin(&pool).await;
    let stale =
        PartnershipRepositoryFactory::in_transaction(&partnership_factory, &mut stale_transaction)
            .dissolve(&dissolved_partnership, loaded.version)
            .await;
    assert!(matches!(
        stale,
        Err(partnership_service::ports::PartnershipRepositoryError::ConcurrencyConflict)
    ));
    commit(stale_transaction).await;

    let mut replay_transaction = begin(&pool).await;
    let loaded =
        PartnershipRepositoryFactory::in_transaction(&partnership_factory, &mut replay_transaction)
            .find_by_id(partnership_id)
            .await
            .unwrap_or_else(|error| panic!("load dissolved partnership for replay: {error}"))
            .unwrap_or_else(|| panic!("dissolved partnership should still exist"));
    let replayed =
        PartnershipRepositoryFactory::in_transaction(&partnership_factory, &mut replay_transaction)
            .dissolve(&loaded.value, loaded.version)
            .await
            .unwrap_or_else(|error| panic!("replay dissolved partnership: {error}"));
    assert_eq!(PartnershipLifecycle::Dissolved, replayed.value.lifecycle());
    assert_eq!(2, replayed.version.into_inner());
    commit(replay_transaction).await;

    let persisted_version =
        sqlx::query_scalar::<_, i64>("SELECT version FROM partnerships WHERE partnership_id = $1")
            .bind(uuid::Uuid::from(partnership_id))
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("read replayed partnership version: {error}"));
    assert_eq!(2, persisted_version);
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
