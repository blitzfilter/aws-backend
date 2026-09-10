use application::error::box_error;
use platform_postgres::SqlxTransaction;
use sqlx::{PgConnection, PgPool};
use user_core::user_id::UserId;
use user_service::ports::{
    CognitoIdentity, CognitoIssuer, CognitoSubject, CognitoUserIdentityReadError,
    CognitoUserIdentityReader, UserCognitoIdentityRegistry, UserCognitoIdentityRegistryError,
    UserCognitoIdentityRegistryFactory,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxUserCognitoIdentityRegistryFactory;

struct SqlxUserCognitoIdentityRegistry<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, Clone)]
pub struct SqlxCognitoUserIdentityReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct CognitoIdentityRow {
    issuer: String,
    subject: String,
}

impl SqlxUserCognitoIdentityRegistryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl UserCognitoIdentityRegistryFactory<SqlxTransaction>
    for SqlxUserCognitoIdentityRegistryFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl UserCognitoIdentityRegistry + 'tx {
        SqlxUserCognitoIdentityRegistry {
            connection: tx.connection(),
        }
    }
}

impl SqlxCognitoUserIdentityReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl UserCognitoIdentityRegistry for SqlxUserCognitoIdentityRegistry<'_> {
    async fn lock_and_find_user_id(
        &mut self,
        identity: &CognitoIdentity,
    ) -> Result<Option<UserId>, UserCognitoIdentityRegistryError> {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1 || chr(31) || $2, 0))")
            .bind(identity.issuer.as_str())
            .bind(identity.subject.as_str())
            .execute(&mut *self.connection)
            .await
            .map_err(registry_temporary)?;

        let user_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT user_id FROM user_cognito_identities WHERE issuer = $1 AND subject = $2",
        )
        .bind(identity.issuer.as_str())
        .bind(identity.subject.as_str())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(registry_temporary)?;

        user_id
            .map(UserId::try_from)
            .transpose()
            .map_err(registry_invalid)
    }

    async fn find_by_user_id(
        &mut self,
        user_id: UserId,
    ) -> Result<Option<CognitoIdentity>, UserCognitoIdentityRegistryError> {
        let row = sqlx::query_as::<_, CognitoIdentityRow>(
            "SELECT issuer, subject FROM user_cognito_identities WHERE user_id = $1",
        )
        .bind(user_id.as_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(registry_temporary)?;

        row.map(CognitoIdentity::try_from)
            .transpose()
            .map_err(registry_invalid)
    }

    async fn bind(
        &mut self,
        identity: &CognitoIdentity,
        user_id: UserId,
    ) -> Result<(), UserCognitoIdentityRegistryError> {
        sqlx::query(
            "INSERT INTO user_cognito_identities (issuer, subject, user_id) VALUES ($1, $2, $3)",
        )
        .bind(identity.issuer.as_str())
        .bind(identity.subject.as_str())
        .bind(user_id.as_uuid())
        .execute(&mut *self.connection)
        .await
        .map(|_| ())
        .map_err(registry_write)
    }
}

#[async_trait::async_trait]
impl CognitoUserIdentityReader for SqlxCognitoUserIdentityReader {
    async fn find_user_id(
        &self,
        identity: &CognitoIdentity,
    ) -> Result<Option<UserId>, CognitoUserIdentityReadError> {
        let user_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT user_id FROM user_cognito_identities WHERE issuer = $1 AND subject = $2",
        )
        .bind(identity.issuer.as_str())
        .bind(identity.subject.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(
            |source| CognitoUserIdentityReadError::TemporarilyUnavailable {
                source: box_error(source),
            },
        )?;

        user_id.map(UserId::try_from).transpose().map_err(|source| {
            CognitoUserIdentityReadError::InvalidPersistedIdentity {
                source: box_error(source),
            }
        })
    }
}

impl TryFrom<CognitoIdentityRow> for CognitoIdentity {
    type Error = user_service::ports::InvalidCognitoIdentityValue;

    fn try_from(row: CognitoIdentityRow) -> Result<Self, Self::Error> {
        Ok(Self {
            issuer: CognitoIssuer::try_from(row.issuer)?,
            subject: CognitoSubject::try_from(row.subject)?,
        })
    }
}

fn registry_temporary(source: sqlx::Error) -> UserCognitoIdentityRegistryError {
    UserCognitoIdentityRegistryError::TemporarilyUnavailable {
        source: box_error(source),
    }
}

fn registry_invalid(
    source: impl std::error::Error + Send + Sync + 'static,
) -> UserCognitoIdentityRegistryError {
    UserCognitoIdentityRegistryError::InvalidPersistedIdentity {
        source: box_error(source),
    }
}

fn registry_write(source: sqlx::Error) -> UserCognitoIdentityRegistryError {
    if let sqlx::Error::Database(database_error) = &source
        && database_error.is_unique_violation()
    {
        return UserCognitoIdentityRegistryError::Conflict {
            source: box_error(source),
        };
    }
    registry_temporary(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::transaction::{Transaction, UnitOfWork};
    use platform_postgres::SqlxUnitOfWork;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    fn identity(issuer: &str, subject: &str) -> CognitoIdentity {
        CognitoIdentity {
            issuer: CognitoIssuer::try_from(issuer)
                .unwrap_or_else(|error| panic!("invalid test issuer: {error}")),
            subject: CognitoSubject::try_from(subject)
                .unwrap_or_else(|error| panic!("invalid test subject: {error}")),
        }
    }

    async fn seed_user(pool: &PgPool, user_id: uuid::Uuid, email: &str) {
        sqlx::query("INSERT INTO users (user_id, email,tier,role) VALUES ($1, $2, 'FREE', 'USER')")
            .bind(user_id)
            .bind(email)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("failed to seed user: {error}"));
    }

    #[test]
    fn should_map_opaque_cognito_identity_row() {
        let identity = CognitoIdentity::try_from(CognitoIdentityRow {
            issuer: "https://cognito-idp.eu-central-1.amazonaws.com/pool-a".to_owned(),
            subject: "provider|not-a-uuid".to_owned(),
        })
        .unwrap_or_else(|error| panic!("valid identity row rejected: {error}"));

        assert_eq!("provider|not-a-uuid", identity.subject.as_str());
    }

    #[test]
    fn should_reject_invalid_persisted_cognito_identity_row() {
        let result = CognitoIdentity::try_from(CognitoIdentityRow {
            issuer: "https://issuer.example".to_owned(),
            subject: "invalid\nsubject".to_owned(),
        });

        assert!(result.is_err());
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_store_native_user_uuid_and_resolve_identity_both_directions() {
        let pool = get_postgres_client().await;
        let user_id = UserId::new();
        let cognito_identity = identity("https://issuer.example/pool-a", "provider|opaque");
        seed_user(&pool, *user_id.as_uuid(), "ada@example.test").await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("failed to begin transaction: {error}"));
        let factory = SqlxUserCognitoIdentityRegistryFactory::new();

        let existing = factory
            .in_transaction(&mut tx)
            .lock_and_find_user_id(&cognito_identity)
            .await
            .unwrap_or_else(|error| panic!("failed to lock identity: {error}"));
        assert_eq!(None, existing);
        factory
            .in_transaction(&mut tx)
            .bind(&cognito_identity, user_id)
            .await
            .unwrap_or_else(|error| panic!("failed to bind identity: {error}"));
        let by_user = factory
            .in_transaction(&mut tx)
            .find_by_user_id(user_id)
            .await
            .unwrap_or_else(|error| panic!("failed to read identity by user: {error}"));
        tx.commit()
            .await
            .unwrap_or_else(|error| panic!("failed to commit identity: {error}"));

        let resolved = SqlxCognitoUserIdentityReader::new(pool.clone())
            .find_user_id(&cognito_identity)
            .await
            .unwrap_or_else(|error| panic!("failed to resolve identity: {error}"));
        let data_type = sqlx::query_scalar::<_, String>(
            "SELECT data_type FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = 'user_cognito_identities' AND column_name = 'user_id'",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to inspect identity user ID column: {error}"));

        assert_eq!(Some(cognito_identity), by_user);
        assert_eq!(Some(user_id), resolved);
        assert_eq!("uuid", data_type);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_isolate_same_subject_by_issuer() {
        let pool = get_postgres_client().await;
        let first_user_id = UserId::new();
        let second_user_id = UserId::new();
        let first = identity("https://issuer.example/pool-a", "shared-subject");
        let second = identity("https://issuer.example/pool-b", "shared-subject");
        seed_user(&pool, *first_user_id.as_uuid(), "first@example.test").await;
        seed_user(&pool, *second_user_id.as_uuid(), "second@example.test").await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("failed to begin transaction: {error}"));
        let factory = SqlxUserCognitoIdentityRegistryFactory::new();
        factory
            .in_transaction(&mut tx)
            .bind(&first, first_user_id)
            .await
            .unwrap_or_else(|error| panic!("failed to bind first identity: {error}"));
        factory
            .in_transaction(&mut tx)
            .bind(&second, second_user_id)
            .await
            .unwrap_or_else(|error| panic!("failed to bind second identity: {error}"));
        tx.commit()
            .await
            .unwrap_or_else(|error| panic!("failed to commit identities: {error}"));
        let reader = SqlxCognitoUserIdentityReader::new(pool);

        assert_eq!(
            Some(first_user_id),
            reader
                .find_user_id(&first)
                .await
                .unwrap_or_else(|error| panic!("failed to resolve first identity: {error}"))
        );
        assert_eq!(
            Some(second_user_id),
            reader
                .find_user_id(&second)
                .await
                .unwrap_or_else(|error| panic!("failed to resolve second identity: {error}"))
        );
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_enforce_unique_identity_tuple_and_unique_user_binding() {
        let pool = get_postgres_client().await;
        let first_user_id = UserId::new();
        let second_user_id = UserId::new();
        let first = identity("https://issuer.example/pool-a", "first-subject");
        let second = identity("https://issuer.example/pool-a", "second-subject");
        seed_user(&pool, *first_user_id.as_uuid(), "first@example.test").await;
        seed_user(&pool, *second_user_id.as_uuid(), "second@example.test").await;
        let unit_of_work = SqlxUnitOfWork::new(pool);
        let factory = SqlxUserCognitoIdentityRegistryFactory::new();
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("failed to begin transaction: {error}"));
        factory
            .in_transaction(&mut tx)
            .bind(&first, first_user_id)
            .await
            .unwrap_or_else(|error| panic!("failed to bind initial identity: {error}"));
        tx.commit()
            .await
            .unwrap_or_else(|error| panic!("failed to commit initial identity: {error}"));

        let mut same_user_tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("failed to begin same-user transaction: {error}"));
        let same_user = factory
            .in_transaction(&mut same_user_tx)
            .bind(&second, first_user_id)
            .await;
        drop(same_user_tx);
        let mut same_identity_tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("failed to begin same-identity transaction: {error}"));
        let same_identity = factory
            .in_transaction(&mut same_identity_tx)
            .bind(&first, second_user_id)
            .await;

        assert!(matches!(
            same_user,
            Err(UserCognitoIdentityRegistryError::Conflict { .. })
        ));
        assert!(matches!(
            same_identity,
            Err(UserCognitoIdentityRegistryError::Conflict { .. })
        ));
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_roll_back_uncommitted_identity_binding() {
        let pool = get_postgres_client().await;
        let user_id = UserId::new();
        let cognito_identity = identity("https://issuer.example/pool-a", "rollback-subject");
        seed_user(&pool, *user_id.as_uuid(), "rollback@example.test").await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let mut tx = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("failed to begin transaction: {error}"));
        SqlxUserCognitoIdentityRegistryFactory::new()
            .in_transaction(&mut tx)
            .bind(&cognito_identity, user_id)
            .await
            .unwrap_or_else(|error| panic!("failed to bind identity: {error}"));
        drop(tx);

        let resolved = SqlxCognitoUserIdentityReader::new(pool)
            .find_user_id(&cognito_identity)
            .await
            .unwrap_or_else(|error| panic!("failed to resolve rolled-back identity: {error}"));

        assert_eq!(None, resolved);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_report_non_uuid_v7_persisted_user_id_as_invalid_identity_state() {
        let pool = get_postgres_client().await;
        let invalid_user_id = uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000")
            .unwrap_or_else(|error| panic!("invalid UUID fixture: {error}"));
        let cognito_identity = identity("https://issuer.example/pool-a", "invalid-user-id");
        seed_user(&pool, invalid_user_id, "invalid@example.test").await;
        sqlx::query(
            "INSERT INTO user_cognito_identities (issuer, subject, user_id) VALUES ($1, $2, $3)",
        )
        .bind(cognito_identity.issuer.as_str())
        .bind(cognito_identity.subject.as_str())
        .bind(invalid_user_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed invalid identity: {error}"));

        let result = SqlxCognitoUserIdentityReader::new(pool)
            .find_user_id(&cognito_identity)
            .await;

        assert!(matches!(
            result,
            Err(CognitoUserIdentityReadError::InvalidPersistedIdentity { .. })
        ));
    }
}
