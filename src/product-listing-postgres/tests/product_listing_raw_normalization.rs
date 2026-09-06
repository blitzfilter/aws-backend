use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_normalization::{
    NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingProvenance, RawProductListingValues,
    SourcePayload,
};
use product_listing_postgres::{
    SqlxPendingProductListingRawStreamReader, SqlxProductListingEventAppenderFactory,
    SqlxProductListingRawCaptureWriterFactory, SqlxProductListingRawNormalizationWriterFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::ports::{
    ProductListingRawCaptureWrite, ProductListingRawCaptureWriteOutcome,
    ProductListingRawCaptureWriter, ProductListingRawCaptureWriterFactory,
    ProductListingRawIngestionMethod, SourceRecordKeySha256,
};
use product_service::ports::{
    PendingProductListingRawStreamReader, ProductListingRawNormalizationOutcome,
};
use product_service::use_cases::{
    NormalizeProductListingRawRevisionCommand, NormalizeProductListingRawRevisionError,
    NormalizeProductListingRawRevisionHandler, NormalizeProductListingRawRevisionMode,
    NormalizeProductListingRawRevisionUseCase,
};
use serde_json::{Value, json};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_process_stream_in_order_and_ignore_duplicate_late_wakeup() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-normalization-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();

    let first = capture(
        &unit_of_work,
        &capture_writer,
        raw_write(
            listing_source_id,
            RawProductListingOperation::Upsert,
            upsert_values("EUR 100"),
            normalization_context(),
            "first",
        ),
    )
    .await;
    let second = capture(
        &unit_of_work,
        &capture_writer,
        raw_write(
            listing_source_id,
            RawProductListingOperation::Upsert,
            upsert_values("EUR 120"),
            normalization_context(),
            "second",
        ),
    )
    .await;
    let third = capture(
        &unit_of_work,
        &capture_writer,
        raw_write(
            listing_source_id,
            RawProductListingOperation::Delete,
            json!({}),
            json!({}),
            "third",
        ),
    )
    .await;

    let (product_listing_raw_stream_id, product_listing_raw_revision_id, revision) =
        changed_parts(third);
    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        second,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 2, .. }
    ));
    assert_eq!(3, revision);

    let normalizer = NormalizeProductListingRawRevisionHandler::new(
        unit_of_work,
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool.clone()),
    );
    let command = NormalizeProductListingRawRevisionCommand {
        mode: NormalizeProductListingRawRevisionMode::RawRevision {
            product_listing_raw_stream_id,
            product_listing_raw_revision_id,
            revision,
        },
        max_revisions_per_stream: 3,
        pending_stream_limit: 1,
    };

    let result = normalizer
        .execute(command.clone())
        .await
        .unwrap_or_else(|error| panic!("normalize stream: {error}"));
    assert_eq!(3, result.revisions.len());
    assert_eq!(
        vec![1, 2, 3],
        result
            .revisions
            .iter()
            .map(|revision| revision.revision)
            .collect::<Vec<_>>()
    );
    assert!(
        result
            .revisions
            .iter()
            .all(|revision| revision.outcome == ProductListingRawNormalizationOutcome::Applied)
    );

    let duplicate = normalizer
        .execute(command)
        .await
        .unwrap_or_else(|error| panic!("normalize duplicate: {error}"));
    assert!(duplicate.revisions.is_empty());

    let listing: (String, Option<String>, Option<i64>) =
        sqlx::query_as("SELECT lifecycle, availability, price_amount FROM product_listings")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("load normalized product listing: {error}"));
    assert_eq!("WITHDRAWN", listing.0);
    assert_eq!(None, listing.1);
    assert_eq!(Some(12_000), listing.2);

    let normalizations: Vec<(i64, String)> = sqlx::query_as(
        "SELECT revision, outcome FROM product_listing_raw_normalizations ORDER BY revision",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_else(|error| panic!("load normalization results: {error}"));
    assert_eq!(
        vec![
            (1, "APPLIED".to_owned()),
            (2, "APPLIED".to_owned()),
            (3, "APPLIED".to_owned()),
        ],
        normalizations
    );
    let head: (i64, Option<uuid::Uuid>, Option<String>) = sqlx::query_as(
        "SELECT last_processed_revision, product_listing_id, source_listing_id FROM product_listing_raw_normalization_heads",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("load normalization head: {error}"));
    assert_eq!(3, head.0);
    assert!(head.1.is_some());
    assert_eq!(Some("source-123".to_owned()), head.2);

    let event_count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events")
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("count product listing events: {error}"));
    assert_eq!(3, event_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_ignore_delete_without_bound_product_listing() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-normalization-delete-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();
    let deleted = capture(
        &unit_of_work,
        &capture_writer,
        raw_write(
            listing_source_id,
            RawProductListingOperation::Delete,
            json!({}),
            json!({}),
            "delete-only",
        ),
    )
    .await;
    let (product_listing_raw_stream_id, product_listing_raw_revision_id, revision) =
        changed_parts(deleted);
    let normalizer = NormalizeProductListingRawRevisionHandler::new(
        unit_of_work,
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool.clone()),
    );

    let result = normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::RawRevision {
                product_listing_raw_stream_id,
                product_listing_raw_revision_id,
                revision,
            },
            max_revisions_per_stream: 1,
            pending_stream_limit: 1,
        })
        .await
        .unwrap_or_else(|error| panic!("normalize delete: {error}"));
    assert!(matches!(
        result.revisions.as_slice(),
        [revision] if revision.outcome == ProductListingRawNormalizationOutcome::Ignored
    ));

    let listing_count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listings")
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("count product listings: {error}"));
    assert_eq!(0, listing_count);
    let result_code: String =
        sqlx::query_scalar("SELECT outcome FROM product_listing_raw_normalizations")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("load normalization outcome: {error}"));
    assert_eq!("IGNORED", result_code);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_fail_unsupported_stored_schema_without_advancing_progress() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-normalization-schema-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();
    let captured = capture(
        &unit_of_work,
        &capture_writer,
        unsupported_schema_write(listing_source_id),
    )
    .await;
    let (product_listing_raw_stream_id, product_listing_raw_revision_id, revision) =
        changed_parts(captured);
    let normalizer = NormalizeProductListingRawRevisionHandler::new(
        unit_of_work,
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool.clone()),
    );

    let result = normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::RawRevision {
                product_listing_raw_stream_id,
                product_listing_raw_revision_id,
                revision,
            },
            max_revisions_per_stream: 1,
            pending_stream_limit: 1,
        })
        .await;
    assert!(matches!(
        result,
        Err(NormalizeProductListingRawRevisionError::UnsupportedStoredSchemaVersion)
    ));

    let result_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM product_listing_raw_normalizations")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("count normalization results: {error}"));
    assert_eq!(0, result_count);
    let head_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM product_listing_raw_normalization_heads")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("count normalization heads: {error}"));
    assert_eq!(0, head_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_advance_rejection_and_record_no_change_for_later_revisions() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-normalization-rejection-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();
    let invalid = capture(
        &unit_of_work,
        &capture_writer,
        raw_write(
            listing_source_id,
            RawProductListingOperation::Upsert,
            json!({}),
            json!({}),
            "invalid",
        ),
    )
    .await;
    let applied = capture(
        &unit_of_work,
        &capture_writer,
        raw_write(
            listing_source_id,
            RawProductListingOperation::Upsert,
            upsert_values("EUR 100"),
            normalization_context(),
            "valid",
        ),
    )
    .await;
    let no_change = capture(
        &unit_of_work,
        &capture_writer,
        raw_write(
            listing_source_id,
            RawProductListingOperation::Upsert,
            upsert_values("EUR 100"),
            normalization_context(),
            "unknown-source-key-changed",
        ),
    )
    .await;
    let (product_listing_raw_stream_id, product_listing_raw_revision_id, revision) =
        changed_parts(no_change);
    assert!(matches!(
        invalid,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        applied,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 2, .. }
    ));
    assert_eq!(3, revision);

    let normalizer = NormalizeProductListingRawRevisionHandler::new(
        unit_of_work,
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool.clone()),
    );
    let result = normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::RawRevision {
                product_listing_raw_stream_id,
                product_listing_raw_revision_id,
                revision,
            },
            max_revisions_per_stream: 3,
            pending_stream_limit: 1,
        })
        .await
        .unwrap_or_else(|error| panic!("normalize stream: {error}"));
    assert_eq!(
        vec![
            ProductListingRawNormalizationOutcome::Rejected,
            ProductListingRawNormalizationOutcome::Applied,
            ProductListingRawNormalizationOutcome::NoChange,
        ],
        result
            .revisions
            .into_iter()
            .map(|revision| revision.outcome)
            .collect::<Vec<_>>()
    );

    let results: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT revision, outcome, error_code FROM product_listing_raw_normalizations ORDER BY revision",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_else(|error| panic!("load normalization results: {error}"));
    assert_eq!(
        vec![
            (
                1,
                "REJECTED".to_owned(),
                Some("RAW_VALUES_INVALID".to_owned())
            ),
            (2, "APPLIED".to_owned(), None),
            (3, "NO_CHANGE".to_owned(), None),
        ],
        results
    );
    let event_count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events")
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("count product listing events: {error}"));
    assert_eq!(1, event_count);
    let last_processed_revision: i64 = sqlx::query_scalar(
        "SELECT last_processed_revision FROM product_listing_raw_normalization_heads",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("load normalization head: {error}"));
    assert_eq!(3, last_processed_revision);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_list_pending_streams_oldest_first_with_oldest_pending_capture_time() {
    let pool = get_postgres_client().await;
    let first_source = seed_listing_source(&pool, "raw-normalization-pending-first").await;
    let second_source = seed_listing_source(&pool, "raw-normalization-pending-second").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();

    let (first_stream, _, _) = changed_parts(
        capture(
            &unit_of_work,
            &capture_writer,
            raw_write(
                first_source,
                RawProductListingOperation::Upsert,
                upsert_values("EUR 100"),
                normalization_context(),
                "first-pending",
            ),
        )
        .await,
    );
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let (second_stream, _, _) = changed_parts(
        capture(
            &unit_of_work,
            &capture_writer,
            raw_write(
                second_source,
                RawProductListingOperation::Upsert,
                upsert_values("EUR 200"),
                normalization_context(),
                "second-pending",
            ),
        )
        .await,
    );

    let reader = SqlxPendingProductListingRawStreamReader::new(pool);
    let first_page = reader
        .list_pending_streams(1)
        .await
        .unwrap_or_else(|error| panic!("list first pending page: {error}"));
    let full_page = reader
        .list_pending_streams(2)
        .await
        .unwrap_or_else(|error| panic!("list full pending page: {error}"));

    assert_eq!(
        vec![first_stream],
        first_page
            .iter()
            .map(|stream| stream.product_listing_raw_stream_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        vec![first_stream, second_stream],
        full_page
            .iter()
            .map(|stream| stream.product_listing_raw_stream_id)
            .collect::<Vec<_>>()
    );
    assert!(full_page[0].oldest_pending_at < full_page[1].oldest_pending_at);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_changed_derived_source_listing_id_without_second_listing() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-normalization-identity-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();
    let first = capture(
        &unit_of_work,
        &capture_writer,
        raw_write(
            listing_source_id,
            RawProductListingOperation::Upsert,
            upsert_values("EUR 100"),
            normalization_context(),
            "first",
        ),
    )
    .await;
    let mut changed_identity = upsert_values("EUR 110");
    changed_identity["sourceListingId"] = json!("source-456");
    let second = capture(
        &unit_of_work,
        &capture_writer,
        raw_write(
            listing_source_id,
            RawProductListingOperation::Upsert,
            changed_identity,
            normalization_context(),
            "changed-identity",
        ),
    )
    .await;
    let (product_listing_raw_stream_id, product_listing_raw_revision_id, revision) =
        changed_parts(second);
    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));

    let normalizer = NormalizeProductListingRawRevisionHandler::new(
        unit_of_work,
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool.clone()),
    );
    let result = normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::RawRevision {
                product_listing_raw_stream_id,
                product_listing_raw_revision_id,
                revision,
            },
            max_revisions_per_stream: 2,
            pending_stream_limit: 1,
        })
        .await
        .unwrap_or_else(|error| panic!("normalize stream: {error}"));
    assert_eq!(
        vec![
            ProductListingRawNormalizationOutcome::Applied,
            ProductListingRawNormalizationOutcome::Rejected,
        ],
        result
            .revisions
            .into_iter()
            .map(|revision| revision.outcome)
            .collect::<Vec<_>>()
    );
    let listing_count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listings")
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("count product listings: {error}"));
    assert_eq!(1, listing_count);
    let error_code: Option<String> = sqlx::query_scalar(
        "SELECT error_code FROM product_listing_raw_normalizations WHERE revision = 2",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("load rejection code: {error}"));
    assert_eq!(Some("SOURCE_LISTING_ID_MISMATCH".to_owned()), error_code);
}

fn upsert_values(price: &str) -> Value {
    json!({
        "sourceListingId": "source-123",
        "title": {"action": "SET", "value": "An antique ceramic vase from an English collection"},
        "description": {"action": "SET", "value": ["This antique ceramic vase has documented provenance and careful restoration history."]},
        "price": {"action": "SET", "value": price},
        "priceEstimateMin": {"action": "CLEAR"},
        "priceEstimateMax": {"action": "CLEAR"},
        "availability": {"action": "SET", "value": "in stock"},
        "url": {"action": "SET", "value": "https://example.test/listings/source-123"},
        "images": {"action": "SET", "value": ["/images/source-123.jpg"]},
        "auctionStart": {"action": "UNCHANGED"},
        "auctionEnd": {"action": "UNCHANGED"},
        "attributes": {"material": {"action": "SET", "value": ["ceramic"]}}
    })
}

fn normalization_context() -> Value {
    json!({"baseUrl": "https://example.test/listings/source-123", "fallbackCurrency": "EUR"})
}

fn raw_write(
    listing_source_id: ListingSourceId,
    operation: RawProductListingOperation,
    raw_values: Value,
    normalization_context: Value,
    source_event_id: &str,
) -> ProductListingRawCaptureWrite {
    let input = ProductListingNormalizationInput::new(
        operation,
        RawProductListingPayloadFormat::ShopifyProduct,
        1,
        1,
        SourcePayload::new(json!({"retainedUnknown": source_event_id}))
            .unwrap_or_else(|error| panic!("source payload: {error}")),
        RawProductListingValues::new(raw_values)
            .unwrap_or_else(|error| panic!("raw values: {error}")),
        NormalizationContext::new(normalization_context)
            .unwrap_or_else(|error| panic!("normalization context: {error}")),
    )
    .unwrap_or_else(|error| panic!("normalization input: {error}"));
    let input_sha256 = input
        .hash()
        .unwrap_or_else(|error| panic!("normalization input hash: {error}"));
    ProductListingRawCaptureWrite {
        listing_source_id,
        ingestion_method: ProductListingRawIngestionMethod::Shopify,
        source_record_key: "123".to_owned(),
        source_record_key_sha256: SourceRecordKeySha256::new([8; 32]),
        input,
        input_sha256,
        provenance: RawProductListingProvenance::new(json!({"deliveryId": source_event_id}))
            .unwrap_or_else(|error| panic!("provenance: {error}")),
        source_event_id: Some(source_event_id.to_owned()),
        source_occurred_at: None,
    }
}

fn unsupported_schema_write(listing_source_id: ListingSourceId) -> ProductListingRawCaptureWrite {
    let input = ProductListingNormalizationInput::new(
        RawProductListingOperation::Upsert,
        RawProductListingPayloadFormat::ShopifyProduct,
        2,
        1,
        SourcePayload::new(json!({})).unwrap_or_else(|error| panic!("source payload: {error}")),
        RawProductListingValues::new(json!({}))
            .unwrap_or_else(|error| panic!("raw values: {error}")),
        NormalizationContext::new(json!({}))
            .unwrap_or_else(|error| panic!("normalization context: {error}")),
    )
    .unwrap_or_else(|error| panic!("normalization input: {error}"));
    let input_sha256 = input
        .hash()
        .unwrap_or_else(|error| panic!("normalization input hash: {error}"));
    ProductListingRawCaptureWrite {
        listing_source_id,
        ingestion_method: ProductListingRawIngestionMethod::Shopify,
        source_record_key: "unsupported-schema".to_owned(),
        source_record_key_sha256: SourceRecordKeySha256::new([9; 32]),
        input,
        input_sha256,
        provenance: RawProductListingProvenance::new(json!({}))
            .unwrap_or_else(|error| panic!("provenance: {error}")),
        source_event_id: None,
        source_occurred_at: None,
    }
}

async fn capture(
    unit_of_work: &SqlxUnitOfWork,
    factory: &SqlxProductListingRawCaptureWriterFactory,
    write: ProductListingRawCaptureWrite,
) -> ProductListingRawCaptureWriteOutcome {
    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin capture transaction: {error}"));
    let outcome = factory
        .in_transaction(&mut tx)
        .capture(write)
        .await
        .unwrap_or_else(|error| panic!("capture raw revision: {error}"));
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit capture transaction: {error}"));
    outcome
}

fn changed_parts(
    outcome: ProductListingRawCaptureWriteOutcome,
) -> (
    product_listing_service::ports::ProductListingRawStreamId,
    product_listing_service::ports::ProductListingRawRevisionId,
    u64,
) {
    match outcome {
        ProductListingRawCaptureWriteOutcome::Changed {
            product_listing_raw_stream_id,
            product_listing_raw_revision_id,
            revision,
        } => (
            product_listing_raw_stream_id,
            product_listing_raw_revision_id,
            revision,
        ),
        ProductListingRawCaptureWriteOutcome::Unchanged { .. } => {
            panic!("test input must create a raw revision")
        }
    }
}

async fn seed_listing_source(pool: &sqlx::PgPool, slug: &str) -> ListingSourceId {
    let party_id = uuid::Uuid::new_v4();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed party: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(slug)
        .bind(slug)
        .bind(party_id)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed listing source: {error}"));
    listing_source_id
}
