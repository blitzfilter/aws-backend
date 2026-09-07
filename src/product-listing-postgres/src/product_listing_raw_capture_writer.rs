use application::error::box_error;
use product_listing_service::ports::{
    ProductListingRawCaptureWrite, ProductListingRawCaptureWriteError,
    ProductListingRawCaptureWriteOutcome, ProductListingRawCaptureWriter,
    ProductListingRawCaptureWriterFactory, ProductListingRawRevisionId, ProductListingRawStreamId,
};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingRawCaptureWriterFactory;

struct SqlxProductListingRawCaptureWriter<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct RawStreamHeadRow {
    product_listing_raw_stream_id: uuid::Uuid,
    source_record_key: String,
    latest_revision: i64,
    latest_input_sha256: Option<Vec<u8>>,
    latest_provider_source_occurred_at: Option<OffsetDateTime>,
    latest_provider_source_observation_sha256: Option<Vec<u8>>,
}

impl SqlxProductListingRawCaptureWriterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingRawCaptureWriterFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingRawCaptureWriterFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingRawCaptureWriter + 'tx {
        SqlxProductListingRawCaptureWriter {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingRawCaptureWriter for SqlxProductListingRawCaptureWriter<'_> {
    async fn capture(
        &mut self,
        write: ProductListingRawCaptureWrite,
    ) -> Result<ProductListingRawCaptureWriteOutcome, ProductListingRawCaptureWriteError> {
        let listing_source_id = uuid::Uuid::from(write.listing_source_id);
        let source_record_key_sha256 = write.source_record_key_sha256.as_bytes().as_slice();
        let provider_receipt = match write.ingestion_method {
            product_listing_service::ports::ProductListingRawIngestionMethod::WebCrawl => None,
            product_listing_service::ports::ProductListingRawIngestionMethod::Shopify
            | product_listing_service::ports::ProductListingRawIngestionMethod::Woocommerce => {
                write.provider_receipt.as_ref()
            }
        };
        let canonical_source_evidence_sha256 = match write.ingestion_method {
            product_listing_service::ports::ProductListingRawIngestionMethod::WebCrawl => None,
            product_listing_service::ports::ProductListingRawIngestionMethod::Shopify
            | product_listing_service::ports::ProductListingRawIngestionMethod::Woocommerce => {
                Some(write.input.source_payload().canonical_sha256())
            }
        }
        .transpose()
        .map_err(|error| ProductListingRawCaptureWriteError::CaptureFailed {
            source: box_error(error),
        })?
        .map(|evidence| *evidence.as_bytes());
        let source_order_observation_sha256 =
            canonical_source_evidence_sha256
                .as_ref()
                .map(|source_evidence_sha256| {
                    provider_source_order_observation_sha256(
                        write.input.operation().as_str(),
                        source_evidence_sha256,
                    )
                });

        sqlx::query(
            r#"
            INSERT INTO product_listing_raw_streams (
                product_listing_raw_stream_id,
                listing_source_id,
                ingestion_method,
                source_record_key,
                source_record_key_sha256,
                latest_revision
            ) VALUES ($1, $2, $3, $4, $5, 0)
            ON CONFLICT (listing_source_id, ingestion_method, source_record_key_sha256) DO NOTHING
            "#,
        )
        .bind(uuid::Uuid::new_v4())
        .bind(listing_source_id)
        .bind(write.ingestion_method.as_str())
        .bind(&write.source_record_key)
        .bind(source_record_key_sha256)
        .execute(&mut *self.connection)
        .await
        .map_err(capture_failed)?;

        let stream = sqlx::query_as::<_, RawStreamHeadRow>(
            r#"
            SELECT
                product_listing_raw_stream_id,
                source_record_key,
                latest_revision,
                latest_input_sha256,
                latest_provider_source_occurred_at,
                latest_provider_source_observation_sha256
            FROM product_listing_raw_streams
            WHERE listing_source_id = $1
              AND ingestion_method = $2
              AND source_record_key_sha256 = $3
            FOR UPDATE
            "#,
        )
        .bind(listing_source_id)
        .bind(write.ingestion_method.as_str())
        .bind(source_record_key_sha256)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(capture_failed)?;

        if stream.source_record_key != write.source_record_key {
            return Err(ProductListingRawCaptureWriteError::SourceRecordKeyHashCollision);
        }

        let latest_revision = u64::try_from(stream.latest_revision)
            .map_err(|_| invalid_capture_state("raw stream revision is invalid"))?;

        if let Some(provider_receipt) = provider_receipt {
            let canonical_source_evidence_sha256 =
                canonical_source_evidence_sha256.as_ref().ok_or_else(|| {
                    invalid_capture_state("provider receipt source evidence is missing")
                })?;
            if provider_receipt.source_evidence_sha256().as_bytes()
                != canonical_source_evidence_sha256
            {
                return Err(ProductListingRawCaptureWriteError::ProviderReceiptDigestConflict);
            }

            sqlx::query(
                r#"
                DELETE FROM product_listing_raw_provider_observation_receipts
                WHERE product_listing_raw_stream_id = $1
                  AND provider_scope = $2
                  AND provider_delivery_id = $3
                  AND expires_at <= clock_timestamp()
                "#,
            )
            .bind(stream.product_listing_raw_stream_id)
            .bind(provider_receipt.scope().as_str())
            .bind(provider_receipt.delivery_id())
            .execute(&mut *self.connection)
            .await
            .map_err(capture_failed)?;

            let existing_evidence_sha256 = sqlx::query_scalar::<_, Vec<u8>>(
                r#"
                SELECT observation_sha256
                FROM product_listing_raw_provider_observation_receipts
                WHERE product_listing_raw_stream_id = $1
                  AND provider_scope = $2
                  AND provider_delivery_id = $3
                "#,
            )
            .bind(stream.product_listing_raw_stream_id)
            .bind(provider_receipt.scope().as_str())
            .bind(provider_receipt.delivery_id())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(capture_failed)?;

            if let Some(existing_evidence_sha256) = existing_evidence_sha256 {
                if existing_evidence_sha256.len() != canonical_source_evidence_sha256.len() {
                    return Err(invalid_capture_state(
                        "provider receipt evidence hash is invalid",
                    ));
                }
                if existing_evidence_sha256.as_slice()
                    == canonical_source_evidence_sha256.as_slice()
                {
                    return Ok(ProductListingRawCaptureWriteOutcome::Duplicate {
                        product_listing_raw_stream_id: ProductListingRawStreamId::from_uuid(
                            stream.product_listing_raw_stream_id,
                        ),
                        latest_revision,
                    });
                }
                return Err(ProductListingRawCaptureWriteError::ProviderReceiptDigestConflict);
            }
        }

        let (
            source_ordering_advancement,
            source_observation_is_stale,
            source_observation_matches_stream_head,
        ) = match (write.source_occurred_at, source_order_observation_sha256) {
            (Some(source_occurred_at), Some(source_order_observation_sha256)) => {
                match latest_provider_source_order(&stream, source_order_observation_sha256.len())?
                {
                    None => (
                        Some((source_occurred_at, source_order_observation_sha256)),
                        false,
                        false,
                    ),
                    Some((latest_source_occurred_at, latest_source_evidence_sha256)) => {
                        if source_occurred_at < latest_source_occurred_at {
                            (None, true, false)
                        } else if source_occurred_at == latest_source_occurred_at
                            && latest_source_evidence_sha256
                                != source_order_observation_sha256.as_slice()
                        {
                            return Err(
                                ProductListingRawCaptureWriteError::ProviderSourceOrderConflict,
                            );
                        } else if source_occurred_at > latest_source_occurred_at {
                            (
                                Some((source_occurred_at, source_order_observation_sha256)),
                                false,
                                false,
                            )
                        } else {
                            (None, false, true)
                        }
                    }
                }
            }
            _ => (None, false, false),
        };

        if let Some(provider_receipt) = provider_receipt {
            let canonical_source_evidence_sha256 =
                canonical_source_evidence_sha256.as_ref().ok_or_else(|| {
                    invalid_capture_state("provider receipt source evidence is missing")
                })?;
            sqlx::query(
                r#"
                INSERT INTO product_listing_raw_provider_observation_receipts (
                    product_listing_raw_stream_id,
                    provider_scope,
                    provider_delivery_id,
                    observation_sha256
                ) VALUES ($1, $2, $3, $4)
                "#,
            )
            .bind(stream.product_listing_raw_stream_id)
            .bind(provider_receipt.scope().as_str())
            .bind(provider_receipt.delivery_id())
            .bind(canonical_source_evidence_sha256.as_slice())
            .execute(&mut *self.connection)
            .await
            .map_err(capture_failed)?;
        }

        if source_observation_matches_stream_head {
            return Ok(ProductListingRawCaptureWriteOutcome::Unchanged {
                product_listing_raw_stream_id: ProductListingRawStreamId::from_uuid(
                    stream.product_listing_raw_stream_id,
                ),
                latest_revision,
            });
        }

        if source_observation_is_stale {
            return Ok(ProductListingRawCaptureWriteOutcome::Stale {
                product_listing_raw_stream_id: ProductListingRawStreamId::from_uuid(
                    stream.product_listing_raw_stream_id,
                ),
                latest_revision,
            });
        }

        if stream.latest_input_sha256.as_deref() == Some(write.input_sha256.as_bytes().as_slice()) {
            if let Some((source_occurred_at, source_evidence_sha256)) =
                source_ordering_advancement.as_ref()
            {
                sqlx::query(
                    r#"
                    UPDATE product_listing_raw_streams
                    SET latest_provider_source_occurred_at = $1,
                        latest_provider_source_observation_sha256 = $2,
                        updated = now()
                    WHERE product_listing_raw_stream_id = $3
                    "#,
                )
                .bind(*source_occurred_at)
                .bind(source_evidence_sha256.as_slice())
                .bind(stream.product_listing_raw_stream_id)
                .execute(&mut *self.connection)
                .await
                .map_err(capture_failed)?;
            }

            return Ok(ProductListingRawCaptureWriteOutcome::Unchanged {
                product_listing_raw_stream_id: ProductListingRawStreamId::from_uuid(
                    stream.product_listing_raw_stream_id,
                ),
                latest_revision,
            });
        }

        let revision = latest_revision
            .checked_add(1)
            .ok_or_else(|| invalid_capture_state("raw stream revision overflow"))?;
        let revision_as_i64 = i64::try_from(revision)
            .map_err(|_| invalid_capture_state("raw stream revision exceeds storage range"))?;
        let product_listing_raw_revision_id = uuid::Uuid::new_v4();

        sqlx::query(
            r#"
            INSERT INTO product_listing_raw_revisions (
                product_listing_raw_revision_id,
                product_listing_raw_stream_id,
                revision,
                operation,
                payload_format,
                payload_schema_version,
                raw_values_schema_version,
                source_payload,
                raw_values,
                normalization_context,
                provenance,
                input_sha256,
                source_event_id,
                source_occurred_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14
            )
            "#,
        )
        .bind(product_listing_raw_revision_id)
        .bind(stream.product_listing_raw_stream_id)
        .bind(revision_as_i64)
        .bind(write.input.operation().as_str())
        .bind(write.input.payload_format().as_str())
        .bind(
            i16::try_from(write.input.payload_schema_version()).map_err(|_| {
                invalid_capture_state("payload schema version exceeds storage range")
            })?,
        )
        .bind(
            i16::try_from(write.input.raw_values_schema_version()).map_err(|_| {
                invalid_capture_state("raw-values schema version exceeds storage range")
            })?,
        )
        .bind(write.input.source_payload().value().clone())
        .bind(write.input.raw_values().value().clone())
        .bind(write.input.normalization_context().value().clone())
        .bind(write.provenance.value().clone())
        .bind(write.input_sha256.as_bytes().as_slice())
        .bind(write.source_event_id)
        .bind(write.source_occurred_at)
        .execute(&mut *self.connection)
        .await
        .map_err(capture_failed)?;

        let advances_provider_source_order = source_ordering_advancement.is_some();
        let advanced_source_occurred_at = source_ordering_advancement
            .as_ref()
            .map(|(source_occurred_at, _)| *source_occurred_at);
        let advanced_source_evidence_sha256 = source_ordering_advancement
            .as_ref()
            .map(|(_, source_evidence_sha256)| source_evidence_sha256.as_slice());

        sqlx::query(
            r#"
            UPDATE product_listing_raw_streams
            SET latest_revision = $1,
                latest_input_sha256 = $2,
                latest_provider_source_occurred_at = CASE
                    WHEN $3 THEN $4
                    ELSE latest_provider_source_occurred_at
                END,
                latest_provider_source_observation_sha256 = CASE
                    WHEN $3 THEN $5
                    ELSE latest_provider_source_observation_sha256
                END,
                updated = now()
            WHERE product_listing_raw_stream_id = $6
            "#,
        )
        .bind(revision_as_i64)
        .bind(write.input_sha256.as_bytes().as_slice())
        .bind(advances_provider_source_order)
        .bind(advanced_source_occurred_at)
        .bind(advanced_source_evidence_sha256)
        .bind(stream.product_listing_raw_stream_id)
        .execute(&mut *self.connection)
        .await
        .map_err(capture_failed)?;

        Ok(ProductListingRawCaptureWriteOutcome::Changed {
            product_listing_raw_stream_id: ProductListingRawStreamId::from_uuid(
                stream.product_listing_raw_stream_id,
            ),
            product_listing_raw_revision_id: ProductListingRawRevisionId::from_uuid(
                product_listing_raw_revision_id,
            ),
            revision,
        })
    }
}

fn provider_source_order_observation_sha256(
    operation: &str,
    source_evidence_sha256: &[u8; 32],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"PRODUCT_LISTING_PROVIDER_SOURCE_ORDER_V1\0");
    digest.update(operation.as_bytes());
    digest.update([0]);
    digest.update(source_evidence_sha256);
    digest.finalize().into()
}

fn latest_provider_source_order(
    stream: &RawStreamHeadRow,
    expected_evidence_sha256_length: usize,
) -> Result<Option<(OffsetDateTime, &[u8])>, ProductListingRawCaptureWriteError> {
    match (
        stream.latest_provider_source_occurred_at,
        stream.latest_provider_source_observation_sha256.as_deref(),
    ) {
        (None, None) => Ok(None),
        (Some(source_occurred_at), Some(source_evidence_sha256))
            if source_evidence_sha256.len() == expected_evidence_sha256_length =>
        {
            Ok(Some((source_occurred_at, source_evidence_sha256)))
        }
        (Some(_), Some(_)) => Err(invalid_capture_state(
            "raw stream provider source evidence hash is invalid",
        )),
        _ => Err(invalid_capture_state(
            "raw stream provider source ordering state is invalid",
        )),
    }
}

fn capture_failed(error: sqlx::Error) -> ProductListingRawCaptureWriteError {
    ProductListingRawCaptureWriteError::CaptureFailed {
        source: box_error(error),
    }
}

fn invalid_capture_state(message: &'static str) -> ProductListingRawCaptureWriteError {
    ProductListingRawCaptureWriteError::CaptureFailed {
        source: box_error(std::io::Error::other(message)),
    }
}
