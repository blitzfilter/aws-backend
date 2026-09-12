# DOX

## Purpose

- Own scheduled process runtime.
- Trigger service use cases. Own no business rule or service port.

## Core Design

- `cron_tab` triggers only. Aura owns overlap, timeout, panic handling, shutdown drain, and status.
- UTC schedules only. Scheduled and `--run-once` execution share the guarded timeout, panic containment, overlap, and terminal-outcome path. Preserve job error sources.
- Start health before scheduler; mark ready only after scheduler starts. Stop accepting work, stop scheduler, then drain active jobs when shutdown, health, or scheduler fails.
- Observe scheduler termination. SIGINT and SIGTERM start graceful shutdown.
- Runtime wiring composes adapters. No `aura-historia-worker` or `common` dependency.
- `SEARCH_FILTER_PERIODIC_MATCH_CRON` is a validated seven-field UTC expression. `PERIODIC_MATCH_MAX_RUN_SECONDS` must be positive.

## Ownership

- This doc rules `src/aura-historia-cron/**`.
- Parent: `src/AGENTS.md`.

## Work Guidance

- Keep runtime glue thin.
- Never queue an overlapping tick.
- Do not log job payloads, credentials, or secrets.
- Emit `cron.scheduler.started`, `cron.scheduler.drained`, `cron.job.started`, and `cron.job.completed`. Job completion needs `job`, `outcome`, and `duration_ms`.

## PostgreSQL startup

- Periodic-match wiring uses `platform-postgres::PostgresPoolConfig::from_lookup`. Required: `STAGE`, `POSTGRES_SSL_MODE`, `POSTGRES_HOST`, `POSTGRES_DATABASE`, `POSTGRES_USERNAME`, and exactly one of `POSTGRES_PASSWORD` / `POSTGRES_PASSWORD_FILE`.
- `dev`/`prod` require `verify-full` plus `POSTGRES_SSL_ROOT_CERT` (PEM CA file). Only explicit `local`/`test`/`ephemeral` may use `disable`; no stage or TLS default. Password files require Unix mode `0400` or `0600` and no final symlink.
- Port defaults to `5432`; positive max connections defaults to `2`. Unlike other cron numeric inputs, PostgreSQL numbers no longer trim whitespace. Shared parsing rejects malformed/non-Unicode inputs and unsupported ambient `PGSSLCERT`, `PGSSLKEY`, `PGSSLROOTCERT`, `PGOPTIONS`; lookup must forward them. Application name is `aura-historia-cron`; config/connect causes stay typed and redacted.
- Private `postgres_config_tests` need no database. `src/postgres-test-ca.crt` is public test-only CA material; never deploy it. Runtime fixtures need explicit local stage/TLS. Rollout wiring remains out of this slice; default-off stays off.

## Verification

- `cargo check -p aura-historia-cron --all-targets --all-features`
- `cargo test -p aura-historia-cron --all-features`

## Child DOX Index

- None.
