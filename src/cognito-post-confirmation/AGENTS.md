# DOX

## Purpose

- Own `cognito-post-confirmation` crate.

## Core Design

- Cognito trigger that finishes user setup after signup.
- Main neighbors: `application`, `platform-observability`, `platform-postgres`, `user-service`, `user-postgres`.
- Event/runtime edge crate. Build the canonical issuer from event region and user-pool ID, preserve opaque Cognito `sub`, and call `RegisterCognitoUserUseCase` under `Principal::System`; Postgres is canonical user truth.
- Cognito may redeliver or overlap. Same issuer/subject/email is idempotent; mismatched replay, email conflict, and unresolved service failures stay retry-visible.

## Ownership

- This doc rule `src/cognito-post-confirmation/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- If trigger, retry, env var, queue/topic, or side effect change, update `infra/` and test wiring too.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Bootstrap thin. Push reusable work into service or domain crate.
- Be clear about event source, idempotency, and side effects.

## Verification

- `cargo check -p cognito-post-confirmation`
- `cargo test -p cognito-post-confirmation --all-features`

## Child DOX Index

- None.
