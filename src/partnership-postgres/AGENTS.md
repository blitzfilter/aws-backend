# DOX

## Purpose

- Own PostgreSQL repositories/readers for Partnership and PartnershipApplication.

## Core Design

- `partnerships` is one Party-scoped aggregate table.
- `partnership_applications` persists validated proposal JSON, state codes, approval result IDs, and optimistic version; submitted proposal data has no Party or ListingSource FK.
- Membership and ListingSource grants use idempotent join-table writes. Membership writes report whether a row was added, removed, or already in the requested state; ListingSource grant writes report whether a row was added or already existed; revocation of an absent membership or source grant is a successful no-op. Source-grant removal reports `Removed` or `AlreadyAbsent` and deletes only `(partnership_id, listing_source_id)`. A source grant belongs to `(partnership_id, listing_source_id)`; authorization requires both a user membership and that Partnership grant.
- The Partnership repository supports ID lookup for service-owned membership grants; rows and membership outcomes stay adapter-private.
- Rows/mappings remain private; public factories bind caller-owned SQLx transactions.
- Admin application search uses one bounded keyset query with `(created|updated, partnership_application_id)` continuation, exact state/proposal/source filters, and inclusive timestamp ranges.
- Admin partnership search uses one bounded joined reader query with `created DESC, partnership_id DESC` keyset pagination; filters use `EXISTS` so member and source-grant counts stay complete. The `partnerships_created_id_idx` index must match that order.
- Admin Partnership detail uses one joined reader statement with UUID-ordered, SQL-limited member and ListingSource reference arrays (100 each), empty-array decoding, and complete association counts; it performs no N+1 reads.

## Verification

- `cargo check -p partnership-postgres`
- `cargo test -p partnership-postgres --all-features`
