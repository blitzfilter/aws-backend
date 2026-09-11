use application::error::box_error;
use auction_service::ports::{
    AuctionMetadataField, AuctionMetadataPolicyAudit, AuctionMetadataPolicyRepository,
    AuctionMetadataPolicyRepositoryError, AuctionMetadataPolicyRepositoryFactory,
};
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;
use std::collections::BTreeSet;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxAuctionMetadataPolicyRepositoryFactory;
struct SqlxAuctionMetadataPolicyRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxAuctionMetadataPolicyRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl AuctionMetadataPolicyRepositoryFactory<SqlxTransaction>
    for SqlxAuctionMetadataPolicyRepositoryFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl AuctionMetadataPolicyRepository + 'tx {
        SqlxAuctionMetadataPolicyRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl AuctionMetadataPolicyRepository for SqlxAuctionMetadataPolicyRepository<'_> {
    async fn find_protected_fields(
        &mut self,
        auction_id: auction_core::AuctionId,
    ) -> Result<BTreeSet<AuctionMetadataField>, AuctionMetadataPolicyRepositoryError> {
        let codes = sqlx::query_scalar::<_, String>(
            "SELECT field_code FROM auction_metadata_field_protections WHERE auction_id=$1",
        )
        .bind(auction_id.as_uuid())
        .fetch_all(&mut *self.connection)
        .await
        .map_err(read_error)?;
        codes
            .into_iter()
            .map(|code| {
                AuctionMetadataField::from_str(&code).map_err(|error| {
                    AuctionMetadataPolicyRepositoryError::InvalidPersistedState {
                        source: box_error(error),
                    }
                })
            })
            .collect()
    }

    async fn protect(
        &mut self,
        audit: &AuctionMetadataPolicyAudit,
    ) -> Result<(), AuctionMetadataPolicyRepositoryError> {
        sqlx::query("INSERT INTO auction_metadata_policy_audits (audit_id, auction_id, actor_label, recorded_at) VALUES ($1,$2,$3,$4)")
            .bind(audit.audit_id.as_uuid()).bind(audit.auction_id.as_uuid()).bind(&audit.actor_label).bind(audit.recorded_at)
            .execute(&mut *self.connection).await.map_err(write_error)?;
        for field in &audit.fields {
            sqlx::query("INSERT INTO auction_metadata_field_protections (auction_id, field_code, latest_audit_id) VALUES ($1,$2,$3) ON CONFLICT (auction_id, field_code) DO UPDATE SET latest_audit_id=EXCLUDED.latest_audit_id")
                .bind(audit.auction_id.as_uuid()).bind(field.as_str()).bind(audit.audit_id.as_uuid())
                .execute(&mut *self.connection).await.map_err(write_error)?;
        }
        Ok(())
    }
}

fn read_error(error: sqlx::Error) -> AuctionMetadataPolicyRepositoryError {
    AuctionMetadataPolicyRepositoryError::PersistenceFailed {
        source: box_error(error),
    }
}
fn write_error(error: sqlx::Error) -> AuctionMetadataPolicyRepositoryError {
    AuctionMetadataPolicyRepositoryError::PersistenceFailed {
        source: box_error(error),
    }
}
