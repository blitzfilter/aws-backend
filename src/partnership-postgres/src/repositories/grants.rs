use application::error::box_error;
use listing_source_core::ListingSourceId;
use partnership_core::partnership_id::PartnershipId;
use partnership_service::ports::*;
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxListingSourceGrantRepositoryFactory;
struct Repository<'a> {
    connection: &'a mut PgConnection,
}
impl SqlxListingSourceGrantRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}
impl ListingSourceGrantRepositoryFactory<SqlxTransaction>
    for SqlxListingSourceGrantRepositoryFactory
{
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl ListingSourceGrantRepository + 'a {
        Repository {
            connection: tx.connection(),
        }
    }
}
#[async_trait::async_trait]
impl ListingSourceGrantRepository for Repository<'_> {
    async fn grant_source_access(
        &mut self,
        partnership_id: PartnershipId,
        listing_source_id: ListingSourceId,
    ) -> Result<ListingSourceGrantOutcome, PartnershipGrantError> {
        let result = sqlx::query(
            "INSERT INTO partnership_listing_source_grants(partnership_id, listing_source_id) VALUES($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(uuid::Uuid::from(partnership_id))
        .bind(uuid::Uuid::from(listing_source_id))
        .execute(&mut *self.connection)
        .await
        .map_err(|source| PartnershipGrantError::Internal {
            source: box_error(source),
        })?;
        Ok(if result.rows_affected() > 0 {
            ListingSourceGrantOutcome::Granted
        } else {
            ListingSourceGrantOutcome::AlreadyGranted
        })
    }

    async fn remove_source_access(
        &mut self,
        partnership_id: PartnershipId,
        listing_source_id: ListingSourceId,
    ) -> Result<ListingSourceGrantRemoveOutcome, PartnershipGrantError> {
        let result = sqlx::query(
            "DELETE FROM partnership_listing_source_grants WHERE partnership_id=$1 AND listing_source_id=$2",
        )
        .bind(uuid::Uuid::from(partnership_id))
        .bind(uuid::Uuid::from(listing_source_id))
        .execute(&mut *self.connection)
        .await
        .map_err(|source| PartnershipGrantError::Internal {
            source: box_error(source),
        })?;
        Ok(if result.rows_affected() > 0 {
            ListingSourceGrantRemoveOutcome::Removed
        } else {
            ListingSourceGrantRemoveOutcome::AlreadyAbsent
        })
    }
}
