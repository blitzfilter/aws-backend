# DOX

## Purpose

- Own PostgreSQL reader for the admin overview.

## Core Design

- `SqlxAdminOverviewReaderFactory` binds to the caller transaction.
- One CTE aggregate statement reads authoritative PostgreSQL tables in one MVCC statement snapshot.
- SQL row and count mapping stay private; invalid signed counts fail as invalid read models.

## Verification

- `cargo check -p admin-overview-postgres`
- `cargo test -p admin-overview-postgres --all-features`
