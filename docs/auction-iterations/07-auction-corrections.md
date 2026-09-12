# Iteration 07 — Auction corrections

**Status:** implementation complete; full worker process-durability verification pending

## Objective

Add administrator ProductListing Auction-context correction, a listing-owned override barrier, and safe release.

## Delivered

- Initial-schema policy, correction-audit, release-audit, and raw-floor rows.
- One independent auction-policy version. Missing policy is version `0`.
- Full context replacement or removal. Correction requires a restricted 1–1,024 byte reason, expected listing/policy versions, expected current resolved Auction, admin authority, active listing, and same-source Auction membership.
- Correction activates the barrier even when domain facts are unchanged. Policy-only work emits no ProductListing event and does not advance listing/current/projection revisions.
- Raw context `SET` and `CLEAR` preserve the corrected context while active. Pre-release raw captures remain blocked after release through global capture generation plus linked-stream durable floor rows. Unrelated raw facts still normalize.
- Direct typed partner context writes on an existing protected listing fail atomically.
- Admin endpoints:
  - `GET /api/v1/admin/product-listings/{productListingId}/auction-context`
  - `POST /api/v1/admin/product-listings/{productListingId}/auction-corrections`
  - `DELETE /api/v1/admin/product-listings/{productListingId}/auction-override`
- Context reads and mutations return `Cache-Control: no-store` and `ETag: "plv-{listing}-apv-{policy}"`. Correction/release require the exact strong `If-Match` value.

## Non-goals

No crawler extraction, public Auction/catalogue browsing, Auction summary hydration, Auction-ID search, Auction deletion, reoffer occurrence model, or generic policy administration.

## Verification

Passed:

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::result-large-err
cargo depgraph-check check
cargo check --workspace
cargo test --workspace --lib --all-features
cargo test -p product-listing-service --all-features
cargo test -p product-service --all-features
cargo test -p product-listing-postgres --all-features
cargo test -p aura-historia-api --all-features
cargo test -p aura-historia-worker --lib --all-features
cargo test -p aura-historia-worker --test product_listing_raw_normalization --all-features
```

OpenAPI was parsed with PyYAML after correcting the Auction-context `If-Match` parameter description.

Pending verification:

```text
cargo test -p aura-historia-worker --all-features
```

The command exceeded its 10-minute bound in the unrelated `process_durability` integration suite after its worker library and focused raw-normalization tests passed. Several timing-sensitive process-durability cases had failed before the timeout. It needs isolated follow-up; do not claim the full worker suite passed.

No shared database, remote data, queue, or deployment was reset. This changes the initial business schema. A matching development checkout needs an explicitly authorized disposable-environment reset before use.
