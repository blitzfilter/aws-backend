# DOX

## Purpose

- Own `search-filter-postgres` crate.
- Own Postgres adapter for canonical search-filter ports.

## Core Design

- Implements `search-filter-service` repositories for `platform_postgres::SqlxTransaction`; structural JSON storage types own persisted shape while local codecs map ProductListingSearch semantic leaves to canonical domain types and preserve their values.
- ProductListing search JSON stores `listing_source_id_query` and `exclude_listing_source_id_query` as UUID sets. It has no Shop, seller, type, country, continent, or geo filter keys. It also stores optional `availability_query` with canonical exact `ListingAvailability` and derived `ListingOrderability` codes plus `include_unspecified`; state and lifecycle query fields are absent.
- Implements ordinary `SqlxSearchFilterReader` for read models, `SqlxSearchFilterIndexReader` for complete versioned index reads, and focused transaction-scoped active-candidate, monthly notification-rank quota, match-write, and typed match-notification source reader factories.
- Maps `search_filters` and `search_filter_matches` rows, including nullable paired `CURRENT`/`EVENT`/`SALE` price-match FX provenance. Invalid partial or unknown persisted provenance fails mapping.
- Advisory-lock sessions use shared verified PostgreSQL configuration, a 5s connection deadline, and redacted SQLx causes; their extra connection is outside the pool cap.
- Owns focused periodic-match candidate, existing-match, progress, and dedicated-session advisory-lock adapters. Candidates use the closed window end; progress SQL can only advance a checkpoint. The final progress lock holds the `search_filters` row and revalidates its `ACTIVE` state, selected version, and selected progress before match writes or checkpoint advancement. Periodic state remains separate from ordinary Search Filter views.
- Final ProductListing-event match candidates are batch-read in filter-ID order with `FOR SHARE OF filter` on authoritative `ACTIVE` rows, held through match commit. Fallible semantic search mapping and exact embedding equality must match the evaluated `expected_search`/`expected_embedding`; no whole-row version comparison. Search/embedding changes, deactivation, or deletion suppress stale candidates. Unrelated name/notification edits remain eligible; return the current name. This lock is separate from periodic progress/version fencing.
- Repository writes return storage-neutral persisted search-filter state.
- ProductListing ID is enough for listing references; no `product-listing-service` dependency.

## Ownership

- This doc rule `src/search-filter-postgres/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p search-filter-postgres`
- `cargo test -p search-filter-postgres --all-features`
