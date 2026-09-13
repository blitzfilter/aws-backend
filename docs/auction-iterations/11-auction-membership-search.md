# Iteration 11 — Auction membership search

**Status:** PASS.

## Objective

Add exact resolved `AuctionId` membership filtering to ProductListing OpenSearch search and saved-search percolation. Do not add an Auction index or shared-Auction metadata fan-out.

## Non-goals

No Auction execution, metadata projection, compatibility reader, API version, index generation, or schema migration.

## Starting point

- Starting checkout: `d84939040dffc946f5517180b3eb71669346c340`.
- No commit was created.

## Current contract

- Public `GET /api/v1/product-listings` accepts repeated `auctionId=auc_...` values.
- Saved-search create/read/update persists `search.auctionId`; PATCH omission preserves and `null` clears it.
- IDs are strict `auc_` TypeIDs. Values OR within the Auction-ID set and intersect other filters before OpenSearch pagination.
- Only resolved listing membership matches. Missing document `auctionId` covers both no Auction context and unresolved context.
- Percolation uses the same `terms` predicate. The filter is restricted for Free users and allowed for Pro/Ultimate, matching `listingSourceId`.
- Saved-filter PostgreSQL JSON persists backing UUIDs with strict UUIDv7 rehydration. OpenSearch saved-filter documents persist TypeIDs. Both are one current strict shape.
- `user_search_filters.search.auctionId` is a keyword mapping. No Auction index or metadata fan-out exists.

## Reset

The saved-filter OpenSearch mapping and persisted search JSON changed directly. Recreate disposable development saved-filter/OpenSearch state only through approved reset tooling before a matching checkout is deployed. This task performed no database, queue, index, or remote reset.

## Verification

Passed:

```text
cargo fmt --all -- --check
cargo check --workspace
cargo depgraph-check check
cargo test -p product-listing-core --all-features
cargo test -p product-listing-opensearch --lib --all-features -- --skip should_fence_in_flight_writes_after_withdrawal_gc_and_restore
cargo test -p search-filter-service --all-features
cargo test -p search-filter-postgres --all-features
cargo test -p search-filter-opensearch --lib --all-features -- --skip should_fence_in_flight_filter_writes_after_delete_gc_and_newer_projection
cargo test -p aura-historia-api --lib product_listings::search_products::tests --all-features
cargo test -p aura-historia-api --lib search_filters::types::tests --all-features
```

Full real-infrastructure verification also passed:

```text
cargo test -p product-listing-opensearch --all-features
cargo test -p search-filter-opensearch --all-features
cargo test -p aura-historia-api --all-features
cargo test -p aura-historia-worker --test search_filter_percolator --all-features
```

The two OpenSearch suites include their existing deletion-fence races. The worker suite exercises real PostgreSQL, OpenSearch, SQS, percolation, event redelivery, withdrawal, and current-event ordering.

## Architecture audit

The new identifier is listing-owned search state. API maps only transport TypeIDs; service owns patch/tier policy; PostgreSQL and OpenSearch adapters own storage/document codecs. No adapter type escapes, no controller orchestration, and no new cross-store repository or distributed transaction was added.
