use crate::object_id::try_from_uuid;
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxTransaction;
use product_listing_service::ports::{
    ProductListingEmbeddingWrite, ProductListingEmbeddingWriteError,
    ProductListingEmbeddingWriteOutcome, ProductListingEmbeddingWriter,
    ProductListingEmbeddingWriterFactory,
};
use serde_json::json;
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingEmbeddingWriterFactory;

struct SqlxProductListingEmbeddingWriter<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, thiserror::Error)]
#[error("product embedding SQL write failed")]
struct ProductListingEmbeddingWriteSqlxError(#[source] sqlx::Error);

impl SqlxProductListingEmbeddingWriterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingEmbeddingWriterFactory<SqlxTransaction>
    for SqlxProductListingEmbeddingWriterFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingEmbeddingWriter + 'tx {
        SqlxProductListingEmbeddingWriter {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingEmbeddingWriter for SqlxProductListingEmbeddingWriter<'_> {
    async fn apply(
        &mut self,
        write: &ProductListingEmbeddingWrite,
    ) -> Result<ProductListingEmbeddingWriteOutcome, ProductListingEmbeddingWriteError> {
        let embedding_source_event_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT embedding_source_event_id FROM product_listings WHERE product_listing_id = $1 FOR UPDATE",
        )
        .bind(write.product_listing_id.as_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductListingEmbeddingWriteSqlxError)?;
        let Some(embedding_source_event_id) = embedding_source_event_id else {
            return Ok(ProductListingEmbeddingWriteOutcome::ProductListingNotFound);
        };
        if duplicate_embedding_exists(self.connection, write).await? {
            return Ok(ProductListingEmbeddingWriteOutcome::Duplicate);
        }
        let embedding_source_event_id =
            try_from_uuid::<EventId>(embedding_source_event_id, "embedding source event ID")
                .map_err(|source| ProductListingEmbeddingWriteError::WriteFailed {
                    source: application::error::box_error(source),
                })?;
        if embedding_source_event_id != write.source_event_id {
            return Ok(ProductListingEmbeddingWriteOutcome::Stale);
        }

        let payload = json!({
            "sourceEventId": write.source_event_id.as_uuid().to_string(),
        });
        sqlx::query(
            r#"
            INSERT INTO product_listing_events (
                event_id, product_listing_id, event_type, event_group, event_type_schema_version,
                payload, event_time
            ) VALUES ($1, $2, 'ENRICHMENT_EMBEDDED', 'ENRICHMENT', 1, $3, now())
        "#,
        )
        .bind(write.enrichment_event_id.as_uuid())
        .bind(write.product_listing_id.as_uuid())
        .bind(payload)
        .execute(&mut *self.connection)
        .await
        .map_err(ProductListingEmbeddingWriteSqlxError)?;
        let update = sqlx::query(
            "UPDATE product_listings SET embedding = $1, current_event_id = $2, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $3 AND embedding_source_event_id = $4",
        )
        .bind(&write.embedding)
        .bind(write.enrichment_event_id.as_uuid())
        .bind(write.product_listing_id.as_uuid())
        .bind(write.source_event_id.as_uuid())
        .execute(&mut *self.connection).await.map_err(ProductListingEmbeddingWriteSqlxError)?;
        if update.rows_affected() != 1 {
            return Err(ProductListingEmbeddingWriteError::WriteFailed {
                source: application::error::static_error(
                    "locked product embedding source revision changed unexpectedly",
                ),
            });
        }
        Ok(ProductListingEmbeddingWriteOutcome::Applied)
    }
}

async fn duplicate_embedding_exists(
    connection: &mut PgConnection,
    write: &ProductListingEmbeddingWrite,
) -> Result<bool, ProductListingEmbeddingWriteError> {
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM product_listing_events
            WHERE product_listing_id = $1
              AND event_type = 'ENRICHMENT_EMBEDDED'
              AND event_group = 'ENRICHMENT'
              AND event_type_schema_version = 1
              AND payload ->> 'sourceEventId' = $2
        )
    "#,
    )
    .bind(write.product_listing_id.as_uuid())
    .bind(write.source_event_id.as_uuid().to_string())
    .fetch_one(&mut *connection)
    .await
    .map_err(ProductListingEmbeddingWriteSqlxError)
    .map_err(Into::into)
}

impl From<ProductListingEmbeddingWriteSqlxError> for ProductListingEmbeddingWriteError {
    fn from(source: ProductListingEmbeddingWriteSqlxError) -> Self {
        Self::WriteFailed {
            source: application::error::box_error(source),
        }
    }
}
