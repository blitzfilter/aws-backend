# DOX

## Purpose

- Own `party-postgres` crate.
- Own Party SQLx adapters for PostgreSQL.

## Core Design

- Depends on `party-core`, `party-service`, and shared `platform-postgres` transaction mechanics.
- Exports the public SQLx Party repository and bounded search-reader factories.
- Keeps SQL rows, SQL, mapping, and scoped repository/reader implementations private.
- Repository methods bind to caller-owned transactions, map persisted Party state with `TryFrom`, and enforce optimistic version updates. Party deletion locks the row, reads typed ListingSource/Partnership blockers, then makes a version-checked Party-only delete; restrictive Party FKs backstop both references. The Party search reader uses bounded keyset pagination and maps rows to service summaries. The initial business schema maintains adapter-only Party `name_search` with a trigger; it is never aggregate or admin-read state.
- Integration tests use the business schema through `test-api`.

## Ownership

- This doc rules `src/party-postgres/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p party-postgres`
- `cargo test -p party-postgres --all-features`
- `cargo test -p party-postgres --tests`

## Child DOX Index

- None.
