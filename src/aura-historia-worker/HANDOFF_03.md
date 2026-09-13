# Iteration 03 review handoff

## Scope / model

- Baseline: `d5bd9ca854e713b0c587528f02037211b2020fd4`, `task/#1412-deployment`.
- Model: GPT-6-Astra (reported model label).
- Worker-only changes. No manifest/dependency edge added; no lockfile edit by this task. Shared schema helper and unrelated API/lockfile changes arrived concurrently from other work; left untouched. Validation uses that current shared helper/lockfile, not a standalone baseline checkout.
- No git writes, live rollout, real account/queue, Google/SES/website call, or paid service used. Existing default-off/no-live-path remains.

## Final static repair — current pass

- Two follow-up findings confirmed and repaired. Fresh independent review remains pending.
- Whole-runtime watchdog is armed at every supervision exit before either shutdown branch is polled. It is declared before the HTTP/consumer futures, so it remains alive through both joins and future destruction. Bound: **max(drain, 20s HTTP)+5s cleanup**. Consumer drain+cleanup retains its tighter deadline; parent/retained-child joins still share one 5s allowance. Confirmed Tokio teardown remains 1s or exit 1.
- Config now rejects either drain or external-stop budget above **3600s**, for every stage. Defaults 270s/300s, real-stage floors/headroom, short local tests and HTTP 20s allowance stay unchanged. Largest valid real pair is 3570s/3600s. Near-u64-max pair `18446744073709551585`/`18446744073709551615` and finite over-limit values fail parsing. Watchdog duration addition and monotonic deadline construction also use checked arithmetic, failing closed without panic.
- Exact files touched in this follow-up, relative to `src/aura-historia-worker/`: `src/main.rs`, `src/shutdown.rs`, `src/shutdown_tests.rs`, `src/lib.rs`, `src/operations.rs`, `AGENTS.md`, `HANDOFF_03.md`. No shared/dependency/lockfile or process-fixture edits. `WORKER_HTTP_DRAIN_TIMEOUT` exposes the existing library allowance to the production binary, not a test-only hook.
- Shared PostgreSQL fixture accepted independently as `7dd57256-43d2-416d-a5b8-b13910e28574`; previous fixture execution restriction is lifted for the seven `preflight_lifecycle` cases. Those seven now pass against real local PostgreSQL and loopback SDK spies. LocalStack Pro/full workspace or acceptance suites remain excluded. Earlier reports below remain history.

### Current follow-up verification

**51 unique tests passed**: 24 targeted library (7 operations + 13 HTTP + 4 retained-child ownership), 20 binary, 7 real-PostgreSQL/loopback process tests. No failures or terminal timeouts in this follow-up. Every terminal call bounded <=240s. No full library/workspace/integration-suite run; no LocalStack Pro, real cloud or paid calls. Current worktree/shared fixture used, not a standalone baseline checkout.

| Exact command | Current outcome |
| --- | --- |
| `cargo test --locked --offline -p aura-historia-worker --lib --all-features operations::tests --quiet` | 7 passed; 0.00s. Defaults/all scopes, local short budgets, real-stage headroom/max pair, near-u64-max and finite over-limit inputs. |
| `cargo test --locked --offline -p aura-historia-worker --lib --all-features http::tests --quiet` | 13 passed; 10.05s. |
| `cargo test --locked --offline -p aura-historia-worker --lib --all-features queue::consumer::tests::shutdown_tests --quiet` | 4 passed; 0.04s. |
| `cargo test --locked --offline -p aura-historia-worker --bin aura-historia-worker --quiet` | 20 passed; 25.01s. Includes isolated subprocess entry test. |
| `env -u LOCALSTACK_AUTH_TOKEN -u LOCALSTACK_API_KEY DOCKER_HOST=unix:///var/run/docker.sock AURA_TEST_POSTGRES_IMAGE=ghcr.io/aura-historia/test-postgres:pg16-pgttl-3.0.0-r1 cargo test --locked --offline -p aura-historia-worker --test preflight_lifecycle -- --test-threads=1` | 7 passed; 74.88s. Cached local PG, actual SIGINT/SIGTERM workers, no LocalStack. Expected negative-case error logs are not test failures. |
| `cargo check --locked --offline -p aura-historia-worker --all-targets --quiet` | Passed. |
| `cargo clippy --locked --offline -p aura-historia-worker --all-targets -- -D warnings` | Passed. |
| `CARGO_NET_OFFLINE=true cargo fmt -p aura-historia-worker -- --check` | Passed. |
| `git --no-pager diff --check -- src/aura-historia-worker` | Passed. |

New/extended tests in this follow-up:

- `shutdown_tests::should_exit_nonzero_within_bound_when_cancellation_or_runtime_stalls`: added `http_drop_after_consumer`, `http_poll_after_consumer`, `http_poll_immediate_after_consumer`, `http_drop_after_signal`, `http_poll_after_signal`. Consumer-first completion then stalled HTTP Drop/poll, including the first shutdown poll. Require controlled stall marker and OS exit 1 before the parent's 28s kill bound; production outer bound is 25s for these local cases.
- `shutdown_tests::should_preserve_full_twenty_second_http_drain_after_consumer_finishes`: real wall-clock subprocess, >=20s delay, successful teardown/exit 0 before 24s parent limit.
- `shutdown_tests::should_exit_nonzero_without_panicking_when_watchdog_arithmetic_overflows`: both `Duration` addition and `Instant` construction reject `Duration::MAX`, exit 1 rather than panic/hang.
- `operations::tests::should_reject_shutdown_budgets_above_one_hour_during_config_parsing`: exact reviewer near-u64-max pair, maximal drain, finite excessive drain and finite excessive stop. Existing defaults/headroom test now also accepts real 3570s/3600s.

Architecture/DOX self-review: runtime-only changes; no service/core reversal, persistence/CDC/receipt-policy change or test-only public hook. No architecture deviation. Shared fixture edits belong to the integrator and were not changed here. Seven process passes prove only the local PG/loopback-spy lifecycle/preflight cases, not Standard SQS/Sequin retention/redelivery or a live rollout. Fresh independent review remains required.

## First narrow reviewer repair — prior pass

- Continues uncommitted worker03 from implementer `eb13ad40-f695-4910-8d4a-f9e24bb01f2f`; addresses reviewer `ca3c416f-2e56-4004-8de7-f281aa5c59b4`. Independent re-review remains pending.
- Finding 1 confirmed: drain timeout aborted then awaited the parent without a bound; child cleanup began later; `tokio::main` teardown could also hang. Parent and retained-child joins now share **one 5s allowance**. An owned OS-thread deadline starts before graceful drain and caps drain+cleanup even when Tokio workers/timers stall. Explicit runtime teardown must finish within **1s** or exit 1. Fatal watchdog path avoids logging locks and Rust destructors; ordinary clean shutdown still joins work.
- Finding 2 confirmed: reconciliation used a plain `JoinSet`. It now obtains `OwnedTasks` from the same `RuntimeControl` registry as receive/receipt handlers. Outer cancellation retains the reconciliation join; shutdown cannot claim completion before both child owners are destroyed.
- No preflight, admission, receipt settlement, heartbeat, reconciliation priority/cursor/FIFO, drain/stop defaults, HTTP limits or listener-close policy changed. No dependency/manifest/lockfile or non-worker source edits. The new shutdown types stay binary-private; the task factory/type is crate-private for real normalizer use, not public for tests.
- Exact files touched by **this repair only**, relative to `src/aura-historia-worker/`: `AGENTS.md`, `HANDOFF_03.md`, `src/main.rs`, `src/shutdown.rs` (new), `src/shutdown_tests.rs` (new), `src/lib.rs` (join contract doc), `src/product_listing_raw_normalization.rs`, `src/queue/consumer.rs`, `src/queue/tasks.rs`, `src/queue/consumer_tests.rs` (test wiring), `src/queue/consumer_shutdown_tests.rs` (new). All inherited process fixtures remain untouched.

### Prior repair verification

All terminal calls bounded at <=240s; none timed out. **215 library + 12 binary tests passed**; binary count includes the subprocess entry test (no-op unless selected by its isolated parent). No Docker, service-fixture, cloud or paid calls. Tests use the current shared dependency/lockfile worktree, not a standalone baseline checkout.

| Exact command | Outcome |
| --- | --- |
| `cargo test --locked --offline -p aura-historia-worker --lib --all-features queue::consumer::tests::shutdown_tests -- --test-threads=1` | 4 passed after deterministic test setup fix. |
| `cargo test --locked --offline -p aura-historia-worker --lib --all-features --quiet` | 215 passed; 10.05s. Includes existing fairness, receipts, HTTP/admission and preflight-config tests. |
| `cargo test --locked --offline -p aura-historia-worker --bin aura-historia-worker --quiet` | 12 passed; 5.05s. |
| `cargo check --locked --offline -p aura-historia-worker --all-targets --quiet` | Passed; compile only, no process fixtures executed. |
| `cargo clippy --locked --offline -p aura-historia-worker --all-targets -- -D warnings` | Passed. |
| `CARGO_NET_OFFLINE=true cargo fmt -p aura-historia-worker -- --check` | Passed. |
| `git --no-pager diff --check -- src/aura-historia-worker` | Passed. |

Architecture self-review: runtime-only ownership/deadlines, no service/core dependency reversal, no storage/controller orchestration, no public test hooks, no payload logging or receipt-policy change. No architecture deviation. Independent review is still required.

Exact new regression tests:

- `queue::consumer::tests::shutdown_tests::should_join_active_reconciliation_and_receive_before_cancelled_cleanup_completes`: `pending_receive_first`, `pending_reconcile_first`, `held_receive_first`, `held_reconcile_first`. Real synchronous destructor barriers; cleanup stays pending until both release, both destroyed before completion; no delete, visibility release or new receive; only reconciliation invoked.
- `shutdown_tests::should_exit_nonzero_within_bound_when_cancellation_or_runtime_stalls`: `consumer_drop`, `consumer_poll`, `runtime_drop`, `runtime_blocking`, `shared_cleanup`. Isolated binary-unit **test-harness** subprocesses call production supervision/shutdown functions; one Tokio worker is synchronously blocked. Require stall-entry marker and OS exit code 1 before independent parent kill bounds. Shared-budget case spends 3s joining parent, then stalls child cleanup; must exit by the original 5s allowance, not 8s.
- `shutdown_tests::should_confirm_runtime_destruction_before_clean_teardown_returns`; `shutdown_tests::stalled_shutdown_subprocess` is the child entry.
- First focused repair run: 3/4 passed; held receipt could beat the initial reconciliation tick. Fixed only the fixture: reserve receive until reconciliation starts, then release for held cases. Focused and full reruns passed. No production fairness change.

**Prior-pass acceptance restriction (now superseded for local PostgreSQL):** Docker process fixtures were not executed while shared ownership/migration repair was pending. That ownership repair has since been accepted independently; only the seven local-PostgreSQL/loopback-spy cases are authorized in this follow-up. Fatal exits may not flush profiles/logs; cancellation never proves rollback of a remote accepted effect. Real SQS/Sequin durability is still not revalidated.

## Actual worker03 changed files (including inherited work)

All paths below are relative to `src/aura-historia-worker/`.

| File | Change |
| --- | --- |
| `AGENTS.md` | Current crate contracts, inputs, tests and rollback warning. |
| `HANDOFF_03.md` | This handoff. New. |
| `src/cdc.rs` | Private configured-destination admission query only. |
| `src/http.rs` | Private admission/state/version routes, no-store responses. |
| `src/http_tests.rs` | Allowlisted identity/state, HEAD/cache/drain/unconfigured cases. |
| `src/lib.rs` | Startup identity, declared stop budget, validation, runtime metadata and cancellation-join entry point. |
| `src/main.rs` | Strict CLI, early signals, preflight boundary, shared bounded parent/child cleanup, explicit runtime teardown, safe failure categories. |
| `src/shutdown.rs` | Binary-only shared cleanup allowance, owned OS-thread fatal deadline and confirmed runtime teardown. New in repair. |
| `src/shutdown_tests.rs` | Controlled stalled-cancellation/runtime subprocesses and clean teardown. New in repair. |
| `src/product_listing_raw_normalization.rs` | Reconciliation uses runtime-owned retained task joins; scheduling unchanged. |
| `src/operations.rs` | Release identity and real-stage budget rules/tests. New. |
| `src/preflight.rs` | Shared schema gate, source/DLQ attribute validation, bounded read-only OpenSearch identity check. New. |
| `src/queue/config.rs` | Redacted queue-config Debug. |
| `src/queue/config_tests.rs` | Debug redaction test. |
| `src/queue/consumer.rs` | Keep aborted handler/receive tasks joinable by supervisor; expose crate-private task factory for reconciliation; receipt policy unchanged. |
| `src/queue/consumer_tests.rs` | Real HTTP admission during downstream circuit failure; confirmed abort cleanup/no ack/no subsequent receive; new private test wiring. |
| `src/queue/consumer_shutdown_tests.rs` | Active reconciliation plus pending/held receive cancellation barriers; both release orders. New in repair. |
| `src/queue/mod.rs` | Private task-ownership module wiring. |
| `src/queue/tasks.rs` | Retained aborted JoinSets and confirmed cleanup test. New. |
| `tests/preflight_lifecycle.rs` | Seven actual-process tests using real local PostgreSQL and loopback SDK/endpoint spies. New. |
| `tests/process_durability.rs` | Reuse real drain case for SIGINT; real-SQS fatal drain/native-redelivery case; genuine SQLx fixture. |
| `tests/process_support/mod.rs` | Genuine SQLx migration fixture, ledger-preserving cleanup, reusable child spawn/exit/signal helpers, safe failure output. |

## Behavior / compatibility

- `/admission` reports local ingress configuration/drain admission independently of `/ready` consumer health. Consumer dependency outage does not block confirmed CDC publication. Admission is not a promise of SQS availability for the next send.
- `/version` and `/state` expose versioned allowlists only. No credentials, endpoint/queue URLs, database names, source payloads or provider errors. Router responses, including errors and HEAD, are `Cache-Control: no-store, max-age=0`. Same private listener, no public route. Listener closes on drain; not a persistent management endpoint after signal.
- All real-stage scopes now require canonical non-placeholder 40-character lowercase `COMMIT_SHA`. Parse once; explicit local stages alone may omit it. EMAIL still requires its SHA asset prefix. This checks claimed SHA syntax, not artifact provenance.
- Default drain remains 270s. HTTP remains 20s, execution remains 45s/240s, worst terminal delete settlement remains 18s. New declared external stop env defaults to 300s. All stages reject either budget >3600s. Real stages also reject drain <270s, stop <300s or stop <drain+30s; overflow rejects. Local short drain tests still work. Aborted parent and retained-child joins share a further bounded 5s inside external headroom; confirmed runtime teardown gets 1s. Independent OS-thread deadlines force exit 1 on stalls; the whole-runtime watchdog stays alive until HTTP and consumer/children terminate, with max(drain,20s)+5s allowance. External supervisor must actually supply the declared stop allowance.
- SIGINT/SIGTERM register before meaningful startup. No detached signal task. Signal stops ingress/new receives; a legitimate attempt keeps heartbeats and may complete/delete. Deadline expiry aborts consumer; parent plus retained receive/handler/reconciliation children join under one cleanup deadline, or forced OS exit prevents a hang. Drain expiry remains nonzero even when cleanup succeeds. Lost/unfinished receipts do not become Complete.
- `--check-config` parses full scoped config, verifies PostgreSQL through `platform_postgres::verify_business_schema`, checks source+DLQ attributes, and GETs OpenSearch identity only for search scopes. Search check: normal TLS/basic auth, no proxy/redirect following, 5s/64KiB bounds, OpenSearch 3.x >=3.1, no future major.
- Preflight returns before runtime composition, reconciliation/timers, receive/send/delete/visibility, Google auth/inference, EMAIL adapters or domain writes. Interrupted preflight is nonzero. Normal startup runs the same checks. Never migrates/stamps/repairs. Shared helper fails closed on non-exact/unknown SQLx history and missing required extensions/baseline.
- Ten scopes, queue/DLQ identity, custody ordering, retries, heartbeat/hold/execution limits, normalizer fairness, claims and tombstones remain unchanged. No extra consuming candidate.

## Prior implementer verification — historical, not rerun here

Prior implementer reported **223 passed** (211 library + 5 binary + 7 process), with <=240s calls and no terminal timeout. Retained below as history, not current acceptance evidence: the subsequent review identified unsafe shared fixture ownership. Current repair evidence and restrictions above supersede this section.

| Exact command | Final outcome |
| --- | --- |
| `cargo test --locked --offline -p aura-historia-worker --lib --all-features --quiet` | 211 passed, 0 failed/ignored; 10.05s. |
| `cargo test --locked --offline -p aura-historia-worker --bin aura-historia-worker --quiet` | 5 passed, 0 failed/ignored. |
| `env -u LOCALSTACK_AUTH_TOKEN -u LOCALSTACK_API_KEY DOCKER_HOST=unix:///var/run/docker.sock AURA_TEST_POSTGRES_IMAGE=ghcr.io/aura-historia/test-postgres:pg16-pgttl-3.0.0-r1 cargo test --locked --offline -p aura-historia-worker --test preflight_lifecycle -- --test-threads=1` | 7 passed, 0 failed/ignored; 75.80s. Cached local PostgreSQL, no LocalStack. |
| `cargo check --locked --offline -p aura-historia-worker --all-targets --quiet` | Passed. |
| `cargo clippy --locked --offline -p aura-historia-worker --all-targets -- -D warnings` | Passed. |
| `cargo test --locked --offline -p aura-historia-worker --test process_durability --no-run` | Compiled; no process-durability test executed. |
| `CARGO_NET_OFFLINE=true cargo fmt -p aura-historia-worker -- --check` | Passed. |
| `git --no-pager diff --check -- src/aura-historia-worker` | Passed. |

Earlier iterations: library runs passed at 207 and 210 tests; binary run passed at 4 tests. Initial new integration compile failed with 20 missing-trait-method errors (missing `IntegrationTestService` import), fixed. First two executions of the then-five-test process suite each failed 0/5: shared test fixture had no SQLx ledger; then Axum Json rejected AWS's `application/x-amz-json-1.0` media type in the spy. Fixed with real SQLx migrations, not stamping, and SDK-body extraction. Five-test rerun passed 5/5 in 68.92s before the final seven-test run. No production gate was weakened for tests.

Prior implementer reported these seven process cases (not revalidated in this repair):

1. Actual SIGINT and SIGTERM: blocked PostgreSQL work stays active, visibility heartbeats continue, terminal completion deletes, no new receive after drain.
2. Both signals with explicit local 1s drain: nonzero exit, no ack, unfinished PostgreSQL effect absent.
3. Invalid source/DLQ attributes and scoped config: no custody operations. Missing schema blocks both startup/preflight and remains unmigrated.
4. Lost receipt/heartbeat during signal drain: handler cancelled, no delete or committed effect.
5. All ten scoped preflights: schema-only read-only PostgreSQL role (no business access), invalid Google credentials; exactly two attribute calls per scope, exactly three GET-only search checks, no listener/reconciliation/domain writes.
6. Both signals during blocked startup attributes: no consumer/listener; interrupted check is failure, ordinary startup stops cleanly.
7. Wrong search major, redirect and unauthorized response: fail closed, no redirect following or consumption.

## Skipped / acceptance gaps

- Shared local PostgreSQL fixture ownership is now independently accepted. Real `process_durability` remains excluded: it starts LocalStack Pro with possible external/license calls, which this task forbids. Its SIGINT drain and native redelivery cases still need authorized isolated execution. Existing real source->Sequin->SQS receipt/DLQ durability was not re-proven here. Loopback spies are explicitly not SQS retention/redelivery evidence.
- No live PostgreSQL TLS, OpenSearch mapping/plugin or provider permission/inference validation; preflight does not claim these. Real external stop configuration and immutable image/manifest binding remain operator work.
- No full workspace library rerun (prior 240s timeout, explicitly forbidden), infra/deploy tests, real AWS/GCP/SES smoke, coverage instrumentation or rollout.
- Shared helper source landed and compiled during work; no pending compile dependency remains in this worktree. Integrator must include it with the worker changes.
- Skill-referenced `docs/durable-worker-runbook.md` is absent in this checkout. No out-of-scope docs were created/changed.

## Operator inputs

- Bind the worker listener privately; keep all operational routes off public ingress.
- Supply actual release `COMMIT_SHA` matching immutable manifest/image; do not treat version response as provenance attestation.
- Supply existing full scoped PG/SQS/search/provider configuration. Grant read-only schema gate access to catalogs/public schema/SQLx ledger and GetQueueAttributes on source+DLQ. Search scopes need a reachable compatible HTTPS endpoint/auth in real stages.
- Wire declared `AURA_HISTORIA_WORKER_STOP_TIMEOUT_SECONDS` to the actual external stop setting. Default 300s supports default 270s drain. Raising drain requires stop >=drain+30s; both must be <=3600s. Shortening real-stage drain is rejected.
- Deployment runtime/catalog owners must consume `/admission` separately from consumer `/ready`; no controller wiring or extra worker candidate added here.

## Safe rollback / removal

- No migration, infrastructure or queued-job format change to undo. Never purge/recreate source/DLQ or remove tombstones for this rollback.
- Restore only this task's listed worker hunks together and remove its new files; retain other agents' API/shared-helper/lock changes. No rollback command was executed.
- **Old baseline binary ignored CLI arguments. Running `--check-config` on it can start a consumer.** Disable/co-version the preflight invocation when rolling back; never use an old worker as a non-consuming candidate.
- Coordinate removal of operational routes with deployment probes; keep external >=300s stop and existing durable custody behavior during rollback.
- Architecture review: runtime-owned operational types/transport only, no business use case or shared storage type added, no service/core dependency reversal, no controller orchestration, no new dependency/lock edges.
