# DOX

## Purpose

- Own `fxrate-service` capture use case and FX capability ports.

## Core Design

- Capture uses canonical `money::Currency` quotes before a short PostgreSQL transaction.
- Write port inserts one immutable snapshot idempotently by source event ID.
- Repository rehydrates immutable snapshots and inserts them; its factory binds aggregate lookup/write work to a caller transaction. `FxRateSnapshotReader` is a separate ordinary read capability for presentation reads; it returns immutable semantic snapshots only. `CachedFxRateSnapshotReader` is a service-owned public-search-only decorator: exact IDs use a bounded immutable FIFO payload map, while latest selection has a fixed monotonic 30-second reuse bound and cutoff checks. It caches no missing/error result and never serves stale-on-error; all non-search consumers use direct fresh readers or transaction-scoped repositories. Each search lookup emits one safe cache-operation event with component/outcome and aggregate wait/backend durations only; no FX IDs or quote data are logged.

## Ownership

- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- No SQLx, provider DTO, or adapter import.

## Verification

- `cargo check -p fxrate-service`
- `cargo test -p fxrate-service --all-features`
