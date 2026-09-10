# DOX

## Purpose

- Own `search-filter-core` crate.
- Own canonical Search Filter domain types.

## Core Design

- Domain-only crate.
- Owns strict UUIDv7-backed `sf_` `UserSearchFilterId`, `UserSearchFilterName`, `EnhancedMatchReason`, and `SearchFilterState`.
- Reuses canonical `user-core::UserId`, `product-listing-core::product_listing_search::ProductListingSearch`, and neutral event/outcome values from `domain-primitives`.
- Aggregate has no persistence timestamps.
- No legacy, service, adapter, transport, or runtime dependency.

## Ownership

- This doc rule `src/search-filter-core/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p search-filter-core`
- `cargo test -p search-filter-core --all-features`
