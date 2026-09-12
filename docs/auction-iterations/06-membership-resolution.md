# Auction iteration 06 — membership resolution

## Gate

**PASS** — source-key Auction membership. Stop before iteration 07.

## Objective and non-goals

Objective: resolve a reliable `SourceAuctionId` to one source-scoped Auction in the caller-owned ProductListing transaction.

Non-goals: no correction/override barrier, reoffer model, crawler extraction, public Auction directory/catalogue, Auction hydration, or Auction-ID filter.

## Delivered

- `ProductListingAuction` now has optional `AuctionMembership`.
  - No context means no participation assertion.
  - A present context without membership is unresolved participation.
  - Resolved membership stores strict `AuctionId` only.
- The initial schema directly persists `auction_id` in listing-owned context rows. Composite source foreign keys reject cross-source membership.
- `auction-service::resolve_auction_for_listing` resolves or creates by `(ListingSourceId, SourceAuctionId)` in the outer transaction and appends one Auction discovery/change event only for semantic Auction changes.
- Existing membership plus a different reliable key returns `MEMBERSHIP_CHANGE_REQUIRES_CORRECTION`; it does not create or attach the conflicting Auction.
- Raw normalization and typed partner create/update/upsert use the transactional resolver. Typed input accepts `auction.sourceAuctionId` and fill-only embedded Auction metadata.
- ProductListing events, PostgreSQL readers, OpenSearch listing documents, percolation input, mappings, search summaries, and listing REST data carry `auctionId` when resolved plus `hasAuctionContext` to distinguish unresolved context.
- OpenSearch stays listing-owned. No Auction metadata is denormalized and no Auction-ID query filter exists.

## Development reset boundary

The initial business schema, current ProductListing event payload, partner listing payload, and OpenSearch mappings changed directly. Old disposable development data, queues, and indexes must not run with this checkout. Under explicit authorization, stop matching local processes, recreate compatible local fixtures through established tooling, then restart matching producers and consumers. No reset, queue purge, deployment, or remote mutation ran here.

## Verification

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `cargo check --workspace` | PASS |
| `cargo depgraph-check check` | PASS |
| `cargo test --workspace --all-features --no-run` | PASS |
| `cargo test --workspace --lib --all-features` | PASS |
| `cargo test -p product-listing-service --lib --all-features` | PASS — 171 tests |
| `cargo test -p product-listing-opensearch product_listing_percolation_document::tests::should_keep_every_maximal_temporary_percolation_document_path_mapping_compatible --all-features -- --exact` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::result-large-err` | PASS |

## Audit

- No compatibility DTO, legacy decoder, alias, schema/event generation, dual write, backfill, or transition migration was added.
- PostgreSQL remains authoritative. OpenSearch remains rebuildable and external-version fenced.
- The resolver never uses name, URL, schedule, or Party identity as an Auction key.
- A membership change is blocked rather than silently reassigned. Iteration 07 owns authorized correction and override release.

## Next iteration

**07 — correction, override barrier, and safe release.**
