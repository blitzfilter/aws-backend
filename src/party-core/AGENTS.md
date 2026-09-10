# DOX

## Purpose

- Own `party-core` crate.
- Own canonical Party domain types.

## Core Design

- Domain-only crate.
- Root modules: `party`, `party_id`, `party_name`, `party_search`, `party_slug_id`, `sort_party_field`.
- `PartyId` is a strict UUIDv7-backed `pty_` object ID.
- `party::Party` has private identity, stable slug, name, and contact fields. `PartyName` trims Unicode outer whitespace, rejects blank values, and caps persisted values at 255 UTF-8 bytes without truncation. Creation derives its slug once with the backing UUID and falls back to `party-<uuid>` when slugification is empty; rename preserves it. Rehydration validates the exact persisted slug without comparing it to name.
- No events, role/type, lifecycle, merge, or address behavior.
- No dependency on `party-service` or adapters.

## Ownership

- This doc rules `src/party-core/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p party-core`
- `cargo test -p party-core --all-features`

## Child DOX Index

- None.
