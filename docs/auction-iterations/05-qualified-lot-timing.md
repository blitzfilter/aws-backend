# Auction iteration 05 — qualified lot context and timing

## Gate

**PASS** — direct replacement of ambiguous ProductListing auction timestamps. Stop before iteration 06.

## Objective and non-goals

Objective: add an optional asserted listing-owned auction context with lot label, catalogue position, and precision-bearing lot timing across current ProductListing contracts.

Non-goals: no `AuctionId` membership, source auction reference, resolver/create path, shared Auction metadata, correction/override policy, Auction directory/catalogue, public Auction reads, or Auction-ID search.

## Delivered

- `ProductListing.auction` is `Option<ProductListingAuction>`.
  - `None` means no reliable auction-participation assertion.
  - `Some` may be empty and remains distinct from `None`.
- Context has optional opaque `LotNumber`, positive one-based `CataloguePosition`, and optional `LotAuctionTiming`.
  - `bidding_opens` and `scheduled_closes` retain `INSTANT` or source `DATE` precision plus optional validated IANA timezone.
  - `reported_closed_at` is an exact source assertion only.
  - Comparable exact bounds and same-timezone dates reject opening after scheduled close; date-only values never become midnight.
- Raw values use the current schema-`1` nested `auction` patch. Flat Aura timestamp fields are removed and rejected.
  - Invalid optional timing is isolated: other fields complete, context is preserved, and `AUCTION_TIMING_INVALID` is durable on `APPLIED`/`NO_CHANGE`.
  - Raw outer `CLEAR` preserves an existing context; it does not detach it.
  - `NORMALIZER_VERSION` is `3` because normalization interpretation changed.
- Partner create accepts absent/null context as no assertion. Partner update/upsert omit to preserve or send a complete context replacement; `auction: null` rejects because a later correction owns retraction.
- Initial business schema directly owns listing context/timing rows. PostgreSQL readers/history preserve `None` rather than fabricating an empty context.
- ProductListing events/history, OpenSearch documents/percolation, mappings, and saved filters use lot-specific names. Exact search ranges are half-open; date-only timing is excluded from exact OpenSearch fields. No Auction ID is indexed.
- Crawler no longer generically extracts timing from unqualified selectors. It emits `auction: UNCHANGED`; fixture-backed source-specific extraction remains iteration 08.

## Development reset boundary

The initial business schema, ProductListing event payload, current raw values, partner payloads, crawler schema fixtures, saved-filter payloads, and ProductListing OpenSearch mapping changed directly. Old disposable development rows/messages/indexes/fixtures must not run with this checkout. Under explicit authorization, stop matching processes, discard only incompatible local development data through established tooling, recreate current fixtures, then restart matching producers and consumers. No shared reset, queue purge, deployment, push, merge, or remote mutation was run.

## Verification

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `cargo check --workspace` | PASS |
| `cargo test -p product-listing-core -p product-listing-normalization -p product-listing-service -p product-service --all-features` | PASS — core 150; service 168; normalizer 372; product-service 9 |
| `cargo test -p product-listing-postgres --test product_listing_raw_normalization --all-features` | PASS — 13 real PostgreSQL tests |
| `cargo test -p product-listing-postgres --lib --all-features` | PASS — 68 |
| `cargo test -p aura-historia-api --lib --all-features` | PASS — 301 |
| `cargo test -p crawler --all-features` | PASS — unit and integration suites |
| `cargo test -p product-listing-opensearch -p search-filter-core -p search-filter-service -p search-filter-postgres -p search-filter-opensearch --lib --all-features` | PASS — includes real OpenSearch fence tests |
| `python3 -c "import yaml; yaml.safe_load(open('docs/swagger.yaml'))"` | PASS |

| `cargo depgraph-check check` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::result-large-err` | PASS |
| `cargo test --workspace --lib --all-features` | PASS |
| `git diff --check` | PASS |

No final gate was skipped. Real PostgreSQL and OpenSearch suites above use the isolated test harness; no shared infrastructure was changed.

## Audit

- No compatibility DTO, old decoder, alias, raw schema version, event version, transition migration, dual write, or backfill was added.
- Common journal and worker-envelope versions remain unchanged.
- PostgreSQL remains authoritative; OpenSearch is rebuildable and external-version fenced.
- No context reader maps absence to a fabricated empty object.
- No membership, Auction key, source URL/name matching, auction metadata, or auction-execution feature was added.

## Next iteration

**06 — source-key membership and transactional resolution.** It must build on this current context; do not reintroduce the retired timestamp fields.
