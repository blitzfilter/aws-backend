# DOX

## Purpose

- Own canonical OAuth use cases and outbound ports.

## Core Design

- One OAuth use-case module per command/query.
- `ports/` owns transaction-scoped OAuth aggregate repositories, including one-time code repositories that only insert or atomically consume, plus purpose-specific client details/list readers returning `OAuthClientView` and persisted client write metadata, and a narrow client-authentication reader that exposes secret hash material only to OAuth service handlers. The admin client list reader is bounded, supports exact ID/name search, and returns a deterministic created/client-ID cursor.
- OAuth client metadata updates and deletion are admin-only: user and delegated-user principals require the persisted `ADMIN` role, while delegated callers also require `access-tokens:write`; update responses never expose or rotate the client secret. Deletion atomically revokes OAuth-issued access tokens and authorization/exchange codes through PostgreSQL cascades.
- OAuth token issue/revoke flows compose public User repository contracts in the same PostgreSQL transaction.
- OAuth token issue/revoke flows compose public User repository contracts in the same PostgreSQL transaction. OAuth authorization derives identity from `OperationContext`; delegated callers need `access-tokens:write` and may request only scopes present on their credential. Client registration requires delegated `access-tokens:write` plus the persisted `ADMIN` role for user and delegated-user principals. Client detail and collection reads require delegated `access-tokens:read` plus the persisted `ADMIN` role for user and delegated-user principals; collection reads also use bounded deterministic cursor search.
- No HTTP, Lambda, or storage records.

## Ownership

- This doc rule `src/oauth-service/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read repo root, `src/AGENTS.md`, then here before edit.
- Update this doc when use case, port, auth behavior, or dependency edge changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- No god service. Split use cases.
- No secret payload logging.

## Verification

- `cargo check -p oauth-service`
- `cargo test -p oauth-service --all-features`

## Child DOX Index

- None.
