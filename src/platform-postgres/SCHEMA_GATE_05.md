# Task03 gate / task05 foundation evidence

Baseline: `d5bd9ca854e713b0c587528f02037211b2020fd4`, SQLx exactly `0.9.0`.
Original task03 scope: shared business startup verification only, no writer/adoption/activation. Later runtime wiring and isolated fresh-only `bootstrap-local` modes are recorded in `docs/deployment/implementation-status.md` and `src/crawler/AGENTS.md`. Owner defers incremental migration/adoption/backfill machinery; the historical task05 proposals below are not current implementation requirements.

## Task03 interface and limits

`platform_postgres::verify_business_schema(&PgPool) -> Result<(), PostgresSchemaError>`.
`PostgresSchemaError::code()` is safe to log; all exposed error-chain formatting redacts provider data. See `AGENTS.md` for codes, grants and deadlines.

- Exact compiled up-history: version + successful application + SQLx checksum. Descriptions, timestamps and execution durations are not schema identity.
- Installed public extensions and 33 persistent baseline table names. No business-row reads, DDL, mutation, history creation or stamping.
- Read-only repeatable-read snapshot; bounded rows/checksum payloads and query/lock/overall deadlines. No migration advisory lock. Snapshot checks cannot serialize or attest later DDL.
- Unknown future migrations block old binaries, including claimed additive changes. Compatible-superset evidence/runtime protocol remain deferred by the owner's dev-stage scope override. Do not replace this with `set_ignore_missing(true)`.
- Not a full schema drift audit: no attestation of column types, constraints, indexes, triggers/function bodies, extension versions, preload configuration or TTL-worker health. Database/catalog administrators remain trusted.
- Read-only plan code may reuse this exact-current startup check, but it is not a pending-migration planner. A fresh database is a rejection here, not authorization to stamp or migrate.

## Pinned SQLx source facts

Local source prefix: `/home/jbruder/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. Paths below are relative to that prefix. These are observed 0.9.0 APIs, not proposed API names.

| Fact | Evidence |
| --- | --- |
| `sqlx::migrate::Migrate::lock(&mut self)` / `unlock(&mut self)` return `BoxFuture<Result<(), MigrateError>>`; implemented directly for `PgConnection`. | `sqlx-core-0.9.0/src/migrate/migrate.rs:55-62`; `sqlx-postgres-0.9.0/src/migrate.rs:186-220` |
| Lock is blocking, session-scoped `pg_advisory_lock`; unlock uses `pg_advisory_unlock`. Both derive key from current database, not migration table/stream. Key = `0x3d32ad9e * CRC32_ISO_HDLC(database_name)`. Unlock implementation discards the returned boolean. | `sqlx-postgres-0.9.0/src/migrate.rs:186-220,362-374` |
| No timeout argument/setter in these lock APIs. Set PostgreSQL session `lock_timeout` / `statement_timeout` on the dedicated connection before locking; keep an outer deadline. Do not use transaction-local settings before a later nontransactional operation. | `sqlx-core-0.9.0/src/migrate/migrate.rs:55-71`; PostgreSQL implementation above; private real-PG lock test in `src/schema_tests.rs` |
| `Migrate::apply(&mut self, table_name: &str, migration: &Migration)` executes one migration. SQLx 0.9 history methods also require `table_name`: `ensure_migrations_table`, `dirty_version`, `list_applied_migrations`. | `sqlx-core-0.9.0/src/migrate/migrate.rs:35-71` |
| `Migrator::run` calls hidden `run_direct(None, conn, false)`: lock, optional schema creation, **ensure history table**, dirty/known-history checks, checksum validation/apply, then unlock. Early errors return before explicit unlock. It is not read-only plan/verify. | `sqlx-core-0.9.0/src/migrate/migrator.rs:176-183,225-295` |
| `Migration.no_tx` is true only when SQL text `starts_with("-- no-transaction")`: exact case at byte zero; no trim/BOM handling, and no whole-line validation. This marker itself participates in the checksum. | `sqlx-core-0.9.0/src/migrate/source.rs:225-245`; `sqlx-core-0.9.0/src/migrate/migration.rs:86-87` |
| PostgreSQL `apply` wraps SQL + essential history insert in one transaction unless `migration.no_tx`. It commits, then separately updates execution time (nanoseconds); a post-commit timing-update error can report failure after successful migration. | `sqlx-postgres-0.9.0/src/migrate.rs:222-264,315-339` |
| Critical gap: PostgreSQL executes migration SQL **before** inserting `success = TRUE, execution_time = -1`. It does not first insert `success = FALSE`. Nontransactional effects or SQL-success/history-insert failure can exist with **no ledger row**. Never treat missing history as proof of no effects or automatically retry it. | `sqlx-postgres-0.9.0/src/migrate.rs:230-242,315-339` |
| `dirty_version` reads the first `success = false` row. `list_applied_migrations` reads every version/checksum without filtering success. Dirty detection needs the separate check; transactional failures normally roll back both SQL and history. | `sqlx-postgres-0.9.0/src/migrate.rs:146-183,234-242` |
| `no_tx` removes SQLx's wrapper, not PostgreSQL's own simple-query batching semantics. SQLx executes the script through one simple-query message when no bind arguments exist; it does not split statements. Concurrent-index scripts/multi-statement partial effects need real-PG acceptance tests. | `sqlx-postgres-0.9.0/src/migrate.rs:320-323`; `sqlx-postgres-0.9.0/src/connection/executor.rs:282-290`; `sqlx-postgres-0.9.0/src/connection/mod.rs:132-138` |
| Default checksum is SHA-384 over exact UTF-8 SQL bytes (48 bytes). Whitespace/newlines/comments matter. Resolver can explicitly ignore characters, changing identity; no `sqlx.toml` found in this workspace. Macro and runtime resolver share code; macro embeds checksum bytes and `include_str!` SQL. | `sqlx-core-0.9.0/src/migrate/migration.rs:41-58,86-87`; `sqlx-core-0.9.0/src/migrate/source.rs:92-145,256-262`; `sqlx-macros-core-0.9.0/src/migrate.rs:61-75,91-115` |
| Default unknown applied version => `VersionMissing`; changed checksum => `VersionMismatch`; false success => `Dirty`. `set_ignore_missing(true)` bypasses the unknown-version rejection, not a compatibility proof. | `sqlx-core-0.9.0/src/migrate/migrator.rs:129-133,249-255,272-275,363-379` |
| Existing files are tracked by `include_str!`; directory tracking is conditional on unstable macro configuration. A new migration must reliably trigger rebuilding binaries embedding the gate; task05 should own a build `rerun-if-changed` solution or equivalent clean-build guarantee. | `sqlx-macros-core-0.9.0/src/migrate.rs:61-62,123-133` |

## Repository evidence

- Business baseline: `migrations/20260725090000_initial_business_schema.sql:1-2` creates `pg_trgm`/`unaccent` in public. Lines `998-1012` require pre-provisioned `pg_ttl_index`; lines `1160-1169` register four absolute-expiry indexes. No no-transaction marker at byte zero. SQL stays immutable.
- `docs/storage.md:38-47`: pg_ttl is asynchronous cleanup, not credential-expiry correctness; provision/preload before baseline.
- Crawler: six independent migrations under `src/crawler/migrations/`; initial `20260101000000_initial_schema.sql:1` requires `pgcrypto`. `src/crawler/src/local_db/schema.rs:43-76` reads the crawler ledger using its own `sqlx::migrate!("./migrations")`; it now rejects extra successful versions too (strict-preflight slice), in its own bounded read-only snapshot. Do not conflate the separate ledgers.
- `src/test-api/src/postgres.rs:291-329` lexically replays raw SQL, **not** SQLx migration bookkeeping. Existing fixtures cannot satisfy the new startup gate without fixture-owner work. Lines `132-165` describe the pg_ttl image/preload/worker setup. This task reuses the image reference, not that process-global lifecycle.
- Metadata source is `deploy/control/src/contracts/release.ts:49-67,257-280`: two streams, transaction mode, positive bounded lock/statement timeouts, extensions/capabilities, SHA-384 history checksum, SHA-256 SQL/artifact/evidence identities and declared compatibility. `primitives.ts:10` specifies `sha384:` plus 96 lowercase hex characters. Structural parsing/evidence digests are not evidence verification.
- Dedicated safe connection boundary already exists: `src/platform-postgres/src/config.rs:316-326`, `PostgresPoolConfig::connect_session()`. Do not reconstruct raw URL/options or use a transaction-pooling endpoint for session locks.

## Historical task05 boundary proposal — deferred by owner

1. Migration composition root owns explicit **business or crawler** target, trusted artifact/metadata inputs, approved credentials and one dedicated shared-TLS session per active target. Keep per-stream histories separate. No hidden cross-database transaction.
2. Private read-only planner reads bounded catalog/history snapshots without ensure/DDL and distinguishes fresh, missing/dirty/changed/unknown, pending and exact states. It must not interpret existing tables as an adoptable baseline. Keep startup's exact interface/policy until compatible-superset acceptance is reviewed.
3. Apply-only path sets reviewed nonzero session deadlines, takes SQLx's direct lock, rechecks the plan under that same session/lock, and provisions history only with explicit fresh-database authorization. Use direct `apply(table_name, migration)` only after enforcing source/metadata/checksum/transaction-mode checks. Do not use hidden `run_direct` or `skip` for adoption.
4. Own lock cleanup on every error/cancellation: explicit unlock on normal exits plus dedicated-session close fallback. No pooled session carrying a migration lock. Treat disconnect/timeout/post-commit failure as possibly committed until reconciled.
5. Before allowing nontransactional migrations, approve durable in-progress/failure evidence and repair-forward policy covering SQLx's no-dirty-row gap. Define crash/resume ownership and index-invalid/partial-effect checks. Default block until that protocol exists; never rewrite applied SQL or infer a stamp.

### Deferred upgrade machinery acceptance / unresolved risks

- Real isolated PG: competing direct locks, lock timeout, unlock/disconnect, process death, statement timeout, apply success, transactional rollback, nontransactional/concurrent-index partial failure, successful SQL with failed ledger/timing update, retry/reconciliation, and no writes/history creation in plan/verify.
- Compile/artifact tests: exact-byte checksums including CRLF/BOM/comments; byte-zero marker vs metadata mismatch; ascending unique versions; two ledgers; new-file rebuild invalidation; old/new binaries against reviewed supersets; evidence identity **and** trusted evidence verification.
- Explicit operator policy still needed: fresh vs legacy/adopted database, migration credentials/session endpoint, extension provisioning/versions/preload/health, timeout budgets, evidence trust, maintenance/backfill/rollback permissions and nontransactional recovery. Runtime/test-fixture owners must wire the gate and produce genuine histories, not inferred stamps.
- This task makes no nontransactional migration writes/probes and proves no crash-recovery protocol. Pinned-source inspection identifies the gaps; task05 must test and resolve them before enablement.

## Task03 review handoff

Actual files changed by this task: `src/lib.rs`, `src/schema.rs`, `src/schema_tests.rs`, `src/schema_fixture.rs`, `AGENTS.md`, this evidence file; all under `src/platform-postgres/`. No manifest/dependency/TLS/fixture-ownership/applied-SQL edits. Concurrent API/worker and root-lock changes were observed and left untouched.

Validation from workspace root; every invocation bounded to 120 seconds or less:

| Command | Final outcome |
| --- | --- |
| `cargo check --locked --offline -p platform-postgres --all-targets --all-features` | Passed. |
| `cargo test --locked --offline -p platform-postgres --all-features --lib` | 145 passed, 19 ignored, 0 failed; 5.23s test execution. Includes duplicated existing guard tests. |
| `cargo test --locked --offline -p platform-postgres --all-features --lib schema::tests::should_ -- --ignored --test-threads=1` | 12 passed, 0 failed; 36.37s. Every successful real-PG fixture explicitly cleaned its owned container/network IDs. Direct lock probe proved SQLSTATE `55P03`, explicit unlock, disconnect release, and no ledger creation. |
| `cargo clippy --locked --offline -p platform-postgres --all-targets --all-features -- -D warnings` | Passed. Test-only `duplicate_mod` expectation keeps the existing guard unchanged. |
| `cargo fmt --package platform-postgres -- --check` | Passed. |
| `git --no-pager diff --check -- src/platform-postgres` | Passed. Protected baseline SQL, crate manifest, config/TLS/fixture files also had no diff. |

Earlier targeted positive test found invalid qualified substring syntax; fixed to `pg_catalog.substr`, then positive and full local suite passed. Initial Clippy duplicate-module failure was resolved with the explicit test-only expectation, not a fixture ownership/visibility change. A broader protected-path diff returned 1 because another agent added API edges to `Cargo.lock`; this task did not alter/revert them.

Skipped: prior-timeout workspace lib suite (explicit instruction), unrelated workspace/infra checks, existing task02 TLS Docker cases (unchanged), task05 migration-write/crash/nontransactional tests (outside this read-only slice). Ordinary unit run leaves opt-in infrastructure/subprocess helpers ignored; schema infrastructure ran separately, helper parents ran normally.

Removal safety: gate makes no persistent schema/history/business changes; removing it needs caller/export coordination, not database rollback. Fixture cleanup only uses validated IDs returned by successful creation; no names/volumes/broad cleanup. Existing config/TLS/resource-ownership files remain unchanged.

Operator input: genuine pre-provisioned business SQLx history/extensions/baseline, shared-TLS configured business pool, public USAGE/catalog access/ledger SELECT. No new environment variables, write grants or inferred adoption policy. API/worker wiring remains with their owners. Architecture review: infrastructure-only boundary; private rows/tests; no new dependencies or public test hooks.

Model: GPT-6-Astra (reported by editor agent configuration).
