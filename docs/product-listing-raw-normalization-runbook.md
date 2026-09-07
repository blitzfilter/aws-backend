# ProductListing raw-normalization runbook

## Scope

`product_listing_raw_revisions` is immutable source evidence. Only the `product-listing-normalization` worker subscription consumes its `INSERT` rows. Raw source JSON, webhook bytes, signatures, headers, and provenance values must not be copied into logs, dashboards, tickets, or ad-hoc query output.

Startup repairs a missed CDC wake-up with one bounded global reconciliation page. Later periodic reconciliation turns alternate one worker-local FIFO continuation and one global cursor page when a continuation exists. Only a clean capped drain enters that non-durable FIFO, which holds at most two global pages of stream IDs (currently 200). Blocked or transient stream errors are reported, remain pending, and are revisited on a later global traversal; they never enter the FIFO. Every global page is limited to available FIFO capacity. When a full FIFO continuation is popped, its vacated slot is reserved for the next global page; its immediate capped hint is suppressed only until authoritative traversal reoffers it. That bounded page adopts all of its continuations and advances the worker-local pending-stream cursor, so later global pages remain reachable. A successful terminal global page clears the cursor; retry exhaustion retains it. Direct CDC wake-ups never move or cancel cursor/FIFO state, even when their stream reaches the per-stream cap. Missed timer ticks skip rather than create a burst. Once a reconciliation turn is due, at most one ready CDC job runs before it; after a global page, one ready CDC job gets a turn before another due reconciliation turn. Restarting safely loses cursor and FIFO state and begins a new authoritative traversal; do not delete raw revisions to repair backlog.

Graceful shutdown finishes active work, then exits without draining queued raw wake-ups or FIFO continuations. Those in-memory items retain the documented post-ack loss risk; authoritative reconciliation repairs normalization progress on the next run.

## Provider receipt and intake retention

Shopify and WooCommerce provider receipts are bounded operational idempotency state, not replay history. They are created or reused only for provider observations that map to raw capture. An authorized ignored WooCommerce create/update status event returns before receipt construction and persists no receipt, even when it carries a delivery ID. Each `(raw stream, provider scope, delivery ID)` receipt stores only a canonical source-evidence digest, never source evidence or JSON, for a 90-day logical window. Capture logically expires a matching receipt by deleting it in the capture transaction before lookup/reuse; asynchronous `pg_ttl_index` cleanup only reclaims physical rows. Raw revisions independently retain optional `source_event_id` and provenance after receipt expiry. Expiry does not reset source ordering or alter raw revisions. No delivery identity or source timestamp is required for accepted intake.

For Shopify intake, EventBridge uses its default target delivery policy (up to 24 hours and 185 retries) with no custom retry policy or EventBridge DLQ override. The primary SQS queue uses partial batch failure reporting, 4-day retention, and a 5-receive redrive policy; its DLQ retains messages for 14 days. A Shopify `PROVIDER_SOURCE_ORDER_CONFLICT` is returned as a failure for only that SQS record. It retries, then lands in this DLQ; it is not automatically resolved by delay. Alert on this error code. Operator inspects safe metadata only (source ID, topic, delivery IDs, source timestamp, digest), reconciles the provider truth, then redrives the retained record when order can be established. A reused delivery ID with different source evidence is invalid and acknowledged, not retried. Manual redrive must begin while the message remains retained by the applicable SQS queue; it cannot recover an expired message.

## Signals

Structured metric events are safe to count by their fixed fields:

- `product_listing_raw_capture`: `ingestion_method`, `outcome`, attempt/insert/unchanged counters, byte sizes, and latency.
- `product_listing_raw_normalization`: terminal `outcome` (`APPLIED`, `NO_CHANGE`, `IGNORED`, `REJECTED`) and latency; retryable `failure` or `stream_failure` records carry a stable `error_code`.
- `product_listing_raw_normalization_backlog`: bounded reconciliation-page count and oldest age.
- `product_listing_raw_normalization_reconciliation`: reconciliation runs, processed revisions, failures, bounded page count, page kind, cursor presence, FIFO depth, deferred-continuation count, and suppressed-continuation count.
- `crawler_disposition_transition`: successful transitions to `DORMANT_SOLD` or `DORMANT_REMOVED`.

`NORMALIZATION_CONFIGURATION_FAILED` is a retryable normalizer configuration failure, such as availability-regex compilation, not `REJECTED`; its raw revision and stream head stay pending. Alert on sustained backlog age, repeated worker failures, or a rising `REJECTED` count. Payload size is an early warning only; capture limits remain authoritative.

## Safe operational queries

Run the first four queries against operational PostgreSQL. They select IDs, timestamps, codes, and aggregate counts only.

### Pending normalization backlog

```sql
SELECT
  count(DISTINCT stream.product_listing_raw_stream_id) AS pending_stream_count,
  min(revision.captured_at) AS oldest_pending_at,
  extract(epoch FROM now() - min(revision.captured_at))::bigint AS oldest_pending_age_seconds
FROM product_listing_raw_streams AS stream
LEFT JOIN product_listing_raw_normalization_heads AS head
  ON head.product_listing_raw_stream_id = stream.product_listing_raw_stream_id
JOIN product_listing_raw_revisions AS revision
  ON revision.product_listing_raw_stream_id = stream.product_listing_raw_stream_id
 AND revision.revision > coalesce(head.last_processed_revision, 0)
WHERE stream.latest_revision > coalesce(head.last_processed_revision, 0);
```

### Oldest pending revision generations

```sql
SELECT
  revision.generation,
  revision.product_listing_raw_stream_id,
  revision.product_listing_raw_revision_id,
  revision.revision,
  revision.captured_at
FROM product_listing_raw_revisions AS revision
LEFT JOIN product_listing_raw_normalization_heads AS head
  ON head.product_listing_raw_stream_id = revision.product_listing_raw_stream_id
WHERE revision.revision > coalesce(head.last_processed_revision, 0)
ORDER BY revision.generation
LIMIT 100;
```

### Rejected revisions by stable code

```sql
SELECT error_code, count(*) AS rejected_count, min(created) AS first_rejected_at, max(created) AS last_rejected_at
FROM product_listing_raw_normalizations
WHERE outcome = 'REJECTED'
GROUP BY error_code
ORDER BY rejected_count DESC, error_code;
```

### Raw-table growth

```sql
SELECT
  stream.ingestion_method,
  revision.payload_format,
  count(*) AS revision_count,
  sum(pg_column_size(revision.source_payload)) AS source_payload_bytes,
  sum(pg_column_size(revision.raw_values)) AS raw_values_bytes
FROM product_listing_raw_revisions AS revision
JOIN product_listing_raw_streams AS stream
  ON stream.product_listing_raw_stream_id = revision.product_listing_raw_stream_id
GROUP BY stream.ingestion_method, revision.payload_format
ORDER BY revision_count DESC;
```

### Dormant crawler URLs

Run this against crawler PostgreSQL, never operational PostgreSQL:

```sql
SELECT crawler_disposition, count(*) AS url_count
FROM listing_source_urls
WHERE url_class = 'product'
GROUP BY crawler_disposition
ORDER BY crawler_disposition;
```

## CDC check

The normalization worker must run with:

```text
AURA_HISTORIA_WORKER_SCOPE=product-listing-normalization
```

Its Sequin subscription must contain only `product_listing_raw_revisions` `INSERT` operations. A raw revision must not appear in any ProductListing event, OpenSearch, notification, translation, embedding, assessment, or matching subscription.

## Candidate-query check

Use production-like crawler data before changing scheduler indexes or ranking:

```sql
EXPLAIN (ANALYZE, BUFFERS)
WITH eligible_urls AS (
  SELECT su.listing_source_id, su.url
  FROM listing_source_urls AS su
  JOIN listing_sources AS source ON source.listing_source_id = su.listing_source_id
  WHERE source.crawl_enabled = TRUE
    AND su.url_class = 'product'
    AND su.crawler_disposition = 'ACTIVE'
    AND (su.next_retry_at IS NULL OR su.next_retry_at <= now())
    AND (su.last_scraped IS NULL OR su.last_scraped < now() - interval '1 day')
)
SELECT count(*) FROM eligible_urls;
```

Do not add `next_scrape_at`, dormant revisit scheduling, a raw-JSON GIN index, or a cross-database join as a response to this check.
