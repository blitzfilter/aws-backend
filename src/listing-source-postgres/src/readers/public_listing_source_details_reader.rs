use super::public_listing_source::{PublicListingSourceRow, map_public_listing_source};
use application::error::box_error;
use listing_source_core::ListingSourceSlugId;
use listing_source_service::{
    ports::{
        PublicListingSourceDetailsReadError, PublicListingSourceDetailsReader,
        PublicListingSourceDetailsReaderFactory,
    },
    use_cases::queries::public_listing_source::PublicListingSourceSummary,
};
use platform_postgres::SqlxTransaction;

const STATEMENT_TIMEOUT: &str = "150ms";
pub(super) const DETAIL_BY_SLUG_SQL: &str = "SELECT s.listing_source_id, s.listing_source_slug_id, s.name, p.name AS operator_name, s.url, s.image FROM listing_sources s JOIN parties p ON p.party_id = s.operator_party_id WHERE s.listing_source_slug_id = $1";

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPublicListingSourceDetailsReaderFactory;

struct SqlxPublicListingSourceDetailsReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxPublicListingSourceDetailsReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PublicListingSourceDetailsReaderFactory<SqlxTransaction>
    for SqlxPublicListingSourceDetailsReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PublicListingSourceDetailsReader + 'tx {
        SqlxPublicListingSourceDetailsReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PublicListingSourceDetailsReader for SqlxPublicListingSourceDetailsReader<'_> {
    async fn find_by_slug(
        &mut self,
        slug_id: &ListingSourceSlugId,
    ) -> Result<Option<PublicListingSourceSummary>, PublicListingSourceDetailsReadError> {
        set_statement_timeout(self.connection).await?;

        sqlx::query_as::<_, PublicListingSourceRow>(DETAIL_BY_SLUG_SQL)
            .bind(slug_id.as_ref())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(temporary)?
            .map(map_public_listing_source)
            .transpose()
            .map_err(invalid)
    }
}

async fn set_statement_timeout(
    connection: &mut sqlx::PgConnection,
) -> Result<(), PublicListingSourceDetailsReadError> {
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(STATEMENT_TIMEOUT)
        .execute(connection)
        .await
        .map_err(temporary)?;
    Ok(())
}

fn temporary(source: sqlx::Error) -> PublicListingSourceDetailsReadError {
    PublicListingSourceDetailsReadError::TemporarilyUnavailable {
        source: box_error(source),
    }
}

fn invalid(
    source: impl std::error::Error + Send + Sync + 'static,
) -> PublicListingSourceDetailsReadError {
    PublicListingSourceDetailsReadError::InvalidReadModel {
        source: box_error(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::transaction::{Transaction, UnitOfWork};
    use listing_source_service::ports::PublicListingSourceDetailsReaderFactory;
    use platform_postgres::SqlxUnitOfWork;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_find_exact_slug_with_public_projection() {
        let pool = get_postgres_client().await;
        let source_id = uuid::Uuid::now_v7();
        let party_id = uuid::Uuid::now_v7();
        insert_party(&pool, party_id, "detail-operator", "Detail Operator").await;
        insert_source(
            &pool,
            source_id,
            "detail-source",
            "Detail Source",
            party_id,
            Some("https://example.test/source"),
            None,
        )
        .await;

        let result = read(&pool, "detail-source").await;

        assert!(matches!(
            result,
            Some(summary)
                if *summary.listing_source_id.as_uuid() == source_id
                    && summary.listing_source_slug_id.as_ref() == "detail-source"
                    && summary.name.as_ref() == "Detail Source"
                    && summary.operator.name.as_ref() == "Detail Operator"
                    && summary.url.as_ref().is_some_and(|url| url.as_str() == "https://example.test/source")
                    && summary.image.is_none()
        ));
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_return_none_for_unknown_or_deleted_exact_slug() {
        let pool = get_postgres_client().await;
        assert!(read(&pool, "unknown-source").await.is_none());

        let source_id = uuid::Uuid::now_v7();
        let party_id = uuid::Uuid::now_v7();
        insert_party(&pool, party_id, "deleted-operator", "Deleted Operator").await;
        insert_source(
            &pool,
            source_id,
            "deleted-source",
            "Deleted Source",
            party_id,
            None,
            None,
        )
        .await;
        sqlx::query("DELETE FROM listing_sources WHERE listing_source_id = $1")
            .bind(source_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("delete test source: {error}"));

        assert!(read(&pool, "deleted-source").await.is_none());
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_show_renamed_source_and_operator_at_immutable_slug() {
        let pool = get_postgres_client().await;
        let source_id = uuid::Uuid::now_v7();
        let party_id = uuid::Uuid::now_v7();
        insert_party(&pool, party_id, "renamed-operator", "Original Operator").await;
        insert_source(
            &pool,
            source_id,
            "immutable-source",
            "Original Source",
            party_id,
            None,
            None,
        )
        .await;
        sqlx::query("UPDATE parties SET name = $1 WHERE party_id = $2")
            .bind("Renamed Operator")
            .bind(party_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("rename operator: {error}"));
        sqlx::query("UPDATE listing_sources SET name = $1 WHERE listing_source_id = $2")
            .bind("Renamed Source")
            .bind(source_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("rename source: {error}"));

        let result = read(&pool, "immutable-source").await;

        assert!(matches!(
            result,
            Some(summary)
                if summary.name.as_ref() == "Renamed Source"
                    && summary.operator.name.as_ref() == "Renamed Operator"
        ));
    }

    async fn read(pool: &sqlx::PgPool, slug: &str) -> Option<PublicListingSourceSummary> {
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let mut transaction = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin detail reader transaction: {error}"));
        let slug = ListingSourceSlugId::raw(slug)
            .unwrap_or_else(|error| panic!("valid test slug: {error}"));
        let result = SqlxPublicListingSourceDetailsReaderFactory::new()
            .in_transaction(&mut transaction)
            .find_by_slug(&slug)
            .await
            .unwrap_or_else(|error| panic!("read public detail: {error}"));
        transaction
            .commit()
            .await
            .unwrap_or_else(|error| panic!("commit detail reader transaction: {error}"));
        result
    }

    async fn insert_party(pool: &sqlx::PgPool, id: uuid::Uuid, slug: &str, name: &str) {
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(slug)
            .bind(name)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert test party: {error}"));
    }

    async fn insert_source(
        pool: &sqlx::PgPool,
        id: uuid::Uuid,
        slug: &str,
        name: &str,
        party_id: uuid::Uuid,
        url: Option<&str>,
        image: Option<&str>,
    ) {
        sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, url, image) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(id)
            .bind(slug)
            .bind(name)
            .bind(party_id)
            .bind(url)
            .bind(image)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert test source: {error}"));
    }
}
