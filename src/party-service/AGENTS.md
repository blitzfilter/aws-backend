# DOX

## Purpose

- Own `party-service` crate.
- Own Party use-case contracts, handlers, and outbound ports.

## Core Design

- Depends on `party-core`, shared `application` contracts, and `user-service` admin policy.
- Root modules: `ports`, `use_cases`.
- `search_parties` owns the admin Party collection query, application summary, authorization, and transaction-scoped search-reader call.
- Create, update, delete, and internal details use cases own transaction scope through `application::transaction::UnitOfWork` and transaction-scoped Party repository factories. Delete authorizes admin-or-internal callers before transaction, locks the Party, checks ListingSource and Partnership blockers, then version-deletes only unused Party state.
- Party mutations and internal details require the established admin-or-service/system policy in the service layer.
- Repository writes return storage-neutral Party state. Search returns application-owned summaries through a purpose-specific reader; no use case rereads after a write.
- No SQLx or transport dependency.

## Ownership

- This doc rules `src/party-service/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p party-service`
- `cargo test -p party-service --all-features`

## Child DOX Index

- None.
