# DOX

## Purpose

- Own ListingSource PostgreSQL repository and readers.

## Core Design

- Rows, SQL, provider configuration, and secrets stay adapter-private. Shopify/WooCommerce readers and WooCommerce signature verification require the exact `(partnership_id, listing_source_id)` grant for the ListingSource operator's Partnership. The `WebCrawlSourceReader` maps a complete canonical ListingSource snapshot with ID/name/slug, derived `WEB_CRAWL` enablement, and optional fallback currency.
- Repository uses caller-owned `SqlxTransaction`; unknown persisted ingestion values fail.
- `lib.rs` only declares and re-exports; the aggregate repository lives in `repositories/listing_source_repository.rs`.
- Each reader implementation owns one `readers/<capability>.rs` file; `readers/mod.rs` holds shared adapter state, helpers, and narrow reader re-exports. The bounded ListingSource search reader joins only business ListingSource/Party data and never selects provider configuration or crawler-local state.

## Verification

- `cargo check -p listing-source-postgres`
- `cargo test -p listing-source-postgres --all-features`
