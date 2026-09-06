# DOX

## Purpose

- Own `user-service` crate.
- Own canonical User use-case contracts, handlers, and outbound ports.

## Core Design

- Depends on `user-core` identifiers and values, pure `money`/`localization` values, and shared `application` contracts.
- Root modules: `ports`, `use_case_bundle`, `use_cases`.
- `use_cases::authorization` owns shared service-layer admin actor policy helpers.
- Admin actor checks use transaction-scoped `UserAdminReader::find_admin_actor`, not controller checks.
- Own-user reads and admin-user reads are separate use cases: `GetOwnUserUseCase` and `AdminGetUserUseCase`.
- Operational handlers use `application::transaction::UnitOfWork` and transaction-scoped repository/reader factories.
- User read/search/update/delete use cases authorize self where allowed, service/system, or admin actor in service layer; admin-only handlers require the persisted admin role.
- Repository writes return persisted user state; handlers must not read after write for responses.
- Ports are public because adapter crates implement them.
- `UserTierEntitlements` locks one authoritative user row and reconciles tier-restricted search filters and watchlist entries inside the caller transaction; it avoids a User-service dependency on either resource service.
- Admin role-removal and user-deletion commands use a transaction-bound `UserAdminMutationGuard` so PostgreSQL can protect the last active administrator.
- Access-token writes use an `AccessTokenRepositoryFactory` inside a service-owned `UnitOfWork`; details/list use presentation readers, while bounded admin metadata listing uses a transaction-bound purpose-specific reader and validates the explicit target user in the same transaction. `AccessTokenAuthenticationReader::find_authentication_by_hashed_token` returns only an authentication model. Self deletion checks ownership; admin-targeted single and bulk deletion check the persisted admin role in the same transaction. Bulk deletion validates the explicit target user in that transaction, deletes only that user's rows, and returns the affected count for operational logging. The repository's same-key lookup is only for transactional aggregate mutation.
- `AuthenticateAccessTokenUseCase` only validates token existence/expiry and returns token scopes; protected use cases enforce credential capability via `OperationContext`.
- Port errors carry boxed sources for adapter/read-model failures; do not swallow underlying causes.
- No SQLx, OpenSearch, or transport dependency.

## Ownership

- This doc rule `src/user-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, dependency edge, or use-case boundary changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep orchestration here. Keep rules in `user-core`.
- Keep adapters outside.
- Keep unit tests inside the use-case file that owns the handler. No shared test-support module.

## Verification

- `cargo check -p user-service`
- `cargo test -p user-service --all-features`

## Child DOX Index

- None.
