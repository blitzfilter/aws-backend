use application::{
    error::box_error,
    pagination::{Cursor, CursoredResult},
};
use partnership_core::partnership_id::PartnershipId;
use partnership_service::{
    ports::{PartnershipSearchReadError, PartnershipSearchReader, PartnershipSearchReaderFactory},
    use_cases::queries::list_admin_partnerships::{
        AdminPartnershipSummary, ListAdminPartnershipsRequest, ListAdminPartnershipsResult,
        PartnershipPartySummary, PartnershipSearchCursor,
    },
};
use party_core::{
    party_id::PartyId,
    party_name::{PartyName, PartyNameError},
    party_slug_id::{InvalidPartySlugId, PartySlugId},
};
use platform_postgres::SqlxTransaction;
use sqlx::{Postgres, QueryBuilder};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnershipSearchReaderFactory;

struct SqlxPartnershipSearchReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxPartnershipSearchReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartnershipSearchReaderFactory<SqlxTransaction> for SqlxPartnershipSearchReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartnershipSearchReader + 'tx {
        SqlxPartnershipSearchReader {
            connection: tx.connection(),
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct PartnershipSearchRow {
    partnership_id: uuid::Uuid,
    party_id: uuid::Uuid,
    party_slug_id: String,
    party_name: String,
    member_count: i64,
    listing_source_grant_count: i64,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
enum PartnershipSearchRowMappingError {
    #[error("invalid persisted party slug")]
    PartySlug(#[source] InvalidPartySlugId),
    #[error("invalid persisted party name")]
    PartyName(#[source] PartyNameError),
    #[error("invalid persisted member count")]
    MemberCount(#[source] std::num::TryFromIntError),
    #[error("invalid persisted listing source grant count")]
    ListingSourceGrantCount(#[source] std::num::TryFromIntError),
}

impl TryFrom<PartnershipSearchRow> for AdminPartnershipSummary {
    type Error = PartnershipSearchRowMappingError;

    fn try_from(row: PartnershipSearchRow) -> Result<Self, Self::Error> {
        let member_count = u64::try_from(row.member_count)
            .map_err(PartnershipSearchRowMappingError::MemberCount)?;
        let listing_source_grant_count = u64::try_from(row.listing_source_grant_count)
            .map_err(PartnershipSearchRowMappingError::ListingSourceGrantCount)?;

        Ok(Self {
            partnership_id: PartnershipId::from(row.partnership_id),
            party: PartnershipPartySummary {
                party_id: PartyId::from(row.party_id),
                party_slug_id: PartySlugId::raw(row.party_slug_id)
                    .map_err(PartnershipSearchRowMappingError::PartySlug)?,
                name: PartyName::try_from(row.party_name)
                    .map_err(PartnershipSearchRowMappingError::PartyName)?,
            },
            member_count,
            listing_source_grant_count,
            created: row.created,
            updated: row.updated,
        })
    }
}

#[async_trait::async_trait]
impl PartnershipSearchReader for SqlxPartnershipSearchReader<'_> {
    async fn search(
        &mut self,
        request: &ListAdminPartnershipsRequest,
    ) -> Result<ListAdminPartnershipsResult, PartnershipSearchReadError> {
        let cursor = request.cursor.unwrap_or_default();
        let size = cursor.size.clamp(1, 100);
        let size_usize =
            usize::try_from(size).map_err(|source| PartnershipSearchReadError::Internal {
                source: box_error(source),
            })?;
        let limit =
            i64::try_from(size + 1).map_err(|source| PartnershipSearchReadError::Internal {
                source: box_error(source),
            })?;

        let mut builder = QueryBuilder::<Postgres>::new(
            "WITH candidate_partnerships AS (SELECT p.partnership_id, p.party_id, p.created, p.updated FROM partnerships p WHERE TRUE",
        );
        push_filters(&mut builder, request);
        if let Some(search_after) = cursor.search_after {
            builder
                .push(" AND (p.created, p.partnership_id) < (")
                .push_bind(search_after.position)
                .push(", ")
                .push_bind(uuid::Uuid::from(search_after.partnership_id))
                .push(")");
        }
        builder
            .push(" ORDER BY p.created DESC, p.partnership_id DESC LIMIT ")
            .push_bind(limit)
            .push(") SELECT candidate.partnership_id, candidate.party_id, party.party_slug_id, party.name AS party_name, (SELECT COUNT(*) FROM partnership_members members WHERE members.partnership_id = candidate.partnership_id) AS member_count, (SELECT COUNT(*) FROM partnership_listing_source_grants grants WHERE grants.partnership_id = candidate.partnership_id) AS listing_source_grant_count, candidate.created, candidate.updated FROM candidate_partnerships candidate JOIN parties party ON party.party_id = candidate.party_id ORDER BY candidate.created DESC, candidate.partnership_id DESC");

        let mut rows = builder
            .build_query_as::<PartnershipSearchRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(
                |source| PartnershipSearchReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?;

        let has_more = rows.len() > size_usize;
        if has_more {
            rows.truncate(size_usize);
        }
        let items = rows
            .into_iter()
            .map(AdminPartnershipSummary::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| PartnershipSearchReadError::InvalidReadModel {
                source: box_error(source),
            })?;
        let search_after = if has_more {
            items.last().map(|item| PartnershipSearchCursor {
                position: item.created,
                partnership_id: item.partnership_id,
            })
        } else {
            None
        };

        Ok(CursoredResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn push_filters(builder: &mut QueryBuilder<Postgres>, request: &ListAdminPartnershipsRequest) {
    if let Some(party_id) = request.party_id {
        builder
            .push(" AND p.party_id = ")
            .push_bind(uuid::Uuid::from(party_id));
    }
    if let Some(member_user_id) = request.member_user_id {
        builder.push(
            " AND EXISTS (SELECT 1 FROM partnership_members filter_members WHERE filter_members.partnership_id = p.partnership_id AND filter_members.user_id = ",
        );
        builder
            .push_bind(uuid::Uuid::from(member_user_id))
            .push(")");
    }
    if let Some(listing_source_id) = request.listing_source_id {
        builder.push(
            " AND EXISTS (SELECT 1 FROM partnership_listing_source_grants filter_grants WHERE filter_grants.partnership_id = p.partnership_id AND filter_grants.listing_source_id = ",
        );
        builder
            .push_bind(uuid::Uuid::from(listing_source_id))
            .push(")");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        pagination::Cursor,
        transaction::{Transaction, UnitOfWork},
    };
    use listing_source_core::ListingSourceId;
    use partnership_service::use_cases::queries::list_admin_partnerships::ListAdminPartnershipsRequest;
    use sqlx::PgPool;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
    use time::macros::datetime;
    use user_core::user_id::UserId;

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    async fn seed_user(pool: &PgPool) -> UserId {
        let user_id = UserId::new();
        sqlx::query(
            "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
        )
        .bind(uuid::Uuid::from(user_id))
        .bind(format!("{user_id}@partnership-reader.test"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader user: {error}"));
        user_id
    }

    async fn seed_party(pool: &PgPool, name: &str) -> PartyId {
        let party_id = PartyId::new();
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(uuid::Uuid::from(party_id))
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
        created: time::OffsetDateTime,
    ) -> PartnershipId {
        let partnership_id = PartnershipId::new();
        sqlx::query(
            "INSERT INTO partnerships (partnership_id, party_id, created, updated) VALUES ($1, $2, $3, $3)",
        )
        .bind(uuid::Uuid::from(partnership_id))
        .bind(uuid::Uuid::from(party_id))
        .bind(created)
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
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(format!("source-{listing_source_id}"))
        .bind("Reader source")
        .bind(uuid::Uuid::from(operator_party_id))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader listing source: {error}"));
        listing_source_id
    }

    async fn add_member(pool: &PgPool, user_id: UserId, partnership_id: PartnershipId) {
        sqlx::query("INSERT INTO partnership_members (user_id, partnership_id) VALUES ($1, $2)")
            .bind(uuid::Uuid::from(user_id))
            .bind(uuid::Uuid::from(partnership_id))
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
        .bind(uuid::Uuid::from(partnership_id))
        .bind(uuid::Uuid::from(listing_source_id))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader source grant: {error}"));
    }

    async fn read_page(request: &ListAdminPartnershipsRequest) -> ListAdminPartnershipsResult {
        let pool = get_postgres_client().await;
        let unit_of_work = platform_postgres::SqlxUnitOfWork::new(pool);
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("failed to begin reader transaction: {error}"));
        let result = SqlxPartnershipSearchReaderFactory::new()
            .in_transaction(&mut tx)
            .search(request)
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
        let result = AdminPartnershipSummary::try_from(PartnershipSearchRow {
            partnership_id: uuid::Uuid::new_v4(),
            party_id: uuid::Uuid::new_v4(),
            party_slug_id: "Not a slug".to_owned(),
            party_name: "Valid party".to_owned(),
            member_count: 0,
            listing_source_grant_count: 0,
            created: datetime!(2026-01-01 00:00 UTC),
            updated: datetime!(2026-01-01 00:00 UTC),
        });

        assert!(matches!(
            result,
            Err(PartnershipSearchRowMappingError::PartySlug(_))
        ));
    }

    #[test]
    fn should_reject_negative_persisted_counts() {
        let result = AdminPartnershipSummary::try_from(PartnershipSearchRow {
            partnership_id: uuid::Uuid::new_v4(),
            party_id: uuid::Uuid::new_v4(),
            party_slug_id: "valid-party".to_owned(),
            party_name: "Valid party".to_owned(),
            member_count: -1,
            listing_source_grant_count: 0,
            created: datetime!(2026-01-01 00:00 UTC),
            updated: datetime!(2026-01-01 00:00 UTC),
        });

        assert!(matches!(
            result,
            Err(PartnershipSearchRowMappingError::MemberCount(_))
        ));
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_filter_by_party_member_and_listing_source_without_changing_counts() {
        let pool = get_postgres_client().await;
        let matching_party = seed_party(&pool, "Matching party").await;
        let other_party = seed_party(&pool, "Other party").await;
        let matching_user = seed_user(&pool).await;
        let second_matching_user = seed_user(&pool).await;
        let other_user = seed_user(&pool).await;
        let matching_partnership =
            seed_partnership(&pool, matching_party, datetime!(2026-01-02 00:00 UTC)).await;
        let other_partnership =
            seed_partnership(&pool, other_party, datetime!(2026-01-01 00:00 UTC)).await;
        let matching_source = seed_listing_source(&pool, matching_party).await;
        let second_matching_source = seed_listing_source(&pool, matching_party).await;
        let other_source = seed_listing_source(&pool, other_party).await;
        add_member(&pool, matching_user, matching_partnership).await;
        add_member(&pool, second_matching_user, matching_partnership).await;
        add_member(&pool, other_user, other_partnership).await;
        add_grant(&pool, matching_partnership, matching_source).await;
        add_grant(&pool, matching_partnership, second_matching_source).await;
        add_grant(&pool, other_partnership, other_source).await;

        let mut request = ListAdminPartnershipsRequest {
            party_id: Some(matching_party),
            member_user_id: None,
            listing_source_id: None,
            cursor: Some(Cursor {
                size: 10,
                search_after: None,
            }),
        };
        let by_party = read_page(&request).await;
        assert_eq!(
            vec![matching_partnership],
            by_party
                .items
                .iter()
                .map(|item| item.partnership_id)
                .collect::<Vec<_>>()
        );
        assert_eq!(2, by_party.items[0].member_count);
        assert_eq!(2, by_party.items[0].listing_source_grant_count);

        request.party_id = None;
        request.member_user_id = Some(matching_user);
        let by_member = read_page(&request).await;
        assert_eq!(
            vec![matching_partnership],
            by_member
                .items
                .iter()
                .map(|item| item.partnership_id)
                .collect::<Vec<_>>()
        );

        request.member_user_id = None;
        request.listing_source_id = Some(matching_source);
        let by_source = read_page(&request).await;
        assert_eq!(
            vec![matching_partnership],
            by_source
                .items
                .iter()
                .map(|item| item.partnership_id)
                .collect::<Vec<_>>()
        );
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_return_partnerships_in_created_descending_order_with_tied_cursor() {
        let pool = get_postgres_client().await;
        let created = datetime!(2026-05-05 12:00 UTC);
        let mut ids = Vec::new();
        for index in 0..3 {
            let party = seed_party(&pool, &format!("Tied party {index}")).await;
            ids.push(seed_partnership(&pool, party, created).await);
        }
        let mut expected_ids = ids;
        expected_ids.sort_by(|left, right| right.cmp(left));

        let request = ListAdminPartnershipsRequest {
            cursor: Some(Cursor {
                size: 2,
                search_after: None,
            }),
            ..Default::default()
        };
        let first = read_page(&request).await;
        assert_eq!(
            expected_ids[..2].to_vec(),
            first
                .items
                .iter()
                .map(|item| item.partnership_id)
                .collect::<Vec<_>>()
        );
        let first_cursor = first
            .cursor
            .search_after
            .unwrap_or_else(|| panic!("first page should have a continuation cursor"));

        let second = read_page(&ListAdminPartnershipsRequest {
            cursor: Some(Cursor {
                size: 2,
                search_after: Some(first_cursor),
            }),
            ..request
        })
        .await;

        assert_eq!(1, second.items.len());
        assert_eq!(expected_ids[2], second.items[0].partnership_id);
        assert!(second.cursor.search_after.is_none());
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_return_an_empty_partnership_page() {
        let result = read_page(&ListAdminPartnershipsRequest {
            cursor: Some(Cursor {
                size: 2,
                search_after: None,
            }),
            ..Default::default()
        })
        .await;

        assert!(result.items.is_empty());
        assert!(result.cursor.search_after.is_none());
    }
}
