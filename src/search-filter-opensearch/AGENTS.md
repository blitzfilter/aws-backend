# DOX

## Purpose

- Own `search-filter-opensearch` crate.
- Own OpenSearch adapter for canonical search-filter index port.

## Core Design

- Writes canonical documents directly through `user_search_filters`; structural documents own index shape while local codecs preserve legacy values for canonical semantic leaves. Aura object identity fields and typed search sets use domain serde; object-derived `_id`, percolator terms, and stable ID sorts use canonical typed TypeIDs. Wrong-prefix and bare UUID documents are invalid, with no legacy read compatibility.
- ProductListing search documents persist positive and negative ListingSource ID filters plus exact resolved-membership `auctionId` filters, and the optional availability query object with canonical exact `ListingAvailability` and derived `ListingOrderability` codes plus `includeUnspecifiedAvailability`; lot time filters target exact-only `lotBiddingOpensAt` and `lotScheduledClosesAt` with half-open upper bounds. Documents also map listing-local `lotLabel`, `lotPosition`, and `lotReportedClosedAt`. Retired Shop, seller, type, location, and address filters are rejected without aliases.
- Builds percolator queries from complete authoritative SearchFilter state using the public ProductListing percolator JSON builder. Price ranges target private `priceByCurrency.<currency>` fields and carry no FX metadata. Percolation receives only an application-owned event-time input with closed-world currency values from the service; it has no FX repository or selection policy.
- Uses Postgres `version` as OpenSearch external versioning; stale or duplicate writes are no-op outcomes. Delete replaces the full document (including its percolator query) with `{userSearchFilterId, sourceVersion, projectionDeleted: true}` at the service-supplied successor version. Query and PIT percolation exclude this boolean marker; absent markers remain live. No physical DELETE: its version memory expires after `index.gc_deletes` (default 60s) and cannot fence delayed already-prepared writes.
- Vertex AI product matching is a service-orchestrated use of the neutral `large-language-model` capability, not part of this OpenSearch adapter.
- Persists every ProductListingSearch field and rejects incomplete or unknown persisted search payloads. Periodic matching progress is operational PostgreSQL state and never enters an OpenSearch document.
- Percolates complete deterministic result sets through a PIT with a bounded page size, stable `userSearchFilterId` sort, exact totals, and defensive ID deduplication; it fails instead of truncating or accepting partial results.
- Uses the shared application cursor default (currently 21) when a search-filter index query omits a cursor, never OpenSearch's implicit page size. Query sorting uses only persisted document fields; periodic progress and removed legacy checkpoints never enter the document or mapping.
- Keeps OpenSearch documents private; generic response envelopes come from `platform-opensearch`. Percolation completeness rules stay in this adapter.

## Ownership

- This doc rule `src/search-filter-opensearch/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Work Guidance

- ProductListing percolator JSON may cross from `product-listing-opensearch`; ProductListing document types may not.
- Preserve complete ProductListingSearch round-trip and percolator tests.

## Deletion fence rollout and rebuild

- Expand shared boolean mapping and deploy all read filters before tombstone writers. Retire old physical-delete writers first; they can erase the durable fence. Client cancellation is not a remote fence.
- No tombstone TTL/delete-by-query, unversioned writes, or ID/version reuse. A higher authoritative version may replace a fence; time alone may not. Index deletion or unsafe alias cutover discards protection.
- Rebuild live filters from PostgreSQL into a fresh generation, with old writers unable to target it; replay committed changes before verification/cutover. Hard-deleted filter IDs/versions are not present in current rows: safe online deletion backfill needs retained delete facts or an externally fenced generation rebuild. Coordinator owns this operational gap and worker acceptance/runbook updates.

## Verification

- `cargo check -p search-filter-opensearch`
- `cargo test -p search-filter-opensearch --all-features`
- Private `projection_race_tests.rs` runs against real `test-api` OpenSearch. A relay pauses complete writes across deletion and 65s elapsed with 60s delete GC; checks unseen IDs, duplicate deletes, query/percolation invisibility, and newer projection/stale-delete ordering. No ignored tests or mocked target acceptance.
