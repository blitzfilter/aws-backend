# DOX

## Purpose

- Own `product-listing-opensearch` crate.
- Own canonical Product Listing OpenSearch adapters and private search documents.

## Core Design

- Depends on `product-listing-core`, `product-listing-service`, `listing-source-core`, `money`/`localization` canonical values, and `platform-opensearch` generic response envelopes.
- Exports public OpenSearch reader factory/type, external-versioned Product Listing projection writer, saved-filter percolator JSON builder, and typed-source-to-percolation JSON mapper.

- Keeps OpenSearch documents and mappings private; structural documents use canonical semantic leaves through local codecs, and public percolation helpers expose no document type. Aura object identity fields, object-derived `_id` values, ID terms, and ID sort tie-breakers use canonical typed TypeIDs through domain serde/`Display`; wrong-prefix and bare UUID documents are invalid, with no legacy read compatibility. Persistent and temporary percolation documents use `productListingTitleSlugId` for the ProductListing title slug and carry source identity only as `listingSourceId` and `sourceListingId`; source name and slug stay in PostgreSQL hydration. They retain the raw source URL and the joined ListingSource-derived outbound view URL. Product Listing language keeps the historical uppercase OpenSearch vocabulary through its local codec. Withdrawn listings replace their projection with a content-free `{productListingId, projectionDeleted: true}` document at the withdrawal's external version. Search, hybrid BM25/KNN, and similar-listing KNN exclude this marker before pagination; absent markers remain live for compatibility. Live documents carry optional `availability` but no lifecycle field. Persistent Product Listing source price is a tagged `MONETARY` or `ON_REQUEST` assertion; absent stays absent. Only `MONETARY` has numeric `amount`/`currency`, FX conversion, or optional sale prices. An active `SoldOut` listing with an explicit sale observation retains `saleObservationFxRateId` and `saleObservedAt`; its `salePrices` use the immutable snapshot only for a monetary source price. Active relisted listings use current pricing; temporary percolation prices use the closed-world `priceByCurrency` shape, including every supported currency such as `ZAR`.
- OpenSearch reads are ordinary readers. No transaction or unit of work. Auction context stays listing-local: documents carry optional `lotLabel`, `lotPosition`, and exact-only `lotBiddingOpensAt`, `lotScheduledClosesAt`, and `lotReportedClosedAt`; they never carry an Auction ID.

## Ownership

- This doc rule `src/product-listing-opensearch/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- Update this file when crate contract, dependency edge, index shape, or exported adapter changes.
- Keep `opensearch/mappings` aligned when document fields change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Search documents do not escape this adapter.
- Map OpenSearch payloads only into factual `product-listing-service` read models. Search and KNN return raw `ProductListingSearchItem` values; content policy and presented image URLs stay in service.
- Preserve query-building tests for source and availability filters, cursors, canonical percolator semantics, pinned price conversion, exact-only lot timing, and invalid sale-observation documents. Lot-time range maxima use half-open `lt` bounds. Availability query clauses intersect exact values with derived orderability expansions and add an `exists`-based missing-field clause only when unspecified availability is requested.
- Price sorting is unsupported. Product Listing search and similar readers consume one compiled request. Search filters use exact optional-availability fields and never lifecycle clauses. Active summary prices use its pinned plan and sold summaries use exact indexed target values; summary valuation metadata names the current or sale-observation basis. Product Listing projection writes use `product_listings.projection_version` with OpenSearch external versioning; conflicts are stale no-ops. Never physically delete a withdrawal fence: OpenSearch physical-delete version memory expires after `index.gc_deletes` (default 60s), while an already-prepared remote write may still arrive. Client cancellation/timeouts are not remote fences. Only a strictly newer source version restores a listing.

## Deletion fence rollout and rebuild

- Add the boolean `projectionDeleted` mapping and deploy all read filters before enabling tombstone writers. Old physical-delete writers must not coexist: they can remove the durable fence.
- No tombstone TTL or routine delete-by-query. Fence durability is bounded by index retention; index deletion, unversioned writes, ID/version reuse, or unsafe alias cutover invalidate it.
- Rebuild from current PostgreSQL state, including withdrawn rows and their projection versions, into a fresh generation. Fence old writers from the new generation, catch up committed changes, verify visible IDs/versions and withdrawn markers, then cut over. Stopping a client alone does not drain remote work.
- Existing physically deleted IDs need authoritative withdrawal replay/backfill; this change cannot recover versions already forgotten by OpenSearch. Worker acceptance and deployment/runbook changes belong to coordinator.

## Verification

- `cargo check -p product-listing-opensearch`
- `cargo test -p product-listing-opensearch --all-features`
- Private `projection_race_tests.rs` uses real `test-api` OpenSearch and a paused HTTP relay. It waits 65s with 60s delete GC, verifies a physical-delete control loses its fence, then checks stale in-flight writes, unseen IDs, duplicate withdrawals, all supported readers, and restoration/stale-delete ordering. No ignored tests or mocked target acceptance.

## Child DOX Index

- None.
