use std::time::Duration;

use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_normalization::{
    NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingProvenance, RawProductListingValues,
    SourcePayload,
};
use product_listing_postgres::SqlxProductListingRawCaptureWriterFactory;
use product_listing_service::ports::{
    ProductListingRawCaptureWrite, ProductListingRawCaptureWriteError,
    ProductListingRawCaptureWriteOutcome, ProductListingRawCaptureWriter,
    ProductListingRawCaptureWriterFactory, ProductListingRawIngestionMethod,
    ProductListingRawProviderReceipt, ProviderReceiptScope, SourceEvidenceSha256,
    SourceRecordKeySha256,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[derive(sqlx::FromRow)]
struct ProviderSourceOrderStateRow {
    latest_provider_source_ordering_state: String,
    latest_provider_source_epoch_seconds: Option<i64>,
    latest_provider_source_nanoseconds: Option<i32>,
    latest_provider_source_operation: Option<String>,
    latest_provider_source_observation_sha256: Option<Vec<u8>>,
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_append_only_material_raw_input_changes() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-capture-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    let first = capture(
        &unit_of_work,
        &factory,
        write(
            listing_source_id,
            json!({"unknown": {"number": 42, "array": [true, null]}}),
            json!({"title": "Vase"}),
            json!({"baseUrl": "https://example.test"}),
            "delivery-1",
        ),
    )
    .await;
    let unchanged = capture(
        &unit_of_work,
        &factory,
        write(
            listing_source_id,
            json!({"unknown": {"array": [true, null], "number": 42}}),
            json!({"title": "Vase"}),
            json!({"baseUrl": "https://example.test"}),
            "delivery-2",
        ),
    )
    .await;
    let second = capture(
        &unit_of_work,
        &factory,
        write(
            listing_source_id,
            json!({"unknown": "changed"}),
            json!({"title": "Vase"}),
            json!({"baseUrl": "https://example.test"}),
            "delivery-3",
        ),
    )
    .await;
    let third = capture(
        &unit_of_work,
        &factory,
        write(
            listing_source_id,
            json!({"unknown": {"number": 42, "array": [true, null]}}),
            json!({"title": "Vase"}),
            json!({"baseUrl": "https://example.test"}),
            "delivery-4",
        ),
    )
    .await;

    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        unchanged,
        ProductListingRawCaptureWriteOutcome::Unchanged {
            latest_revision: 1,
            ..
        }
    ));
    assert!(matches!(
        second,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 2, .. }
    ));
    assert!(matches!(
        third,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 3, .. }
    ));

    let revisions: Vec<(i64, Value, Value, Value, String)> = sqlx::query_as(
        "SELECT revision, source_payload, raw_values, normalization_context, source_event_id FROM product_listing_raw_revisions ORDER BY revision",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_else(|error| panic!("read raw revisions: {error}"));
    assert_eq!(3, revisions.len());
    assert_eq!(
        json!({"unknown": {"number": 42, "array": [true, null]}}),
        revisions[0].1
    );
    assert_eq!(json!({"title": "Vase"}), revisions[0].2);
    assert_eq!(json!({"baseUrl": "https://example.test"}), revisions[0].3);
    assert_eq!("delivery-1", revisions[0].4);

    let product_listing_count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listings")
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("count product listings: {error}"));
    let event_count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events")
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("count product listing events: {error}"));
    assert_eq!(0, product_listing_count);
    assert_eq!(0, event_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_serialize_concurrent_equal_captures_into_one_revision() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-capture-concurrent-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    let (first, second) = tokio::join!(
        capture(
            &unit_of_work,
            &factory,
            write(
                listing_source_id,
                json!({"title": "same"}),
                json!({}),
                json!({}),
                "delivery-1"
            ),
        ),
        capture(
            &unit_of_work,
            &factory,
            write(
                listing_source_id,
                json!({"title": "same"}),
                json!({}),
                json!({}),
                "delivery-2"
            ),
        )
    );
    let outcomes = [first, second];
    let changed_count = outcomes
        .iter()
        .filter(|outcome| {
            matches!(
                outcome,
                ProductListingRawCaptureWriteOutcome::Changed { .. }
            )
        })
        .count();
    let unchanged_count = outcomes
        .iter()
        .filter(|outcome| {
            matches!(
                outcome,
                ProductListingRawCaptureWriteOutcome::Unchanged { .. }
            )
        })
        .count();
    assert_eq!(1, changed_count);
    assert_eq!(1, unchanged_count);

    let revision_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM product_listing_raw_revisions")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("count raw revisions: {error}"));
    assert_eq!(1, revision_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_serialize_concurrent_different_captures_into_ordered_revisions() {
    let pool = get_postgres_client().await;
    let listing_source_id =
        seed_listing_source(&pool, "raw-capture-concurrent-different-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    let (first, second) = tokio::join!(
        capture(
            &unit_of_work,
            &factory,
            write(
                listing_source_id,
                json!({"title": "first"}),
                json!({}),
                json!({}),
                "delivery-1"
            ),
        ),
        capture(
            &unit_of_work,
            &factory,
            write(
                listing_source_id,
                json!({"title": "second"}),
                json!({}),
                json!({}),
                "delivery-2"
            ),
        )
    );
    let revisions = [first, second]
        .into_iter()
        .map(|outcome| match outcome {
            ProductListingRawCaptureWriteOutcome::Changed { revision, .. } => revision,
            ProductListingRawCaptureWriteOutcome::Unchanged { .. }
            | ProductListingRawCaptureWriteOutcome::Duplicate { .. }
            | ProductListingRawCaptureWriteOutcome::Stale { .. } => {
                panic!("different inputs must create revisions")
            }
        })
        .collect::<Vec<_>>();
    assert!(revisions.contains(&1));
    assert!(revisions.contains(&2));

    let stored_revisions: Vec<i64> =
        sqlx::query_scalar("SELECT revision FROM product_listing_raw_revisions ORDER BY revision")
            .fetch_all(&pool)
            .await
            .unwrap_or_else(|error| panic!("read raw revisions: {error}"));
    assert_eq!(vec![1, 2], stored_revisions);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_detect_source_record_key_hash_collision() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-capture-collision-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    let first = capture(
        &unit_of_work,
        &factory,
        write(
            listing_source_id,
            json!({}),
            json!({}),
            json!({}),
            "delivery-1",
        ),
    )
    .await;
    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { .. }
    ));

    let mut collision = write(
        listing_source_id,
        json!({"changed": true}),
        json!({}),
        json!({}),
        "delivery-2",
    );
    collision.source_record_key = "https://example.test/other".to_owned();
    let result = capture_result(&unit_of_work, &factory, collision).await;
    assert!(matches!(
        result,
        Err(product_listing_service::ports::ProductListingRawCaptureWriteError::SourceRecordKeyHashCollision)
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_preserve_a_b_a_for_new_provider_delivery_ids() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-capture-provider-a-b-a-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    let first = capture(
        &unit_of_work,
        &factory,
        provider_write(
            listing_source_id,
            json!({"state": "a"}),
            json!({}),
            json!({}),
            "delivery-a-1",
            occurred_at(1),
        ),
    )
    .await;
    let second = capture(
        &unit_of_work,
        &factory,
        provider_write(
            listing_source_id,
            json!({"state": "b"}),
            json!({}),
            json!({}),
            "delivery-b",
            occurred_at(2),
        ),
    )
    .await;
    let third = capture(
        &unit_of_work,
        &factory,
        provider_write(
            listing_source_id,
            json!({"state": "a"}),
            json!({}),
            json!({}),
            "delivery-a-2",
            occurred_at(3),
        ),
    )
    .await;

    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        second,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 2, .. }
    ));
    assert!(matches!(
        third,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 3, .. }
    ));
    assert_eq!(3, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(3, provider_receipt_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_duplicate_for_provider_receipt_after_later_change() {
    let pool = get_postgres_client().await;
    let listing_source_id =
        seed_listing_source(&pool, "raw-capture-provider-duplicate-after-change-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();
    let first_write = provider_write(
        listing_source_id,
        json!({"state": "first"}),
        json!({}),
        json!({}),
        "delivery-first",
        occurred_at(1),
    );

    let first = capture(&unit_of_work, &factory, first_write.clone()).await;
    let later = capture(
        &unit_of_work,
        &factory,
        provider_write(
            listing_source_id,
            json!({"state": "later"}),
            json!({}),
            json!({}),
            "delivery-later",
            occurred_at(2),
        ),
    )
    .await;
    let duplicate = capture(&unit_of_work, &factory, first_write).await;

    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        later,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 2, .. }
    ));
    assert!(matches!(
        duplicate,
        ProductListingRawCaptureWriteOutcome::Duplicate {
            latest_revision: 2,
            ..
        }
    ));
    assert_eq!(2, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(2, provider_receipt_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reuse_expired_provider_receipt_identity() {
    let pool = get_postgres_client().await;
    let listing_source_id =
        seed_listing_source(&pool, "raw-capture-provider-expired-receipt-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();
    let first_write = provider_write(
        listing_source_id,
        json!({"state": "first"}),
        json!({}),
        json!({}),
        "delivery-reused",
        occurred_at(1),
    );
    let reused_write = provider_write(
        listing_source_id,
        json!({"state": "reused"}),
        json!({}),
        json!({}),
        "delivery-reused",
        occurred_at(2),
    );

    let mut transaction = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin transaction: {error}"));
    let first = factory
        .in_transaction(&mut transaction)
        .capture(first_write)
        .await
        .unwrap_or_else(|error| panic!("capture first provider receipt: {error}"));

    // Keep the expired row uncommitted so asynchronous pg_ttl cannot satisfy this test.
    let expired = sqlx::query(
        r#"
        UPDATE product_listing_raw_provider_observation_receipts AS receipts
        SET expires_at = now() - interval '1 second'
        FROM product_listing_raw_streams AS streams
        WHERE receipts.product_listing_raw_stream_id = streams.product_listing_raw_stream_id
          AND streams.listing_source_id = $1
          AND receipts.provider_delivery_id = $2
        "#,
    )
    .bind(listing_source_id.into_uuid())
    .bind("delivery-reused")
    .execute(transaction.connection())
    .await
    .unwrap_or_else(|error| panic!("expire provider receipt: {error}"))
    .rows_affected();
    assert_eq!(1, expired);

    let reused = factory
        .in_transaction(&mut transaction)
        .capture(reused_write)
        .await
        .unwrap_or_else(|error| panic!("reuse expired provider receipt: {error}"));
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("commit transaction: {error}"));

    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        reused,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 2, .. }
    ));
    assert_eq!(2, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(1, provider_receipt_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reuse_expired_provider_receipt_identity_when_capture_waits_on_stream_lock() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(
        &pool,
        "raw-capture-provider-expired-receipt-stream-lock-source",
    )
    .await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();
    let first_write = provider_write(
        listing_source_id,
        json!({"state": "first"}),
        json!({}),
        json!({}),
        "delivery-reused-after-lock",
        occurred_at(1),
    );
    let retry_write = first_write.clone();

    let first = capture(&unit_of_work, &factory, first_write).await;
    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));

    let raw_stream_id: uuid::Uuid = sqlx::query_scalar(
        r#"
        SELECT product_listing_raw_stream_id
        FROM product_listing_raw_streams
        WHERE listing_source_id = $1
        "#,
    )
    .bind(listing_source_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("read raw stream: {error}"));

    let mut stream_lock = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin stream lock transaction: {error}"));
    let stream_lock_backend_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *stream_lock)
        .await
        .unwrap_or_else(|error| panic!("read stream lock backend pid: {error}"));
    sqlx::query(
        r#"
        SELECT product_listing_raw_stream_id
        FROM product_listing_raw_streams
        WHERE product_listing_raw_stream_id = $1
        FOR UPDATE
        "#,
    )
    .bind(raw_stream_id)
    .execute(&mut *stream_lock)
    .await
    .unwrap_or_else(|error| panic!("lock raw stream: {error}"));

    let (capture_transaction_started_sender, capture_transaction_started_receiver) =
        tokio::sync::oneshot::channel();
    let (start_capture_sender, start_capture_receiver) = tokio::sync::oneshot::channel();
    let capture_unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let capture_factory = SqlxProductListingRawCaptureWriterFactory::new();
    let capture_task = tokio::spawn(async move {
        let mut capture_transaction = capture_unit_of_work
            .begin()
            .await
            .map_err(|error| format!("begin retry capture transaction: {error}"))?;
        let capture_backend_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(capture_transaction.connection())
            .await
            .map_err(|error| format!("read retry capture backend pid: {error}"))?;
        capture_transaction_started_sender
            .send(capture_backend_pid)
            .map_err(|_| "retry capture transaction coordinator dropped".to_owned())?;
        start_capture_receiver
            .await
            .map_err(|_| "retry capture start signal dropped".to_owned())?;

        let outcome = capture_factory
            .in_transaction(&mut capture_transaction)
            .capture(retry_write)
            .await
            .map_err(|error| format!("capture retry: {error}"))?;
        capture_transaction
            .commit()
            .await
            .map_err(|error| format!("commit retry capture transaction: {error}"))?;

        Ok::<ProductListingRawCaptureWriteOutcome, String>(outcome)
    });

    let capture_backend_pid = capture_transaction_started_receiver
        .await
        .unwrap_or_else(|error| panic!("wait for retry capture transaction: {error}"));
    let expired = sqlx::query(
        r#"
        UPDATE product_listing_raw_provider_observation_receipts
        SET expires_at = clock_timestamp() + interval '500 milliseconds'
        WHERE product_listing_raw_stream_id = $1
          AND provider_delivery_id = $2
        "#,
    )
    .bind(raw_stream_id)
    .bind("delivery-reused-after-lock")
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("set provider receipt expiry: {error}"))
    .rows_affected();
    assert_eq!(1, expired);
    start_capture_sender
        .send(())
        .unwrap_or_else(|_| panic!("start retry capture"));

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let stream_lock_blocks_capture: bool =
                sqlx::query_scalar("SELECT $1 = ANY(pg_blocking_pids($2))")
                    .bind(stream_lock_backend_pid)
                    .bind(capture_backend_pid)
                    .fetch_one(&pool)
                    .await
                    .unwrap_or_else(|error| panic!("inspect retry capture lock wait: {error}"));
            if stream_lock_blocks_capture {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("retry capture did not wait on the stream lock"));

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let receipt_expired: bool = sqlx::query_scalar(
                r#"
                SELECT expires_at <= clock_timestamp()
                FROM product_listing_raw_provider_observation_receipts
                WHERE product_listing_raw_stream_id = $1
                  AND provider_delivery_id = $2
                "#,
            )
            .bind(raw_stream_id)
            .bind("delivery-reused-after-lock")
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("inspect provider receipt expiry: {error}"));
            if receipt_expired {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("provider receipt did not expire"));

    stream_lock
        .commit()
        .await
        .unwrap_or_else(|error| panic!("commit stream lock transaction: {error}"));
    let reused = capture_task
        .await
        .unwrap_or_else(|error| panic!("join retry capture: {error}"))
        .unwrap_or_else(|error| panic!("retry capture: {error}"));

    assert!(matches!(
        reused,
        ProductListingRawCaptureWriteOutcome::Unchanged {
            latest_revision: 1,
            ..
        }
    ));
    assert_eq!(1, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(1, provider_receipt_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_duplicate_for_unchanged_provider_receipt_after_later_change() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(
        &pool,
        "raw-capture-provider-unchanged-retry-after-change-source",
    )
    .await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();
    let unchanged_write = provider_write(
        listing_source_id,
        json!({"state": "same"}),
        json!({}),
        json!({}),
        "delivery-unchanged",
        occurred_at(2),
    );
    let unchanged_evidence = source_order_observation(&unchanged_write);

    let first = capture(
        &unit_of_work,
        &factory,
        provider_write(
            listing_source_id,
            json!({"state": "same"}),
            json!({}),
            json!({}),
            "delivery-first",
            occurred_at(1),
        ),
    )
    .await;
    let unchanged = capture(&unit_of_work, &factory, unchanged_write.clone()).await;
    let source_order_head = provider_source_order_head(&pool, listing_source_id).await;
    let later = capture(
        &unit_of_work,
        &factory,
        provider_write(
            listing_source_id,
            json!({"state": "later"}),
            json!({}),
            json!({}),
            "delivery-later",
            occurred_at(3),
        ),
    )
    .await;
    let duplicate = capture(&unit_of_work, &factory, unchanged_write).await;

    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        unchanged,
        ProductListingRawCaptureWriteOutcome::Unchanged {
            latest_revision: 1,
            ..
        }
    ));
    assert_eq!(
        (Some(2), Some(0), Some(unchanged_evidence.to_vec())),
        source_order_head
    );
    assert!(matches!(
        later,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 2, .. }
    ));
    assert!(matches!(
        duplicate,
        ProductListingRawCaptureWriteOutcome::Duplicate {
            latest_revision: 2,
            ..
        }
    ));
    assert_eq!(2, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(3, provider_receipt_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_provider_receipt_digest_mismatch_before_duplicate_check() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(
        &pool,
        "raw-capture-provider-digest-mismatch-before-duplicate-source",
    )
    .await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();
    let first_write = provider_write(
        listing_source_id,
        json!({"state": "first"}),
        json!({}),
        json!({}),
        "delivery-shared",
        occurred_at(1),
    );
    let first_evidence = canonical_source_evidence(&first_write);

    let first = capture(&unit_of_work, &factory, first_write).await;
    let mut mismatched_receipt = provider_write(
        listing_source_id,
        json!({"state": "changed"}),
        json!({}),
        json!({}),
        "delivery-shared",
        occurred_at(2),
    );
    mismatched_receipt.provider_receipt = Some(provider_receipt("delivery-shared", first_evidence));
    let mismatch = capture_result(&unit_of_work, &factory, mismatched_receipt).await;

    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        mismatch,
        Err(ProductListingRawCaptureWriteError::ProviderReceiptDigestConflict)
    ));
    assert_eq!(1, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(1, provider_receipt_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_conflicting_provider_delivery_id_with_canonical_evidence() {
    let pool = get_postgres_client().await;
    let listing_source_id =
        seed_listing_source(&pool, "raw-capture-provider-delivery-conflict-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    let first = capture(
        &unit_of_work,
        &factory,
        provider_write(
            listing_source_id,
            json!({"state": "first"}),
            json!({}),
            json!({}),
            "delivery-shared",
            occurred_at(1),
        ),
    )
    .await;
    let conflict = capture_result(
        &unit_of_work,
        &factory,
        provider_write(
            listing_source_id,
            json!({"state": "changed"}),
            json!({}),
            json!({}),
            "delivery-shared",
            occurred_at(2),
        ),
    )
    .await;

    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        conflict,
        Err(ProductListingRawCaptureWriteError::ProviderReceiptDigestConflict)
    ));
    assert_eq!(1, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(1, provider_receipt_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_conflicting_provider_source_order_evidence() {
    let pool = get_postgres_client().await;
    let listing_source_id =
        seed_listing_source(&pool, "raw-capture-provider-source-order-conflict-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    let first = capture(
        &unit_of_work,
        &factory,
        provider_write(
            listing_source_id,
            json!({"state": "first"}),
            json!({}),
            json!({}),
            "delivery-first",
            occurred_at(1),
        ),
    )
    .await;
    let source_order_conflict = capture_result(
        &unit_of_work,
        &factory,
        provider_write(
            listing_source_id,
            json!({"state": "different"}),
            json!({}),
            json!({}),
            "delivery-same-time",
            occurred_at(1),
        ),
    )
    .await;

    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        source_order_conflict,
        Err(ProductListingRawCaptureWriteError::ProviderSourceOrderConflict)
    ));
    assert_eq!(1, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(1, provider_receipt_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_not_collapse_equal_source_payload_with_different_operation() {
    let pool = get_postgres_client().await;
    let listing_source_id =
        seed_listing_source(&pool, "raw-capture-provider-operation-source-order-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();
    let upsert = provider_write(
        listing_source_id,
        json!({"id": "same-source-object"}),
        json!({"title": "Cabinet"}),
        json!({"baseUrl": "https://example.test"}),
        "delivery-upsert",
        occurred_at(1),
    );
    let delete = with_operation(
        provider_write(
            listing_source_id,
            json!({"id": "same-source-object"}),
            json!({}),
            json!({"baseUrl": "https://example.test"}),
            "delivery-delete",
            occurred_at(1),
        ),
        RawProductListingOperation::Delete,
    );

    assert!(matches!(
        capture(&unit_of_work, &factory, upsert).await,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        capture_result(&unit_of_work, &factory, delete).await,
        Err(ProductListingRawCaptureWriteError::ProviderSourceOrderAmbiguous)
    ));
    assert_eq!(1, raw_revision_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_block_timestamped_upsert_after_timestamp_free_delete_without_recording_receipt() {
    let pool = get_postgres_client().await;
    let listing_source_id =
        seed_listing_source(&pool, "raw-capture-unknown-delete-barrier-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    assert!(matches!(
        capture(
            &unit_of_work,
            &factory,
            provider_write(
                listing_source_id,
                json!({"state": "active"}),
                json!({}),
                json!({}),
                "delivery-active",
                occurred_at(10)
            ),
        )
        .await,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    let mut delete = with_operation(
        provider_write(
            listing_source_id,
            json!({"id": "123"}),
            json!({}),
            json!({}),
            "delivery-delete",
            occurred_at(11),
        ),
        RawProductListingOperation::Delete,
    );
    delete.source_occurred_at = None;
    assert!(matches!(
        capture(&unit_of_work, &factory, delete).await,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 2, .. }
    ));

    let mut timestamp_free_upsert = provider_write(
        listing_source_id,
        json!({"state": "timestamp-free"}),
        json!({}),
        json!({}),
        "delivery-timestamp-free",
        occurred_at(12),
    );
    timestamp_free_upsert.source_occurred_at = None;
    let timestamp_free_blocked =
        capture_result(&unit_of_work, &factory, timestamp_free_upsert).await;
    let timestamped_blocked = capture_result(
        &unit_of_work,
        &factory,
        provider_write(
            listing_source_id,
            json!({"state": "delayed"}),
            json!({}),
            json!({}),
            "delivery-delayed",
            occurred_at(13),
        ),
    )
    .await;

    assert!(matches!(
        timestamp_free_blocked,
        Err(ProductListingRawCaptureWriteError::ProviderSourceOrderAmbiguous)
    ));
    assert!(matches!(
        timestamped_blocked,
        Err(ProductListingRawCaptureWriteError::ProviderSourceOrderAmbiguous)
    ));
    let source_order_state: ProviderSourceOrderStateRow = sqlx::query_as(
        "SELECT latest_provider_source_ordering_state, \
                    latest_provider_source_epoch_seconds, \
                    latest_provider_source_nanoseconds, \
                    latest_provider_source_operation, \
                    latest_provider_source_observation_sha256 \
             FROM product_listing_raw_streams WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("read unknown-delete source order: {error}"));
    assert_eq!(
        "UNKNOWN_DELETE",
        source_order_state.latest_provider_source_ordering_state
    );
    assert_eq!(
        None,
        source_order_state.latest_provider_source_epoch_seconds
    );
    assert_eq!(None, source_order_state.latest_provider_source_nanoseconds);
    assert_eq!(None, source_order_state.latest_provider_source_operation);
    assert!(matches!(
        source_order_state.latest_provider_source_observation_sha256,
        Some(digest) if digest.len() == 32
    ));
    assert_eq!(2, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(2, provider_receipt_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_keep_nanosecond_order_after_postgres_round_trip() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-capture-nanosecond-order-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();
    let deleted_at = occurred_at(10)
        .replace_nanosecond(123_456_900)
        .unwrap_or_else(|error| panic!("timestamp: {error}"));
    let older_at = occurred_at(10)
        .replace_nanosecond(123_456_100)
        .unwrap_or_else(|error| panic!("timestamp: {error}"));

    let delete = with_operation(
        provider_write(
            listing_source_id,
            json!({"id": "123"}),
            json!({}),
            json!({}),
            "delivery-delete",
            deleted_at,
        ),
        RawProductListingOperation::Delete,
    );
    assert!(matches!(
        capture(&unit_of_work, &factory, delete).await,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    let delayed = capture(
        &unit_of_work,
        &factory,
        provider_write(
            listing_source_id,
            json!({"state": "older"}),
            json!({}),
            json!({}),
            "delivery-older",
            older_at,
        ),
    )
    .await;

    assert!(matches!(
        delayed,
        ProductListingRawCaptureWriteOutcome::Stale {
            latest_revision: 1,
            ..
        }
    ));
    assert_eq!(1, raw_revision_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_unchanged_after_recording_receipt_when_equal_provider_observation_changes_config()
 {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(
        &pool,
        "raw-capture-provider-equal-observation-config-change-source",
    )
    .await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    let first_write = provider_write(
        listing_source_id,
        json!({"id": "same-observation", "title": "Provider title"}),
        json!({"configuredTitle": "first"}),
        json!({"baseUrl": "https://first.example.test"}),
        "delivery-first",
        occurred_at(1),
    );
    let changed_config_write = provider_write(
        listing_source_id,
        json!({"title": "Provider title", "id": "same-observation"}),
        json!({"configuredTitle": "changed"}),
        json!({"baseUrl": "https://changed.example.test"}),
        "delivery-config-changed",
        occurred_at(1),
    );
    assert_eq!(
        canonical_source_evidence(&first_write),
        canonical_source_evidence(&changed_config_write)
    );
    assert_ne!(
        first_write.input_sha256.as_bytes(),
        changed_config_write.input_sha256.as_bytes()
    );

    let first = capture(&unit_of_work, &factory, first_write).await;
    let unchanged = capture(&unit_of_work, &factory, changed_config_write).await;

    assert!(matches!(
        first,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        unchanged,
        ProductListingRawCaptureWriteOutcome::Unchanged {
            latest_revision: 1,
            ..
        }
    ));
    assert_eq!(1, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(2, provider_receipt_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_order_shopify_and_woocommerce_source_timestamps_without_provider_receipts() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    for (ingestion_method, payload_format, slug) in [
        (
            ProductListingRawIngestionMethod::Shopify,
            RawProductListingPayloadFormat::ShopifyProduct,
            "raw-capture-shopify-timestamp-without-receipt-source",
        ),
        (
            ProductListingRawIngestionMethod::Woocommerce,
            RawProductListingPayloadFormat::WoocommerceProduct,
            "raw-capture-woocommerce-timestamp-without-receipt-source",
        ),
    ] {
        let listing_source_id = seed_listing_source(&pool, slug).await;
        let newer_write = provider_write_without_receipt(
            listing_source_id,
            ingestion_method,
            payload_format,
            json!({"state": "newer"}),
            json!({}),
            json!({}),
            occurred_at(2),
        );
        let newer_evidence = source_order_observation(&newer_write);

        let newer = capture(&unit_of_work, &factory, newer_write).await;
        let stale = capture(
            &unit_of_work,
            &factory,
            provider_write_without_receipt(
                listing_source_id,
                ingestion_method,
                payload_format,
                json!({"state": "older"}),
                json!({}),
                json!({}),
                occurred_at(1),
            ),
        )
        .await;

        assert!(matches!(
            newer,
            ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
        ));
        assert!(matches!(
            stale,
            ProductListingRawCaptureWriteOutcome::Stale {
                latest_revision: 1,
                ..
            }
        ));
        assert_eq!(1, raw_revision_count(&pool, listing_source_id).await);
        assert_eq!(0, provider_receipt_count(&pool, listing_source_id).await);
        assert_eq!(
            (Some(2), Some(0), Some(newer_evidence.to_vec())),
            provider_source_order_head(&pool, listing_source_id).await
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_persist_stale_provider_receipt_and_return_duplicate_on_retry() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(&pool, "raw-capture-provider-stale-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();

    let newer = capture(
        &unit_of_work,
        &factory,
        provider_write(
            listing_source_id,
            json!({"state": "newer"}),
            json!({}),
            json!({}),
            "delivery-newer",
            occurred_at(2),
        ),
    )
    .await;
    let stale_write = provider_write(
        listing_source_id,
        json!({"state": "older"}),
        json!({}),
        json!({}),
        "delivery-older",
        occurred_at(1),
    );
    let stale = capture(&unit_of_work, &factory, stale_write.clone()).await;

    assert!(matches!(
        newer,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert!(matches!(
        stale,
        ProductListingRawCaptureWriteOutcome::Stale {
            latest_revision: 1,
            ..
        }
    ));
    assert_eq!(1, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(2, provider_receipt_count(&pool, listing_source_id).await);

    let retry = capture(&unit_of_work, &factory, stale_write).await;

    assert!(matches!(
        retry,
        ProductListingRawCaptureWriteOutcome::Duplicate {
            latest_revision: 1,
            ..
        }
    ));
    assert_eq!(1, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(2, provider_receipt_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_serialize_concurrent_provider_receipt_retries() {
    let pool = get_postgres_client().await;
    let listing_source_id =
        seed_listing_source(&pool, "raw-capture-provider-concurrent-retry-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();
    let write = provider_write(
        listing_source_id,
        json!({"state": "same"}),
        json!({}),
        json!({}),
        "delivery-same",
        occurred_at(1),
    );

    let (first, second) = tokio::join!(
        capture(&unit_of_work, &factory, write.clone()),
        capture(&unit_of_work, &factory, write),
    );
    let outcomes = [first, second];
    let changed_count = outcomes
        .iter()
        .filter(|outcome| {
            matches!(
                outcome,
                ProductListingRawCaptureWriteOutcome::Changed { .. }
            )
        })
        .count();
    let duplicate_count = outcomes
        .iter()
        .filter(|outcome| {
            matches!(
                outcome,
                ProductListingRawCaptureWriteOutcome::Duplicate { .. }
            )
        })
        .count();

    assert_eq!(1, changed_count);
    assert_eq!(1, duplicate_count);
    assert_eq!(1, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(1, provider_receipt_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_rollback_provider_receipt_with_capture() {
    let pool = get_postgres_client().await;
    let listing_source_id =
        seed_listing_source(&pool, "raw-capture-provider-rollback-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();
    let write = provider_write(
        listing_source_id,
        json!({"state": "first"}),
        json!({}),
        json!({}),
        "delivery-first",
        occurred_at(1),
    );

    let mut transaction = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin transaction: {error}"));
    {
        let captured = factory
            .in_transaction(&mut transaction)
            .capture(write.clone())
            .await
            .unwrap_or_else(|error| panic!("capture raw input: {error}"));
        assert!(matches!(
            captured,
            ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
        ));
    }
    drop(transaction);

    let replay = capture(&unit_of_work, &factory, write).await;

    assert!(matches!(
        replay,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert_eq!(1, raw_revision_count(&pool, listing_source_id).await);
    assert_eq!(1, provider_receipt_count(&pool, listing_source_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_null_required_provider_source_ordering_fields() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(
        &pool,
        "raw-capture-provider-source-ordering-null-constraint-source",
    )
    .await;
    let digest = vec![9; 32];

    let known_with_null_nanoseconds = insert_raw_stream_with_provider_source_ordering(
        &pool,
        listing_source_id,
        "KNOWN",
        Some(1),
        None,
        Some("UPSERT"),
        Some(digest.clone()),
    )
    .await;
    assert_provider_source_ordering_check_violation(known_with_null_nanoseconds);

    let known_with_null_operation = insert_raw_stream_with_provider_source_ordering(
        &pool,
        listing_source_id,
        "KNOWN",
        Some(1),
        Some(123),
        None,
        Some(digest.clone()),
    )
    .await;
    assert_provider_source_ordering_check_violation(known_with_null_operation);

    let known_with_null_digest = insert_raw_stream_with_provider_source_ordering(
        &pool,
        listing_source_id,
        "KNOWN",
        Some(1),
        Some(123),
        Some("UPSERT"),
        None,
    )
    .await;
    assert_provider_source_ordering_check_violation(known_with_null_digest);

    let unknown_delete_with_null_digest = insert_raw_stream_with_provider_source_ordering(
        &pool,
        listing_source_id,
        "UNKNOWN_DELETE",
        None,
        None,
        None,
        None,
    )
    .await;
    assert_provider_source_ordering_check_violation(unknown_delete_with_null_digest);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_accept_valid_provider_source_ordering_shapes() {
    let pool = get_postgres_client().await;
    let listing_source_id = seed_listing_source(
        &pool,
        "raw-capture-provider-source-ordering-valid-constraint-source",
    )
    .await;

    let no_ordering = insert_raw_stream_with_provider_source_ordering(
        &pool,
        listing_source_id,
        "NO_ORDERING",
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(
        no_ordering.is_ok(),
        "NO_ORDERING shape must be valid: {no_ordering:?}"
    );

    let known = insert_raw_stream_with_provider_source_ordering(
        &pool,
        listing_source_id,
        "KNOWN",
        Some(1),
        Some(999_999_999),
        Some("UPSERT"),
        Some(vec![8; 32]),
    )
    .await;
    assert!(known.is_ok(), "KNOWN shape must be valid: {known:?}");

    let unknown_delete = insert_raw_stream_with_provider_source_ordering(
        &pool,
        listing_source_id,
        "UNKNOWN_DELETE",
        None,
        None,
        None,
        Some(vec![7; 32]),
    )
    .await;
    assert!(
        unknown_delete.is_ok(),
        "UNKNOWN_DELETE shape must be valid: {unknown_delete:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_not_persist_provider_receipt_for_web_crawl() {
    let pool = get_postgres_client().await;
    let listing_source_id =
        seed_listing_source(&pool, "raw-capture-crawler-no-receipt-source").await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let factory = SqlxProductListingRawCaptureWriterFactory::new();
    let mut crawl_write = crawler_write(
        listing_source_id,
        json!({"state": "crawled"}),
        json!({}),
        json!({}),
        "crawler-delivery",
    );
    crawl_write.provider_receipt = Some(provider_receipt(
        "crawler-delivery",
        canonical_source_evidence(&crawl_write),
    ));
    crawl_write.source_occurred_at = Some(occurred_at(1));

    let captured = capture(&unit_of_work, &factory, crawl_write).await;

    assert!(matches!(
        captured,
        ProductListingRawCaptureWriteOutcome::Changed { revision: 1, .. }
    ));
    assert_eq!(0, provider_receipt_count(&pool, listing_source_id).await);
    assert_eq!(
        (None, None, None),
        provider_source_order_head(&pool, listing_source_id).await
    );
}

async fn insert_raw_stream_with_provider_source_ordering(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
    ordering_state: &str,
    epoch_seconds: Option<i64>,
    nanoseconds: Option<i32>,
    operation: Option<&str>,
    observation_sha256: Option<Vec<u8>>,
) -> Result<(), sqlx::Error> {
    let stream_id = uuid::Uuid::now_v7();
    let source_record_key = stream_id.to_string();
    let source_record_key_sha256 = Sha256::digest(source_record_key.as_bytes()).to_vec();

    sqlx::query(
        r#"
        INSERT INTO product_listing_raw_streams (
            product_listing_raw_stream_id,
            listing_source_id,
            ingestion_method,
            source_record_key,
            source_record_key_sha256,
            latest_revision,
            latest_provider_source_ordering_state,
            latest_provider_source_epoch_seconds,
            latest_provider_source_nanoseconds,
            latest_provider_source_operation,
            latest_provider_source_observation_sha256
        ) VALUES ($1, $2, 'SHOPIFY', $3, $4, 0, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(stream_id)
    .bind(listing_source_id.into_uuid())
    .bind(source_record_key)
    .bind(source_record_key_sha256)
    .bind(ordering_state)
    .bind(epoch_seconds)
    .bind(nanoseconds)
    .bind(operation)
    .bind(observation_sha256)
    .execute(pool)
    .await
    .map(|_| ())
}

fn assert_provider_source_ordering_check_violation(result: Result<(), sqlx::Error>) {
    assert!(matches!(
        result,
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("23514")
    ));
}

fn write(
    listing_source_id: ListingSourceId,
    source_payload: Value,
    raw_values: Value,
    context: Value,
    delivery_id: &str,
) -> ProductListingRawCaptureWrite {
    write_for(
        listing_source_id,
        ProductListingRawIngestionMethod::Shopify,
        RawProductListingPayloadFormat::ShopifyProduct,
        source_payload,
        raw_values,
        context,
        delivery_id,
    )
}

fn crawler_write(
    listing_source_id: ListingSourceId,
    source_payload: Value,
    raw_values: Value,
    context: Value,
    delivery_id: &str,
) -> ProductListingRawCaptureWrite {
    write_for(
        listing_source_id,
        ProductListingRawIngestionMethod::WebCrawl,
        RawProductListingPayloadFormat::CrawlerExtractedProduct,
        source_payload,
        raw_values,
        context,
        delivery_id,
    )
}

fn write_for(
    listing_source_id: ListingSourceId,
    ingestion_method: ProductListingRawIngestionMethod,
    payload_format: RawProductListingPayloadFormat,
    source_payload: Value,
    raw_values: Value,
    context: Value,
    delivery_id: &str,
) -> ProductListingRawCaptureWrite {
    let input = ProductListingNormalizationInput::new(
        RawProductListingOperation::Upsert,
        payload_format,
        1,
        1,
        SourcePayload::new(source_payload)
            .unwrap_or_else(|error| panic!("source payload: {error}")),
        RawProductListingValues::new(raw_values)
            .unwrap_or_else(|error| panic!("raw values: {error}")),
        NormalizationContext::new(context).unwrap_or_else(|error| panic!("context: {error}")),
    )
    .unwrap_or_else(|error| panic!("normalization input: {error}"));
    let input_sha256 = input
        .hash()
        .unwrap_or_else(|error| panic!("input hash: {error}"));
    ProductListingRawCaptureWrite {
        listing_source_id,
        ingestion_method,
        source_record_key: "123".to_owned(),
        source_record_key_sha256: SourceRecordKeySha256::new([7; 32]),
        input,
        input_sha256,
        provenance: RawProductListingProvenance::new(json!({"deliveryId": delivery_id}))
            .unwrap_or_else(|error| panic!("provenance: {error}")),
        source_event_id: Some(delivery_id.to_owned()),
        source_occurred_at: None,
        provider_receipt: None,
    }
}

fn with_operation(
    mut write: ProductListingRawCaptureWrite,
    operation: RawProductListingOperation,
) -> ProductListingRawCaptureWrite {
    write.input = ProductListingNormalizationInput::new(
        operation,
        write.input.payload_format(),
        write.input.payload_schema_version(),
        write.input.raw_values_schema_version(),
        SourcePayload::new(write.input.source_payload().value().clone())
            .unwrap_or_else(|error| panic!("source payload: {error}")),
        RawProductListingValues::new(write.input.raw_values().value().clone())
            .unwrap_or_else(|error| panic!("raw values: {error}")),
        NormalizationContext::new(write.input.normalization_context().value().clone())
            .unwrap_or_else(|error| panic!("normalization context: {error}")),
    )
    .unwrap_or_else(|error| panic!("normalization input: {error}"));
    write.input_sha256 = write
        .input
        .hash()
        .unwrap_or_else(|error| panic!("input hash: {error}"));
    write
}

fn provider_write(
    listing_source_id: ListingSourceId,
    source_payload: Value,
    raw_values: Value,
    context: Value,
    delivery_id: &str,
    source_occurred_at: OffsetDateTime,
) -> ProductListingRawCaptureWrite {
    let mut write = write(
        listing_source_id,
        source_payload,
        raw_values,
        context,
        delivery_id,
    );
    write.provider_receipt = Some(provider_receipt(
        delivery_id,
        canonical_source_evidence(&write),
    ));
    write.source_occurred_at = Some(source_occurred_at);
    write
}

fn provider_write_without_receipt(
    listing_source_id: ListingSourceId,
    ingestion_method: ProductListingRawIngestionMethod,
    payload_format: RawProductListingPayloadFormat,
    source_payload: Value,
    raw_values: Value,
    context: Value,
    source_occurred_at: OffsetDateTime,
) -> ProductListingRawCaptureWrite {
    let mut write = write_for(
        listing_source_id,
        ingestion_method,
        payload_format,
        source_payload,
        raw_values,
        context,
        "missing-delivery-id",
    );
    write.source_event_id = None;
    write.source_occurred_at = Some(source_occurred_at);
    write
}

fn provider_receipt(
    delivery_id: &str,
    source_evidence_sha256: [u8; 32],
) -> ProductListingRawProviderReceipt {
    ProductListingRawProviderReceipt::new(
        ProviderReceiptScope::new("test-provider:products".to_owned())
            .unwrap_or_else(|error| panic!("provider receipt scope: {error}")),
        delivery_id.to_owned(),
        SourceEvidenceSha256::new(source_evidence_sha256),
    )
    .unwrap_or_else(|error| panic!("provider receipt: {error}"))
}

fn source_order_observation(write: &ProductListingRawCaptureWrite) -> [u8; 32] {
    let source_evidence_sha256 = canonical_source_evidence(write);
    let mut digest = Sha256::new();
    digest.update(b"PRODUCT_LISTING_PROVIDER_SOURCE_ORDER\0");
    digest.update(write.input.operation().as_str().as_bytes());
    digest.update([0]);
    digest.update(source_evidence_sha256);
    digest.finalize().into()
}

fn canonical_source_evidence(write: &ProductListingRawCaptureWrite) -> [u8; 32] {
    *write
        .input
        .source_payload()
        .canonical_sha256()
        .unwrap_or_else(|error| panic!("canonical source evidence: {error}"))
        .as_bytes()
}

fn occurred_at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(seconds)
        .unwrap_or_else(|error| panic!("source occurred-at timestamp: {error}"))
}

async fn raw_revision_count(pool: &sqlx::PgPool, listing_source_id: ListingSourceId) -> i64 {
    sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM product_listing_raw_revisions AS revisions
        JOIN product_listing_raw_streams AS streams
          ON streams.product_listing_raw_stream_id = revisions.product_listing_raw_stream_id
        WHERE streams.listing_source_id = $1
        "#,
    )
    .bind(listing_source_id.into_uuid())
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("count raw revisions: {error}"))
}

async fn provider_receipt_count(pool: &sqlx::PgPool, listing_source_id: ListingSourceId) -> i64 {
    sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM product_listing_raw_provider_observation_receipts AS receipts
        JOIN product_listing_raw_streams AS streams
          ON streams.product_listing_raw_stream_id = receipts.product_listing_raw_stream_id
        WHERE streams.listing_source_id = $1
        "#,
    )
    .bind(listing_source_id.into_uuid())
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("count provider receipts: {error}"))
}

async fn provider_source_order_head(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
) -> (Option<i64>, Option<i32>, Option<Vec<u8>>) {
    sqlx::query_as(
        r#"
        SELECT
            latest_provider_source_epoch_seconds,
            latest_provider_source_nanoseconds,
            latest_provider_source_observation_sha256
        FROM product_listing_raw_streams
        WHERE listing_source_id = $1
        "#,
    )
    .bind(listing_source_id.into_uuid())
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("read provider source order head: {error}"))
}

async fn capture(
    unit_of_work: &SqlxUnitOfWork,
    factory: &SqlxProductListingRawCaptureWriterFactory,
    write: ProductListingRawCaptureWrite,
) -> ProductListingRawCaptureWriteOutcome {
    capture_result(unit_of_work, factory, write)
        .await
        .unwrap_or_else(|error| panic!("capture raw input: {error}"))
}

async fn capture_result(
    unit_of_work: &SqlxUnitOfWork,
    factory: &SqlxProductListingRawCaptureWriterFactory,
    write: ProductListingRawCaptureWrite,
) -> Result<
    ProductListingRawCaptureWriteOutcome,
    product_listing_service::ports::ProductListingRawCaptureWriteError,
> {
    let mut tx = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin transaction: {error}"));
    let outcome = factory.in_transaction(&mut tx).capture(write).await;
    match outcome {
        Ok(outcome) => {
            tx.commit()
                .await
                .unwrap_or_else(|error| panic!("commit transaction: {error}"));
            Ok(outcome)
        }
        Err(error) => Err(error),
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
