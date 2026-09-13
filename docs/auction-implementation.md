# Auction implementation plan

**Status:** iterations 00–11 are complete.

- Target issue: #1465; reliable identifier path of #1464.
- Baseline: `c10ea0f44e63f249d398c10a7211933a3c48868f`.
- Development policy: direct replacement; no successor raw/API/event/index version, compatibility reader, aliases, dual writes, backfill, or transition migration.
- Iteration records: [00 inventory](auction-iterations/00-inventory.md), [01 auction core](auction-iterations/01-auction-core.md), [02 Auction persistence](auction-iterations/02-auction-persistence.md), [03 admin HTTP](auction-iterations/03-auction-admin-api.md), [04 current raw contract](auction-iterations/04-current-raw-contract.md), [05 qualified lot timing](auction-iterations/05-qualified-lot-timing.md), [06 membership resolution](auction-iterations/06-membership-resolution.md), [07 auction corrections](auction-iterations/07-auction-corrections.md), [08 crawler Auction extraction](auction-iterations/08-crawler-auctions.md), [09 Auction summary reads](auction-iterations/09-auction-summary-reads.md), [10 public Auction browsing](auction-iterations/10-auction-public-browsing.md), [11 Auction membership search](auction-iterations/11-auction-membership-search.md). Incomplete records state their blocker and must not be treated as passing gates.

## Observed baseline

| Area | Current owner and observed state |
| --- | --- |
| ProductListing auction | `product-listing-core::ProductListingAuction` is optional asserted context with optional lot label, catalogue position, and qualified `bidding_opens`/`scheduled_closes`/exact `reported_closed_at`. PostgreSQL, events, API, OpenSearch, and saved filters use this one current shape. |
| Raw values | `product-listing-normalization` owns one strict current shape. `raw_values_schema_version` remains `1` and `priceFormat` is required; it supports `DISPLAY_TEXT` and `MACHINE_DECIMAL`. Other values are rejected without conversion. |
| Raw path | `product-listing-service` captures immutable revisions; `product-service` normalizes ordered streams in one caller-owned PostgreSQL transaction through `canonical_product_listing_write`. |
| Direct partner path | `aura-historia-api::partner_product_listings` maps typed input to direct canonical service writes. It does not capture raw revisions. |
| Producers | Crawler emits schema `1` `DISPLAY_TEXT`. Shopify Lambda and WooCommerce emit schema `1` `MACHINE_DECIMAL`. All current producers match the same contract. |
| Journal and CDC | `product_listing_events` uses domain schema `1`; the worker validates/routs listing events and raw revisions separately. Existing worker SQS envelope schema is `2`; it is a separate protocol marker. |
| Search | ProductListing OpenSearch indexes exact `auctionId`, `lotBiddingOpensAt`, `lotScheduledClosesAt`, and `lotReportedClosedAt` plus lot label/position; date-only values are omitted from exact time fields. Saved filters persist and percolate exact resolved Auction membership. No Auction index exists. |
| Source deletion | `listing-source-service`/`listing-source-postgres` block source deletion for retained ProductListings, raw streams, and partnership-application references. Auction becomes an additional blocker in iteration 02. |

The original broad inventory scan requested `rg`, but this checkout does not have `rg` installed. The same locations were confirmed through source inspection and the recorded baseline source map.

## Ownership and dependency plan

| Iteration | Complete capability | Contract owners and consumers |
| --- | --- | --- |
| 00 | Inventory only | `docs/auction.md`, this plan, iteration record, documentation index. |
| 01 | Pure Auction model and time values | **PASS.** `auction-core`, object-ID registry, workspace/dependency rules, pure core tests. No listing changes. |
| 02 | Persisted standalone Auction and admin write use cases | **PASS.** `auction-service`, `auction-postgres`, initial business schema, source deletion blocker, PostgreSQL tests. No HTTP/public reads, listing membership, raw changes, crawler, or search. |
| 03 | Admin Auction HTTP | **PASS.** `aura-historia-api`, service wiring/DTOs, OpenAPI, changelog, black-box API tests. |
| 04 | One raw-values shape | **PASS.** Full workspace library tests, check, dependency graph, clippy, format, and focused integration suites passed; details are in [the iteration 04 handoff](auction-iterations/04-current-raw-contract.md). `product-listing-normalization`, capture/hash/dispatch, crawler, Shopify, WooCommerce, and raw fixtures/runbook use one schema `1` shape requiring `priceFormat`; both price formats remain. |
| 05 | Qualified listing auction context and lot timing | **PASS.** Listing core/service/PostgreSQL/OpenSearch, current raw and partner inputs, events/history, saved filters/percolation, API and affected listing consumers. Generic crawler timing extraction is removed rather than inferred; fixture-backed source extraction belongs to iteration 08. |
| 06 | Source-key membership and transactional resolution | **PASS.** `auction-*`, listing context persistence/events, canonical writer, `product-service`, direct partner path, and projection membership mapping. Correction barriers, crawler extraction, public Auction reads, and Auction-ID filters remain later iterations. |
| 07 | Corrections, override barrier, and safe release | **PASS.** Listing-owned policy/audit/floors, canonical/raw/partner guards, admin context/correction/release endpoints, strict `If-Match`, and full worker verification pass. |
| 08 | Fixture-backed crawler auction extraction | **PASS.** `crawler` maps the fixture-backed Lot-tissimo source key, catalogue URL, reviewed catalogue-name/lot-label selectors, and current raw Auction metadata. It adds no canonical crawler write. A real PostgreSQL raw capture → normalizer → resolver test proves one linked Auction, fill-only metadata, and conflict preservation. |
| 09 | Batched Auction summaries on listing reads | **PASS.** `auction-service` batch-reader port, `auction-postgres` adapter, and ProductListing detail/search/similar/watchlist presentation return safe current Auction summaries without shared-metadata indexing or fan-out. |
| 10 | Public browsing | **PASS.** Auction directory/detail readers, a bounded full ProductListing catalogue presentation, PostgreSQL adapters, public controllers/OpenAPI, real PostgreSQL reader coverage, public API acceptance coverage, and workspace library tests pass. |
| 11 | Auction-ID search and saved search | **PASS.** ProductListing search model, OpenSearch predicates/projection fixtures, API parser, strict saved-filter codecs/percolation, ListingSource-equivalent tier policy, and full API/worker/OpenSearch verification pass. |
| 12 | Release audit | Full acceptance fixtures, clean initialization rehearsal, documentation and compatibility audit. No deferred feature implementation. |

Target dependency direction is `auction-core -> auction-service -> auction-postgres -> composition`, with `product-listing-core` depending only on Auction semantic values required by the final context. Auction service/core must not depend on ProductListing service/core. Catalogue ownership stays in `product-listing-service`; presentation reads use bounded readers, not repositories.

## Contract replacement inventory

| Contract | Baseline | Owning replacement iteration | Required closure |
| --- | --- | --- | --- |
| Object ID registry | no `auc` prefix | 01 — complete | `docs/object-ids.md`, strict codec tests, workspace graph. |
| Auction state/event/schema | absent | 02 | initial DDL, repository/event codec, CAS/protection, source delete blocker. |
| Admin Auction REST | absent | 03 | routes, mappings, OpenAPI, changelog, service authorization/API tests. |
| Raw values | one schema-`1` shape with required `priceFormat` | 04 — complete | all producers, capture/hash/dispatch, fixtures and reset note; no historical decoder remains. |
| Ambiguous listing time | legacy flat fields | 05 — complete | one optional context with lot label, position, and precision-bearing timing across core/DDL/events/raw/partner/API/OpenSearch/saved search/crawler/fixtures; removed Aura fields reject. |
| Membership/reference and metadata | absent | 06 | current raw and typed input, transaction-bound resolver, listing/auction events, evidence/diagnostics, source FK, projector mapping. |
| Manual correction policy | absent | 07 — complete | policy/audit/floors, both ingestion guards, admin ETag flows, transaction-scoped policy writes, and full worker verification. |
| Crawler reference extraction | absent | 08 — complete | selector schema/review/output and fixture-backed provider rules; no direct canonical write; durable raw-to-resolution coverage. |
| Listing summary hydration | absent | 09 — complete | batch Auction reader and existing detail/search/similar/watchlist presentation surfaces; no Auction data in OpenSearch. |
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
