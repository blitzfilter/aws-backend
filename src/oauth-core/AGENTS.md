# DOX

## Purpose

- Own canonical OAuth domain types.

## Core Design

- Hold OAuth client, OAuth client search, authorization-code, and third-party exchange-code domain types. Third-party exchange grants retain their issued access-token identity so revoking that token also invalidates its one-time exchange code. OAuth client search carries optional exact client identity and name-query values for bounded admin reads.
- OAuth aggregates keep only domain state. PostgreSQL owns timestamps and storage versions; persisted application views expose those operational values.
- No persistence, API, Lambda, or service orchestration.

## Ownership

- This doc rule `src/oauth-core/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read repo root, `src/AGENTS.md`, then here before edit.
- Update this doc when domain contract changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep rules here. Keep storage elsewhere.

## Verification

- `cargo check -p oauth-core`
- `cargo test -p oauth-core --all-features`

## Child DOX Index

- None.
