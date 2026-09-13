# DOX

## Purpose

- Own `watchlist-service` crate.
- Own watchlist use cases and outbound ports.
- Exposes watch, list, update, and unwatch ProductListing use cases.

## Core Design

- Depends on `watchlist-core`, shared `application` ports, and ProductListing and FX read contracts.
- Write use cases own transactions.
- Persistence hidden behind repository factory.
- Repository writes return persisted watchlist state with a service-owned storage version; ordinary updates and deletes compare the loaded version and fail conflicts without retry. Version is never a REST/result field. Query failures retain causes; expected conflicts do not.
- List uses transaction-scoped ProductListing watchlist-details and FX snapshot readers for one PostgreSQL cursor page. After its authoritative read transaction commits, it batch-hydrates resolved Auction summaries through the Auction service read port; missing resolved IDs are integrity errors, while unresolved contexts remain unresolved. ProductListing details return canonical `ProductListingUserState`, including notification state, directly. Service applies the ProductListing pricing presentation policy: one latest FX snapshot for all current valuations and one batch lookup for sale valuation snapshots. Missing or invalid FX data fails explicitly.
- Watchlist pagination uses `created DESC, product_listing_id ASC`; the cursor contains both values so tied creation times cannot skip or duplicate product_listings.
- ProductListing views are public `application::personalized::Personalized` ProductListing-service contracts. Watchlist owns orchestration, authorization, canonical user-state retention, and hidden-listing redaction.
- Watchlist writes require `watchlist:write`.
- Create locks the authoritative user tier, checks an existing entry, then locks the authoritative ProductListing lifecycle in the same transaction before quota and write. Update locks ProductListing lifecycle only for inactive-to-active reactivation, after tier lock and before quota; retained withdrawn-watch notification, deactivation, and active-idempotent operations bypass it. Missing listing is not found; withdrawn listing is unavailable. Tier reconciliation locks user first, then affected rows, and increments changed watchlist versions. Quotas are Free 20, Pro 100, Ultimate unlimited.
- Watchlist list reads require owner/service/system access and delegated `watchlist:read`.

## Ownership

- This doc rule `src/watchlist-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p watchlist-service`
- `cargo test -p watchlist-service --all-features`
