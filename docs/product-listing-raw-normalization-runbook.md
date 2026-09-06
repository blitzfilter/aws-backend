# ProductListing raw-normalization runbook

## Scope

`product_listing_raw_revisions` is immutable source evidence. Only the `product-listing-normalization` worker subscription consumes its `INSERT` rows. Raw source JSON, webhook bytes, signatures, headers, and provenance values must not be copied into logs, dashboards, tickets, or ad-hoc query output.

The worker repairs a missed CDC wake-up with bounded startup and periodic reconciliation. Restarting the worker is safe; do not delete raw revisions to repair backlog.

## Signals

Structured metric events are safe to count by their fixed fields:

- `product_listing_raw_capture`: `ingestion_method`, `outcome`, attempt/insert/unchanged counters, byte sizes, and latency.
- `product_listing_raw_normalization`: terminal `outcome` (`APPLIED`, `NO_CHANGE`, `IGNORED`, `REJECTED`) and latency.
- `product_listing_raw_normalization_backlog`: bounded reconciliation-page count and oldest age.
- `product_listing_raw_normalization_reconciliation`: reconciliation runs, processed revisions, and failures.
- `crawler_disposition_transition`: successful transitions to `DORMANT_SOLD` or `DORMANT_REMOVED`.

Alert on sustained backlog age, repeated worker failures, or a rising `REJECTED` count. Payload size is an early warning only; capture limits remain authoritative.

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
