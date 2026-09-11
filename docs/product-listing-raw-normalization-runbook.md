# ProductListing raw-normalization runbook

## Scope

`product_listing_raw_revisions` is immutable source evidence. Only the `product-listing-normalization` worker subscription consumes its `INSERT` rows. Raw source JSON, webhook bytes, signatures, headers, and provenance values must not be copied into logs, dashboards, tickets, or ad-hoc query output.

CDC wake-ups use scoped **Standard SQS**: full prevalidation and confirmed publication precede Sequin `202`; only a completed stream drain permits receipt deletion. A capped direct drain remains retryable (`normalization_continuation`); stream errors also retain the receipt. Source7d/DLQ14d retention is bounded by original enqueue age on transfer, not a fresh 14 days in the DLQ. See the [durable-worker runbook](durable-worker-runbook.md) for configuration, IAM, alarms, cutover, and safe redrive.

Reconciliation is an additional authoritative repair path, not SQS job custody:

- Startup runs one bounded global page; later 30s timer turns alternate a continuation and a global cursor page when available. Each turn has a 240s budget; failed turns retain the cursor and retry on a later interval. Missed ticks skip, not burst.
- Only clean capped drains enter the reconstructible worker-local FIFO (at most 200 stream IDs). Reported blocked/transient streams remain pending for later global traversal. Global pages use available FIFO capacity; popping a full FIFO reserves its slot for the next global page and suppresses the immediate capped hint until traversal reoffers it. Successful pages adopt bounded continuations and advance the cursor; a terminal page clears it.
- Direct CDC wake-ups never move/cancel cursor or FIFO state. Due reconciliation gets a turn after at most one ready CDC job; a ready CDC job gets a turn after a global page. Restart safely resets cursor/FIFO and begins another authoritative traversal. Never delete raw revisions to repair backlog.

Graceful shutdown stops ingress/polling and allows active work up to the runtime's 270s drain deadline; it does not drain queued wake-ups or FIFO continuations. Unsettled SQS jobs survive within retention and reappear after visibility expiry. Only local scheduling hints are lost; PostgreSQL reconciliation reconstructs them. This does not recover previously lost work for other scopes.

## Development raw-contract reset

The current ProductListing raw-values contract uses only discriminator `1` and requires `priceFormat` for UPSERT values. It has no V1/V2 compatibility reader. Before an **authorized local development** cutover, stop crawler, Shopify, WooCommerce, API intake, and normalization worker processes; recreate only the incompatible disposable business raw fixtures/database rows, local worker queues, and crawler capture fixtures using their established test/local tooling; then start all affected processes from the same checkout and recapture current inputs. The `test-api::Postgres::new("migrations")` harness provides isolated schema initialization and data cleanup for tests; crawler local state uses its own migrations/scripts. No coordinated shared-environment reset command is defined here, so do not infer or run destructive commands. Old raw JSON is discarded and recaptured, never rewritten or backfilled. ProductListing journal schema `1` and worker SQS envelope schema `2` are independent and unchanged by this raw-values replacement.

## Provider receipt and intake retention

Shopify and WooCommerce provider receipts are bounded operational idempotency state, not replay history. They are created or reused only for provider observations that map to raw capture. An authorized ignored WooCommerce create/update status event returns before receipt construction and persists no receipt, even when it carries a delivery ID. Each `(raw stream, provider scope, delivery ID)` receipt stores only a canonical source-evidence digest, never source evidence or JSON, for a 90-day logical window. Capture logically expires a matching receipt by deleting it in the capture transaction before lookup/reuse; asynchronous `pg_ttl_index` cleanup only reclaims physical rows. Raw revisions independently retain optional `source_event_id` and provenance after receipt expiry. Expiry does not reset source ordering or alter raw revisions. No delivery identity or source timestamp is required for accepted intake.

Shopify orders captured product operations with EventBridge `X-Shopify-Triggered-At`; WooCommerce uses supplied `date_modified_gmt`. Guards retain exact epoch-second/nanosecond pairs and operation-aware evidence in one final state: `NO_ORDERING` (none), `KNOWN` (ordered), or `UNKNOWN_DELETE` (timestamp-free DELETE). They never compare resource `updated_at` with Shopify trigger time.

A timestamp-free DELETE enters `UNKNOWN_DELETE`. It blocks every later UPSERT, including timestamped and redelivered UPSERTs, as `PROVIDER_SOURCE_ORDER_AMBIGUOUS`; no receipt or raw revision is written. Shopify returns an SQS partial failure and WooCommerce a retryable conflict response. Only an explicit developer/operator reset or correction may clear or correct this state, after independently verifying provider state. Future reconciliation may resolve it. Equal time with distinct evidence is a recoverable conflict. A newer timestamped UPSERT may restore only after a `KNOWN` ordered withdrawal.

For Shopify intake, EventBridge uses its default target delivery policy (up to 24 hours and 185 retries) with no custom retry policy or EventBridge DLQ override. The primary SQS queue uses partial batch failure reporting, 4-day retention, and a 5-receive redrive policy; its DLQ retains messages for 14 days. A Shopify `PROVIDER_SOURCE_ORDER_CONFLICT` is returned as a failure for only that SQS record. It retries, then lands in this DLQ; it is not automatically resolved by delay. Alert on this error code. Operator inspects safe metadata only (source ID, topic, delivery IDs, source timestamp, digest), independently verifies provider truth, explicitly resets or corrects the guard when warranted, then redrives the retained record. A reused delivery ID with different source evidence is invalid and acknowledged, not retried. Manual redrive must begin while the message remains retained by the applicable SQS queue; it cannot recover an expired message.

## Signals

Structured log events are safe to count by their fixed fields; their `metric` names do not imply provisioned CloudWatch custom metrics or dashboards:

- `product_listing_raw_capture`: `ingestion_method`, `outcome`, attempt/insert/unchanged counters, byte sizes, and latency.
- `product_listing_raw_normalization`: terminal `outcome` (`APPLIED`, `NO_CHANGE`, `IGNORED`, `REJECTED`) and latency; retryable `failure` or `stream_failure` records carry a stable `error_code`.
- `product_listing_raw_normalization_backlog`: bounded reconciliation-page count and oldest age.
- `product_listing_raw_normalization_reconciliation`: reconciliation runs, processed revisions, failures, bounded page count, page kind, cursor presence, FIFO depth, deferred-continuation count, and suppressed-continuation count.
- `crawler_disposition_transition`: successful transitions to `DORMANT_SOLD`.

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

Also set the matching source `AURA_HISTORIA_WORKER_QUEUE_URL`, `AWS_REGION`, `STAGE`, and `POSTGRES_*`; no in-memory production fallback. Effective consumer concurrency is deliberately 1, poll 20s, visibility 300s, budget 240s, retry 30–900s with jitter, max receives 5.

Its Sequin subscription must contain only `product_listing_raw_revisions` `INSERT` operations. A raw revision must not appear in any ProductListing event, OpenSearch, notification, translation, embedding, assessment, or matching subscription. The external Sequin owner must set delivery timeout >10s (15s recommended), batch <=100, and body <=1 MiB; only test fixtures exist in this repo, not a deployed Sequin configuration. Malformed upstream CDC stays unacknowledged in Sequin and never reaches the worker DLQ.

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
    AND su.crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
    AND (su.next_retry_at IS NULL OR su.next_retry_at <= now())
    AND (su.last_scraped IS NULL OR su.last_scraped < now() - interval '1 day')
)
SELECT count(*) FROM eligible_urls;
```

Do not add `next_scrape_at`, dormant revisit scheduling, a raw-JSON GIN index, or a cross-database join as a response to this check.
