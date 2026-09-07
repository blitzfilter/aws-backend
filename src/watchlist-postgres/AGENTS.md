# DOX

## Purpose

- Own `watchlist-postgres` crate.
- Own Postgres adapter for canonical watchlist ports.

## Core Design

- Implements `watchlist-service` repositories for `platform_postgres::SqlxTransaction`.
- Implements transaction-scoped `SqlxWatchlistReaderFactory` for read models and `SqlxWatchlistQuotaReaderFactory` for tier-policy invariants.
- Maps `product_listing_watchlist` rows to `watchlist-core` domain or reader views.
- Repository writes return storage-neutral persisted watchlist state with a private storage version. Ordinary updates and deletes compare the loaded version and fail conflicts; no REST view exposes it. SQL query failures retain their causes; expected version conflicts do not. Insert and update maintain `active_since` and `notifications_enabled_since` as repository-owned current-interval metadata.
- Repository and presentation reads use separate private rows; operational version never enters watchlist views.
- Notification recipient reads require active interval start at or before persisted ProductListing event time and suppress email when the current email interval started later. ProductListing withdrawal and restore preserve retained watch state, active quota occupancy, and both interval timestamps; explicit physical ProductListing deletion cascades the watch row.
- Tier reconciliation locks the user first, then affected watchlist rows; it increments version for each changed row.
- Schema key is `(user_id, product_listing_id)`. User watchlist reads order by `created DESC, product_listing_id ASC`; reverse product reads order by `user_id ASC`.

## Ownership

- This doc rule `src/watchlist-postgres/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p watchlist-postgres`
- `cargo test -p watchlist-postgres --all-features`
