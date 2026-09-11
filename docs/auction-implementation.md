# Auction implementation plan

**Status:** iterations 00–01 are complete; iteration 02 implements standalone Auction persistence and admin service use cases. This remains a change plan, not a declaration of Auction HTTP or public-read behavior. ProductListing contracts remain the current baseline until their owning iteration passes.

- Target issue: #1465; reliable identifier path of #1464.
- Baseline: `c10ea0f44e63f249d398c10a7211933a3c48868f`.
- Development policy: direct replacement; no successor raw/API/event/index version, compatibility reader, aliases, dual writes, backfill, or transition migration.
- Iteration records: [00 inventory](auction-iterations/00-inventory.md), [01 auction core](auction-iterations/01-auction-core.md), [02 Auction persistence](auction-iterations/02-auction-persistence.md). Later records are added only after their own passing gates.

## Observed baseline

| Area | Current owner and observed state |
| --- | --- |
| ProductListing auction | `product-listing-core::ProductListingAuction` has only optional exact `start`/`end`; PostgreSQL, event, API, OpenSearch, and saved filters carry those ambiguous terms. |
| Raw values | `product-listing-normalization` owns strict V1/V2 values. The envelope's `raw_values_schema_version` selects the branch. V1 is display text; V2 requires `priceFormat` and supports `DISPLAY_TEXT`/`MACHINE_DECIMAL`. |
| Raw path | `product-listing-service` captures immutable revisions; `product-service` normalizes ordered streams in one caller-owned PostgreSQL transaction through `canonical_product_listing_write`. |
| Direct partner path | `aura-historia-api::partner_product_listings` maps typed input to direct canonical service writes. It does not capture raw revisions. |
| Producers | Crawler emits V1. Shopify Lambda and WooCommerce emit V2 `MACHINE_DECIMAL`. All must be current-shape producers before an owning raw contract gate passes. |
| Journal and CDC | `product_listing_events` uses domain schema `1`; the worker validates/routs listing events and raw revisions separately. Existing worker SQS envelope schema is `2`; it is a separate protocol marker. |
| Search | ProductListing OpenSearch has `auctionStart`/`auctionEnd`; `search-filter-*` persists and percolates `ProductListingSearch`. No Auction index exists. |
| Source deletion | `listing-source-service`/`listing-source-postgres` block source deletion for retained ProductListings, raw streams, and partnership-application references. Auction becomes an additional blocker in iteration 02. |

The original broad inventory scan requested `rg`, but this checkout does not have `rg` installed. The same locations were confirmed through source inspection and the recorded baseline source map.

## Ownership and dependency plan

| Iteration | Complete capability | Contract owners and consumers |
| --- | --- | --- |
| 00 | Inventory only | `docs/auction.md`, this plan, iteration record, documentation index. |
| 01 | Pure Auction model and time values | **PASS.** `auction-core`, object-ID registry, workspace/dependency rules, pure core tests. No listing changes. |
| 02 | Persisted standalone Auction and admin write use cases | **Implemented; full workspace library suite pending rerun.** `auction-service`, `auction-postgres`, initial business schema, source deletion blocker, PostgreSQL tests. No HTTP/public reads, listing membership, raw changes, crawler, or search. |
| 03 | Admin Auction HTTP | `aura-historia-api`, service wiring/DTOs, OpenAPI, changelog, black-box API tests. |
| 04 | One raw-values shape | `product-listing-normalization`, capture/hash/dispatch, crawler, Shopify, WooCommerce, raw fixtures/runbook. Replace V1/V2 directly while retaining both price formats. |
| 05 | Qualified listing auction context and lot timing | Listing core/service/PostgreSQL/OpenSearch, raw and partner inputs, events/history, crawler selectors, saved filters/percolation, API and affected listing consumers. |
| 06 | Source-key membership and transactional resolution | `auction-*`, listing context persistence/events, canonical writer, `product-service`, raw diagnostics/evidence, direct partner path, projection membership mapping. |
| 07 | Corrections, override barrier, and safe release | Listing-owned policy/audit/floors, canonical/raw/partner guards, admin context/correction/release endpoints and concurrency tests. |
| 08 | Fixture-backed crawler auction extraction | `crawler` selector generation/evaluation/review, raw producer mapping, provider fixtures, capture-to-resolution integration. |
| 09 | Batched Auction summaries on listing reads | Auction summary reader plus ProductListing detail/search/similar/watchlist presentation, API DTOs/OpenAPI, bounded-query tests. |
| 10 | Public browsing | Auction directory/detail readers, listing-service catalogue reader, PostgreSQL adapters, public controllers/OpenAPI/API tests. |
| 11 | Auction-ID search and saved search | ProductListing search model, OpenSearch predicates/projection fixtures, API parser, search-filter codecs/percolation/tier policy. |
| 12 | Release audit | Full acceptance fixtures, clean initialization rehearsal, documentation and compatibility audit. No deferred feature implementation. |

Target dependency direction is `auction-core -> auction-service -> auction-postgres -> composition`, with `product-listing-core` depending only on Auction semantic values required by the final context. Auction service/core must not depend on ProductListing service/core. Catalogue ownership stays in `product-listing-service`; presentation reads use bounded readers, not repositories.

## Contract replacement inventory

| Contract | Baseline | Owning replacement iteration | Required closure |
| --- | --- | --- | --- |
| Object ID registry | no `auc` prefix | 01 — complete | `docs/object-ids.md`, strict codec tests, workspace graph. |
| Auction state/event/schema | absent | 02 | initial DDL, repository/event codec, CAS/protection, source delete blocker. |
| Admin Auction REST | absent | 03 | routes, mappings, OpenAPI, changelog, service authorization/API tests. |
| Raw values | V1/V2 branches | 04 | all producers, capture/hash/dispatch, fixtures and reset note; discriminator remains only `1`. |
| Ambiguous listing time | `auctionStart`/`auctionEnd` everywhere | 05 | core/DDL/events/raw/partner/API/OpenSearch/saved search/crawler/fixtures; remove accepted old Aura fields. |
| Membership/reference and metadata | absent | 06 | current raw and typed input, transaction-bound resolver, listing/auction events, evidence/diagnostics, source FK, projector mapping. |
| Manual correction policy | absent | 07 | policy/audit/floors, both ingestion guards, admin ETag flows, race tests. |
| Crawler reference extraction | absent | 08 | selector schema/review/output and fixture-backed provider rules; no direct canonical write. |
| Listing summary hydration | absent | 09 | batch Auction reader and every named listing presentation surface; no N+1. |
| Public Auction browsing | absent | 10 | PostgreSQL directory/detail/catalogue, cursors, visibility, no-store. |
| Search/save filter Auction ID | absent | 11 | public query, document predicate, persisted filter codec, percolation and tier tests. |

## Test and reset ownership

Unit tests stay beside core/service implementation. PostgreSQL adapter/race tests use the real `test-api::Postgres::new("migrations")` harness. API black-box tests use the process-lived `test-api::AuraHistoriaApi`. Worker and projection tests use real Postgres, Sequin, LocalStack SQS, and OpenSearch where their current contracts change.

Known local reset support:

- `test-api` applies the root `migrations` schema once per suite and truncates application data between tests; it clears canonical OpenSearch documents, including `user_search_filters`.
- `WorkerSqs` test setup/teardown creates and deletes only its process-isolated queue pair.
- Crawler owns separate local migrations and `scripts/linux/`/`scripts/windows/` tooling.

No single repository command for a coordinated development business-schema/raw-queue/index reset was found during this inventory. Later persisted/wire-contract iterations must name the reviewed, authorized tooling and exact dependency order before claiming a reset rehearsal. This task performed no reset, deployment, queue purge, or remote action.

## Later scope guard

Do not add bids, outcomes, results, reminder delivery, Party attribution, venue/location, sessions, global Auction merge, generic evidence graph, generic entity resolver, or Auction OpenSearch index. A failed gate belongs to its owning iteration; a later iteration cannot repair it silently.
