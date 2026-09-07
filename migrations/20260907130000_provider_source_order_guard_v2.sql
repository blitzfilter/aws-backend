-- Provider source-order guards preserve nanoseconds and retain an explicit
-- timestamp-free delete barrier. Receipt digests and immutable raw history stay unchanged.
ALTER TABLE product_listing_raw_streams
    DROP CONSTRAINT product_listing_raw_streams_latest_provider_source_ordering_pair_check,
    ADD COLUMN latest_provider_source_epoch_seconds bigint,
    ADD COLUMN latest_provider_source_nanoseconds integer,
    ADD COLUMN latest_provider_source_operation text,
    ADD COLUMN latest_provider_source_ordering_state text NOT NULL DEFAULT 'UNKNOWN';

-- A legacy WooCommerce watermark has only microsecond precision and a payload-only
-- digest. Keep its time boundary, but mark its equality evidence legacy rather than
-- comparing it to the V1 operation-aware digest.
UPDATE product_listing_raw_streams
SET latest_provider_source_epoch_seconds = floor(extract(epoch FROM latest_provider_source_occurred_at))::bigint,
    latest_provider_source_nanoseconds = mod(
        extract(microseconds FROM latest_provider_source_occurred_at)::integer,
        1000000
    ) * 1000,
    latest_provider_source_ordering_state = 'LEGACY'
WHERE ingestion_method = 'WOOCOMMERCE'
  AND latest_provider_source_occurred_at IS NOT NULL;

-- Shopify's old resource clock was already cleared by the preceding migration. A
-- retained final delete must still block an unproven restoration after the upgrade.
UPDATE product_listing_raw_streams AS stream
SET latest_provider_source_operation = 'DELETE',
    latest_provider_source_ordering_state = 'UNKNOWN'
WHERE stream.ingestion_method = 'SHOPIFY'
  AND (
      SELECT revision.operation
      FROM product_listing_raw_revisions AS revision
      WHERE revision.product_listing_raw_stream_id = stream.product_listing_raw_stream_id
      ORDER BY revision.revision DESC
      LIMIT 1
  ) = 'DELETE';

ALTER TABLE product_listing_raw_streams
    ADD CONSTRAINT product_listing_raw_streams_provider_source_ordering_state_check
        CHECK (latest_provider_source_ordering_state IN ('UNKNOWN', 'KNOWN', 'LEGACY')),
    ADD CONSTRAINT product_listing_raw_streams_provider_source_nanoseconds_check
        CHECK (latest_provider_source_nanoseconds IS NULL
            OR latest_provider_source_nanoseconds BETWEEN 0 AND 999999999),
    ADD CONSTRAINT product_listing_raw_streams_provider_source_operation_check
        CHECK (latest_provider_source_operation IS NULL
            OR latest_provider_source_operation IN ('UPSERT', 'DELETE'));
