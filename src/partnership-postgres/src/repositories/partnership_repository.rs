use application::error::box_error;
use domain_primitives::object_id::ObjectIdError;
use domain_primitives::versioned::Versioned;
use partnership_core::{
    partnership::{Partnership, RehydratedPartnershipState},
    partnership_id::PartnershipId,
    partnership_lifecycle::PartnershipLifecycle,
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
    business_state: String,
    version: i64,
}

#[derive(Debug, thiserror::Error)]
enum RowMappingError {
    #[error("invalid Partnership ID persisted")]
    PartnershipId(#[source] ObjectIdError),
    #[error("invalid Party ID persisted")]
    PartyId(#[source] ObjectIdError),
    #[error("invalid partnership lifecycle")]
    Lifecycle,
    #[error("invalid partnership version")]
    Version(#[source] domain_primitives::version::InvalidVersionError),
}

fn map(row: Row) -> Result<VersionedPartnership, PartnershipRepositoryError> {
    let lifecycle = PartnershipLifecycle::from_code(&row.business_state)
        .ok_or(RowMappingError::Lifecycle)
        .map_err(|source| PartnershipRepositoryError::InvalidPersistedState {
            source: box_error(source),
        })?;
    let version = PartnershipStorageVersion::try_from(row.version)
        .map_err(RowMappingError::Version)
        .map_err(|source| PartnershipRepositoryError::InvalidPersistedState {
            source: box_error(source),
        })?;
    Ok(Versioned::new(
        Partnership::rehydrate(RehydratedPartnershipState {
            id: PartnershipId::try_from(row.partnership_id)
                .map_err(RowMappingError::PartnershipId)
                .map_err(|source| PartnershipRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                })?,
            party_id: PartyId::try_from(row.party_id)
                .map_err(RowMappingError::PartyId)
                .map_err(|source| PartnershipRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                })?,
            lifecycle,
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
            "SELECT partnership_id,party_id,business_state,version FROM partnerships WHERE partnership_id=$1",
        )
        .bind(partnership_id.into_uuid())
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
            "INSERT INTO partnerships(partnership_id,party_id,business_state) VALUES($1,$2,$3) \
             ON CONFLICT (party_id) DO UPDATE \
             SET business_state=$3,version=partnerships.version+1,updated=now() \
             WHERE partnerships.business_state=$4 \
             RETURNING partnership_id,party_id,business_state,version",
        )
        .bind(new_partnership_id.into_uuid())
        .bind(party_id.into_uuid())
        .bind(PartnershipLifecycle::Active.as_str())
        .bind(PartnershipLifecycle::Dissolved.as_str())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(|error| PartnershipRepositoryError::Internal {
            source: box_error(error),
        })?;
        let row = match inserted {
            Some(row) => row,
            None => sqlx::query_as::<_, Row>(
                "SELECT partnership_id,party_id,business_state,version FROM partnerships WHERE party_id=$1",
            )
            .bind(party_id.into_uuid())
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

    async fn dissolve(
        &mut self,
        partnership: &Partnership,
        expected: PartnershipStorageVersion,
    ) -> Result<VersionedPartnership, PartnershipRepositoryError> {
        let expected = i64::try_from(expected.into_inner()).map_err(|source| {
            PartnershipRepositoryError::InvalidPersistedState {
                source: box_error(source),
            }
        })?;
        let row = sqlx::query_as::<_, Row>(
            "WITH dissolved AS ( \
                UPDATE partnerships \
                SET business_state = $1, version = version + 1, updated = now() \
                WHERE partnership_id = $2 \
                  AND version = $3 \
                  AND business_state = $4 \
                RETURNING partnership_id, party_id, business_state, version \
            ), deleted_members AS ( \
                DELETE FROM partnership_members \
                WHERE partnership_id IN (SELECT partnership_id FROM dissolved) \
            ), deleted_grants AS ( \
                DELETE FROM partnership_listing_source_grants \
                WHERE partnership_id IN (SELECT partnership_id FROM dissolved) \
            ) \
            SELECT partnership_id, party_id, business_state, version FROM dissolved \
            UNION ALL \
            SELECT partnership_id, party_id, business_state, version \
            FROM partnerships \
            WHERE partnership_id = $2 \
              AND version = $3 \
              AND business_state = $1 \
              AND NOT EXISTS (SELECT 1 FROM dissolved)",
        )
        .bind(PartnershipLifecycle::Dissolved.as_str())
        .bind(partnership.id().into_uuid())
        .bind(expected)
        .bind(PartnershipLifecycle::Active.as_str())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(
            |source| PartnershipRepositoryError::TemporarilyUnavailable {
                source: box_error(source),
            },
        )?
        .ok_or_else(|| PartnershipRepositoryError::ConcurrencyConflict)?;

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
        .bind(user_id.into_uuid())
        .bind(partnership_id.into_uuid())
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
                .bind(user_id.into_uuid())
                .bind(partnership_id.into_uuid())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Row {
        Row {
            partnership_id: uuid::Uuid::now_v7(),
            party_id: uuid::Uuid::now_v7(),
            business_state: "ACTIVE".to_owned(),
            version: 1,
        }
    }

    #[test]
    fn should_reject_noncanonical_persisted_lifecycle() {
        let mut row = row();
        row.business_state = "dissolved".to_owned();
        let result = map(row);

        assert!(matches!(
            result,
            Err(PartnershipRepositoryError::InvalidPersistedState { .. })
        ));
    }

    #[test]
    fn should_reject_wrong_version_partnership_id() {
        let mut row = row();
        row.partnership_id = uuid::Uuid::new_v4();

        assert!(matches!(
            map(row),
            Err(PartnershipRepositoryError::InvalidPersistedState { .. })
        ));
    }

    #[test]
    fn should_reject_wrong_version_party_id() {
        let mut row = row();
        row.party_id = uuid::Uuid::new_v4();

        assert!(matches!(
            map(row),
            Err(PartnershipRepositoryError::InvalidPersistedState { .. })
        ));
    }

    #[test]
    fn should_reject_invalid_storage_version() {
        let mut row = row();
        row.version = 0;

        assert!(matches!(
            map(row),
            Err(PartnershipRepositoryError::InvalidPersistedState { .. })
        ));
    }
}
