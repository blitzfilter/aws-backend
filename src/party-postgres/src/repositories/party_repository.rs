use crate::mapping::{PartyRow, party_columns, version_to_i64};
use application::error::box_error;
use party_core::{party::Party, party_id::PartyId, party_slug_id::PartySlugId};
use party_service::ports::{
    PartyDeletionBlocker, PartyRepository, PartyRepositoryError, PartyRepositoryFactory,
    PartyStorageVersion, StoredParty,
};
use platform_postgres::SqlxTransaction;
use sqlx::{PgConnection, Postgres, QueryBuilder};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartyRepositoryFactory;

struct SqlxPartyRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxPartyRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartyRepositoryFactory<SqlxTransaction> for SqlxPartyRepositoryFactory {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut SqlxTransaction) -> impl PartyRepository + 'tx {
        SqlxPartyRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartyRepository for SqlxPartyRepository<'_> {
    async fn find_by_id(
        &mut self,
        id: PartyId,
    ) -> Result<Option<StoredParty>, PartyRepositoryError> {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        builder
            .push(party_columns())
            .push(" FROM parties WHERE party_id = ")
            .push_bind(id.into_uuid());
        let row = builder
            .build_query_as::<PartyRow>()
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(PartyLookupSqlxError)?;

        row.map(StoredParty::try_from)
            .transpose()
            .map_err(|source| PartyRepositoryError::InvalidPersistedState {
                source: box_error(source),
            })
    }

    async fn find_by_id_for_update(
        &mut self,
        id: PartyId,
    ) -> Result<Option<StoredParty>, PartyRepositoryError> {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        builder
            .push(party_columns())
            .push(" FROM parties WHERE party_id = ")
            .push_bind(id.into_uuid())
            .push(" FOR UPDATE");
        let row = builder
            .build_query_as::<PartyRow>()
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(PartyLookupSqlxError)?;

        row.map(StoredParty::try_from)
            .transpose()
            .map_err(|source| PartyRepositoryError::InvalidPersistedState {
                source: box_error(source),
            })
    }

    async fn find_deletion_blocker(
        &mut self,
        id: PartyId,
    ) -> Result<Option<PartyDeletionBlocker>, PartyRepositoryError> {
        let blocker = sqlx::query_scalar::<_, Option<String>>(
            r#"
            SELECT CASE
                WHEN EXISTS (
                    SELECT 1 FROM listing_sources WHERE operator_party_id = $1
                ) THEN 'LISTING_SOURCES'
                WHEN EXISTS (
                    SELECT 1 FROM partnerships WHERE party_id = $1
                ) THEN 'PARTNERSHIP'
                ELSE NULL
            END
            "#,
        )
        .bind(id.into_uuid())
        .fetch_one(&mut *self.connection)
        .await
        .map_err(PartyLookupSqlxError)?;

        match blocker.as_deref() {
            None => Ok(None),
            Some("LISTING_SOURCES") => Ok(Some(PartyDeletionBlocker::ListingSources)),
            Some("PARTNERSHIP") => Ok(Some(PartyDeletionBlocker::Partnership)),
            Some(value) => Err(PartyRepositoryError::InvalidPersistedState {
                source: box_error(std::io::Error::other(format!(
                    "unknown party deletion blocker: {value}"
                ))),
            }),
        }
    }

    async fn delete_unused(
        &mut self,
        id: PartyId,
        expected_version: PartyStorageVersion,
    ) -> Result<(), PartyRepositoryError> {
        let expected_version = version_to_i64(expected_version).map_err(|source| {
            PartyRepositoryError::InvalidPersistedState {
                source: box_error(source),
            }
        })?;
        let result = sqlx::query("DELETE FROM parties WHERE party_id = $1 AND version = $2")
            .bind(id.into_uuid())
            .bind(expected_version)
            .execute(&mut *self.connection)
            .await
            .map_err(PartyWriteSqlxError)?;
        if result.rows_affected() != 1 {
            return Err(PartyRepositoryError::ConcurrencyConflict);
        }
        Ok(())
    }

    async fn find_by_slug(
        &mut self,
        slug_id: &PartySlugId,
    ) -> Result<Option<StoredParty>, PartyRepositoryError> {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        builder
            .push(party_columns())
            .push(" FROM parties WHERE party_slug_id = ")
            .push_bind(slug_id.as_ref());
        let row = builder
            .build_query_as::<PartyRow>()
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(PartyLookupSqlxError)?;

        row.map(StoredParty::try_from)
            .transpose()
            .map_err(|source| PartyRepositoryError::InvalidPersistedState {
                source: box_error(source),
            })
    }

    async fn insert(&mut self, party: &Party) -> Result<StoredParty, PartyRepositoryError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            INSERT INTO parties (party_id, party_slug_id, name, phone, email)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING "#,
        );
        builder.push(party_columns());
        let row = builder
            .build_query_as::<PartyRow>()
            .bind(party.id().into_uuid())
            .bind(party.slug_id().as_ref())
            .bind(party.name().as_ref())
            .bind(party.contact().phone.as_deref())
            .bind(party.contact().email.as_ref().map(ToString::to_string))
            .fetch_one(&mut *self.connection)
            .await
            .map_err(PartyWriteSqlxError)?;

        StoredParty::try_from(row).map_err(|source| PartyRepositoryError::InvalidPersistedState {
            source: box_error(source),
        })
    }

    async fn update(
        &mut self,
        party: &Party,
        expected_version: PartyStorageVersion,
    ) -> Result<StoredParty, PartyRepositoryError> {
        let expected_version = version_to_i64(expected_version).map_err(|source| {
            PartyRepositoryError::InvalidPersistedState {
                source: box_error(source),
            }
        })?;
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            UPDATE parties
            SET
                name = $1,
                phone = $2,
                email = $3,
                version = version + 1,
                updated = now()
            WHERE party_id = $4 AND version = $5
            RETURNING "#,
        );
        builder.push(party_columns());
        let row = builder
            .build_query_as::<PartyRow>()
            .bind(party.name().as_ref())
            .bind(party.contact().phone.as_deref())
            .bind(party.contact().email.as_ref().map(ToString::to_string))
            .bind(party.id().into_uuid())
            .bind(expected_version)
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(PartyWriteSqlxError)?
            .ok_or(PartyRepositoryError::ConcurrencyConflict)?;

        StoredParty::try_from(row).map_err(|source| PartyRepositoryError::InvalidPersistedState {
            source: box_error(source),
        })
    }
}

struct PartyLookupSqlxError(sqlx::Error);
struct PartyWriteSqlxError(sqlx::Error);

impl From<PartyLookupSqlxError> for PartyRepositoryError {
    fn from(error: PartyLookupSqlxError) -> Self {
        let PartyLookupSqlxError(source) = error;
        Self::TemporarilyUnavailable {
            source: box_error(source),
        }
    }
}

impl From<PartyWriteSqlxError> for PartyRepositoryError {
    fn from(error: PartyWriteSqlxError) -> Self {
        let PartyWriteSqlxError(source) = error;
        match &source {
            sqlx::Error::Database(database_error)
                if database_error.constraint() == Some("parties_slug_unique") =>
            {
                Self::SlugConflict {
                    source: box_error(source),
                }
            }
            _ => Self::Internal {
                source: box_error(source),
            },
        }
    }
}
