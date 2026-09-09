use crate::mapping::{APPLICATION_COLUMNS, ApplicationRow, admin_summary, view};
use application::error::box_error;
use application::pagination::{Cursor, CursoredResult};
use domain_primitives::sort::{Sort, SortOrder};
use partnership_core::{
    partnership_application_search::PartnershipApplicationSearch,
    sort_partnership_application_field::SortPartnershipApplicationField,
};
use partnership_service::ports::{
    PartnershipApplicationReadError, PartnershipApplicationReader,
    PartnershipApplicationReaderFactory, PartnershipApplicationView,
};
use partnership_service::use_cases::queries::list_admin_partnership_applications::{
    AdminPartnershipApplicationSummary, ListAdminPartnershipApplicationsRequest,
    ListAdminPartnershipApplicationsResult, PartnershipApplicationSearchCursor,
};
use platform_postgres::SqlxTransaction;
use sqlx::{AssertSqlSafe, Postgres, QueryBuilder};
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnershipApplicationReaderFactory;

struct Reader<'a> {
    connection: &'a mut sqlx::PgConnection,
}

impl SqlxPartnershipApplicationReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartnershipApplicationReaderFactory<SqlxTransaction>
    for SqlxPartnershipApplicationReaderFactory
{
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl PartnershipApplicationReader + 'a {
        Reader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartnershipApplicationReader for Reader<'_> {
    async fn list_by_user(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<PartnershipApplicationView>, PartnershipApplicationReadError> {
        let query = format!(
            "SELECT {APPLICATION_COLUMNS} FROM partnership_applications WHERE applicant_user_id=$1 ORDER BY created DESC, partnership_application_id DESC"
        );
        let rows = sqlx::query_as::<_, ApplicationRow>(AssertSqlSafe(query))
            .bind(user_id.into_uuid())
            .fetch_all(&mut *self.connection)
            .await
            .map_err(
                |source| PartnershipApplicationReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?;
        rows.into_iter()
            .map(view)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| PartnershipApplicationReadError::InvalidReadModel {
                source: box_error(source),
            })
    }

    async fn search_admin(
        &mut self,
        request: &ListAdminPartnershipApplicationsRequest,
    ) -> Result<ListAdminPartnershipApplicationsResult, PartnershipApplicationReadError> {
        let cursor = request.cursor.unwrap_or_default();
        let size = cursor.size.clamp(1, 100);
        let size_usize =
            usize::try_from(size).map_err(|source| PartnershipApplicationReadError::Internal {
                source: box_error(source),
            })?;
        let limit = i64::try_from(size + 1).map_err(|source| {
            PartnershipApplicationReadError::Internal {
                source: box_error(source),
            }
        })?;
        let sort = request.sort.unwrap_or(Sort {
            sort: SortPartnershipApplicationField::Created,
            order: SortOrder::Desc,
        });
        let sort_column = match sort.sort {
            SortPartnershipApplicationField::Created => "created",
            SortPartnershipApplicationField::Updated => "updated",
        };
        let order = match sort.order {
            SortOrder::Asc => "ASC",
            SortOrder::Desc => "DESC",
        };

        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        builder
            .push(APPLICATION_COLUMNS)
            .push(" FROM partnership_applications WHERE TRUE");
        push_filters(&mut builder, &request.search);
        if let Some(search_after) = cursor.search_after {
            let comparison = match sort.order {
                SortOrder::Asc => ">",
                SortOrder::Desc => "<",
            };
            builder
                .push(" AND (")
                .push(sort_column)
                .push(", partnership_application_id) ")
                .push(comparison)
                .push(" (")
                .push_bind(search_after.position)
                .push(", ")
                .push_bind(search_after.application_id.into_uuid())
                .push(")");
        }
        builder
            .push(" ORDER BY ")
            .push(sort_column)
            .push(" ")
            .push(order)
            .push(", partnership_application_id ")
            .push(order)
            .push(" LIMIT ")
            .push_bind(limit);

        let mut rows = builder
            .build_query_as::<ApplicationRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(
                |source| PartnershipApplicationReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?;
        let has_more = rows.len() > size_usize;
        if has_more {
            rows.truncate(size_usize);
        }
        let items = rows
            .into_iter()
            .map(admin_summary)
            .collect::<Result<Vec<AdminPartnershipApplicationSummary>, _>>()
            .map_err(|source| PartnershipApplicationReadError::InvalidReadModel {
                source: box_error(source),
            })?;
        let search_after = if has_more {
            items.last().map(|item| PartnershipApplicationSearchCursor {
                position: match sort.sort {
                    SortPartnershipApplicationField::Created => item.created,
                    SortPartnershipApplicationField::Updated => item.updated,
                },
                application_id: item.id,
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

fn push_filters(builder: &mut QueryBuilder<Postgres>, search: &PartnershipApplicationSearch) {
    if !search.state_query.is_empty() {
        let states = search
            .state_query
            .iter()
            .copied()
            .map(|state| state.as_str().to_owned())
            .collect::<Vec<_>>();
        builder
            .push(" AND business_state = ANY(")
            .push_bind(states)
            .push(")");
    }
    if let Some(applicant_user_id) = search.applicant_user_id {
        builder
            .push(" AND applicant_user_id = ")
            .push_bind(applicant_user_id.into_uuid());
    }
    if !search.proposal_type_query.is_empty() {
        let proposal_types = search
            .proposal_type_query
            .iter()
            .copied()
            .map(|proposal_type| proposal_type.as_str().to_owned())
            .collect::<Vec<_>>();
        builder
            .push(" AND proposal->>'type' = ANY(")
            .push_bind(proposal_types)
            .push(")");
    }
    if let Some(listing_source_id) = search.listing_source_id {
        let listing_source_uuid = listing_source_id.into_uuid();
        builder
            .push(" AND (approved_listing_source_id = ")
            .push_bind(listing_source_uuid)
            .push(" OR (proposal->>'type' = 'EXISTING_LISTING_SOURCE' AND proposal->>'listing_source_id' = ")
            .push_bind(listing_source_uuid.to_string())
            .push("))");
    }
    if let Some(created) = search.created {
        if let Some(min) = created.min {
            builder.push(" AND created >= ").push_bind(min);
        }
        if let Some(max) = created.max {
            builder.push(" AND created <= ").push_bind(max);
        }
    }
    if let Some(updated) = search.updated {
        if let Some(min) = updated.min {
            builder.push(" AND updated >= ").push_bind(min);
        }
        if let Some(max) = updated.max {
            builder.push(" AND updated <= ").push_bind(max);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        pagination::Cursor,
        transaction::{Transaction, UnitOfWork},
    };
    use domain_primitives::{
        query::range_query::RangeQuery,
        sort::{Sort, SortOrder},
    };
    use listing_source_core::ListingSourceId;
    use partnership_core::{
        partnership_application_id::PartnershipApplicationId,
        partnership_application_state::PartnershipApplicationState,
        partnership_proposal_type::PartnershipProposalType,
    };
    use serde_json::json;
    use sqlx::PgPool;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
    use time::{OffsetDateTime, macros::datetime};

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    async fn seed_user(pool: &PgPool) -> UserId {
        let user_id = UserId::new();
        sqlx::query(
            "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
        )
        .bind(user_id.into_uuid())
        .bind(format!("{user_id}@reader.test"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader user: {error}"));
        user_id
    }

    async fn seed_application(
        pool: &PgPool,
        applicant_user_id: UserId,
        state: &str,
        proposal: serde_json::Value,
        created: OffsetDateTime,
        updated: OffsetDateTime,
    ) -> PartnershipApplicationId {
        ensure_existing_listing_source_proposal(pool, &proposal).await;
        let application_id = PartnershipApplicationId::new();
        sqlx::query(
            "INSERT INTO partnership_applications (partnership_application_id, applicant_user_id, business_state, proposal, created, updated) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(application_id.into_uuid())
        .bind(applicant_user_id.into_uuid())
        .bind(state)
        .bind(proposal)
        .bind(created)
        .bind(updated)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader application: {error}"));
        application_id
    }

    async fn read_page(
        request: &ListAdminPartnershipApplicationsRequest,
    ) -> ListAdminPartnershipApplicationsResult {
        let pool = get_postgres_client().await;
        let unit_of_work = platform_postgres::SqlxUnitOfWork::new(pool);
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("failed to begin reader transaction: {error}"));
        let result = SqlxPartnershipApplicationReaderFactory::new()
            .in_transaction(&mut tx)
            .search_admin(request)
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

    async fn ensure_existing_listing_source_proposal(pool: &PgPool, proposal: &serde_json::Value) {
        if proposal.get("type").and_then(serde_json::Value::as_str)
            != Some("EXISTING_LISTING_SOURCE")
        {
            return;
        }
        let listing_source_uuid = proposal
            .get("listing_source_id")
            .cloned()
            .map(serde_json::from_value::<uuid::Uuid>)
            .transpose()
            .unwrap_or_else(|error| panic!("invalid existing proposal ListingSource UUID: {error}"))
            .unwrap_or_else(|| panic!("existing proposal must contain a ListingSource UUID"));
        let listing_source_id = ListingSourceId::try_from(listing_source_uuid)
            .unwrap_or_else(|error| panic!("invalid existing proposal ListingSource ID: {error}"));
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM listing_sources WHERE listing_source_id = $1)",
        )
        .bind(listing_source_id.into_uuid())
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to check reader ListingSource: {error}"));
        if exists {
            return;
        }

        let party_id = party_core::party_id::PartyId::new();
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(party_id.into_uuid())
            .bind(format!("reader-party-{}", party_id.as_uuid().simple()))
            .bind(format!("Reader Party {party_id}"))
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("failed to seed reader Party: {error}"));
        sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(listing_source_id.into_uuid())
        .bind(format!(
                    "reader-source-{}",
                    listing_source_id.as_uuid().simple()
                ))
        .bind(format!("Reader ListingSource {listing_source_id}"))
        .bind(party_id.into_uuid())
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed reader ListingSource: {error}"));
    }

    fn existing_proposal(listing_source_id: ListingSourceId) -> serde_json::Value {
        json!({
            "type": "EXISTING_LISTING_SOURCE",
            "listing_source_id": listing_source_id.into_uuid(),
        })
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_apply_admin_application_filters_and_inclusive_ranges() {
        let pool = get_postgres_client().await;
        let applicant_user_id = seed_user(&pool).await;
        let listing_source_id = ListingSourceId::new();
        let matching_id = seed_application(
            &pool,
            applicant_user_id,
            "SUBMITTED",
            existing_proposal(listing_source_id),
            datetime!(2026-01-01 00:00 UTC),
            datetime!(2026-02-01 00:00 UTC),
        )
        .await;
        let _other_id = seed_application(
            &pool,
            applicant_user_id,
            "IN_REVIEW",
            existing_proposal(ListingSourceId::new()),
            datetime!(2026-01-02 00:00 UTC),
            datetime!(2026-02-02 00:00 UTC),
        )
        .await;
        let mut search = PartnershipApplicationSearch::default();
        search
            .state_query
            .extend([PartnershipApplicationState::Submitted]);
        search
            .proposal_type_query
            .extend([PartnershipProposalType::ExistingListingSource]);
        search.applicant_user_id = Some(applicant_user_id);
        search.listing_source_id = Some(listing_source_id);
        search.created = Some(RangeQuery {
            min: Some(datetime!(2026-01-01 00:00 UTC)),
            max: Some(datetime!(2026-01-01 00:00 UTC)),
        });
        search.updated = Some(RangeQuery {
            min: Some(datetime!(2026-02-01 00:00 UTC)),
            max: Some(datetime!(2026-02-01 00:00 UTC)),
        });
        let request = ListAdminPartnershipApplicationsRequest {
            search,
            sort: Some(Sort {
                sort: SortPartnershipApplicationField::Updated,
                order: SortOrder::Desc,
            }),
            cursor: Some(Cursor {
                size: 100,
                search_after: None,
            }),
        };

        let result = read_page(&request).await;

        assert_eq!(1, result.items.len());
        assert_eq!(matching_id, result.items[0].id);
        assert_eq!(datetime!(2026-01-01 00:00 UTC), result.items[0].created);
        assert_eq!(datetime!(2026-02-01 00:00 UTC), result.items[0].updated);
        assert!(result.cursor.search_after.is_none());
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_follow_descending_cursor_through_tied_created_timestamps() {
        let pool = get_postgres_client().await;
        let applicant_user_id = seed_user(&pool).await;
        let created = datetime!(2026-05-05 12:00 UTC);
        let ids = vec![
            seed_application(
                &pool,
                applicant_user_id,
                "SUBMITTED",
                existing_proposal(ListingSourceId::new()),
                created,
                created,
            )
            .await,
            seed_application(
                &pool,
                applicant_user_id,
                "SUBMITTED",
                existing_proposal(ListingSourceId::new()),
                created,
                created,
            )
            .await,
            seed_application(
                &pool,
                applicant_user_id,
                "SUBMITTED",
                existing_proposal(ListingSourceId::new()),
                created,
                created,
            )
            .await,
        ];
        let mut expected_ids = ids.clone();
        expected_ids.sort_by(|left, right| right.cmp(left));
        let request = ListAdminPartnershipApplicationsRequest {
            search: PartnershipApplicationSearch::default(),
            sort: None,
            cursor: Some(Cursor {
                size: 2,
                search_after: None,
            }),
        };

        let first = read_page(&request).await;
        assert_eq!(
            expected_ids[..2].to_vec(),
            first.items.iter().map(|item| item.id).collect::<Vec<_>>()
        );
        let first_cursor = first
            .cursor
            .search_after
            .unwrap_or_else(|| panic!("first page should have a continuation cursor"));
        let second_request = ListAdminPartnershipApplicationsRequest {
            cursor: Some(Cursor {
                size: 2,
                search_after: Some(first_cursor),
            }),
            ..request
        };

        let second = read_page(&second_request).await;

        assert_eq!(1, second.items.len());
        assert_eq!(expected_ids[2], second.items[0].id);
        assert!(second.cursor.search_after.is_none());
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_return_an_empty_admin_application_page() {
        let request = ListAdminPartnershipApplicationsRequest {
            search: PartnershipApplicationSearch::default(),
            sort: None,
            cursor: Some(Cursor {
                size: 2,
                search_after: None,
            }),
        };

        let result = read_page(&request).await;

        assert!(result.items.is_empty());
        assert!(result.cursor.search_after.is_none());
    }
}
