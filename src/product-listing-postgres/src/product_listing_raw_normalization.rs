use crate::object_id::try_from_uuid;
use application::error::box_error;

use product_listing_core::source_listing_id::SourceListingId;
use product_listing_normalization::{
    NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingValues, SourcePayload,
};
use product_listing_service::ports::ProductListingRawStreamId;
use product_service::ports::{
    PendingProductListingRawStream, PendingProductListingRawStreamPage,
    PendingProductListingRawStreamPageRequest, PendingProductListingRawStreamReader,
    ProductListingRawNormalizationCompletion, ProductListingRawNormalizationHead,
    ProductListingRawNormalizationPortError, ProductListingRawNormalizationWork,
    ProductListingRawNormalizationWriter, ProductListingRawNormalizationWriterFactory,
    ProductListingRawRevision, ProductListingRawRevisionReader,
};
use sqlx::{PgConnection, PgPool};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingRawNormalizationWriterFactory;

struct SqlxProductListingRawNormalizationWriter<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, Clone)]
pub struct SqlxPendingProductListingRawStreamReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct RawNormalizationHeadRow {
    product_listing_raw_stream_id: uuid::Uuid,
    listing_source_id: uuid::Uuid,
    last_processed_revision: i64,
    product_listing_id: Option<uuid::Uuid>,
    source_listing_id: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct PendingRawStreamRow {
    product_listing_raw_stream_id: uuid::Uuid,
    oldest_pending_at: time::OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
struct RawRevisionRow {
    product_listing_raw_revision_id: uuid::Uuid,
    product_listing_raw_stream_id: uuid::Uuid,
    revision: i64,
    operation: String,
    payload_format: String,
    payload_schema_version: i16,
    raw_values_schema_version: i16,
    source_payload: serde_json::Value,
    raw_values: serde_json::Value,
    normalization_context: serde_json::Value,
}

impl SqlxProductListingRawNormalizationWriterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl SqlxPendingProductListingRawStreamReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl ProductListingRawNormalizationWriterFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingRawNormalizationWriterFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingRawNormalizationWriter + 'tx {
        SqlxProductListingRawNormalizationWriter {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingRawNormalizationWriter for SqlxProductListingRawNormalizationWriter<'_> {
    async fn lock_next(
        &mut self,
        product_listing_raw_stream_id: ProductListingRawStreamId,
    ) -> Result<ProductListingRawNormalizationWork, ProductListingRawNormalizationPortError> {
        let stream_id = product_listing_raw_stream_id.as_uuid();
        sqlx::query(
            r#"
            INSERT INTO product_listing_raw_normalization_heads (product_listing_raw_stream_id)
            VALUES ($1)
            ON CONFLICT (product_listing_raw_stream_id) DO NOTHING
            "#,
        )
        .bind(stream_id)
        .execute(&mut *self.connection)
        .await
        .map_err(persistence)?;

        let row = sqlx::query_as::<_, RawNormalizationHeadRow>(
            r#"
            SELECT
                head.product_listing_raw_stream_id,
                stream.listing_source_id,
                head.last_processed_revision,
                head.product_listing_id,
                head.source_listing_id
            FROM product_listing_raw_normalization_heads AS head
            JOIN product_listing_raw_streams AS stream
              ON stream.product_listing_raw_stream_id = head.product_listing_raw_stream_id
            WHERE head.product_listing_raw_stream_id = $1
            FOR UPDATE OF head
            "#,
        )
        .bind(stream_id)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(persistence)?;
        let head = head_from_row(row)?;
        let next_revision = head
            .last_processed_revision
            .checked_add(1)
            .ok_or_else(|| invalid_state("raw normalization revision overflow"))?;
        let next_revision_i64 = i64::try_from(next_revision)
            .map_err(|_| invalid_state("raw normalization revision exceeds storage range"))?;
        let revision = sqlx::query_as::<_, RawRevisionRow>(
            r#"
            SELECT
                product_listing_raw_revision_id,
                product_listing_raw_stream_id,
                revision,
                operation,
                payload_format,
                payload_schema_version,
                raw_values_schema_version,
                source_payload,
                raw_values,
                normalization_context
            FROM product_listing_raw_revisions
            WHERE product_listing_raw_stream_id = $1
              AND revision = $2
            "#,
        )
        .bind(stream_id)
        .bind(next_revision_i64)
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(persistence)?
        .map(revision_from_row)
        .transpose()?;
        Ok(ProductListingRawNormalizationWork {
            head,
            next_revision: revision,
        })
    }

    async fn complete(
        &mut self,
        completion: ProductListingRawNormalizationCompletion,
    ) -> Result<(), ProductListingRawNormalizationPortError> {
        let revision = i64::try_from(completion.revision)
            .map_err(|_| invalid_state("raw normalization revision exceeds storage range"))?;
        let normalizer_version = i16::try_from(completion.normalizer_version)
            .map_err(|_| invalid_state("normalizer version exceeds storage range"))?;
        sqlx::query(
            r#"
            INSERT INTO product_listing_raw_normalizations (
                product_listing_raw_revision_id,
                product_listing_raw_stream_id,
                revision,
                normalizer_version,
                outcome,
                product_listing_id,
                product_listing_event_id,
                error_code
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(completion.product_listing_raw_revision_id.as_uuid())
        .bind(completion.product_listing_raw_stream_id.as_uuid())
        .bind(revision)
        .bind(normalizer_version)
        .bind(completion.outcome.as_str())
        .bind(completion.product_listing_id.map(|value| value.into_uuid()))
        .bind(
            completion
                .product_listing_event_id
                .map(|value| value.into_uuid()),
        )
        .bind(completion.error_code)
        .execute(&mut *self.connection)
        .await
        .map_err(persistence)?;

        let updated = sqlx::query(
            r#"
            UPDATE product_listing_raw_normalization_heads
            SET last_processed_revision = $1,
                product_listing_id = $2,
                source_listing_id = $3,
                updated = now()
            WHERE product_listing_raw_stream_id = $4
              AND last_processed_revision = $5
            "#,
        )
        .bind(revision)
        .bind(
            completion
                .next_product_listing_id
                .map(|value| value.into_uuid()),
        )
        .bind(
            completion
                .next_source_listing_id
                .map(|value| value.to_string()),
        )
        .bind(completion.product_listing_raw_stream_id.as_uuid())
        .bind(revision - 1)
        .execute(&mut *self.connection)
        .await
        .map_err(persistence)?;
        if updated.rows_affected() != 1 {
            return Err(invalid_state("raw normalization head did not advance"));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl ProductListingRawRevisionReader for SqlxPendingProductListingRawStreamReader {
    async fn find_next_revision(
        &self,
        product_listing_raw_stream_id: ProductListingRawStreamId,
    ) -> Result<Option<ProductListingRawRevision>, ProductListingRawNormalizationPortError> {
        let row = sqlx::query_as::<_, RawRevisionRow>(
            r#"
            SELECT
                revision.product_listing_raw_revision_id,
                revision.product_listing_raw_stream_id,
                revision.revision,
                revision.operation,
                revision.payload_format,
                revision.payload_schema_version,
                revision.raw_values_schema_version,
                revision.source_payload,
                revision.raw_values,
                revision.normalization_context
            FROM product_listing_raw_revisions AS revision
            LEFT JOIN product_listing_raw_normalization_heads AS head
              ON head.product_listing_raw_stream_id = revision.product_listing_raw_stream_id
            WHERE revision.product_listing_raw_stream_id = $1
              AND revision.revision = COALESCE(head.last_processed_revision, 0) + 1
            "#,
        )
        .bind(product_listing_raw_stream_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(persistence)?;
        row.map(revision_from_row).transpose()
    }
}

#[async_trait::async_trait]
impl PendingProductListingRawStreamReader for SqlxPendingProductListingRawStreamReader {
    async fn list_pending_stream_page(
        &self,
        request: PendingProductListingRawStreamPageRequest,
    ) -> Result<PendingProductListingRawStreamPage, ProductListingRawNormalizationPortError> {
        let limit = usize::try_from(request.limit)
            .map_err(|_| invalid_state("raw pending page limit exceeds memory range"))?;
        if limit == 0 {
            return Err(invalid_state(
                "raw pending page limit must be greater than zero",
            ));
        }
        let cursor = request.cursor;
        let mut rows = sqlx::query_as::<_, PendingRawStreamRow>(
            r#"
            WITH pending_streams AS (
                SELECT
                    stream.product_listing_raw_stream_id,
                    MIN(revision.captured_at) AS oldest_pending_at
                FROM product_listing_raw_streams AS stream
                LEFT JOIN product_listing_raw_normalization_heads AS head
                  ON head.product_listing_raw_stream_id = stream.product_listing_raw_stream_id
                JOIN product_listing_raw_revisions AS revision
                  ON revision.product_listing_raw_stream_id = stream.product_listing_raw_stream_id
                 AND revision.revision > COALESCE(head.last_processed_revision, 0)
                WHERE stream.latest_revision > COALESCE(head.last_processed_revision, 0)
                GROUP BY stream.product_listing_raw_stream_id
            )
            SELECT product_listing_raw_stream_id, oldest_pending_at
            FROM pending_streams
            WHERE $2::timestamptz IS NULL
               OR (oldest_pending_at, product_listing_raw_stream_id) > ($2, $3::uuid)
            ORDER BY oldest_pending_at, product_listing_raw_stream_id
            LIMIT $1
            "#,
        )
        .bind(i64::from(request.limit) + 1)
        .bind(cursor.map(|cursor| cursor.oldest_pending_at))
        .bind(cursor.map(|cursor| cursor.product_listing_raw_stream_id.into_uuid()))
        .fetch_all(&self.pool)
        .await
        .map_err(persistence)?;
        let has_next_page = rows.len() > limit;
        rows.truncate(limit);
        let streams = rows
            .into_iter()
            .map(|row| {
                Ok(PendingProductListingRawStream {
                    product_listing_raw_stream_id: try_from_uuid(
                        row.product_listing_raw_stream_id,
                        "ProductListing raw stream ID",
                    )
                    .map_err(invalid_state_error)?,
                    oldest_pending_at: row.oldest_pending_at,
                })
            })
            .collect::<Result<Vec<_>, ProductListingRawNormalizationPortError>>()?;
        let next_cursor = has_next_page
            .then(|| streams.last().map(|stream| stream.cursor()))
            .flatten();
        Ok(PendingProductListingRawStreamPage {
            streams,
            next_cursor,
        })
    }
}

fn head_from_row(
    row: RawNormalizationHeadRow,
) -> Result<ProductListingRawNormalizationHead, ProductListingRawNormalizationPortError> {
    let last_processed_revision = u64::try_from(row.last_processed_revision)
        .map_err(|_| invalid_state("raw normalization head revision is invalid"))?;
    let source_listing_id = row
        .source_listing_id
        .map(|value| SourceListingId::try_from(value.as_str()))
        .transpose()
        .map_err(|_| invalid_state("raw normalization source listing ID is invalid"))?;
    if row.product_listing_id.is_some() != source_listing_id.is_some() {
        return Err(invalid_state(
            "raw normalization head binding is incomplete",
        ));
    }
    Ok(ProductListingRawNormalizationHead {
        product_listing_raw_stream_id: try_from_uuid(
            row.product_listing_raw_stream_id,
            "ProductListing raw stream ID",
        )
        .map_err(invalid_state_error)?,
        listing_source_id: try_from_uuid(row.listing_source_id, "ListingSource ID")
            .map_err(invalid_state_error)?,
        last_processed_revision,
        product_listing_id: row
            .product_listing_id
            .map(|value| try_from_uuid(value, "ProductListing ID"))
            .transpose()
            .map_err(invalid_state_error)?,
        source_listing_id,
    })
}

fn revision_from_row(
    row: RawRevisionRow,
) -> Result<ProductListingRawRevision, ProductListingRawNormalizationPortError> {
    let operation = RawProductListingOperation::from_code(&row.operation)
        .ok_or_else(|| invalid_state("raw revision operation is invalid"))?;
    let payload_format = RawProductListingPayloadFormat::from_code(&row.payload_format)
        .ok_or_else(|| invalid_state("raw revision payload format is invalid"))?;
    let payload_schema_version = u16::try_from(row.payload_schema_version)
        .map_err(|_| invalid_state("raw revision payload schema version is invalid"))?;
    let raw_values_schema_version = u16::try_from(row.raw_values_schema_version)
        .map_err(|_| invalid_state("raw revision raw-values schema version is invalid"))?;
    let input = ProductListingNormalizationInput::new(
        operation,
        payload_format,
        payload_schema_version,
        raw_values_schema_version,
        SourcePayload::new(row.source_payload).map_err(invalid_state_error)?,
        RawProductListingValues::new(row.raw_values).map_err(invalid_state_error)?,
        NormalizationContext::new(row.normalization_context).map_err(invalid_state_error)?,
    )
    .map_err(invalid_state_error)?;
    let revision =
        u64::try_from(row.revision).map_err(|_| invalid_state("raw revision number is invalid"))?;
    Ok(ProductListingRawRevision {
        product_listing_raw_revision_id: try_from_uuid(
            row.product_listing_raw_revision_id,
            "ProductListing raw revision ID",
        )
        .map_err(invalid_state_error)?,
        product_listing_raw_stream_id: try_from_uuid(
            row.product_listing_raw_stream_id,
            "ProductListing raw stream ID",
        )
        .map_err(invalid_state_error)?,
        revision,
        input,
    })
}

fn persistence(error: sqlx::Error) -> ProductListingRawNormalizationPortError {
    ProductListingRawNormalizationPortError::Persistence {
        source: box_error(error),
    }
}

fn invalid_state(message: &'static str) -> ProductListingRawNormalizationPortError {
    ProductListingRawNormalizationPortError::InvalidPersistedState {
        source: box_error(std::io::Error::other(message)),
    }
}

fn invalid_state_error(
    error: impl std::error::Error + Send + Sync + 'static,
) -> ProductListingRawNormalizationPortError {
    ProductListingRawNormalizationPortError::InvalidPersistedState {
        source: box_error(error),
    }
}
