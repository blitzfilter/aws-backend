# DOX

## Purpose

- Own scheduled process runtime.
- Trigger service use cases. Own no business rule or service port.

## Core Design

- `cron_tab` triggers only. Aura owns overlap, timeout, panic handling, shutdown drain, and status.
- UTC schedules only. Daemon and `--run-once` share execution ownership, absolute deadline, panic containment, overlap, and terminal outcomes. Judge actual future completion, not when a delayed Tokio join/timer gets polled.
- Bind private health before dependency startup in both modes. Ready only after scheduler start or run-once admission. On signal/server/scheduler failure: close admission, stop scheduler, drain owned attempts, stop health.
- SIGINT and SIGTERM handlers register before config/composition. Independent signal thread stays responsive when job polls stall. Repeat signals never reopen admission or shorten cleanup.
- Tracker owns joins and terminal outcomes; trigger/result-receiver drop cannot detach work. Retain admission through actual future destruction and cancellation join. Terminal publication and stop-admission share one lock.
- Only attempts still owned at stop-admission can fail daemon drain. Earlier terminal failures remain reported job outcomes, not poison for every later stop. Run-once always reports its terminal failure. Drain keeps all failing outcomes plus timeout count.
- Runtime errors retain opaque typed causes. `Multiple` keeps primary and additional failures; source traversal follows primary, so inspect both branches for full history. `CronErrorCause` stops raw source traversal and redacts Debug/Display; no raw accessor. Wiring uses its existing `WiringError` boundary.
- Runtime wiring composes adapters. No `aura-historia-worker` or `common` dependency.
- No business transactions, migrations, checkpoints, or retries in runtime. Services/adapters keep those boundaries.

## Ownership

- This doc rules `src/aura-historia-cron/**`.
- Parent: `src/AGENTS.md`.

## Work Guidance

- Keep runtime glue thin.
- Never queue an overlapping tick.
- Do not log job payloads, credentials, or secrets.
- Emit `cron.scheduler.started`, `cron.scheduler.drained`, `cron.job.started`, and `cron.job.completed`. Job completion needs `job`, `outcome`, and `duration_ms`.
- Process failure logs fixed categories only; never format error/source chains or panic payloads.

## Inputs and private operations

- CLI: no args = daemon; `--run-once search-filter-periodic-match`; `--check-config`. Reject other args. `AURA_HISTORIA_CRON_ENABLED_JOBS` is a comma-separated known-job set; reject empty entries, duplicates, unknown names. Daemon/check require the periodic job enabled; run-once selects it explicitly. Real-stage config still requires a nonempty set.
- `STAGE` explicitly `dev`, `prod`, `local`, `test`, or `ephemeral`. `COMMIT_SHA`: canonical non-placeholder 40-character lowercase SHA; required in dev/prod, optional only in local stages. `LOG_LEVEL` controls logging.
- **Breaking private bind:** `AURA_HISTORIA_CRON_HEALTH_BIND_ADDR` now defaults to `127.0.0.1:8082`, not public `0.0.0.0:8082`. Only literal loopback IPv4/IPv6 binds allowed (including `::1`). Reject wildcard, private-network, public, mapped IPv6, and hostname binds. No public override. Move existing health probes onto host loopback; do not publish these unauthenticated endpoints through a proxy.
- **Run-once now binds the same listener**, before composition. Occupied port fails before job execution; choose a free loopback port when daemon also runs. Check-config validates bind policy but opens no health listener.
- `/health` = liveness; `/ready` = 200 only ready, otherwise 503. `/ops/version` = schema version 1, component, stage, nullable source SHA. `/ops/status` adds lifecycle, admission, active count, execution/drain/stop/cleanup/teardown seconds. All send `Cache-Control: no-store`; ops bodies contain only validated identifiers/counts, never config payloads or secrets. These are private runtime routes, not public REST API.
- `AURA_HISTORIA_CRON_SHUTDOWN_GRACE_SECONDS` defaults 300; `AURA_HISTORIA_CRON_STOP_TIMEOUT_SECONDS` defaults 330. Positive, untrimmed integers; each capped at 3600. Stop must be at least drain + 30; dev/prod drain at least 300. Effective max drain 3570. Operator must configure the actual supervisor stop timeout to match; this input does not change the supervisor.
- `SEARCH_FILTER_PERIODIC_MATCH_CRON`: seven-field UTC expression, default `0 0 15 * * * *`. `PERIODIC_MATCH_MAX_RUN_SECONDS`: 1..7200, default 7200. **7200 execution does not fit 300 drain.** Shutdown cancels/joins unfinished attempts after grace; later attempts use service-owned lock/checkpoint/idempotency behavior, not a runtime retry loop.
- Watchdogs use OS deadlines, independent of Tokio timers. Execution/grace deadline permits at most 5s cancellation cleanup; runtime destruction has a separate 1s watchdog. Defaults leave 24s further headroom inside the 330s supervisor budget. Stuck poll/drop exits 1 without claiming stopped/released. Normal cleanup cancels and joins watchdog threads; subprocess tests kill/reap children and join output readers on every exit.

## Preflight and job inputs

- `--check-config` parses full runtime/job config, connects PostgreSQL, calls read-only `verify_business_schema`, then closes the pool. No schema repair/migrations, job, advisory lock, checkpoint writes, OpenSearch request, ADC construction/token refresh, or Vertex call. Normal startup uses the same schema gate before adapter composition. Missing/non-green schema is an error, not permission to mutate it.
- Required job inputs: `OPENSEARCH_ENDPOINT_URL`, `VERTEX_AI_PROJECT_ID`, `VERTEX_AI_LOCATION`, `VERTEX_AI_MODEL`; dev/prod also require `OPENSEARCH_USERNAME` and `OPENSEARCH_PASSWORD`. No actual credentials needed by private fake-job tests.
- Job numeric defaults: `PERIODIC_MATCH_FILTER_PAGE_SIZE=100`, `PERIODIC_MATCH_HYBRID_SCAN_LIMIT=100`, `PERIODIC_MATCH_EVALUATION_LIMIT=50`, `PERIODIC_MATCH_LLM_CONCURRENCY=8`, `PERIODIC_MATCH_MAX_ATTEMPTS=3`, `PERIODIC_MATCH_PROJECTION_LAG_SECONDS=900`, `PERIODIC_MATCH_REPLAY_OVERLAP_SECONDS=7200`. Positive counts; hybrid limit at most 100, evaluation at most hybrid limit, attempts at most 10; lag/overlap allow zero.
- Job text/numbers retain trim behavior. Defaults only for absent inputs; present empty, malformed, or non-Unicode schedule/numeric inputs fail before dependency I/O. Wiring error causes remain typed and redacted.

## PostgreSQL startup

- Periodic-match wiring uses `platform-postgres::PostgresPoolConfig::from_lookup`. Required: `STAGE`, `POSTGRES_SSL_MODE`, `POSTGRES_HOST`, `POSTGRES_DATABASE`, `POSTGRES_USERNAME`, and exactly one of `POSTGRES_PASSWORD` / `POSTGRES_PASSWORD_FILE`.
- `dev`/`prod` require `verify-full` plus `POSTGRES_SSL_ROOT_CERT` (PEM CA file). Only explicit `local`/`test`/`ephemeral` may use `disable`; no stage or TLS default. Password files require Unix mode `0400` or `0600` and no final symlink.
- Port defaults to `5432`; positive max connections defaults to `2`. Unlike other cron numeric inputs, PostgreSQL numbers no longer trim whitespace. Shared parsing rejects malformed/non-Unicode inputs and unsupported ambient `PGSSLCERT`, `PGSSLKEY`, `PGSSLROOTCERT`, `PGOPTIONS`; lookup must forward them. Application name is `aura-historia-cron`; config/connect causes stay typed and redacted.
- Private `postgres_config_tests` need no database. `src/postgres-test-ca.crt` is public test-only CA material; never deploy it. Runtime fixtures need explicit local stage/TLS. Rollout wiring remains out of this slice; default-off stays off.

## Verification

- Keep commands bounded to 240s, locked/offline: `cargo check -p aura-historia-cron --all-targets --all-features --locked --offline`; `cargo test -p aura-historia-cron --lib --all-features --locked --offline`.
- Signal slice: `cargo test -p aura-historia-cron --lib --all-features --locked --offline process::tests:: -- --test-threads=1`. Covers 18 real SIGINT/SIGTERM scenarios (daemon/once completion, failure, cancel, stuck poll/drop, idle, startup) plus 14 private-process bind/budget negatives. Fake inbound service only; no database.
- Beside-code tests cover 20ms execution / 200ms OS-controlled blocking poll with one Tokio worker, retained admission during destructor join, active vs historic drain failures, opaque typed startup causes, compound cleanup failures, private bind policy, and 300/330 headroom/3600 caps. Source/error formatting tests use synthetic canaries, never secrets.
- Ignored `signal_child` is a parent-owned fixture, not standalone validation. Wiring's ignored binary matrix requires a separately built production binary; do not run ignored tests indiscriminately.
- Existing unit/fake-service OS-process tests alone are **not PostgreSQL advisory-lock/release, rollback, or checkpoint evidence**. No workspace library suite, cloud daemon, paid call, secret use, or new migration in this bounded slice.
- Iteration04 real-PG suite: `AURA_CRON_ISOLATED_LOCAL_POSTGRES=1 cargo test -p aura-historia-cron --test postgres_reliability --all-features --locked --offline -- --test-threads=1 --nocapture`. Run from a clean environment; reject ambient PG connection options and image overrides. Accepted PG run: **14 passed, 0 failed, 1 ignored** (19.69s; before the P3 cleanup tests). Ignored `custody::custody_child` is parent-owned only; never blanket-run ignored tests.
- `tests/postgres_reliability.rs` groups three private support modules. Uses test-api's guarded Postgres bootstrap via `get_postgres_client`, not raw-SQL migration replay. Cached shipped `pg16-pgttl-3.0.0-r1` pin only, fixed local Docker socket, `--pull=never`, exact acquired-ID cleanup. Fixture publishes **0.0.0.0** for gateway compatibility: isolated local host only. One process-owned container, fresh template0 database per case, actual `sqlx::migrate!` baseline/ledger. No stamping, adoption, incremental/upgrade migration, or backfill.
- Preflight proof: **13 actual binary launches** through `env!(CARGO_BIN_EXE_aura-historia-cron)` (valid baseline, four broken-schema cases, eight strict-config negatives). Cleared child env; explicit test stage/TLS disable; read-only role gets public USAGE and ledger SELECT, no business SELECT/DML/DDL or advisory-function EXECUTE. Compare public data/schema catalogs before/after; verify no sessions/locks remain. Invalid ADC, occupied health bind, loopback search/metadata/proxy tripwires; no cloud credentials or paid service. Tripwires are observation, not an OS egress firewall.
- Custody proof: public `ScheduledJobRunner`/`ActiveExecutionTracker` plus public SQLx lease; **seven private subprocess runs**, synthetic job writing a real baseline `users` row in a real transaction. Independent sessions exclude contenders; explicit release, lease drop, and SIGKILL permit reacquire. Timeout/cancel retain local admission and PG resources through gated future destruction; check commit/rollback and closed sessions after join/pool close while child is still alive. Internal destruction handshake uses notifications, not Tokio timer polling. Stuck-drop watchdog exits 1 without claiming join; OS disconnect rolls back/releases.
- Limits: not the production periodic-match handler, daemon/signal wiring, service UnitOfWork/checkpoint/idempotency, or production TLS proof. Lease-connection-loss negative proves **no distributed fencing**: successor acquires while old transaction can still commit. Controller must confirm old source stopped. No visibility widened. Independent review a07af53f accepted PG proof; P3 reader cleanup has separate focused coverage.
- P3 cleanup validation: integration `--no-run` passed; `cargo test -p aura-historia-cron --test postgres_reliability --all-features --locked --offline fixture::tests:: -- --test-threads=1 --nocapture` passed **2 tests** (0.20s). Both error-first/panic-first cases retain a controlled blocked reader until join; success preserves output order. No PG rerun for this output-reader-only change.
- Cleanup: parent children killed/reaped; all output readers joined before returning aggregated error/panic counts, never raw failure payloads. Pools explicitly closed; owned databases dropped without FORCE, then owned roles dropped. Container cleanup stays with test-api's acquired-ID guard. Never remove by name/prefix, guessed ID, volume, or prune; cleanup failures report exact owned ID. Local database/temp names use `cron_i04_` plus PID/time/counter; names alone grant no cleanup authority.

## Child DOX Index

- None.
