use admin_overview_service::{
    ports::{AdminOverviewReadError, AdminOverviewReader, AdminOverviewReaderFactory},
    use_cases::get_admin_overview::{
        AdminOverview, AdminOverviewActiveListingAvailabilityCounts,
        AdminOverviewListingSourceMethodAssignmentCounts, AdminOverviewListingSources,
        AdminOverviewPartnershipApplicationStateCounts, AdminOverviewPartnershipApplications,
        AdminOverviewProductListingLifecycleCounts, AdminOverviewProductListings,
        AdminOverviewUserRoleCounts, AdminOverviewUserTierCounts, AdminOverviewUsers,
    },
};
use application::error::box_error;
use platform_postgres::SqlxTransaction;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxAdminOverviewReaderFactory;

struct SqlxAdminOverviewReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct AdminOverviewRow {
    users_total: i64,
    users_free: i64,
    users_pro: i64,
    users_ultimate: i64,
    users_user: i64,
    users_admin: i64,
    partnership_applications_total: i64,
    partnership_applications_submitted: i64,
    partnership_applications_in_review: i64,
    partnership_applications_approved: i64,
    partnership_applications_rejected: i64,
    partnership_applications_withdrawn: i64,
    parties_total: i64,
    listing_sources_total: i64,
    listing_sources_without_ingestion_method: i64,
    listing_source_web_crawl_assignments: i64,
    listing_source_shopify_assignments: i64,
    listing_source_woocommerce_assignments: i64,
    listing_source_partner_api_assignments: i64,
    partnerships_total: i64,
    product_listings_total: i64,
    product_listings_active: i64,
    product_listings_withdrawn: i64,
    active_available: i64,
    active_in_stock: i64,
    active_limited_availability: i64,
    active_back_order: i64,
    active_made_to_order: i64,
    active_pre_order: i64,
    active_pre_sale: i64,
    active_unavailable: i64,
    active_reserved: i64,
    active_out_of_stock: i64,
    active_sold_out: i64,
    active_without_availability: i64,
}

#[derive(Debug, thiserror::Error)]
enum AdminOverviewRowMappingError {
    #[error("invalid persisted {field} count")]
    Count {
        field: &'static str,
        #[source]
        source: std::num::TryFromIntError,
    },
}

impl SqlxAdminOverviewReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl AdminOverviewReaderFactory<SqlxTransaction> for SqlxAdminOverviewReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl AdminOverviewReader + 'tx {
        SqlxAdminOverviewReader {
            connection: tx.connection(),
        }
    }
}

const ADMIN_OVERVIEW_SQL: &str = r#"
WITH
user_counts AS (
    SELECT
        COUNT(*)::bigint AS users_total,
        COUNT(*) FILTER (WHERE tier = 'FREE')::bigint AS users_free,
        COUNT(*) FILTER (WHERE tier = 'PRO')::bigint AS users_pro,
        COUNT(*) FILTER (WHERE tier = 'ULTIMATE')::bigint AS users_ultimate,
        COUNT(*) FILTER (WHERE role = 'USER')::bigint AS users_user,
        COUNT(*) FILTER (WHERE role = 'ADMIN')::bigint AS users_admin
    FROM users
),
partnership_application_counts AS (
    SELECT
        COUNT(*)::bigint AS partnership_applications_total,
        COUNT(*) FILTER (WHERE business_state = 'SUBMITTED')::bigint AS partnership_applications_submitted,
        COUNT(*) FILTER (WHERE business_state = 'IN_REVIEW')::bigint AS partnership_applications_in_review,
        COUNT(*) FILTER (WHERE business_state = 'APPROVED')::bigint AS partnership_applications_approved,
        COUNT(*) FILTER (WHERE business_state = 'REJECTED')::bigint AS partnership_applications_rejected,
        COUNT(*) FILTER (WHERE business_state = 'WITHDRAWN')::bigint AS partnership_applications_withdrawn
    FROM partnership_applications
),
party_counts AS (
    SELECT COUNT(*)::bigint AS parties_total
    FROM parties
),
listing_source_counts AS (
    SELECT
        COUNT(*)::bigint AS listing_sources_total,
        COUNT(*) FILTER (
            WHERE NOT EXISTS (
                SELECT 1
                FROM listing_source_ingestion_methods methods
                WHERE methods.listing_source_id = sources.listing_source_id
            )
        )::bigint AS listing_sources_without_ingestion_method
    FROM listing_sources sources
),
listing_source_method_counts AS (
    SELECT
        COUNT(*) FILTER (WHERE ingestion_method = 'WEB_CRAWL')::bigint AS listing_source_web_crawl_assignments,
        COUNT(*) FILTER (WHERE ingestion_method = 'SHOPIFY')::bigint AS listing_source_shopify_assignments,
        COUNT(*) FILTER (WHERE ingestion_method = 'WOOCOMMERCE')::bigint AS listing_source_woocommerce_assignments,
        COUNT(*) FILTER (WHERE ingestion_method = 'PARTNER_API')::bigint AS listing_source_partner_api_assignments
    FROM listing_source_ingestion_methods
),
partnership_counts AS (
    SELECT COUNT(*)::bigint AS partnerships_total
    FROM partnerships
),
product_listing_counts AS (
    SELECT
        COUNT(*)::bigint AS product_listings_total,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE')::bigint AS product_listings_active,
        COUNT(*) FILTER (WHERE lifecycle = 'WITHDRAWN')::bigint AS product_listings_withdrawn,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'AVAILABLE')::bigint AS active_available,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'IN_STOCK')::bigint AS active_in_stock,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'LIMITED_AVAILABILITY')::bigint AS active_limited_availability,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'BACK_ORDER')::bigint AS active_back_order,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'MADE_TO_ORDER')::bigint AS active_made_to_order,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'PRE_ORDER')::bigint AS active_pre_order,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'PRE_SALE')::bigint AS active_pre_sale,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'UNAVAILABLE')::bigint AS active_unavailable,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'RESERVED')::bigint AS active_reserved,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'OUT_OF_STOCK')::bigint AS active_out_of_stock,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'SOLD_OUT')::bigint AS active_sold_out,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability IS NULL)::bigint AS active_without_availability
    FROM product_listings
)
SELECT *
FROM user_counts
CROSS JOIN partnership_application_counts
CROSS JOIN party_counts
CROSS JOIN listing_source_counts
CROSS JOIN listing_source_method_counts
CROSS JOIN partnership_counts
CROSS JOIN product_listing_counts
"#;

#[async_trait::async_trait]
impl AdminOverviewReader for SqlxAdminOverviewReader<'_> {
    async fn read_overview(&mut self) -> Result<AdminOverview, AdminOverviewReadError> {
        let row = sqlx::query_as::<_, AdminOverviewRow>(ADMIN_OVERVIEW_SQL)
            .fetch_one(&mut *self.connection)
            .await
            .map_err(|source| AdminOverviewReadError::TemporarilyUnavailable {
                source: box_error(source),
            })?;

        AdminOverview::try_from(row).map_err(|source| AdminOverviewReadError::InvalidReadModel {
            source: box_error(source),
        })
    }
}

impl TryFrom<AdminOverviewRow> for AdminOverview {
    type Error = AdminOverviewRowMappingError;

    fn try_from(row: AdminOverviewRow) -> Result<Self, Self::Error> {
        let count = |field, value| {
            u64::try_from(value)
                .map_err(|source| AdminOverviewRowMappingError::Count { field, source })
        };

        Ok(Self {
            users: AdminOverviewUsers {
                total: count("users_total", row.users_total)?,
                by_tier: AdminOverviewUserTierCounts {
                    free: count("users_free", row.users_free)?,
                    pro: count("users_pro", row.users_pro)?,
                    ultimate: count("users_ultimate", row.users_ultimate)?,
                },
                by_role: AdminOverviewUserRoleCounts {
                    user: count("users_user", row.users_user)?,
                    admin: count("users_admin", row.users_admin)?,
                },
            },
            partnership_applications: AdminOverviewPartnershipApplications {
                total: count(
                    "partnership_applications_total",
                    row.partnership_applications_total,
                )?,
                by_state: AdminOverviewPartnershipApplicationStateCounts {
                    submitted: count(
                        "partnership_applications_submitted",
                        row.partnership_applications_submitted,
                    )?,
                    in_review: count(
                        "partnership_applications_in_review",
                        row.partnership_applications_in_review,
                    )?,
                    approved: count(
                        "partnership_applications_approved",
                        row.partnership_applications_approved,
                    )?,
                    rejected: count(
                        "partnership_applications_rejected",
                        row.partnership_applications_rejected,
                    )?,
                    withdrawn: count(
                        "partnership_applications_withdrawn",
                        row.partnership_applications_withdrawn,
                    )?,
                },
            },
            parties_total: count("parties_total", row.parties_total)?,
            listing_sources: AdminOverviewListingSources {
                total: count("listing_sources_total", row.listing_sources_total)?,
                without_ingestion_method: count(
                    "listing_sources_without_ingestion_method",
                    row.listing_sources_without_ingestion_method,
                )?,
                method_assignments: AdminOverviewListingSourceMethodAssignmentCounts {
                    web_crawl: count(
                        "listing_source_web_crawl_assignments",
                        row.listing_source_web_crawl_assignments,
                    )?,
                    shopify: count(
                        "listing_source_shopify_assignments",
                        row.listing_source_shopify_assignments,
                    )?,
                    woocommerce: count(
                        "listing_source_woocommerce_assignments",
                        row.listing_source_woocommerce_assignments,
                    )?,
                    partner_api: count(
                        "listing_source_partner_api_assignments",
                        row.listing_source_partner_api_assignments,
                    )?,
                },
            },
            partnerships_total: count("partnerships_total", row.partnerships_total)?,
            product_listings: AdminOverviewProductListings {
                total: count("product_listings_total", row.product_listings_total)?,
                by_lifecycle: AdminOverviewProductListingLifecycleCounts {
                    active: count("product_listings_active", row.product_listings_active)?,
                    withdrawn: count("product_listings_withdrawn", row.product_listings_withdrawn)?,
                },
                active_availability: AdminOverviewActiveListingAvailabilityCounts {
                    available: count("active_available", row.active_available)?,
                    in_stock: count("active_in_stock", row.active_in_stock)?,
                    limited_availability: count(
                        "active_limited_availability",
                        row.active_limited_availability,
                    )?,
                    back_order: count("active_back_order", row.active_back_order)?,
                    made_to_order: count("active_made_to_order", row.active_made_to_order)?,
                    pre_order: count("active_pre_order", row.active_pre_order)?,
                    pre_sale: count("active_pre_sale", row.active_pre_sale)?,
                    unavailable: count("active_unavailable", row.active_unavailable)?,
                    reserved: count("active_reserved", row.active_reserved)?,
                    out_of_stock: count("active_out_of_stock", row.active_out_of_stock)?,
                    sold_out: count("active_sold_out", row.active_sold_out)?,
                },
                active_without_availability: count(
                    "active_without_availability",
                    row.active_without_availability,
                )?,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::transaction::{Transaction, UnitOfWork};
    use serde_json::json;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
    use uuid::Uuid;

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    async fn read_overview() -> AdminOverview {
        let pool = get_postgres_client().await;
        let unit_of_work = platform_postgres::SqlxUnitOfWork::new(pool);
        let mut tx = match unit_of_work.begin().await {
            Ok(transaction) => transaction,
            Err(error) => panic!("failed to begin overview transaction: {error}"),
        };
        let result = SqlxAdminOverviewReaderFactory::new()
            .in_transaction(&mut tx)
            .read_overview()
            .await;
        match result {
            Ok(overview) => match tx.commit().await {
                Ok(()) => overview,
                Err(error) => panic!("failed to commit overview transaction: {error}"),
            },
            Err(error) => panic!("failed to read overview: {error}"),
        }
    }

    async fn seed_user(pool: &sqlx::PgPool, tier: &str, role: &str) -> Uuid {
        let user_id = Uuid::new_v4();
        let result =
            sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, $3, $4)")
                .bind(user_id)
                .bind(format!("{user_id}@admin-overview.test"))
                .bind(tier)
                .bind(role)
                .execute(pool)
                .await;
        if let Err(error) = result {
            panic!("failed to seed user: {error}");
        }
        user_id
    }

    async fn seed_party(pool: &sqlx::PgPool) -> Uuid {
        let party_id = Uuid::new_v4();
        let result =
            sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
                .bind(party_id)
                .bind(format!("party-{}", Uuid::new_v4().simple()))
                .bind("Admin overview party")
                .execute(pool)
                .await;
        if let Err(error) = result {
            panic!("failed to seed party: {error}");
        }
        party_id
    }

    async fn seed_listing_source(pool: &sqlx::PgPool, party_id: Uuid) -> Uuid {
        let listing_source_id = Uuid::new_v4();
        let result = sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(listing_source_id)
        .bind(format!("source-{}", Uuid::new_v4().simple()))
        .bind("Admin overview source")
        .bind(party_id)
        .execute(pool)
        .await;
        if let Err(error) = result {
            panic!("failed to seed listing source: {error}");
        }
        listing_source_id
    }

    async fn add_ingestion_method(pool: &sqlx::PgPool, listing_source_id: Uuid, method: &str) {
        let result = sqlx::query(
            "INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, $2)",
        )
        .bind(listing_source_id)
        .bind(method)
        .execute(pool)
        .await;
        if let Err(error) = result {
            panic!("failed to seed ingestion method: {error}");
        }
    }

    async fn seed_partnership(pool: &sqlx::PgPool, party_id: Uuid) -> Uuid {
        let partnership_id = Uuid::new_v4();
        let result =
            sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
                .bind(partnership_id)
                .bind(party_id)
                .execute(pool)
                .await;
        if let Err(error) = result {
            panic!("failed to seed partnership: {error}");
        }
        partnership_id
    }

    async fn seed_application(
        pool: &sqlx::PgPool,
        applicant_user_id: Uuid,
        state: &str,
        listing_source_id: Uuid,
        approved_partnership_id: Option<Uuid>,
    ) {
        let result = sqlx::query(
            "INSERT INTO partnership_applications (partnership_application_id, applicant_user_id, business_state, proposal, approved_partnership_id, approved_listing_source_id) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(applicant_user_id)
        .bind(state)
        .bind(json!({ "type": "EXISTING_LISTING_SOURCE", "listing_source_id": listing_source_id.to_string() }))
        .bind(approved_partnership_id)
        .bind(approved_partnership_id.map(|_| listing_source_id))
        .execute(pool)
        .await;
        if let Err(error) = result {
            panic!("failed to seed partnership application: {error}");
        }
    }

    async fn seed_product_listing(
        pool: &sqlx::PgPool,
        listing_source_id: Uuid,
        lifecycle: &str,
        availability: Option<&str>,
    ) {
        let product_listing_id = Uuid::new_v4();
        let event_id = Uuid::new_v4();
        let slug_suffix = Uuid::new_v4().simple().to_string();
        let product_listing_slug_id = format!("listing-{}", &slug_suffix[..6]);
        let mut transaction = match pool.begin().await {
            Ok(transaction) => transaction,
            Err(error) => panic!("failed to begin product listing seed transaction: {error}"),
        };
        let listing_result = sqlx::query(
            "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, lifecycle, availability, url) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, $7, $8)",
        )
        .bind(product_listing_id)
        .bind(product_listing_slug_id)
        .bind(event_id)
        .bind(listing_source_id)
        .bind(product_listing_id.to_string())
        .bind(lifecycle)
        .bind(availability)
        .bind("https://example.test/listing")
        .execute(&mut *transaction)
        .await;
        if let Err(error) = listing_result {
            panic!("failed to seed product listing: {error}");
        }
        let event_result = sqlx::query(
            "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())",
        )
        .bind(event_id)
        .bind(product_listing_id)
        .bind(json!({}))
        .execute(&mut *transaction)
        .await;
        if let Err(error) = event_result {
            panic!("failed to seed product listing event: {error}");
        }
        if let Err(error) = transaction.commit().await {
            panic!("failed to commit product listing seed transaction: {error}");
        }
    }

    #[test]
    fn should_reject_negative_persisted_count() {
        let result = AdminOverview::try_from(AdminOverviewRow {
            users_total: -1,
            users_free: 0,
            users_pro: 0,
            users_ultimate: 0,
            users_user: 0,
            users_admin: 0,
            partnership_applications_total: 0,
            partnership_applications_submitted: 0,
            partnership_applications_in_review: 0,
            partnership_applications_approved: 0,
            partnership_applications_rejected: 0,
            partnership_applications_withdrawn: 0,
            parties_total: 0,
            listing_sources_total: 0,
            listing_sources_without_ingestion_method: 0,
            listing_source_web_crawl_assignments: 0,
            listing_source_shopify_assignments: 0,
            listing_source_woocommerce_assignments: 0,
            listing_source_partner_api_assignments: 0,
            partnerships_total: 0,
            product_listings_total: 0,
            product_listings_active: 0,
            product_listings_withdrawn: 0,
            active_available: 0,
            active_in_stock: 0,
            active_limited_availability: 0,
            active_back_order: 0,
            active_made_to_order: 0,
            active_pre_order: 0,
            active_pre_sale: 0,
            active_unavailable: 0,
            active_reserved: 0,
            active_out_of_stock: 0,
            active_sold_out: 0,
            active_without_availability: 0,
        });

        assert!(matches!(
            result,
            Err(AdminOverviewRowMappingError::Count { .. })
        ));
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_return_empty_overview() {
        assert_eq!(AdminOverview::default(), read_overview().await);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_aggregate_representative_authoritative_data() {
        let pool = get_postgres_client().await;
        let free_user = seed_user(&pool, "FREE", "USER").await;
        let pro_admin = seed_user(&pool, "PRO", "ADMIN").await;
        let _ultimate_user = seed_user(&pool, "ULTIMATE", "USER").await;
        let first_party = seed_party(&pool).await;
        let second_party = seed_party(&pool).await;
        let first_source = seed_listing_source(&pool, first_party).await;
        let second_source = seed_listing_source(&pool, second_party).await;
        add_ingestion_method(&pool, first_source, "WEB_CRAWL").await;
        add_ingestion_method(&pool, first_source, "SHOPIFY").await;
        let partnership_id = seed_partnership(&pool, first_party).await;
        seed_application(&pool, free_user, "SUBMITTED", first_source, None).await;
        seed_application(
            &pool,
            pro_admin,
            "APPROVED",
            first_source,
            Some(partnership_id),
        )
        .await;
        seed_product_listing(&pool, first_source, "ACTIVE", Some("AVAILABLE")).await;
        seed_product_listing(&pool, first_source, "ACTIVE", None).await;
        seed_product_listing(&pool, second_source, "WITHDRAWN", None).await;

        let overview = read_overview().await;

        assert_eq!(3, overview.users.total);
        assert_eq!(1, overview.users.by_tier.free);
        assert_eq!(1, overview.users.by_tier.pro);
        assert_eq!(1, overview.users.by_tier.ultimate);
        assert_eq!(2, overview.users.by_role.user);
        assert_eq!(1, overview.users.by_role.admin);
        assert_eq!(2, overview.partnership_applications.total);
        assert_eq!(1, overview.partnership_applications.by_state.submitted);
        assert_eq!(1, overview.partnership_applications.by_state.approved);
        assert_eq!(2, overview.parties_total);
        assert_eq!(2, overview.listing_sources.total);
        assert_eq!(1, overview.listing_sources.without_ingestion_method);
        assert_eq!(1, overview.listing_sources.method_assignments.web_crawl);
        assert_eq!(1, overview.listing_sources.method_assignments.shopify);
        assert_eq!(0, overview.listing_sources.method_assignments.woocommerce);
        assert_eq!(0, overview.listing_sources.method_assignments.partner_api);
        assert_eq!(1, overview.partnerships_total);
        assert_eq!(3, overview.product_listings.total);
        assert_eq!(2, overview.product_listings.by_lifecycle.active);
        assert_eq!(1, overview.product_listings.by_lifecycle.withdrawn);
        assert_eq!(1, overview.product_listings.active_availability.available);
        assert_eq!(1, overview.product_listings.active_without_availability);
    }
}
