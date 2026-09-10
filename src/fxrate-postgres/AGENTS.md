# DOX

## Purpose

- Own `fxrate-postgres` SQLx FX snapshot repository.

## Core Design

- PostgreSQL is authoritative for immutable snapshots and quotes. Canonical capture serializes inserts and rejects retroactive or tied capture times except duplicate source IDs.
- Rows and SQL stay private; persisted currency strings map to canonical `money` values. Insert and all quote rows use one caller transaction.
- Repository maps checked persisted rows into core snapshots; its factory binds aggregate work to caller-owned short or write transactions. `SqlxFxRateSnapshotReader` owns a `PgPool` and uses one statement for each ordinary immutable-snapshot presentation read; it shares canonical persisted-state validation with the repository.

## Ownership

- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- No product ownership here.

## Verification

- `cargo check -p fxrate-postgres`
- `cargo test -p fxrate-postgres --all-features`
