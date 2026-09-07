# DOX

## Purpose

- Own `user-postgres` crate.
- Own canonical User SQLx adapters for Postgres.

## Core Design

- Depends on `user-core`, `user-service`, `domain-primitives` version errors, pure `money`/`localization` values, and shared `platform-postgres` UoW primitives.
- Exports public SQLx factories only.
- Keeps SQL rows, SQL, mapping, repositories, and readers private.
- Readers and repositories bind to caller-owned transactions through service factory ports.
- `SqlxUserTierEntitlementsFactory` locks `users` with `FOR UPDATE` first, then locks eligible watchlist rows before newest-first quota ranking in the same transaction. Changed watchlist rows increment internal storage versions, so stale ordinary watchlist writes conflict.
- `SqlxUserAdminReaderFactory` implements the transaction-bound admin mutation guard; it takes an advisory transaction lock, locks the target user, and protects the final active (not suspended) admin before demotion or deletion. Suspended admins are unavailable for admin authorization.
- User repository writes use `RETURNING` and expose only storage-neutral persisted user state; delete returns row-existence only.
- `insert_if_absent` uses `ON CONFLICT (user_id) DO NOTHING` and returns the existing aggregate for idempotent `CreateUser` replay; email conflicts still fail.
- PostgreSQL owns user suspension and access tokens. User writes durably bind suspension; `SqlxUserAuthenticationReader` reads only suspension for user authentication flows. Repositories rehydrate hashed-token aggregates inside caller transactions; focused pool-backed readers return details/list/authentication models without exposing token hashes. The access-token authentication reader excludes suspended accounts. The bounded admin metadata reader binds to the caller transaction, selects safe metadata plus private creation-cursor state, validates target-user scope through the service flow, and never selects token secrets or hashes. Authentication readers include token identity and origin only for service-side credential flows. The access-token repository also performs target-user-scoped bulk deletion and reports affected rows; unrelated users remain untouched.
- User search sort maps `Name` to `first_name`, then `last_name`; no score sort.

## Ownership

- This doc rule `src/user-postgres/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- Update this file when crate contract, dependency edge, SQL shape, or factory exports change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep adapter types private.
- Map rows with `TryFrom`; never leak SQLx row types.
- Preserve SQLx and row-mapping failures as error sources in service port errors.
- Integration tests mirror dedicated impl files one-to-one; duplicate helpers inline, no shared test support modules.

## Verification

- `cargo check -p user-postgres`
- `cargo test -p user-postgres --all-features`
- `cargo test -p user-postgres --tests` runs real Postgres integration tests split by implementation file.

## Child DOX Index

- None.
