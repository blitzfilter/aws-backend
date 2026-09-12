# DOX

## Purpose

- Own private async worker runtime, strict Sequin CDC ingress, and Standard SQS transport for #1558.
- PostgreSQL owns business truth. OpenSearch owns rebuildable projections. SQS owns queued work and native DLQs, not business state.

## Ownership

- This doc rules `src/aura-historia-worker/**`. Parent: `src/AGENTS.md`.
- Read root, src, and here before edit. Worker implements no service port or business use case.
- Runtime-local queue traits are private. Service use cases still own transactions, durable idempotency, and source guards.

## Shape

- `main.rs`: scoped adapter composition, AWS credential chain, redacted startup categories, eagerly registered SIGINT/SIGTERM, consumer supervision, bounded drain.
- `shutdown.rs`: binary-only shared parent/child cleanup and whole-runtime OS-thread fatal deadlines; checked deadline arithmetic and explicit confirmed Tokio teardown. `shutdown_tests.rs` uses isolated test-harness subprocesses, no service fixtures.
- `preflight.rs`: binary-only read-only startup checks. No service handlers or mutable startup hooks.
- `lib.rs`: typed startup config (drain/stop <=3600s), explicit runtime composition, Axum server entry points. `WORKER_HTTP_DRAIN_TIMEOUT` shares the real HTTP allowance with the binary supervisor.
- `operations.rs`: immutable parsed release identity, real-stage lifecycle budget validation, allowlisted operational metadata.
- `cdc.rs`: existing strict ProductListing v1 event validation and scoped routing. Whole batch and destinations prevalidated before first publication.
- `jobs.rs`: compact worker jobs, canonical IDs, positive versions, logical-key validation. Existing `cdc::*` job exports remain compatible.
- `wire.rs`: private version-two SQS DTO. Envelope has `schema_version: 2`, canonical scope, TypeID object fields and logical keys, explicit SCREAMING_SNAKE_CASE `job_type`, and compact `payload`. Schema 1 is poison; no compatibility decoder.
- `queue/`: typed queue config, read-only startup attribute checks, Standard SQS transport, shared receipt lifecycle, retained cancellation joins (`tasks.rs`), private failure tests.
- `http.rs`: `/health`, `/ready`, `/admission`, `/state`, `/version`, `/cdc/sequin`; private listener only, bounded owned Hyper HTTP/1 connections. Private HTTP tests live under `src/`.
- Scope modules map jobs to existing inbound use cases and translate results to transport dispositions. No local correctness cache or in-process job DLQ/retry loop.

## Production composition

- `SqsQueueConfig::new(scope, queue_url: Url, region: String, stage: String, local_endpoint: Option<Url>)` validates configuration.
- `SqsQueue::new(aws_sdk_sqs::Client, config).await` validates source queue and DLQ; preserves supplied credentials, pins region/endpoint and SDK bounds.
- `SqsQueue::from_config(config).await` uses the workspace AWS default credential chain.
- `WorkerRuntimeComposition::with_sqs(client, config).await` or `from_sqs_queue(validated_queue)` creates ingress and its supervised receiver.
- `into_parts()` returns `(WorkerRuntime, WorkerQueueReceiver)`. Existing `consume_*_queue` functions accept `impl Into<WorkerQueueReceiver>` and the existing scope use-case trait object. Raw normalization also takes its existing shutdown watch receiver.
- `WorkerRuntime::shutdown()` stops ingress acceptance and new receives. Running attempts drain within scope budget. Main bounds aborted parent join and `join_cancelled_tasks()` together at 5s, not 5s after an unbounded parent join. Retained children include receive/receipt handlers and raw reconciliation. Cleanup exhaustion forces nonzero OS exit.
- Explicit legacy in-memory queue constructors remain for existing test composition. `WorkerRuntimeComposition::build` is in-memory only; production never calls it. `WorkerRuntime::default()` is unconfigured, not an implicit in-memory fallback.
- No generic inbox, outbox, processed-job, or worker persistence table.

## Queue contract

- Required: `AURA_HISTORIA_WORKER_SCOPE`, `AURA_HISTORIA_WORKER_QUEUE_URL`, `AWS_REGION`, `STAGE`, and scoped service dependencies below. Existing local scope default remains only for `ephemeral`, `local`, `test`; queue/region/stage never default.
- Queue name: `aura-worker-<scope>-<stage>`. DLQ: `aura-worker-<scope>-dlq-<stage>`.
- Production URL: regional HTTPS SQS URL with twelve-digit account and exact name. ARN must match region, account, scope, and stage.
- Explicit `AWS_ENDPOINT_URL_SQS` accepted only in `ephemeral`/`local`/`test`, never production. Queue URL and explicit endpoint must share exact origin; fixtures canonicalize provider-generated URLs, runtime never rewrites them. Generic `AWS_ENDPOINT_URL` is rejected by env config. Typed constructors likewise need explicit local endpoint. SQS-specific region, endpoint resolver, non-FIPS/non-dual-stack settings and operation bounds override supplied client settings; credentials remain supplied.
- Source retention 7 days; DLQ retention 14 days; native `maxReceiveCount=5`; source long-poll attribute 20 seconds; source/DLQ Standard, encrypted, and covered by unconditional deny-insecure-transport policies. Wildcard and NotPrincipal Allow grants fail startup, even when conditional. AWS's absent `FifoQueue` means Standard; `true` is rejected. Source redrive allow is `denyAll`; DLQ is `byQueue` with exactly the intended source ARN and no onward redrive.
- Visibility: 300s for normalization, percolator, embedding, translation; 360s for delivery; 60s for other scopes. Execution: 240s slow/delivery, 45s short. Notification stays below its five-minute PostgreSQL lease.
- Startup reads attributes only. No queue creation, attribute mutation, custom DLQ publication, FIFO group/dedup fields, or local fallback.
- SDK connect/read/attempt/operation bounds: 3/25/30/55s, at most two SDK attempts. Transport wraps receives at 27s and other operations at 5s. Long poll is 20s.

## CDC and wire

- HTTP body <=1 MiB, changes <=100, jobs <=500. Existing discovery fanout is at most five jobs per change. Sequin subscriptions must honor these limits; oversized immutable source rows require an upstream delivery strategy, not silent truncation.
- At most 16 accepted HTTP/1 sockets and 16 requests; capacity reserved before accept. Headers: 5s, 32 fields, 16 KiB parser buffer. Request deadline 10s; publication deadline 8s; whole connection including response write 20s. One request per connection. Oversized Content-Length rejects before body reads; Axum still collects and limits all complete bodies, including chunked input.
- Server owns connection tasks in a JoinSet. Shutdown drops listener, closes undispatched/partial-header sockets immediately, gracefully drains active requests, joins within 20s, then aborts/joins leftovers. Binary whole-runtime watchdog also bounds stalled HTTP cancellation joins. Owner cancellation aborts children; no detached connection tasks.
- Entire bounded CDC batch routes and checks every registered destination before any send. SQS sends are sequential and awaited. Only all-success returns 202. Mid-publication failure/deadline may leave duplicates on redelivery; source/target guards own correctness.
- Queue messages <=16 KiB encoded UTF-8 bytes; wire encoder, queue publisher and AWS send adapter each enforce the bound. Five supported payload variants: ProductListing event IDs, raw stream/revision IDs plus revision, search-filter IDs/version/operation, historical match IDs, notification delivery ID. Semantic object IDs and derived keys use TypeIDs. CDC still receives PostgreSQL UUID text and converts it immediately to typed IDs.
- Wire intentionally ignores additive unknown fields. Unknown schema/type/scope, missing fields, noncanonical/nil IDs, nonpositive/overflowing versions, and forged logical keys are poison: no handler execution and no delete.
- Logical keys preserve existing formats: `product-event:<event>` / `product:<listing>`; `product-listing-raw-revision:<revision>` / `product-listing-raw-stream:<stream>`; `search-filter:<filter>:<version>:<lowercase operation>` / `search-filter:<filter>`; `search-filter-match:<user>:<filter>:<listing>:<event>` / `user:<user>`; `notification-delivery:<delivery>` for both keys.
- Sequin delivery IDs and LSNs never own idempotency. Raw source payloads never enter SQS jobs.
- Exactly ten production scopes. Legacy `UserTierEnforcement` job types have no wire encoding, registration in `ALL`, or production CDC route.

## Consumer lifecycle

- Capacity one, reserved before receive; no prefetch. Shared receipt owner controls execution, heartbeat, visibility, and deletion. Receive explicitly requests only ApproximateReceiveCount, SentTimestamp and ApproximateFirstReceiveTimestamp. Missing/invalid numeric metadata fails closed; receipts remain unacknowledged. Receipt handles, provider bodies and sender identifiers never logged.
- Structured logs include received scope/attempt, validated epoch-ms timestamps, receive duration, execution duration, and per-publication encoded byte count/latency/outcome. Cancelled, timed-out or invalid-response publication stays acceptance-unknown, never success; cancellation logs through an owned drop guard.
- Normalizer owns one pending receive across reconciliation timer turns. One reserved receipt may wait behind one bounded reconciliation turn; heartbeat continues until explicit handoff joins the owner. Hold budget 245s; lost heartbeat/deadline skips execution and retains receipt. Never poll another receipt during CDC execution/settlement. Shutdown joins the poll owner; owner drop aborts and retains its join. Active reconciliation uses the same retained-task registry.
- Heartbeat every `min(30s, visibility/3)` while a receipt is held or executing; this cap is required for every scope. No detached heartbeat. Handler task is abort-on-owner-drop; timeout/panic/cancellation is not acknowledgment. Heartbeat failure cancels and joins handler.
- Stop heartbeat before terminal delete or retry visibility. Only explicit durable terminal outcomes delete. Failed delete retries at most three times without rerunning handler.
- Retry visibility uses exponential jitter from 30s, cap 900s. Invalid jobs remain for native redrive.
- `AlreadyClaimed { lease_expires_at }` defers to persisted expiry +5s. `ClaimDeferred { retry_after }` defers at least one second. Neither is Complete. Ambiguous send, exhausted finalization, lost lease, and timeout never acknowledge.
- Service dependency error or execution timeout opens consumer circuit with exponential pause (30–900s). Read-only SQS attribute probe precedes one half-open service attempt. Poison, missing source and active claims do not close a service circuit. Preserve any service failure completed during a failing heartbeat call.
- Receive/heartbeat/held-receipt lease/settlement failures pause transport without setting or clearing an outstanding service probe. A successful receive, including an empty long poll, restores readiness only for transport-only outages; the attribute probe alone does not. Shutdown cancels paused/probing receives. No bulk receive during outage. No separate PostgreSQL/provider health probe yet; downstream recovery is checked by that single service attempt.
- Consumer drop/death marks health/readiness failed. Dependency pause keeps liveness but clears readiness. CDC publication does not depend on consumer health. Main supervisor treats consumer exit as process failure.
- Both SIGINT/SIGTERM register before config/dependency setup; one owned watch propagates shutdown without a detached signal task. Interrupted `--check-config` is nonzero, never verification success.
- Shutdown stops HTTP/new receives and drains HTTP concurrently with the active owned execution/settlement attempt. `AURA_HISTORIA_WORKER_DRAIN_TIMEOUT_SECONDS` defaults to 270s. Deadline expiry aborts local consumer work; parent and retained children share one 5s cleanup deadline. An independent owned OS-thread deadline is armed before polling either shutdown branch and stays until HTTP plus consumer/children terminate. Its allowance is max(consumer drain, 20s HTTP)+5s cleanup; consumer drain+cleanup keeps its own tighter bound. Stalled HTTP/consumer destructors, polls or Tokio timers therefore cannot hang shutdown. Exhaustion exits 1 without waiting for destructors or logging locks. No in-memory wake-up or SQS backlog drain.
- `AURA_HISTORIA_WORKER_STOP_TIMEOUT_SECONDS` declares the external supervisor budget; default 300s. Both configured budgets must be <=3600s in every stage; larger values fail config parsing. Real stages still require drain >=270s, stop >=300s and >=drain+30s. Watchdog `Duration`/`Instant` additions are checked and fail closed if unrepresentable. External tooling must actually grant that stop budget; this env var cannot configure the OS/container supervisor. HTTP stays 20s; execution stays 45/240s; worst terminal delete settlement is 18s. Cancellation joins get 5s within external headroom. Explicit Tokio runtime teardown then gets 1s: confirm destruction or force exit 1, never silently leave stalled threads behind. Fatal exits may not flush coverage/logs; cancellation is not a remote-effect fence.
- Short drain overrides remain for explicit `local`/`test`/`ephemeral` fixtures only, not supported shortened real-stage deployments.

## Scope meanings

| Scope | CDC input | Terminal / retry meaning |
| --- | --- | --- |
| `product-listing-normalization` | raw revision inserts | Authoritative stream drain complete; capped continuation retries. Startup repair, bounded cursor/FIFO and alternating timer/CDC turns remain. Missing next revision means drained per service contract. |
| `search-filter-projection` | search_filters changes | Service target write/stale result completes. Missing upsert source is handled by versioned target tombstone in service, not an unconditional transport ack. |
| `search-filter-percolator` | supported ProductListing events | Processed/duplicate/stale/inactive/ignored complete; absent committed source retries. Service checks and rechecks current source revision. |
| `search-filter-match-notification` | match inserts | Exact historical match identity retained. Created/duplicate/quota/deleted-user/stale/withdrawn suppression completes; missing match/listing retries. |
| `watchlist-notification` | changed main price/availability | Exact event plus event-time recipients and locked current lifecycle. Applied/duplicate/ignored/withdrawn complete; missing source retries. No current-event cache. |
| `notification-delivery` | delivery inserts | Delivered/already-delivered/persisted permanent failure completes. SourceMissing completes only after service finalization. Missing delivery retries; claims defer. |
| `product-content-assessment` | discovery inserts | Guarded applied/cleared/duplicate/stale/ignored complete; absent source retries. |
| `product-embedding` | discovery or changed images | Guarded applied/duplicate/stale/ignored and authoritative missing-title no-op complete; absent source retries. |
| `product-translation` | discovery inserts | Guarded applied/duplicate/stale/ignored and authoritative missing/empty title/language no-op complete; absent source retries. |
| `product-listing-opensearch` | supported ProductListing events | Applied/version-stale/deleted complete; missing source or missing required sale snapshot retries. Projection race protection remains target adapter responsibility. |

## Operational probes and preflight

- `/admission`: 200 only with configured ingress destinations and no drain; 503 otherwise. Independent of `/ready` consumer circuit/health. Local admission is not a promise that the next SQS send succeeds; CDC still requires every confirmed publication.
- `/state`: schema-version 1 allowlist of lifecycle, admission, consumer liveness/readiness, identity and actual/declared budgets. `/version`: schema-version 1 component/scope/source SHA/local flag. No database names, queue URLs, endpoints, credentials or provider bodies. Unconfigured identity returns 503; explicit local builds without SHA return null, never a fake release.
- Every router response, including errors/HEAD/overload/drain, uses `Cache-Control: no-store, max-age=0`. Listener closes on drain; DRAINING is visible only to already-dispatched requests, not a persistent management listener. Keep all probes private; no public API route or Swagger contract.
- `COMMIT_SHA` now required for every real-stage scope: 40 lowercase hexadecimal characters; reserved placeholder hashes rejected. Parse once, never reread for probes. This validates claimed identity, not image provenance; operator must bind it to the immutable release manifest/image. Only explicit local stages may omit it. EMAIL still needs SHA for its existing asset prefix.
- No arguments starts one scoped worker. `--check-config` parses the same full scoped config, connects PostgreSQL and calls shared `platform_postgres::verify_business_schema`, then reads source/DLQ SQS attributes. Required search scopes also GET the configured OpenSearch root, with normal TLS/auth, no proxies/redirects, a 5s deadline and 64KiB response cap; require OpenSearch 3.x >=3.1 identity, not a future major.
- Normal startup runs those same checks. Shared schema gate requires exact compiled SQLx history, required extensions and baseline availability; unknown future history fails closed. No migration, stamping, repair or full schema-drift attestation. Need schema/catalog access and ledger SELECT.
- Preflight returns before runtime composition, consumers, ReceiveMessage/timers, normalizer reconciliation, Google auth/inference, EMAIL adapters or domain writes. SQS credentials are needed for attribute reads. OpenSearch check does not attest mappings, plugins, projection content or provider recovery. No paid-service readiness claim.

## Service dependencies

- All scopes use `platform-postgres::PostgresPoolConfig::from_lookup`. Required: `STAGE`, `POSTGRES_SSL_MODE`, `POSTGRES_HOST`, `POSTGRES_DATABASE`, `POSTGRES_USERNAME`, and exactly one of `POSTGRES_PASSWORD` / `POSTGRES_PASSWORD_FILE`. Password files require Unix mode `0400` or `0600` and no final symlink.
- `dev`/`prod` require `verify-full` plus `POSTGRES_SSL_ROOT_CERT` (PEM CA file). Only explicit `local`/`test`/`ephemeral` may use `disable`; no stage or TLS default. Port defaults to `5432`; positive max connections defaults to `2`. Strict shared parsing rejects malformed/non-Unicode inputs and unsupported ambient `PGSSLCERT`, `PGSSLKEY`, `PGSSLROOTCERT`, `PGOPTIONS`; lookup must forward them. Application name is `aura-historia-worker`; config/connect causes stay typed and redacted.
- Private `postgres_config_tests` need no database. `src/postgres-test-ca.crt` is a public test-only CA for production config tests, never deployment trust. Process fixtures set `STAGE=test`, `POSTGRES_SSL_MODE=disable`. Rollout wiring remains out of this slice; default-off stays off.
- Projection/percolator require scoped OpenSearch endpoint and production credentials. Real-stage endpoint must use HTTPS; URL credentials/query/fragment forbidden. Explicit local stages retain HTTP endpoint support.
- Percolator/translation need Vertex project/location/model and Google ADC. Embedding needs Vertex project/location and ADC. Only selected scope initializes adapters.
- EMAIL delivery needs S3 templates, SES credentials, from/reply-to addresses, `STAGE`, `COMMIT_SHA`; generic dispatcher verifies planner channels.
- Worker uses workspace `aws-sdk-sqs`, `axum`, `strum`, `strum_macros`, plus pinned `hyper` (`server,http1`) and `hyper-util` (`tokio,service`). Update manifests/lockfile and black-box process/Sequin/SQS/DLQ acceptance together.

## Verification

- `cargo check --locked --offline -p aura-historia-worker --all-targets`
- `cargo test --locked --offline -p aura-historia-worker --lib --all-features`
- `cargo test --locked --offline -p aura-historia-worker --bin aura-historia-worker`
- Shared local PostgreSQL fixture ownership accepted independently (`7dd57256-43d2-416d-a5b8-b13910e28574`). `cargo test --locked --offline -p aura-historia-worker --test preflight_lifecycle -- --test-threads=1` uses that fixture and loopback SQS/OpenSearch spies. Cached image only (`--pull=never`); no LocalStack/Google/SES. This is not real SQS/Sequin durability proof.
- Safe binary-unit subprocesses cover stalled consumer/HTTP Drop/poll (including consumer-first completion and immediate shutdown polling), full 20s delayed HTTP drain, shared parent+child allowance, arithmetic overflow, stalled runtime Drop/blocking tasks and clean teardown. They execute production shutdown functions through the test harness, not a configured worker/OS-signal acceptance flow. Private receipt tests hold both reconciliation and pending/held receive destructors, release in both orders, and require confirmed destruction with no ack/new receive.
- Private tests cover wire snapshots/negative matrices, lifecycle failures, publication prevalidation/partial/ambiguous failure, safe timing logs, real SDK requests against loopback HTTP stubs, config policy drift, HTTP fragmentation/socket/header/body limits/timeouts/cancellation/drain, sustained outage recovery, maximum fanout, and normalizer fairness/owned polling/held heartbeat/handoff/shutdown.
- Every scope's acceptance uses real PostgreSQL, Sequin, LocalStack SQS and written target stores with independent competing consumers. Raw normalization keeps a four-second direct CDC deadline; timer reconciliation cannot replace prompt receipt handling.
- `tests/process_durability.rs` runs actual worker children against persistent fixtures. Deterministic database/HTTP barriers cover death before completion/deletion, overlapping consumers, native DLQ persistence, lost send/delete responses, SIGINT/SIGTERM drain and fatal short local drain followed by native redelivery. Requires locally authorized LocalStack Pro/Sequin; do not run where their possible external/license calls violate task constraints.
- Both process suites use `ProcessPostgres` with the shared process-lived local PostgreSQL client. Worker setup applies real SQLx migrations; cleanup preserves the ledger. Shared fixture owns only its successfully created container ID. Never stamp a ledger to bypass verification. Instrumented children keep unique profiles beside CI's `LLVM_PROFILE_FILE`; clean exits flush a nonempty child profile. SIGKILL cannot flush coverage. Unit/SDK spies do not prove SQS durability. Real AWS smoke stays opt-in.
- Keep architecture/event-flow/runbook, Sequin limits, queue/DLQ IAM, heartbeat cap, receipt scheduling and deployment shutdown grace aligned. Operational rollout remains external; follow `docs/durable-worker-runbook.md`.

## Review handoff

- `HANDOFF_03.md`: implementation files, exact validation evidence, acceptance gaps, operator inputs and rollback caveats for lifecycle/admission/preflight iteration.
- Older binaries ignored CLI arguments: never run `--check-config` against a rollback binary without proving it implements non-consuming preflight. Coordinate probe/CLI rollback; do not purge queues or undo schema for this worker-only change.

## Child DOX Index

- None.
