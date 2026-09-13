# Iteration 09 — Auction summary reads

**Status:** PASS

## Objective

Add safe current Auction summaries to existing ProductListing detail, search, similar, and watchlist reads without an Auction index, metadata fan-out, public Auction routes, or write-path change.

## Delivered

- `auction-service` owns `AuctionSummaryBatchReader` and its safe read model: Auction ID, optional localized name, format, reported status, and precision-bearing schedule.
- `auction-postgres` implements the port with one bounded root query and one bounded schedule query for all unique requested IDs. Existing strict Auction row/schedule mapping validates persisted state.
- ProductListing detail, search, similar, and watchlist presentation batch-resolve only resolved membership IDs. A missing resolved Auction is an integrity failure; no-context and unresolved contexts remain distinct.
- Existing listing API DTOs expose optional `auctionSummary`. It does not expose source keys, policy/audit data, evidence, or persistence tokens.
- OpenSearch documents and listing write/event/projection contracts are unchanged. Search hits can contain eventually consistent listing facts while the summary is read from current PostgreSQL state.

## Non-goals

No public Auction directory/detail/catalogue endpoints, Auction-ID search filter, saved-search change, Auction OpenSearch index, cache, metadata fan-out, or schema/raw/event replacement.

## Verification

```text
cargo fmt --all -- --check                                                       PASS
cargo depgraph-check check                                                       PASS
cargo check --workspace                                                          PASS
cargo test -p auction-postgres --all-features                                    PASS (5 tests)
cargo test -p auction-postgres -p product-listing-service -p watchlist-service \
  -p aura-historia-api --lib --all-features                                     PASS (523 tests)
cargo clippy --locked -p auction-service -p auction-postgres \
  -p product-listing-service -p watchlist-service -p aura-historia-api \
  --all-targets --all-features -- -D warnings -D clippy::result-large-err       PASS
python3 -c "import yaml; yaml.safe_load(open('docs/swagger.yaml'))"             PASS
cargo test --workspace --lib --all-features                                     PASS
```

No shared database, queue, index, or remote environment was reset. This iteration changes only read code and REST response shape; a matching checkout is required for API consumers.

## Audit

No compatibility reader, version, alias, dual write, stub route, schema change, or out-of-scope Auction execution feature was added. The next iteration is 10 — public Auction browsing.
