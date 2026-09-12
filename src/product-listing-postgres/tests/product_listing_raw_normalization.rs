mod support;

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
    PendingProductListingRawStreamPageRequest, PendingProductListingRawStreamReader,
    ProductListingRawNormalizationOutcome,
};
use product_service::ports::{
    ProductListingRawNormalizationWriter, ProductListingRawNormalizationWriterFactory,
};
use product_service::use_cases::{
    NormalizeProductListingRawRevisionCommand, NormalizeProductListingRawRevisionError,
    NormalizeProductListingRawRevisionHandler, NormalizeProductListingRawRevisionMode,
    NormalizeProductListingRawRevisionUseCase,
};
use serde_json::{Value, json};
use std::time::Duration;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_apply_other_facts_and_preserve_lot_context_when_timing_is_invalid() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-normalization-auction-timing").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();
    let mut initial = upsert_values("EUR 100");
    initial["auction"] = json!({"action": "SET", "value": {
        "lotNumber": "42A",
        "cataloguePosition": 7,
        "timing": {
            "biddingOpens": {"precision": "INSTANT", "value": "2026-01-01T10:00:00Z"},
            "scheduledCloses": {"precision": "INSTANT", "value": "2026-01-01T12:00:00Z"}
        }
    }});
    let first = capture(
        &unit_of_work,
        &capture_writer,
        raw_write(
            listing_source_id,
            RawProductListingOperation::Upsert,
            initial.clone(),
            normalization_context(),
            "auction-timing-first",
        ),
    )
    .await;
    let mut invalid_timing = initial;
    invalid_timing["price"] = json!({"action": "SET", "value": "EUR 120"});
    invalid_timing["auction"]["value"]["timing"]["scheduledCloses"] =
        json!({"precision": "INSTANT", "value": "2026-01-01T09:00:00Z"});
    let second = capture(
        &unit_of_work,
        &capture_writer,
        raw_write(
            listing_source_id,
            RawProductListingOperation::Upsert,
            invalid_timing,
            normalization_context(),
            "auction-timing-invalid",
        ),
    )
    .await;
    let (product_listing_raw_stream_id, product_listing_raw_revision_id, revision) =
        changed_parts(second);
    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));

    let result = NormalizeProductListingRawRevisionHandler::new(
        unit_of_work,
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool.clone()),
    )
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
    .unwrap_or_else(|error| panic!("normalize auction timing stream: {error}"));
    assert_eq!(
        vec![
            ProductListingRawNormalizationOutcome::Applied,
            ProductListingRawNormalizationOutcome::Applied,
        ],
        result
            .revisions
            .into_iter()
            .map(|revision| revision.outcome)
            .collect::<Vec<_>>()
    );

    let (price_amount, lot_number, scheduled_closes_at): (i64, String, Option<time::OffsetDateTime>) =
        sqlx::query_as(
            "SELECT listing.price_amount, context.lot_number, timing.scheduled_closes_instant_at \
             FROM product_listings listing \
             JOIN product_listing_auction_contexts context ON context.product_listing_id = listing.product_listing_id \
             JOIN product_listing_lot_auction_timings timing ON timing.product_listing_id = listing.product_listing_id",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("load normalized auction context: {error}"));
    assert_eq!(12_000, price_amount);
    assert_eq!("42A", lot_number);
    assert_eq!(
        Some(
            time::OffsetDateTime::parse(
                "2026-01-01T12:00:00Z",
                &time::format_description::well_known::Rfc3339,
            )
            .unwrap_or_else(|error| panic!("expected timestamp: {error}"))
        ),
        scheduled_closes_at
    );
    let diagnostic: Option<String> = sqlx::query_scalar(
        "SELECT error_code FROM product_listing_raw_normalizations WHERE revision = 2",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("load timing diagnostic: {error}"));
    assert_eq!(Some("AUCTION_TIMING_INVALID".to_owned()), diagnostic);
}

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
async fn should_reconcile_healthy_stream_when_unsupported_stream_is_blocked_without_cdc() {
    let pool = get_postgres_client().await;
    let blocked_source = seed_listing_source(&pool, "raw-normalization-blocked-source").await;
    let healthy_source = seed_listing_source(&pool, "raw-normalization-healthy-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();
    let (blocked_stream_id, _, _) = changed_parts(
        capture(
            &unit_of_work,
            &capture_writer,
            unsupported_schema_write(blocked_source),
        )
        .await,
    );
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let (healthy_stream_id, _, _) = changed_parts(
        capture(
            &unit_of_work,
            &capture_writer,
            raw_write(
                healthy_source,
                RawProductListingOperation::Upsert,
                upsert_values("EUR 100"),
                normalization_context(),
                "healthy-reconcile",
            ),
        )
        .await,
    );
    let normalizer = NormalizeProductListingRawRevisionHandler::new(
        unit_of_work,
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool.clone()),
    );

    let result = normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::Reconcile,
            max_revisions_per_stream: 1,
            pending_stream_limit: 2,
        })
        .await
        .unwrap_or_else(|error| panic!("reconcile streams: {error}"));

    assert_eq!(
        vec![healthy_stream_id],
        result
            .revisions
            .iter()
            .map(|revision| revision.product_listing_raw_stream_id)
            .collect::<Vec<_>>()
    );
    assert!(
        result
            .revisions
            .iter()
            .all(|revision| revision.outcome == ProductListingRawNormalizationOutcome::Applied)
    );
    assert!(matches!(
        result.stream_failures.as_slice(),
        [failure]
            if failure.product_listing_raw_stream_id == blocked_stream_id
                && failure.error_code == "UNSUPPORTED_STORED_SCHEMA_VERSION"
    ));
    assert_eq!(Some(2), result.pending_stream_page_count);
    assert_eq!(None, result.next_pending_stream_cursor);

    let blocked_result_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_raw_normalizations WHERE product_listing_raw_stream_id = $1",
    )
    .bind(blocked_stream_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("count blocked normalization results: {error}"));
    assert_eq!(0, blocked_result_count);
    let blocked_head_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_raw_normalization_heads WHERE product_listing_raw_stream_id = $1",
    )
    .bind(blocked_stream_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("count blocked normalization heads: {error}"));
    assert_eq!(0, blocked_head_count);
    let event_count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events")
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("count canonical events: {error}"));
    assert_eq!(1, event_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reach_later_healthy_stream_after_blocked_reconciliation_page() {
    let pool = get_postgres_client().await;
    let first_blocked_source =
        seed_listing_source(&pool, "raw-normalization-first-blocked-source").await;
    let second_blocked_source =
        seed_listing_source(&pool, "raw-normalization-second-blocked-source").await;
    let healthy_source =
        seed_listing_source(&pool, "raw-normalization-cursor-healthy-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();
    let (first_blocked_stream_id, _, _) = changed_parts(
        capture(
            &unit_of_work,
            &capture_writer,
            unsupported_schema_write(first_blocked_source),
        )
        .await,
    );
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let (second_blocked_stream_id, _, _) = changed_parts(
        capture(
            &unit_of_work,
            &capture_writer,
            unsupported_schema_write(second_blocked_source),
        )
        .await,
    );
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let (healthy_stream_id, _, _) = changed_parts(
        capture(
            &unit_of_work,
            &capture_writer,
            raw_write(
                healthy_source,
                RawProductListingOperation::Upsert,
                upsert_values("EUR 100"),
                normalization_context(),
                "healthy-after-cursor",
            ),
        )
        .await,
    );
    let normalizer = NormalizeProductListingRawRevisionHandler::new(
        unit_of_work,
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool.clone()),
    );

    let first_page = normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::Reconcile,
            max_revisions_per_stream: 1,
            pending_stream_limit: 2,
        })
        .await
        .unwrap_or_else(|error| panic!("reconcile blocked page: {error}"));

    assert!(first_page.revisions.is_empty());
    assert_eq!(Some(2), first_page.pending_stream_page_count);
    assert!(matches!(
        first_page.stream_failures.as_slice(),
        [first, second]
            if first.product_listing_raw_stream_id == first_blocked_stream_id
                && second.product_listing_raw_stream_id == second_blocked_stream_id
                && first.error_code == "UNSUPPORTED_STORED_SCHEMA_VERSION"
                && second.error_code == "UNSUPPORTED_STORED_SCHEMA_VERSION"
    ));
    let cursor = first_page
        .next_pending_stream_cursor
        .unwrap_or_else(|| panic!("blocked page must provide a next cursor"));
    assert_eq!(
        second_blocked_stream_id,
        cursor.product_listing_raw_stream_id
    );

    let second_page = normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::ReconcileFromCursor {
                pending_stream_cursor: cursor,
            },
            max_revisions_per_stream: 1,
            pending_stream_limit: 2,
        })
        .await
        .unwrap_or_else(|error| panic!("reconcile healthy cursor page: {error}"));

    assert!(second_page.stream_failures.is_empty());
    assert_eq!(
        vec![healthy_stream_id],
        second_page
            .revisions
            .iter()
            .map(|revision| revision.product_listing_raw_stream_id)
            .collect::<Vec<_>>()
    );
    assert!(
        second_page
            .revisions
            .iter()
            .all(|revision| revision.outcome == ProductListingRawNormalizationOutcome::Applied)
    );
    assert_eq!(None, second_page.next_pending_stream_cursor);
    let blocked_result_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_raw_normalizations WHERE product_listing_raw_stream_id IN ($1, $2)",
    )
    .bind(first_blocked_stream_id.as_uuid())
    .bind(second_blocked_stream_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("count blocked normalization results: {error}"));
    assert_eq!(0, blocked_result_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_continue_capped_stream_across_keyset_cursor_and_reach_later_stream() {
    let pool = get_postgres_client().await;
    let capped_source = seed_listing_source(&pool, "raw-normalization-capped-source").await;
    let bridge_source = seed_listing_source(&pool, "raw-normalization-bridge-source").await;
    let later_source = seed_listing_source(&pool, "raw-normalization-later-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();

    // PostgreSQL's `now()` is transaction-stable. The capped stream's next revision therefore has
    // the same keyset timestamp as the cursor produced from its first revision.
    let (capped_first, capped_second) = capture_pair_in_one_transaction(
        &unit_of_work,
        &capture_writer,
        raw_write(
            capped_source,
            RawProductListingOperation::Upsert,
            upsert_values("EUR 100"),
            normalization_context(),
            "capped-first",
        ),
        raw_write(
            capped_source,
            RawProductListingOperation::Upsert,
            upsert_values("EUR 110"),
            normalization_context(),
            "capped-second",
        ),
    )
    .await;
    let (capped_stream_id, _, capped_first_revision) = changed_parts(capped_first);
    let (second_stream_id, _, capped_second_revision) = changed_parts(capped_second);
    assert_eq!(capped_stream_id, second_stream_id);
    assert_eq!(1, capped_first_revision);
    assert_eq!(2, capped_second_revision);

    let capped_capture_times: Vec<time::OffsetDateTime> = sqlx::query_scalar(
        "SELECT captured_at FROM product_listing_raw_revisions WHERE product_listing_raw_stream_id = $1 ORDER BY revision",
    )
    .bind(capped_stream_id.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap_or_else(|error| panic!("load capped capture timestamps: {error}"));
    let [first_captured_at, second_captured_at] = capped_capture_times.as_slice() else {
        panic!("capped stream must have exactly two revisions");
    };
    assert_eq!(first_captured_at, second_captured_at);

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let (bridge_stream_id, _, _) = changed_parts(
        capture(
            &unit_of_work,
            &capture_writer,
            raw_write(
                bridge_source,
                RawProductListingOperation::Upsert,
                upsert_values("EUR 200"),
                normalization_context(),
                "bridge",
            ),
        )
        .await,
    );
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let (later_stream_id, _, _) = changed_parts(
        capture(
            &unit_of_work,
            &capture_writer,
            raw_write(
                later_source,
                RawProductListingOperation::Upsert,
                upsert_values("EUR 300"),
                normalization_context(),
                "later",
            ),
        )
        .await,
    );

    let normalizer = NormalizeProductListingRawRevisionHandler::new(
        unit_of_work,
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool.clone()),
    );

    let capped_page = normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::Reconcile,
            max_revisions_per_stream: 1,
            pending_stream_limit: 1,
        })
        .await
        .unwrap_or_else(|error| panic!("reconcile capped stream: {error}"));
    assert_eq!(
        vec![capped_stream_id],
        capped_page
            .revisions
            .iter()
            .map(|revision| revision.product_listing_raw_stream_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(vec![capped_stream_id], capped_page.continuation_stream_ids);
    let capped_cursor = capped_page
        .next_pending_stream_cursor
        .unwrap_or_else(|| panic!("capped page must provide a cursor"));
    assert_eq!(
        capped_stream_id,
        capped_cursor.product_listing_raw_stream_id
    );
    assert_eq!(*second_captured_at, capped_cursor.oldest_pending_at);

    let continuation = normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::ReconcileContinuation {
                product_listing_raw_stream_id: capped_stream_id,
            },
            max_revisions_per_stream: 1,
            pending_stream_limit: 1,
        })
        .await
        .unwrap_or_else(|error| panic!("continue capped stream: {error}"));
    assert_eq!(
        vec![capped_second_revision],
        continuation
            .revisions
            .iter()
            .map(|revision| revision.revision)
            .collect::<Vec<_>>()
    );
    assert_eq!(None, continuation.pending_stream_page_count);
    assert_eq!(None, continuation.next_pending_stream_cursor);

    // The remaining capped revision compares equal to this cursor. After the worker-local
    // continuation, the global keyset page skips it and reaches the bridge stream.
    let bridge_page = normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::ReconcileFromCursor {
                pending_stream_cursor: capped_cursor,
            },
            max_revisions_per_stream: 1,
            pending_stream_limit: 1,
        })
        .await
        .unwrap_or_else(|error| panic!("reconcile bridge stream: {error}"));
    assert_eq!(
        vec![bridge_stream_id],
        bridge_page
            .revisions
            .iter()
            .map(|revision| revision.product_listing_raw_stream_id)
            .collect::<Vec<_>>()
    );
    let bridge_cursor = bridge_page
        .next_pending_stream_cursor
        .unwrap_or_else(|| panic!("bridge page must provide a cursor"));
    assert_eq!(
        bridge_stream_id,
        bridge_cursor.product_listing_raw_stream_id
    );

    let later_page = normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::ReconcileFromCursor {
                pending_stream_cursor: bridge_cursor,
            },
            max_revisions_per_stream: 1,
            pending_stream_limit: 1,
        })
        .await
        .unwrap_or_else(|error| panic!("reconcile later stream: {error}"));
    assert_eq!(
        vec![later_stream_id],
        later_page
            .revisions
            .iter()
            .map(|revision| revision.product_listing_raw_stream_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(None, later_page.next_pending_stream_cursor);

    let capped_normalization_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_raw_normalizations WHERE product_listing_raw_stream_id = $1",
    )
    .bind(capped_stream_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("count capped normalizations: {error}"));
    assert_eq!(2, capped_normalization_count);
    let later_normalization_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_raw_normalizations WHERE product_listing_raw_stream_id = $1",
    )
    .bind(later_stream_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("count later normalizations: {error}"));
    assert_eq!(1, later_normalization_count);
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
        .list_pending_stream_page(PendingProductListingRawStreamPageRequest {
            limit: 1,
            cursor: None,
        })
        .await
        .unwrap_or_else(|error| panic!("list first pending page: {error}"));
    let second_page = reader
        .list_pending_stream_page(PendingProductListingRawStreamPageRequest {
            limit: 1,
            cursor: first_page.next_cursor,
        })
        .await
        .unwrap_or_else(|error| panic!("list second pending page: {error}"));

    assert_eq!(
        vec![first_stream],
        first_page
            .streams
            .iter()
            .map(|stream| stream.product_listing_raw_stream_id)
            .collect::<Vec<_>>()
    );
    assert!(first_page.next_cursor.is_some());
    assert_eq!(
        vec![second_stream],
        second_page
            .streams
            .iter()
            .map(|stream| stream.product_listing_raw_stream_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(None, second_page.next_cursor);
    assert!(first_page.streams[0].oldest_pending_at < second_page.streams[0].oldest_pending_at);
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

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_normalize_valid_long_incompressible_url() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-normalization-long-url-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let capture_writer = SqlxProductListingRawCaptureWriterFactory::new();
    let long_url = long_incompressible_url();
    assert!(long_url.len() > 2_704);
    let (product_listing_raw_stream_id, product_listing_raw_revision_id, revision) = changed_parts(
        capture(
            &unit_of_work,
            &capture_writer,
            raw_write(
                listing_source_id,
                RawProductListingOperation::Upsert,
                upsert_values_with_url("EUR 100", &long_url),
                normalization_context(),
                "long-url",
            ),
        )
        .await,
    );
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
        .unwrap_or_else(|error| panic!("normalize long URL: {error}"));
    assert!(matches!(
        result.revisions.as_slice(),
        [result] if result.outcome == ProductListingRawNormalizationOutcome::Applied
    ));

    let persisted_url: String = sqlx::query_scalar("SELECT url FROM product_listings")
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("load normalized long URL: {error}"));
    assert_eq!(long_url, persisted_url);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_serialize_reversed_duplicate_wakeups_with_concurrent_reconciliation() {
    let result = concurrent_normalization(3).await;
    assert!(
        result.is_ok(),
        "concurrent reversed normalization: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reconcile_remaining_revisions_after_concurrent_capped_wakeups() {
    let result = concurrent_normalization(1).await;
    assert!(
        result.is_ok(),
        "capped concurrent normalization recovery: {result:?}"
    );
}

async fn concurrent_normalization(max_revisions: u32) -> Result<(), Box<dyn std::error::Error>> {
    let pool = get_postgres_client().await;
    let source = seed_listing_source(&pool, "concurrent-raw-source").await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let captures = SqlxProductListingRawCaptureWriterFactory::new();
    let first = changed_parts(
        capture(
            &unit,
            &captures,
            raw_write(
                source,
                RawProductListingOperation::Upsert,
                upsert_values("EUR 100"),
                normalization_context(),
                "first",
            ),
        )
        .await,
    );
    capture(
        &unit,
        &captures,
        raw_write(
            source,
            RawProductListingOperation::Upsert,
            upsert_values("EUR 120"),
            normalization_context(),
            "second",
        ),
    )
    .await;
    let last = changed_parts(
        capture(
            &unit,
            &captures,
            raw_write(
                source,
                RawProductListingOperation::Delete,
                json!({}),
                json!({}),
                "last",
            ),
        )
        .await,
    );
    let handler = || {
        NormalizeProductListingRawRevisionHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxProductListingRawNormalizationWriterFactory::new(),
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventAppenderFactory::new(),
            SqlxPendingProductListingRawStreamReader::new(pool.clone()),
        )
    };
    let wakeup = |(stream, revision_id, revision)| NormalizeProductListingRawRevisionCommand {
        mode: NormalizeProductListingRawRevisionMode::RawRevision {
            product_listing_raw_stream_id: stream,
            product_listing_raw_revision_id: revision_id,
            revision,
        },
        max_revisions_per_stream: max_revisions,
        pending_stream_limit: 1,
    };
    let reconcile = NormalizeProductListingRawRevisionCommand {
        mode: NormalizeProductListingRawRevisionMode::Reconcile,
        max_revisions_per_stream: max_revisions,
        pending_stream_limit: 1,
    };
    let latest_worker = handler();
    let duplicate_worker = handler();
    let older_worker = handler();
    let reconciler = handler();
    let mut gate = unit.begin().await?;
    let blocker_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(gate.connection())
        .await?;
    let work = SqlxProductListingRawNormalizationWriterFactory::new()
        .in_transaction(&mut gate)
        .lock_next(first.0)
        .await?;
    assert_eq!(
        Some(1),
        work.next_revision.map(|revision| revision.revision)
    );
    let attempts = async {
        tokio::join!(
            latest_worker.execute(wakeup(last)),
            duplicate_worker.execute(wakeup(last)),
            older_worker.execute(wakeup(first)),
            reconciler.execute(reconcile),
        )
    };
    tokio::pin!(attempts);
    support::assert_blocked(&pool, blocker_pid, 4, attempts.as_mut()).await?;
    gate.commit().await?;
    let (latest, duplicate, older, recovered) =
        tokio::time::timeout(Duration::from_secs(20), attempts).await?;
    let mut completed = Vec::new();
    for result in [latest?, duplicate?, older?, recovered?] {
        assert!(result.stream_failures.is_empty());
        completed.extend(
            result
                .revisions
                .into_iter()
                .map(|revision| revision.revision),
        );
    }
    completed.sort_unstable();
    assert_eq!(
        if max_revisions == 1 {
            vec![1]
        } else {
            vec![1, 2, 3]
        },
        completed
    );

    // A fresh process can discover any capped/raced remainder without another SQS message.
    let recovered = handler()
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::Reconcile,
            max_revisions_per_stream: 3,
            pending_stream_limit: 1,
        })
        .await?;
    assert!(recovered.stream_failures.is_empty());
    assert_eq!(
        if max_revisions == 1 {
            vec![2, 3]
        } else {
            vec![]
        },
        recovered
            .revisions
            .iter()
            .map(|revision| revision.revision)
            .collect::<Vec<_>>()
    );
    assert!(handler().execute(wakeup(first)).await?.revisions.is_empty());
    assert!(handler().execute(wakeup(last)).await?.revisions.is_empty());
    let persisted: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT normalized.revision, normalized.outcome, event.event_type FROM product_listing_raw_normalizations normalized JOIN product_listing_events event ON event.event_id = normalized.product_listing_event_id ORDER BY normalized.revision"
    ).fetch_all(&pool).await?;
    assert_eq!(
        vec![
            (
                1,
                "APPLIED".to_owned(),
                "PRODUCT_LISTING_DISCOVERED".to_owned()
            ),
            (
                2,
                "APPLIED".to_owned(),
                "PRODUCT_LISTING_CHANGED".to_owned()
            ),
            (
                3,
                "APPLIED".to_owned(),
                "PRODUCT_LISTING_CHANGED".to_owned()
            ),
        ],
        persisted
    );
    let state: (i64, String, Option<String>, i64, i64, i64) = sqlx::query_as(
        "SELECT head.last_processed_revision, product.lifecycle, product.availability, product.price_amount, product.version, (SELECT count(*) FROM product_listing_events) FROM product_listing_raw_normalization_heads head JOIN product_listings product ON product.product_listing_id = head.product_listing_id"
    ).fetch_one(&pool).await?;
    assert_eq!((3, "WITHDRAWN".to_owned(), None, 12_000, 3, 3), state);
    assert!(
        SqlxPendingProductListingRawStreamReader::new(pool)
            .list_pending_stream_page(PendingProductListingRawStreamPageRequest {
                limit: 1,
                cursor: None
            })
            .await?
            .streams
            .is_empty()
    );
    Ok(())
}

fn upsert_values(price: &str) -> Value {
    json!({
        "sourceListingId": "source-123",
        "priceFormat": "DISPLAY_TEXT",
        "title": {"action": "SET", "value": "An antique ceramic vase from an English collection"},
        "description": {"action": "SET", "value": ["This antique ceramic vase has documented provenance and careful restoration history."]},
        "price": {"action": "SET", "value": price},
        "priceEstimateMin": {"action": "CLEAR"},
        "priceEstimateMax": {"action": "CLEAR"},
        "availability": {"action": "SET", "value": "in stock"},
        "url": {"action": "SET", "value": "https://example.test/listings/source-123"},
        "images": {"action": "SET", "value": ["/images/source-123.jpg"]},
        "auction": {"action": "UNCHANGED"},
        "attributes": {"material": {"action": "SET", "value": ["ceramic"]}}
    })
}

fn upsert_values_with_url(price: &str, url: &str) -> Value {
    let mut values = upsert_values(price);
    values["url"] = json!({"action": "SET", "value": url});
    values
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
        provider_receipt: None,
    }
}

fn long_incompressible_url() -> String {
    const PATH_LENGTH: usize = 4_096;
    const URL_SAFE_ASCII: &[u8] =
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";

    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut path = String::with_capacity(PATH_LENGTH);
    for _ in 0..PATH_LENGTH {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let index = ((state >> 32) as usize) % URL_SAFE_ASCII.len();
        path.push(URL_SAFE_ASCII[index] as char);
    }

    format!("https://example.test/{path}")
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
        provider_receipt: None,
    }
}

async fn capture_pair_in_one_transaction(
    unit_of_work: &SqlxUnitOfWork,
    factory: &SqlxProductListingRawCaptureWriterFactory,
    first: ProductListingRawCaptureWrite,
    second: ProductListingRawCaptureWrite,
) -> (
    ProductListingRawCaptureWriteOutcome,
    ProductListingRawCaptureWriteOutcome,
) {
    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin paired capture transaction: {error}"));
    let outcomes = {
        let mut writer = factory.in_transaction(&mut tx);
        let first = writer
            .capture(first)
            .await
            .unwrap_or_else(|error| panic!("capture first paired raw revision: {error}"));
        let second = writer
            .capture(second)
            .await
            .unwrap_or_else(|error| panic!("capture second paired raw revision: {error}"));
        (first, second)
    };
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("commit paired capture transaction: {error}"));
    outcomes
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
        ProductListingRawCaptureWriteOutcome::Unchanged { .. }
        | ProductListingRawCaptureWriteOutcome::Duplicate { .. }
        | ProductListingRawCaptureWriteOutcome::Stale { .. } => {
            panic!("test input must create a raw revision")
        }
    }
}

async fn seed_listing_source(pool: &sqlx::PgPool, slug: &str) -> ListingSourceId {
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("{slug}-party"))
        .bind(format!("{slug} party"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed party: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(listing_source_id.into_uuid())
        .bind(slug)
        .bind(slug)
        .bind(party_id)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed listing source: {error}"));
    listing_source_id
}
