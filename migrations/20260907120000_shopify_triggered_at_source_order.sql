-- Shopify source ordering now uses EventBridge's X-Shopify-Triggered-At event time.
-- Existing values came from product.updated_at and are incomparable, so clear only this
-- mutable guard metadata. Immutable raw revisions and normalization progress stay intact.
UPDATE product_listing_raw_streams
SET latest_provider_source_occurred_at = NULL,
    latest_provider_source_observation_sha256 = NULL,
    updated = now()
WHERE ingestion_method = 'SHOPIFY'
  AND latest_provider_source_occurred_at IS NOT NULL;
