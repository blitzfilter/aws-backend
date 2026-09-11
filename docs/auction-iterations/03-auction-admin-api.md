# Auction iteration 03 — admin HTTP

**Status:** PASS

## Objective

Expose standalone Auction administration through the existing Auction service only:

- `POST /api/v1/admin/auctions`
- `GET /api/v1/admin/auctions/{auctionId}`
- `PATCH /api/v1/admin/auctions/{auctionId}`

## Delivered

- Added thin `aura-historia-api::auctions` controllers and state using inbound Auction use-case traits.
- Runtime and API acceptance composition use the existing PostgreSQL Auction handlers/adapters.
- Create requires source key, returns `201`, strict `auc_` identity, detail `Location`, and `Cache-Control: no-store`.
- Detail/update require administrator authority. PATCH requires positive `expectedVersion`; omission preserves state and documented `null` clears metadata/schedule roles.
- Exact enum codes, strict object IDs, date-versus-instant time precision, source timezone JSON, and unknown request-member rejection are enforced at the transport boundary.
- Added stable Auction HTTP problem mappings for not-found, conflict, temporary, and internal failures.
- Added real black-box API tests covering create/get/update, field protection, stale version, duplicate source key, invalid IDs, missing Auction, and non-admin denial.
- Updated OpenAPI, changelog, Auction documentation, API DOX, and dependency-graph policy.

## Non-goals

No public Auction routes, directory/catalogue, listing membership/context, raw ingestion, crawler extraction, listing search, correction, override policy, or Auction deletion was added.

## Verification

Passed:

```text
cargo fmt --all -- --check
python3 -c "import yaml; yaml.safe_load(open('docs/swagger.yaml'))"
cargo check --workspace
cargo depgraph-check check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::result-large-err
cargo test -p aura-historia-api --all-features              # 300 unit + 318 API tests
cargo test -p auction-service --all-features                # 9 tests
cargo test -p auction-postgres --all-features               # 3 tests
cargo test --workspace --lib --all-features
```

The API suite used the established isolated PostgreSQL/OpenSearch test harness. No shared database, remote data, queue, deployment, push, or commit was changed.

## Reset and compatibility

This iteration adds only routes over iteration 02's current Auction schema. It adds no schema/version transition, compatibility route, alias, fallback decoder, or reset requirement beyond the existing iteration 02 development-schema reset note.

## Architecture review

Reviewed against `docs/arch.md` §§18 and 20–24. Controllers authenticate and map DTOs only; service retains authorization, transactions, audit, and concurrency. API state holds inbound traits, while concrete adapters are composed only in runtime/test composition roots. No controller reads a repository/database or orchestrates stores.

## Next iteration

Iteration 04: consolidate ProductListing raw values to one current shape. Do not add Auction raw fields or membership in iteration 04.
