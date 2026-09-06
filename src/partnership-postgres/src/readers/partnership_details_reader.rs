use application::error::box_error;
use listing_source_core::ListingSourceId;
use partnership_core::partnership_id::PartnershipId;
use partnership_service::{
    ports::{
        PartnershipDetailsReadError, PartnershipDetailsReader, PartnershipDetailsReaderFactory,
    },
    use_cases::queries::{
        get_admin_partnership::AdminPartnershipDetailsView,
        list_admin_partnerships::PartnershipPartySummary,
    },
};
use party_core::{
    party_id::PartyId,
    party_name::{PartyName, PartyNameError},
    party_slug_id::{InvalidPartySlugId, PartySlugId},
};
use platform_postgres::SqlxTransaction;
use time::OffsetDateTime;
use user_core::user_id::UserId;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnershipDetailsReaderFactory;

struct SqlxPartnershipDetailsReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxPartnershipDetailsReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartnershipDetailsReaderFactory<SqlxTransaction> for SqlxPartnershipDetailsReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartnershipDetailsReader + 'tx {
        SqlxPartnershipDetailsReader {
            connection: tx.connection(),
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct PartnershipDetailsRow {
    partnership_id: Uuid,
    party_id: Uuid,
    party_slug_id: String,
    party_name: String,
    member_user_ids: Vec<Uuid>,
    member_count: i64,
    listing_source_ids: Vec<Uuid>,
    listing_source_grant_count: i64,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
enum PartnershipDetailsRowMappingError {
    #[error("invalid persisted party slug")]
    PartySlug(#[source] InvalidPartySlugId),
    #[error("invalid persisted party name")]
    PartyName(#[source] PartyNameError),
    #[error("invalid persisted member count")]
    MemberCount(#[source] std::num::TryFromIntError),
    #[error("invalid persisted listing source grant count")]
    ListingSourceGrantCount(#[source] std::num::TryFromIntError),
}

impl TryFrom<PartnershipDetailsRow> for AdminPartnershipDetailsView {
    type Error = PartnershipDetailsRowMappingError;

    fn try_from(row: PartnershipDetailsRow) -> Result<Self, Self::Error> {
        let member_count = u64::try_from(row.member_count)
            .map_err(PartnershipDetailsRowMappingError::MemberCount)?;
        let listing_source_grant_count = u64::try_from(row.listing_source_grant_count)
            .map_err(PartnershipDetailsRowMappingError::ListingSourceGrantCount)?;

        Ok(Self {
            partnership_id: PartnershipId::from(row.partnership_id),
            party: PartnershipPartySummary {
                party_id: PartyId::from(row.party_id),
                party_slug_id: PartySlugId::raw(row.party_slug_id)
                    .map_err(PartnershipDetailsRowMappingError::PartySlug)?,
                name: PartyName::try_from(row.party_name)
                    .map_err(PartnershipDetailsRowMappingError::PartyName)?,
            },
            member_user_ids: row.member_user_ids.into_iter().map(UserId::from).collect(),
            listing_source_ids: row
                .listing_source_ids
                .into_iter()
                .map(ListingSourceId::from)
                .collect(),
            member_count,
            listing_source_grant_count,
            created: row.created,
            updated: row.updated,
        })
    }
}

const DETAILS_SQL: &str = "SELECT p.partnership_id,p.party_id,party.party_slug_id,party.name AS party_name,ARRAY(SELECT members.user_id FROM partnership_members members WHERE members.partnership_id=p.partnership_id ORDER BY members.user_id LIMIT 100) AS member_user_ids,(SELECT COUNT(*) FROM partnership_members members WHERE members.partnership_id=p.partnership_id) AS member_count,ARRAY(SELECT grants.listing_source_id FROM partnership_listing_source_grants grants WHERE grants.partnership_id=p.partnership_id ORDER BY grants.listing_source_id LIMIT 100) AS listing_source_ids,(SELECT COUNT(*) FROM partnership_listing_source_grants grants WHERE grants.partnership_id=p.partnership_id) AS listing_source_grant_count,p.created,p.updated FROM partnerships p JOIN parties party ON party.party_id=p.party_id WHERE p.partnership_id=$1";

#[async_trait::async_trait]
impl PartnershipDetailsReader for SqlxPartnershipDetailsReader<'_> {
    async fn find_by_id(
        &mut self,
        partnership_id: PartnershipId,
    ) -> Result<Option<AdminPartnershipDetailsView>, PartnershipDetailsReadError> {
        sqlx::query_as::<_, PartnershipDetailsRow>(DETAILS_SQL)
            .bind(Uuid::from(partnership_id))
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(
                |source| PartnershipDetailsReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?
            .map(AdminPartnershipDetailsView::try_from)
            .transpose()
            .map_err(|source| PartnershipDetailsReadError::InvalidReadModel {
                source: box_error(source),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::transaction::{Transaction, UnitOfWork};
    use listing_source_core::ListingSourceId;
    use sqlx::PgPool;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
    use time::macros::datetime;

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    async fn seed_user(pool: &PgPool) -> UserId {
        let user_id = UserId::new();
        sqlx::query(
            "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
        )
        .bind(Uuid::from(user_id))
        .bind(format!("{user_id}@partnership-details-reader.test"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader user: {error}"));
        user_id
    }

    async fn seed_party(pool: &PgPool, name: &str) -> PartyId {
        let party_id = PartyId::new();
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(Uuid::from(party_id))
            .bind(format!("party-{party_id}"))
            .bind(name)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("failed to seed reader party: {error}"));
        party_id
    }

    async fn seed_partnership(
        pool: &PgPool,
        party_id: PartyId,
        created: OffsetDateTime,
        updated: OffsetDateTime,
    ) -> PartnershipId {
        let partnership_id = PartnershipId::new();
        sqlx::query(
            "INSERT INTO partnerships (partnership_id, party_id, created, updated) VALUES ($1, $2, $3, $4)",
        )
        .bind(Uuid::from(partnership_id))
        .bind(Uuid::from(party_id))
        .bind(created)
        .bind(updated)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader partnership: {error}"));
        partnership_id
    }

    async fn seed_listing_source(pool: &PgPool, operator_party_id: PartyId) -> ListingSourceId {
        let listing_source_id = ListingSourceId::new();
        sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(Uuid::from(listing_source_id))
        .bind(format!("source-{listing_source_id}"))
        .bind("Reader source")
        .bind(Uuid::from(operator_party_id))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader listing source: {error}"));
        listing_source_id
    }

    async fn add_member(pool: &PgPool, user_id: UserId, partnership_id: PartnershipId) {
        sqlx::query("INSERT INTO partnership_members (user_id, partnership_id) VALUES ($1, $2)")
            .bind(Uuid::from(user_id))
            .bind(Uuid::from(partnership_id))
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("failed to seed reader membership: {error}"));
    }

    async fn add_grant(
        pool: &PgPool,
        partnership_id: PartnershipId,
        listing_source_id: ListingSourceId,
    ) {
        sqlx::query(
            "INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)",
        )
        .bind(Uuid::from(partnership_id))
        .bind(Uuid::from(listing_source_id))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader source grant: {error}"));
    }

    async fn read_details(partnership_id: PartnershipId) -> Option<AdminPartnershipDetailsView> {
        let pool = get_postgres_client().await;
        let unit_of_work = platform_postgres::SqlxUnitOfWork::new(pool);
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("failed to begin reader transaction: {error}"));
        let result = SqlxPartnershipDetailsReaderFactory::new()
            .in_transaction(&mut tx)
            .find_by_id(partnership_id)
            .await;
        match result {
            Ok(result) => {
                tx.commit()
                    .await
                    .unwrap_or_else(|error| panic!("failed to commit reader transaction: {error}"));
                result
            }
            Err(error) => panic!("reader failed: {error}"),
        }
    }

    #[test]
    fn should_reject_invalid_persisted_party_mapping() {
        let result = AdminPartnershipDetailsView::try_from(PartnershipDetailsRow {
            partnership_id: Uuid::new_v4(),
            party_id: Uuid::new_v4(),
            party_slug_id: "Not a slug".to_owned(),
            party_name: "Valid party".to_owned(),
            member_user_ids: Vec::new(),
            member_count: 0,
            listing_source_ids: Vec::new(),
            listing_source_grant_count: 0,
            created: datetime!(2026-01-01 00:00 UTC),
            updated: datetime!(2026-01-01 00:00 UTC),
        });

        assert!(matches!(
            result,
            Err(PartnershipDetailsRowMappingError::PartySlug(_))
        ));
    }

    #[test]
    fn should_reject_negative_persisted_counts() {
        let result = AdminPartnershipDetailsView::try_from(PartnershipDetailsRow {
            partnership_id: Uuid::new_v4(),
            party_id: Uuid::new_v4(),
            party_slug_id: "valid-party".to_owned(),
            party_name: "Valid party".to_owned(),
            member_user_ids: Vec::new(),
            member_count: -1,
            listing_source_ids: Vec::new(),
            listing_source_grant_count: 0,
            created: datetime!(2026-01-01 00:00 UTC),
            updated: datetime!(2026-01-01 00:00 UTC),
        });

        assert!(matches!(
            result,
            Err(PartnershipDetailsRowMappingError::MemberCount(_))
        ));
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_return_ordered_current_members_and_grants_with_full_counts() {
        let pool = get_postgres_client().await;
        let party_id = seed_party(&pool, "Details party").await;
        let partnership_id = seed_partnership(
            &pool,
            party_id,
            datetime!(2026-01-02 00:00 UTC),
            datetime!(2026-01-03 00:00 UTC),
        )
        .await;
        let user_ids = [seed_user(&pool).await, seed_user(&pool).await];
        let listing_source_ids = [
            seed_listing_source(&pool, party_id).await,
            seed_listing_source(&pool, party_id).await,
        ];
        for user_id in user_ids.iter().rev() {
            add_member(&pool, *user_id, partnership_id).await;
        }
        for listing_source_id in listing_source_ids.iter().rev() {
            add_grant(&pool, partnership_id, *listing_source_id).await;
        }

        let result = read_details(partnership_id)
            .await
            .unwrap_or_else(|| panic!("partnership details should exist"));
        let mut expected_users = user_ids.map(Uuid::from);
        expected_users.sort();
        let mut expected_sources = listing_source_ids.map(Uuid::from);
        expected_sources.sort();

        assert_eq!(partnership_id, result.partnership_id);
        assert_eq!(party_id, result.party.party_id);
        assert_eq!(
            expected_users.to_vec(),
            result
                .member_user_ids
                .iter()
                .map(|id| Uuid::from(*id))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            expected_sources.to_vec(),
            result
                .listing_source_ids
                .iter()
                .map(|id| Uuid::from(*id))
                .collect::<Vec<_>>()
        );
        assert_eq!(2, result.member_count);
        assert_eq!(2, result.listing_source_grant_count);
        assert_eq!(datetime!(2026-01-02 00:00 UTC), result.created);
        assert_eq!(datetime!(2026-01-03 00:00 UTC), result.updated);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_return_empty_associations_for_partnership_without_members_or_grants() {
        let pool = get_postgres_client().await;
        let party_id = seed_party(&pool, "Empty details party").await;
        let partnership_id = seed_partnership(
            &pool,
            party_id,
            datetime!(2026-02-01 00:00 UTC),
            datetime!(2026-02-01 00:00 UTC),
        )
        .await;

        let result = read_details(partnership_id)
            .await
            .unwrap_or_else(|| panic!("partnership details should exist"));

        assert!(result.member_user_ids.is_empty());
        assert!(result.listing_source_ids.is_empty());
        assert_eq!(0, result.member_count);
        assert_eq!(0, result.listing_source_grant_count);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_return_none_for_missing_partnership() {
        assert!(read_details(PartnershipId::new()).await.is_none());
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_bound_large_member_and_grant_arrays_without_changing_counts() {
        let pool = get_postgres_client().await;
        let party_id = seed_party(&pool, "Bounded details party").await;
        let partnership_id = seed_partnership(
            &pool,
            party_id,
            datetime!(2026-03-01 00:00 UTC),
            datetime!(2026-03-01 00:00 UTC),
        )
        .await;
        let mut user_ids = Vec::new();
        let mut listing_source_ids = Vec::new();
        for _ in 0..101 {
            user_ids.push(seed_user(&pool).await);
            listing_source_ids.push(seed_listing_source(&pool, party_id).await);
        }
        for user_id in &user_ids {
            add_member(&pool, *user_id, partnership_id).await;
        }
        for listing_source_id in &listing_source_ids {
            add_grant(&pool, partnership_id, *listing_source_id).await;
        }

        let result = read_details(partnership_id)
            .await
            .unwrap_or_else(|| panic!("partnership details should exist"));

        assert_eq!(100, result.member_user_ids.len());
        assert_eq!(100, result.listing_source_ids.len());
        assert_eq!(101, result.member_count);
        assert_eq!(101, result.listing_source_grant_count);
    }
}
