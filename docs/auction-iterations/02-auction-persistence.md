# Auction iteration 02 — persistence

**Status:** PASS.

## Objective

Add standalone source-scoped Auction persistence and internal admin service operations. Keep Auction separate from ProductListing until membership iteration 06.

## Included

- `auction-service` ports plus authenticated-admin create, update, and detail use cases.
- `auction-postgres` root repository, bounded schedule rows, journal appender, metadata-protection audit storage, and details reader.
- Initial business-schema tables: `auctions`, `auction_schedule_points`, `auction_events`, `auction_metadata_policy_audits`, and `auction_metadata_field_protections`.
- Root Auction CAS and per-field protection policy. An equal admin touch advances storage policy/version but writes no false Auction event.
- Fill-only embedded metadata policy for later resolver integration.
- ListingSource deletion blocker for retained Auctions.

## Explicit non-goals

No Auction HTTP route or public read; no listing auction context, membership resolution, raw contract, crawler extraction, ProductListing event/search change, CDC route, correction barrier, or Auction delete/lifecycle.

## Contract notes

- `auctions` is keyed uniquely by `(listing_source_id, source_auction_id)` and restricts ListingSource deletion.
- Schedule rows are Auction-owned and changed only with the root Auction write.
- The durable journal accepts `AUCTION_DISCOVERED` and `AUCTION_CHANGED` with schema value `1`; no consumer is added here.
- Administrator field touches protect a closed-world field code and write restricted audit state. Audit actor labels are validated before storage.
- A direct initial-schema rewrite requires a local disposable PostgreSQL recreate via the established test harness or an explicitly authorized local reset. No shared reset, queue purge, deployment, or remote action was performed.

## Verification

Passed:

```sh
cargo fmt --all -- --check
cargo depgraph-check check
cargo check --workspace
cargo clippy --locked --workspace --all-targets --all-features -- \
  -D warnings -D clippy::result-large-err
cargo test -p auction-core --all-features
cargo test -p auction-service --all-features
cargo test -p auction-postgres --all-features
cargo test -p listing-source-service --all-features
cargo test -p listing-source-postgres --all-features
```

Real PostgreSQL coverage verifies schedule/event/policy persistence, source-key uniqueness, root CAS, bounded schedule replacement, rollback, and ListingSource deletion blocking. Service fakes verify authorization, commit/no-commit behavior, expected-version handling, no-op behavior, equal policy-only touches, and failure rollback.

Also passed after a longer bounded run:

```sh
cargo test --workspace --lib --all-features
```

The full workspace library suite completed successfully. Iteration 02 is `PASS`; iteration 03 may begin.
