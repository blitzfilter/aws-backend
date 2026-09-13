# DOX

## Purpose

- Own hybrid deployment evidence, decisions, status and operator runbooks.
- Source code proves implemented behavior. Tests prove only their stated environment. Neither proves live deployment.

## Contracts

- Read root, `docs/AGENTS.md`, then here before edits.
- No live host/cloud/GitHub mutation without separate target-specific authorization.
- Keep iterations independently reviewed and committed on a non-deploying integration branch until safe activation exists.
- Record actual SHA, checks, reviewer, external gates and mixed/uncertain outcomes. Never label scaffolding deployable.
- No secrets, business payloads, receipt handles or provider bodies in examples or records.
- Application rollback never downgrades schemas, purges queues or restores backups.
- Owner steering (2026-09-12): still in development; no new incremental migrations, legacy-schema adoption, or migration/backfill machinery now. Keep existing baseline initialization for fresh installs and isolated tests. This does not authorize resetting/changing an existing database. Defer production upgrade/rollback-schema compatibility machinery, not TLS or data-custody safety.

## Index

- `inventory.md` — checkout evidence and unresolved deployed state.
- `architecture-decisions.md` — deployment boundaries, dependency/file ownership.
- `implementation-status.md` — accepted work, checks and rollout gates.
