use crate::mapping::{APPLICATION_COLUMNS, ApplicationRow, application, invalid, proposal_json};
use application::error::box_error;
use partnership_core::partnership_application::PartnershipApplication;
use partnership_core::partnership_application_id::PartnershipApplicationId;
use partnership_service::ports::*;
use platform_postgres::SqlxTransaction;
use sqlx::{AssertSqlSafe, PgConnection};
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnershipApplicationRepositoryFactory;

pub(crate) struct Repository<'a> {
    connection: &'a mut PgConnection,
}

impl SqlxPartnershipApplicationRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartnershipApplicationRepositoryFactory<SqlxTransaction>
    for SqlxPartnershipApplicationRepositoryFactory
{
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut SqlxTransaction,
    ) -> impl PartnershipApplicationRepository + 'a {
        Repository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartnershipApplicationRepository for Repository<'_> {
    async fn find_by_id(
        &mut self,
        id: PartnershipApplicationId,
    ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
    {
        sqlx::query_as::<_, ApplicationRow>(AssertSqlSafe(format!(
            "SELECT {APPLICATION_COLUMNS} FROM partnership_applications WHERE partnership_application_id=$1"
        )))
        .bind(id.into_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(read)?
        .map(application)
        .transpose()
        .map_err(invalid)
    }

    async fn find_by_id_for_update(
        &mut self,
        id: PartnershipApplicationId,
    ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
    {
        sqlx::query_as::<_, ApplicationRow>(AssertSqlSafe(format!(
            "SELECT {APPLICATION_COLUMNS} FROM partnership_applications WHERE partnership_application_id=$1 FOR UPDATE"
        )))
        .bind(id.into_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(read)?
        .map(application)
        .transpose()
        .map_err(invalid)
    }

    async fn find_by_user_and_id(
        &mut self,
        user: UserId,
        id: PartnershipApplicationId,
    ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
    {
        sqlx::query_as::<_, ApplicationRow>(AssertSqlSafe(format!(
            "SELECT {APPLICATION_COLUMNS} FROM partnership_applications WHERE applicant_user_id=$1 AND partnership_application_id=$2"
        )))
        .bind(user.into_uuid())
        .bind(id.into_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(read)?
        .map(application)
        .transpose()
        .map_err(invalid)
    }

    async fn insert(
        &mut self,
        app: &PartnershipApplication,
    ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError> {
        let proposal = proposal_json(app.proposal()).map_err(invalid)?;
        sqlx::query_as::<_, ApplicationRow>(AssertSqlSafe(format!(
            "INSERT INTO partnership_applications(partnership_application_id,applicant_user_id,business_state,proposal) VALUES($1,$2,$3,$4) RETURNING {APPLICATION_COLUMNS}"
        )))
        .bind(app.id().into_uuid())
        .bind(app.applicant_user_id().into_uuid())
        .bind(app.state().as_str())
        .bind(proposal)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(write)
        .and_then(|row| application(row).map_err(invalid))
    }

    async fn update(
        &mut self,
        app: &PartnershipApplication,
        expected: PartnershipApplicationStorageVersion,
    ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError> {
        let expected = i64::try_from(expected.into_inner())
            .map_err(|_| invalid(crate::mapping::MappingError::Version))?;
        let proposal = proposal_json(app.proposal()).map_err(invalid)?;
        let approval_result = app.approval_result();
        sqlx::query_as::<_, ApplicationRow>(AssertSqlSafe(format!(
            "UPDATE partnership_applications SET business_state=$1,proposal=$2,approved_partnership_id=$3,approved_listing_source_id=$4,version=version+1,updated=now() WHERE partnership_application_id=$5 AND version=$6 RETURNING {APPLICATION_COLUMNS}"
        )))
        .bind(app.state().as_str())
        .bind(proposal)
        .bind(approval_result.map(|result| result.partnership_id().into_uuid()))
        .bind(approval_result.map(|result| result.listing_source_id().into_uuid()))
        .bind(app.id().into_uuid())
        .bind(expected)
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(write)?
        .map(application)
        .transpose()
        .map_err(invalid)?
        .ok_or(PartnershipApplicationRepositoryError::ConcurrencyConflict)
    }
}

fn read(error: sqlx::Error) -> PartnershipApplicationRepositoryError {
    PartnershipApplicationRepositoryError::TemporarilyUnavailable {
        source: box_error(error),
    }
}

fn write(error: sqlx::Error) -> PartnershipApplicationRepositoryError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.constraint()
                == Some("partnership_applications_existing_listing_source_id_fkey") =>
        {
            PartnershipApplicationRepositoryError::ListingSourceNotFound
        }
        _ => PartnershipApplicationRepositoryError::Internal {
            source: box_error(error),
        },
    }
}
