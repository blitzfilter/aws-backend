# DOX

## Purpose

- Own ListingSource use cases and ports.

## Core Design

- Create and update own PostgreSQL transaction scope.
- Create may atomically persist a new Party and ListingSource through transaction-bound factories.
- Provider readers return focused safe models; secrets never leave verifier boundaries. Shopify and WooCommerce source reads, including WooCommerce signature validation, require the exact ListingSource grant owned by the source operator's Partnership. `WebCrawlSourceReader` returns a complete canonical source snapshot with derived WebCrawl enablement and optional persisted fallback currency for crawler sync. Trusted system Shopify intake has no user-membership check, but still requires that exact source grant.
- Party-based provider/grant runtime wiring waits for Iteration 5. Existing Shop composition stays untouched.
- Each outbound capability owns one `ports/<capability>.rs` file; `ports/mod.rs` only assembles exports. The admin ListingSource search use case owns its safe summary and enforces administrator authorization before its bounded reader transaction.

## Verification

- `cargo check -p listing-source-service`
- `cargo test -p listing-source-service --all-features`
