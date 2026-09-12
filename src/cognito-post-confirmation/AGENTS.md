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

## PostgreSQL startup

- Bootstrap uses `platform-postgres::PostgresPoolConfig::from_lookup`. Required: `STAGE`, `POSTGRES_SSL_MODE`, `POSTGRES_HOST`, `POSTGRES_DATABASE`, `POSTGRES_USERNAME`, and exactly one of `POSTGRES_PASSWORD` / `POSTGRES_PASSWORD_FILE`.
- `dev`/`prod` require `verify-full` plus `POSTGRES_SSL_ROOT_CERT` (PEM CA file). Only explicit `local`/`test`/`ephemeral` may use `disable`; no stage or TLS default. Password files require Unix mode `0400` or `0600` and no final symlink.
- Port defaults to `5432`; positive max connections defaults to `2`. Shared parsing rejects malformed/non-Unicode inputs and unsupported ambient `PGSSLCERT`, `PGSSLKEY`, `PGSSLROOTCERT`, `PGOPTIONS`; lookup must forward them. Application name is `cognito-post-confirmation`; config/connect errors retain safe typed causes.
- Binary `postgres_config_tests` need no Lambda runtime or database. `src/postgres-test-ca.crt` is public test-only CA material; never deploy it. Runtime fixtures need explicit local stage/TLS. Infra rollout remains out of this slice; default-off stays off.

## Verification

- `cargo check -p cognito-post-confirmation`
- `cargo test -p cognito-post-confirmation --all-features`

## Child DOX Index

- None.
