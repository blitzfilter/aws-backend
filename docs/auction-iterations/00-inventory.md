# Iteration 00 — inventory, contract lock, and ownership map

## Gate

**PASS** — documentation/planning only. Stop after this record.

## Objective and non-goals

Objective: record the actual baseline, ownership boundaries, contract closure, and reset discovery needed for isolated Auction iterations.

Non-goals: no executable schema, Rust type, route, API/OpenAPI behavior, event, raw field, index mapping, fixture, reset, or data change.

## Baseline and prerequisite

- Prerequisite: authorized starting checkout.
- Starting SHA: `c10ea0f44e63f249d398c10a7211933a3c48868f`.
- Starting worktree: clean (`git status --short` produced no entries).
- Resulting state: uncommitted documentation-only diff; no commit was created.

Read: root/src/docs instructions, architecture, object IDs, ProductListing, Party/ListingSource, storage, event flow, raw-normalization runbook, affected crate instructions, and test/architecture review skills.

## Implemented in this iteration

- Added the planned target-domain record: [`docs/auction.md`](../auction.md).
- Added the contract-owner and producer/consumer plan: [`docs/auction-implementation.md`](../auction-implementation.md).
- Added this handoff record and registered the new documentation in `docs/AGENTS.md`.
- Confirmed `auc` is absent from the current object-ID registry.
- Recorded the separate existing protocol markers: raw-values schema version currently selects V1/V2, ProductListing journal schema is `1`, and worker SQS envelope schema is `2`. No successor is reserved or introduced.

## Actual inventory

| Concern | Current owner/path |
| --- | --- |
| Listing auction state | `src/product-listing-core/src/product_listing.rs`; only optional `start`/`end`. |
| Raw values and input hash | `src/product-listing-normalization/src/raw_values_normalizer.rs`, `normalization_input.rs`. |
| Canonical caller-owned writer | `src/product-listing-service/src/canonical_product_listing_write.rs`. |
| Raw stream normalizer transaction | `src/product-service/src/use_cases/normalize_product_listing_raw_revision.rs`. |
| Raw capture | `src/product-listing-service/src/use_cases/commands/capture_product_listing_raw_observation.rs`. |
| Initial business schema | `migrations/20260725090000_initial_business_schema.sql`. |
| PostgreSQL event codec/append | `src/product-listing-postgres/src/product_listing_event_codec.rs`, `product_listing_event_appender.rs`. |
| Partner mapping | `src/aura-historia-api/src/partner_product_listings/types.rs`. |
| Providers | `src/crawler/src/scraper/raw_input.rs`, `src/shopify-lambda/src/types.rs`, `src/woocommerce-service/src/use_cases/woocommerce_webhook_intake.rs`. |
| CDC/workers | `src/aura-historia-worker/src/cdc.rs`, `main.rs`. |
| Listing OpenSearch/search | `src/product-listing-opensearch/`, `opensearch/mappings/product_listings.json`. |
| Saved search | `src/search-filter-{core,service,postgres,opensearch}/`, `opensearch/mappings/user_search_filters.json`. |
| Source deletion blocker | `src/listing-source-service/src/use_cases/commands/delete_listing_source.rs`, `src/listing-source-postgres/src/repositories/listing_source_repository.rs`. |

The actual layout differs from a simple one-crate model: pure raw normalization is `product-listing-normalization`; raw orchestration is `product-service`; canonical ProductListing writes are `product-listing-service`; adapters are `product-listing-postgres` and `product-listing-opensearch`.

## Current and planned contract distinction

Current behavior is unchanged. Specifically, V1/V2 raw values and ambiguous listing timestamps remain active baseline contracts. Iteration 04 replaces raw versions directly; iteration 05 replaces timing; iteration 06 adds membership. No current doc claims future routes or model fields are already available.

## Reset finding

The approved test reset harness is `test-api::Postgres::new("migrations")` plus its scoped OpenSearch/SQS helpers. Crawler has separate local migrations/scripts. No coordinated shared development reset command was discovered. No destructive action was run. Contract-owning iterations must document the authorized reset sequence before claiming a rehearsal.

## Verification

Environment: local workspace, no shared services reset.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `cargo depgraph-check check` | PASS (`All dependency rules pass`) |
| `cargo check --workspace` | PASS (53.43s) |

No Rust behavior changed, so no focused or infrastructure test was applicable in this documentation-only iteration. No test result is claimed.

The requested `rg` inventory commands could not run because `rg` is not installed in this checkout environment. Source inspection supplied the listed paths instead.

## Audit

- No compatibility mechanism, stub, placeholder, executable change, or reset action added.
- No architecture deviation introduced.
- No source payload or sensitive data recorded.
- Next iteration: **01 — pure Auction domain and shared time values**.
