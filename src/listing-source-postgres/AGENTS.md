# DOX

## Purpose

- Own ListingSource PostgreSQL repository and readers.

## Core Design

- Rows, SQL, provider configuration, and secrets stay adapter-private. Shopify/WooCommerce readers and WooCommerce signature verification require the exact `(partnership_id, listing_source_id)` grant for the ListingSource operator's Partnership. The `WebCrawlSourceReader` maps a complete canonical ListingSource snapshot with ID/name/slug, derived `WEB_CRAWL` enablement, and optional fallback currency.
- Repository uses caller-owned `SqlxTransaction`; unknown persisted ingestion values fail. Eligible hard delete locks the source, checks protected ProductListing/raw/application references, explicitly removes grants and owned configuration without selecting secrets, then uses versioned physical deletion.
- `lib.rs` only declares and re-exports; the aggregate repository lives in `repositories/listing_source_repository.rs`.
- Each reader implementation owns one `readers/<capability>.rs` file; `readers/mod.rs` holds shared adapter state, helpers, and narrow reader re-exports. The bounded ListingSource search reader joins only business ListingSource/Party data and never selects provider configuration or crawler-local state. Public collection and slug readers share a private allowlist mapper, query only source/operator presentation fields, and set their timeout with transaction-local PostgreSQL state. The initial business schema maintains adapter-only `name_search` columns with triggers; no aggregate, row, or existing admin read exposes them. The ignored public-search performance harness runs disposable real PostgreSQL scenarios at 1,000, 10,000, and 100,000 ListingSources, ANALYZEs each, and reports `EXPLAIN ANALYZE BUFFERS` for exact prepared reader SQL under forced custom and generic plans. Its prepared names include the scenario size and its dynamic control SQL is nonpersistent; it must never issue `DEALLOCATE ALL` on a SQLx cached connection or use SQL `OFFSET` for cursor fixtures (fetch the first 11 ordered rows and select the 11th in Rust). It also warms then times the actual public search and slug handlers, reporting nearest-rank p50/p95 against a 150 ms local guard; this is not a deployment SLO. Its report owns the opt-in command and result status.

## Verification

- `cargo check -p listing-source-postgres`
- `cargo test -p listing-source-postgres --all-features`
