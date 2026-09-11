# Auction iteration 04 — current raw-values contract

## Gate

**PASS** — one current ProductListing raw-values shape. Stop before iteration 05.

## Objective and non-goals

Objective: replace historical V1/V2 raw-value DTOs and dispatch with one strict current contract while retaining both price parsers.

Non-goals: no Auction raw fields, membership/reference resolution, shared Auction metadata, listing context change, timing rewrite, API route, schema migration, compatibility reader, or data reset action.

## Prerequisite and state

- Prerequisite: [iteration 03](03-auction-admin-api.md), PASS.
- Starting SHA: `9d89db6920a188519f1ddea6dfccc5287c78f274` on `feat/#1465-auctions`.
- Starting worktree: clean.
- Resulting state: uncommitted implementation/documentation diff; no commit, deployment, push, merge, or shared reset was made.

## Delivered

- `ProductListingRawValues` is the sole strict UPSERT DTO. It requires `priceFormat`; `DISPLAY_TEXT` and `MACHINE_DECIMAL` retain their prior normalization semantics.
- `PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION` remains `1` and is the only accepted raw-values discriminator. The envelope field remains in capture storage and input hashing.
- The normalizer has one decoder and no V1/V2 conversion path. Missing `priceFormat`, obsolete members, and malformed current values are invalid; discriminator `2` and other non-current values are rejected before canonical work and remain pending under the established stored-schema policy.
- Crawler emits schema `1` with `DISPLAY_TEXT`. Shopify and WooCommerce emit schema `1` with `MACHINE_DECIMAL`. DELETE inputs retain the same envelope discriminator and bypass UPSERT decoding.
- Updated capture, PostgreSQL, worker, Shopify, and WooCommerce fixtures. Isolated PostgreSQL, Sequin/SQS worker, and black-box WooCommerce API tests use the current shape.
- Updated ProductListing, raw-normalization, API/OpenAPI, provider, changelog, Auction-plan, and historical inventory documentation.

## Current contract and reset boundary

The changed contract is limited to persisted/captured raw values and their producers/consumers:

| Contract | Current value |
| --- | --- |
| Raw-values discriminator | `1` only |
| UPSERT `priceFormat` | required: `DISPLAY_TEXT` or `MACHINE_DECIMAL` |
| Crawler price format | `DISPLAY_TEXT` |
| Shopify/WooCommerce price format | `MACHINE_DECIMAL` |
| ProductListing journal schema | unchanged: `1` |
| Worker SQS envelope schema | unchanged: `2` |

No table migration, backfill, dual write, alias, successor version, fallback decoder, or old reader was added. Immutable current observations remain immutable.

Old development V1 rows omit required `priceFormat`; old V2 rows use discriminator `2`. They must not be mixed with this checkout. The approved isolated test harness (`test-api::Postgres::new("migrations")`, scoped OpenSearch/SQS helpers) initialized fresh current fixtures successfully in the listed integration tests. For an authorized local development cutover, stop affected producers/consumers, discard only incompatible disposable raw rows/messages/fixtures through reviewed local tooling, recreate current fixtures, then start crawler, Shopify, WooCommerce, API intake, and normalization worker from the same checkout. No generic shared-environment reset command exists in this repository, so none was inferred or run.

## Changed closure

- Pure contract/normalizer: `product-listing-normalization`.
- Ordered raw dispatch: `product-service`.
- Producers: `crawler`, `shopify-lambda`, `woocommerce-service`.
- Persisted/consumer fixtures: `product-listing-postgres`, `aura-historia-worker`, `aura-historia-api`, and Shopify integration tests.
- Documentation: raw-normalization runbook, ProductListing contract, OpenAPI/changelog, affected crate DOX, Auction plan/status, and this record.

## Verification

Environment: local isolated test containers/services only. No shared infrastructure was reset.

| Command | Result |
| --- | --- |
| `cargo check -p product-listing-normalization -p product-service -p crawler -p shopify-lambda -p woocommerce-service` | PASS |
| `cargo fmt --all` | PASS |
| `cargo test -p product-listing-normalization --all-features` | PASS — 369 tests |
| `cargo test -p product-service --all-features` | PASS — 8 tests |
| `cargo test -p crawler --all-features` | PASS — 500 unit tests plus integration suites |
| `cargo test -p shopify-lambda --all-features` | PASS |
| `cargo test -p woocommerce-service --all-features` | PASS — 12 tests |
| `cargo test -p product-listing-postgres --test product_listing_raw_normalization --all-features` | PASS — 12 real PostgreSQL tests |
| `cargo test -p aura-historia-worker --test product_listing_raw_normalization --all-features` | PASS — 4 PostgreSQL/Sequin/SQS worker tests |
| `cargo test -p aura-historia-api --test api --all-features woocommerce_webhook` | PASS — 13 black-box API tests |
| `cargo fmt --all -- --check` | PASS |
| `cargo depgraph-check check` | PASS |
| `cargo check --workspace` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::result-large-err` | PASS |
| `cargo test --workspace --lib --all-features` | PASS |
| `grep -RInE 'ProductListingRawValuesV[0-9]+|RAW_VALUES_SCHEMA_VERSION_V[0-9]+|InvalidRawValuesV[0-9]+|raw-values V2|raw values V2|raw-values schemas V1/V2' src docs migrations opensearch` | PASS — no matches |
| `git diff --check` | PASS |

## Acceptance coverage

- Both current price formats normalize through capture/worker paths.
- All three producers emit discriminator `1` and explicit price format for UPSERTs.
- Current input hashing still includes the raw-values discriminator and full raw values; provenance remains excluded.
- Invalid current structure/missing price format is rejected; discriminator `2` is not decoded or upgraded.
- Raw stream ordering, immutable revisions, replay/idempotency, DELETE bypass, provider receipts, CDC routing, and ordinary non-auction data regressions remain covered by the affected suites.

## Audit

- No Auction behavior was added.
- No compatibility branch, old/new union, alias, stub, schema transition, migration, dual write, or reset action was added.
- Existing raw revision ordering, IDs, input hashes, ProductListing journal schema, worker envelope schema, and projection correctness fences remain.
- No raw source payload was added to ordinary logs or documentation.

## Next iteration

**05 — qualified lot context and timing.** It owns removal of the still-current ambiguous Aura auction timestamps; this iteration does not prepare or add those future fields. No shared reset, deployment, queue purge, or remote action was performed.
