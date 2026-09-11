# Iteration 01 — pure Auction domain and shared time values

## Gate

**PASS** — pure-core capability only. Stop after this record.

## Objective and non-goals

Objective: add a deterministic, independent Auction aggregate and shared precision-bearing Auction time values.

Non-goals: no Auction persistence, service, resolver, HTTP route, raw field, ProductListing state, listing membership, schema, migration, projection, or reset action.

## Baseline and prerequisite

- Prerequisite handoff: [00 inventory](00-inventory.md), **PASS**.
- Starting SHA: `5e8836d26426e9a64a5332a550b099e4ba2062ba`.
- Starting worktree: clean.
- Resulting state: uncommitted implementation/documentation diff; no commit was created.

## Implemented behaviour

- Added `auction-core` with strict UUIDv7 `AuctionId` (`auc_`), immutable `AuctionKey`, and opaque `SourceAuctionId`.
- Added validated `AuctionName`, sanitized plain-text `AuctionDescription`, and explicit `ReportedCatalogueLotCount`; zero remains a meaningful report.
- Added exact canonical `AuctionFormat`, `AuctionReportedStatus`, and Auction event-type codes. Unknown and noncanonical codes reject.
- Added `AuctionTime` with exact-instant or source-date precision and canonical IANA `AuctionTimeZone` validation. Date values never gain an invented midnight instant.
- Added `AuctionSchedule`, which validates only comparable same-precision bounds against the declared scheduled end.
- Added private-field `Auction` creation/rehydration/mutation behavior and coalesced `AUCTION_DISCOVERED`/`AUCTION_CHANGED` payloads. Same-value and net-zero changes create no changed payload; rehydration emits none.
- Registered `auc` in `docs/object-ids.md`; added workspace and dependency-graph ownership.

Later-only capability remains absent: persistence and admin behavior (02–03), raw rewrite (04), listing timing/context (05), membership (06), corrections (07), crawler extraction (08), reads (09–10), and search (11).

## Changed inventory

| Area | Files |
| --- | --- |
| New pure domain crate | `src/auction-core/` |
| Workspace/dependency graph | `Cargo.toml`, `Cargo.lock`, `depgraph-rules.toml` |
| Timezone support | workspace `time-tz` 2.0.0 dependency; `auction-core` validates canonical IANA names without network access |
| Existing dependency declaration | `src/domain-primitives/Cargo.toml` now declares the `time` `formatting` feature required by its existing RFC3339 serializer during narrow package builds |
| Documentation | `docs/object-ids.md`, `docs/auction.md`, `docs/auction-implementation.md`, `src/AGENTS.md`, this handoff |

## Contract and reset status

- New current contract: pure Rust types only. `AuctionId` is public TypeID text at future public boundaries and native UUID at future PostgreSQL boundaries.
- No raw/API/event-envelope/schema/index contract changed. The new pure event payload is not persisted or consumed yet.
- No reset applies. No database, index, queue, source fixture, crawler selector, or shared environment was changed.

## Verification

Environment: local workspace. No infrastructure service or shared environment used.

| Command | Result |
| --- | --- |
| `cargo check -p auction-core --all-targets --all-features` | PASS |
| `cargo test -p auction-core --all-features` | PASS — 27 unit tests |
| `cargo clippy -p auction-core --all-targets --all-features -- -D warnings` | PASS |
| `cargo fmt --all -- --check` | PASS |
| `cargo depgraph-check check` | PASS |
| `cargo check --workspace` | PASS |
| `cargo test --workspace --lib --all-features` (300s first attempt) | Timed out during a pre-existing long-running OpenSearch race test after cold compilation; no failure observed. |
| `cargo test --workspace --lib --all-features` (600s rerun) | PASS |

No PostgreSQL, OpenSearch, API, worker, or reset rehearsal is applicable to this pure iteration.

## Acceptance coverage

- ID-only Auction creation and stable ID/key under metadata changes.
- Strict valid/wrong-prefix/malformed/bare/non-v7 Auction ID paths.
- Source key trimming, byte caps, and NUL/blank rejection.
- Exact canonical enum parsing and unique codes.
- Exact/date precision, IANA timezone validation, and no date-to-midnight conversion.
- Comparable schedule rejection, mixed/unknown date-context preservation, discovery folding, no-op/net-zero coalescing, and rehydration without events.

## Audit

- No compatibility generation, alias, dual write, fallback decoder, transition migration, stub, source resolver, or future-only ProductListing field added.
- No service, ProductListing, adapter, transport, infrastructure, or clock dependency in `auction-core`.
- No architecture deviation found. Event payload variants are boxed only for bounded in-memory layout; this is not a wire or compatibility contract.
- Next iteration: **02 — Auction persistence, metadata protection, and write services**.
