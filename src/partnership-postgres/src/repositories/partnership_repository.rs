use application::error::box_error;
use domain_primitives::versioned::Versioned;
use partnership_core::{
    partnership::{Partnership, RehydratedPartnershipState},
    partnership_id::PartnershipId,
};
use partnership_service::ports::*;
use party_core::party_id::PartyId;
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnershipRepositoryFactory;
struct Repository<'a> {
    connection: &'a mut PgConnection,
}
impl SqlxPartnershipRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}
impl PartnershipRepositoryFactory<SqlxTransaction> for SqlxPartnershipRepositoryFactory {
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl PartnershipRepository + 'a {
        Repository {
            connection: tx.connection(),
        }
    }
}
impl PartnershipMembershipRepositoryFactory<SqlxTransaction> for SqlxPartnershipRepositoryFactory {
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl PartnershipMembershipRepository + 'a {
        Repository {
            connection: tx.connection(),
        }
    }
}
#[derive(sqlx::FromRow)]
struct Row {
    partnership_id: uuid::Uuid,
    party_id: uuid::Uuid,
    version: i64,
}
fn map(row: Row) -> Result<VersionedPartnership, PartnershipRepositoryError> {
    let version = PartnershipStorageVersion::try_from(row.version).map_err(|e| {
        PartnershipRepositoryError::InvalidPersistedState {
            source: box_error(e),
        }
    })?;
    Ok(Versioned::new(
        Partnership::rehydrate(RehydratedPartnershipState {
            id: PartnershipId::from(row.partnership_id),
            party_id: PartyId::from(row.party_id),
        }),
        version,
    ))
}
#[async_trait::async_trait]
impl PartnershipRepository for Repository<'_> {
    async fn find_by_id(
        &mut self,
        partnership_id: PartnershipId,
    ) -> Result<Option<VersionedPartnership>, PartnershipRepositoryError> {
        let row = sqlx::query_as::<_, Row>(
            "SELECT partnership_id,party_id,version FROM partnerships WHERE partnership_id=$1",
        )
        .bind(uuid::Uuid::from(partnership_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(
            |source| PartnershipRepositoryError::TemporarilyUnavailable {
                source: box_error(source),
            },
        )?;
        row.map(map).transpose()
    }

    async fn find_or_create_for_party(
        &mut self,
        party_id: PartyId,
        new_partnership_id: PartnershipId,
    ) -> Result<VersionedPartnership, PartnershipRepositoryError> {
        let inserted = sqlx::query_as::<_, Row>(
            "INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2) \
             ON CONFLICT (party_id) DO NOTHING \
             RETURNING partnership_id,party_id,version",
        )
        .bind(uuid::Uuid::from(new_partnership_id))
        .bind(uuid::Uuid::from(party_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(|error| PartnershipRepositoryError::Internal {
            source: box_error(error),
        })?;
        let row = match inserted {
            Some(row) => row,
            None => sqlx::query_as::<_, Row>(
                "SELECT partnership_id,party_id,version FROM partnerships WHERE party_id=$1",
            )
            .bind(uuid::Uuid::from(party_id))
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(|error| PartnershipRepositoryError::TemporarilyUnavailable {
                source: box_error(error),
            })?
            .ok_or_else(|| PartnershipRepositoryError::Internal {
                source: box_error(std::io::Error::other(
                    "partnership disappeared after party conflict",
                )),
            })?,
        };
        map(row)
    }
}
#[async_trait::async_trait]
impl PartnershipMembershipRepository for Repository<'_> {
    async fn add_member(
        &mut self,
        user_id: user_core::user_id::UserId,
        partnership_id: PartnershipId,
    ) -> Result<PartnershipMembershipAddOutcome, PartnershipGrantError> {
        let result = sqlx::query(
            "INSERT INTO partnership_members(user_id,partnership_id) VALUES($1,$2) ON CONFLICT DO NOTHING",
        )
        .bind(uuid::Uuid::from(user_id))
        .bind(uuid::Uuid::from(partnership_id))
        .execute(&mut *self.connection)
        .await
        .map_err(|source| PartnershipGrantError::Internal {
            source: box_error(source),
        })?;
        Ok(if result.rows_affected() > 0 {
            PartnershipMembershipAddOutcome::Added
        } else {
            PartnershipMembershipAddOutcome::AlreadyMember
        })
    }

    async fn remove_member(
        &mut self,
        user_id: user_core::user_id::UserId,
        partnership_id: PartnershipId,
    ) -> Result<PartnershipMembershipRemoveOutcome, PartnershipGrantError> {
        let result =
            sqlx::query("DELETE FROM partnership_members WHERE user_id=$1 AND partnership_id=$2")
                .bind(uuid::Uuid::from(user_id))
                .bind(uuid::Uuid::from(partnership_id))
                .execute(&mut *self.connection)
                .await
                .map_err(|source| PartnershipGrantError::Internal {
                    source: box_error(source),
                })?;
        Ok(if result.rows_affected() > 0 {
            PartnershipMembershipRemoveOutcome::Removed
        } else {
            PartnershipMembershipRemoveOutcome::AlreadyAbsent
        })
    }
}
