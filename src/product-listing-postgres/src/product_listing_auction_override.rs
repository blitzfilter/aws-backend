use application::error::box_error;
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{
    ProductListingAuctionOverride, ProductListingAuctionOverrideAudit,
    ProductListingAuctionOverrideError, ProductListingAuctionOverrideRepository,
    ProductListingAuctionOverrideRepositoryFactory, ProductListingAuctionPolicyVersion,
};
use sqlx::PgConnection;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingAuctionOverrideRepositoryFactory;

struct SqlxProductListingAuctionOverrideRepository<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(sqlx::FromRow)]
struct OverrideRow {
    policy_version: i64,
    active: bool,
    release_capture_generation: Option<i64>,
}

impl SqlxProductListingAuctionOverrideRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingAuctionOverrideRepositoryFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingAuctionOverrideRepositoryFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingAuctionOverrideRepository + 'tx {
        SqlxProductListingAuctionOverrideRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingAuctionOverrideRepository for SqlxProductListingAuctionOverrideRepository<'_> {
    async fn find(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ProductListingAuctionOverride>, ProductListingAuctionOverrideError> {
        sqlx::query_as::<_, OverrideRow>(
            "SELECT policy_version, active, release_capture_generation \
             FROM product_listing_auction_overrides WHERE product_listing_id = $1",
        )
        .bind(product_listing_id.as_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(persistence)?
        .map(override_from_row)
        .transpose()
    }

    async fn activate(
        &mut self,
        audit: &ProductListingAuctionOverrideAudit,
        expected_version: ProductListingAuctionPolicyVersion,
    ) -> Result<ProductListingAuctionOverride, ProductListingAuctionOverrideError> {
        let expected = version_to_i64(expected_version)?;
        sqlx::query(
            "INSERT INTO product_listing_auction_corrections (\
                audit_id, product_listing_id, actor_label, reason, previous_auction_id, current_auction_id, recorded_at\
             ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(audit.audit_id.as_uuid())
        .bind(audit.product_listing_id.as_uuid())
        .bind(&audit.actor_label)
        .bind(&audit.reason)
        .bind(audit.previous_auction_id.map(|value| *value.as_uuid()))
        .bind(audit.current_auction_id.map(|value| *value.as_uuid()))
        .bind(audit.recorded_at)
        .execute(&mut *self.connection)
        .await
        .map_err(persistence)?;

        let row = if expected == 0 {
            sqlx::query_as::<_, OverrideRow>(
                "INSERT INTO product_listing_auction_overrides (\
                    product_listing_id, policy_version, active, correction_audit_id, updated\
                 ) VALUES ($1, 1, TRUE, $2, now())\
                 ON CONFLICT (product_listing_id) DO NOTHING\
                 RETURNING policy_version, active, release_capture_generation",
            )
            .bind(audit.product_listing_id.as_uuid())
            .bind(audit.audit_id.as_uuid())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(persistence)?
        } else {
            sqlx::query_as::<_, OverrideRow>(
                "UPDATE product_listing_auction_overrides\
                 SET policy_version = policy_version + 1, active = TRUE, correction_audit_id = $1, updated = now()\
                 WHERE product_listing_id = $2 AND policy_version = $3\
                 RETURNING policy_version, active, release_capture_generation",
            )
            .bind(audit.audit_id.as_uuid())
            .bind(audit.product_listing_id.as_uuid())
            .bind(expected)
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(persistence)?
        };
        row.map(override_from_row)
            .transpose()?
            .ok_or(ProductListingAuctionOverrideError::ConcurrencyConflict)
    }

    async fn release(
        &mut self,
        product_listing_id: ProductListingId,
        expected_version: ProductListingAuctionPolicyVersion,
        audit_id: EventId,
        actor_label: String,
        recorded_at: OffsetDateTime,
    ) -> Result<ProductListingAuctionOverride, ProductListingAuctionOverrideError> {
        let expected = version_to_i64(expected_version)?;
        let generation = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(generation), 0) FROM product_listing_raw_revisions",
        )
        .fetch_one(&mut *self.connection)
        .await
        .map_err(persistence)?;
        if generation < 0 {
            return Err(invalid("raw capture generation is invalid"));
        }

        let row = sqlx::query_as::<_, OverrideRow>(
            "UPDATE product_listing_auction_overrides\
             SET policy_version = policy_version + 1, active = FALSE, release_capture_generation = $1, released_audit_id = $2, updated = now()\
             WHERE product_listing_id = $3 AND policy_version = $4 AND active = TRUE\
             RETURNING policy_version, active, release_capture_generation",
        )
        .bind(generation)
        .bind(audit_id.as_uuid())
        .bind(product_listing_id.as_uuid())
        .bind(expected)
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(persistence)?
        .ok_or(ProductListingAuctionOverrideError::ConcurrencyConflict)?;

        sqlx::query(
            "INSERT INTO product_listing_auction_override_releases (\
                audit_id, product_listing_id, actor_label, recorded_at, capture_generation\
             ) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(audit_id.as_uuid())
        .bind(product_listing_id.as_uuid())
        .bind(actor_label)
        .bind(recorded_at)
        .bind(generation)
        .execute(&mut *self.connection)
        .await
        .map_err(persistence)?;

        sqlx::query(
            "INSERT INTO product_listing_auction_override_floors (\
                product_listing_id, product_listing_raw_stream_id, last_capture_revision, last_capture_generation\
             )\
             SELECT $1, head.product_listing_raw_stream_id, stream.latest_revision, revision.generation\
             FROM product_listing_raw_normalization_heads AS head\
             JOIN product_listing_raw_streams AS stream\
               ON stream.product_listing_raw_stream_id = head.product_listing_raw_stream_id\
             JOIN product_listing_raw_revisions AS revision\
               ON revision.product_listing_raw_stream_id = stream.product_listing_raw_stream_id\
              AND revision.revision = stream.latest_revision\
             WHERE head.product_listing_id = $1 AND stream.latest_revision > 0\
             ON CONFLICT (product_listing_id, product_listing_raw_stream_id) DO UPDATE\
             SET last_capture_revision = EXCLUDED.last_capture_revision,\
                 last_capture_generation = EXCLUDED.last_capture_generation",
        )
        .bind(product_listing_id.as_uuid())
        .execute(&mut *self.connection)
        .await
        .map_err(persistence)?;

        override_from_row(row)
    }
}

fn version_to_i64(
    value: ProductListingAuctionPolicyVersion,
) -> Result<i64, ProductListingAuctionOverrideError> {
    i64::try_from(value.into_inner()).map_err(|_| invalid("policy version exceeds storage range"))
}

fn override_from_row(
    row: OverrideRow,
) -> Result<ProductListingAuctionOverride, ProductListingAuctionOverrideError> {
    let version =
        u64::try_from(row.policy_version).map_err(|_| invalid("policy version is invalid"))?;
    let release_capture_generation = row
        .release_capture_generation
        .map(|value| {
            u64::try_from(value).map_err(|_| invalid("release capture generation is invalid"))
        })
        .transpose()?;
    Ok(ProductListingAuctionOverride {
        version: ProductListingAuctionPolicyVersion::from(version),
        active: row.active,
        release_capture_generation,
    })
}

fn persistence(error: sqlx::Error) -> ProductListingAuctionOverrideError {
    ProductListingAuctionOverrideError::Persistence {
        source: box_error(error),
    }
}

fn invalid(message: &'static str) -> ProductListingAuctionOverrideError {
    ProductListingAuctionOverrideError::InvalidPersistedState {
        source: box_error(std::io::Error::other(message)),
    }
}
