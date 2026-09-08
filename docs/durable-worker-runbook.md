# Durable-worker runbook

Operational contract for #1558, checked against repository runtime, adapters, migrations, and CDK. This is not evidence of a deployed worker, attached IAM policies, configured Sequin, or passed acceptance tests. See [architecture §12](arch.md#12-cdc-and-projection-architecture) and [event routing](events/flow.md).

## Custody and effective limits

`PostgreSQL commit → Sequin → scoped worker HTTP → Standard SQS → service handler`.

- Fully validate the entire batch, typed jobs/keys, destinations, and serialization **before any publish**. Return Sequin `202` only after every required SQS send is confirmed. Invalid input, partial/ambiguous publication, timeout, or failed send means no acknowledgment. Retrying may duplicate already-published jobs. A valid event irrelevant to the subscribed scope may acknowledge with zero jobs.
- Delete a receipt only for `Complete`: confirmed handling or an explicitly terminal/idempotent no-op. Handler errors, poison, panic/cancellation, timeout, lost heartbeat, nonterminal claims, and unconfirmed effects do not delete. Delete retries do not rerun the handler; lost settlement can cause later redelivery.
- **Durable at-least-once within retention**, never exactly-once or ordered processing. Source retention **7 days**; DLQ **14 days**. Standard source-to-DLQ transfer preserves the **original enqueue timestamp**: arrival gives neither a fresh 14 days nor a combined 21-day window. Expired messages cannot be redriven.
- All ten scopes use separate source/DLQ pairs; no user-tier scope or tier dimension. Production startup validates existing queues; it neither creates them nor falls back to memory.

| `AURA_HISTORIA_WORKER_SCOPE` | Sequin source | Output stem after `Worker` | Visibility | Execution budget |
|---|---|---|---:|---:|
| `product-listing-opensearch` | `product_listing_events` INSERT | `ProductListingOpensearch` | 60s | 45s |
| `search-filter-projection` | `search_filters` INSERT/UPDATE/DELETE | `SearchFilterProjection` | 60s | 45s |
| `search-filter-percolator` | `product_listing_events` INSERT | `SearchFilterPercolator` | 300s | 240s |
| `search-filter-match-notification` | `search_filter_matches` INSERT | `SearchFilterMatchNotification` | 60s | 45s |
| `watchlist-notification` | `product_listing_events` INSERT | `WatchlistNotification` | 60s | 45s |
| `product-content-assessment` | `product_listing_events` INSERT | `ProductContentAssessment` | 60s | 45s |
| `product-embedding` | `product_listing_events` INSERT | `ProductEmbedding` | 300s | 240s |
| `product-translation` | `product_listing_events` INSERT | `ProductTranslation` | 300s | 240s |
| `product-listing-normalization` | `product_listing_raw_revisions` INSERT | `ProductListingNormalization` | 300s | 240s |
| `notification-delivery` | `notification_deliveries` INSERT | `NotificationDelivery` | 360s | 240s |

These are current fixed values, not environment tuning knobs:

- Deliberate **concurrency 1 per process**, receive batch 1, no prefetch. Normalization reserves one receive across timer turns; a received job may wait behind one bounded reconciliation turn, with heartbeat and a 245s hold budget. Handoff joins that receipt owner before execution. More replicas introduce concurrency; `ordering_key` is not an SQS ordering guarantee. Ingress stays independent of downstream outages.
- Long poll **20s**; receive outer deadline 27s; other SQS operations have a 5s outer deadline. Heartbeat every `min(30s, visibility/3)` (20/30/30s), extending visibility to the scope value. Heartbeat failure cancels local work without deleting; cancellation cannot undo remote effects.
- Retry visibility uses exponential **30–900s** backoff with additive jitter, capped at 900s. Native source redrive uses **`maxReceiveCount = 5`**. Dependency/settlement failures open a jittered consumer circuit; recovery probes allow one job, not a backlog drain. Deferrals also consume receives; circuits reduce outage churn, not guarantee against DLQ entry.
- SIGTERM/SIGINT stops ingress and new receives, then allows only the active owned attempt to drain for `AURA_HISTORIA_WORKER_DRAIN_TIMEOUT_SECONDS` (default **270s**). It does not drain the SQS backlog. On deadline expiry, the worker aborts local work, emits `worker_drain_deadline`, exits non-zero, and relies on SQS visibility/redelivery for any unsettled receipt. Only normalization's reconstructible cursor/FIFO may be lost locally; see its [runbook](product-listing-raw-normalization-runbook.md).
- The external process manager/orchestrator must allow more stop grace than this configured ceiling. With the 270s default, use **300s minimum** unless a deployment owner deliberately coordinates another value. HTTP connection drain is 20s and runs concurrently with consumer drain; do not add the values mechanically. This repository does not configure that external stop grace.

### Private HTTP and wire contract

| Boundary | Effective value |
|---|---|
| CDC body / changes / derived jobs | 1 MiB / 100 / 500 |
| Total publication / HTTP request deadline | 8s / 10s, including body collection in the request deadline |
| Accepted sockets / concurrent requests | 16 / 16 |
| Header count / parser buffer / read deadline | 32 / 16 KiB / 5s |
| Whole connection deadline | 20s; HTTP/1, one request per connection, no keep-alive |
| Serialized SQS job | At most 16 KiB; explicit `schema_version = 1` |

Envelope fields are `schema_version`, `scope`, `job_type`, `payload`, `idempotency_key`, and `ordering_key`. Payloads carry compact IDs/revisions, not raw CDC rows, source JSON, notification content, or credentials. Schema 1 deliberately ignores unknown additive envelope/payload fields. Required fields, exact scope/type/operation, canonical IDs, positive versions, and derived-key equality remain strict. Unsupported schema/type or wrong scope is poison, not a successful no-op. This does not relax strict upstream ProductListing event validation. Keep consumers compatible with retained source/DLQ jobs before changing producers or rollback versions.

`POST /cdc/sequin` returns `202` for confirmed publication, `400` for invalid JSON/UTF-8, `413` for limits, `408` for request timeout, and `503` for routing/publication failure, capacity, or stopping. Header/protocol failures can close the socket before an application response. No non-2xx or lost response is an acknowledgment.

`GET /health` reports supervised consumer liveness; `GET /ready` also reflects consumer circuit readiness (200/503). During a downstream outage, readiness may be 503 while ingress can still publish and return 202. Neither endpoint proves projection freshness. Default bind is `0.0.0.0:8081`, overridden by `AURA_HISTORIA_WORKER_HEALTH_BIND_ADDR`. Keep this unauthenticated worker listener private/trusted; these are not public REST/Swagger routes.

## Deployment handoff: queues, identity, Sequin

CDK defines all ten pairs in `prod`, `dev`, and `ephemeral`:

- Source `aura-worker-<scope>-<stage>`; DLQ `aura-worker-<scope>-dlq-<stage>`. Names include the stage, not a custom stack prefix. Runtime `STAGE` must match exactly.
- Standard, SQS-managed encryption, TLS-only resource policy; DLQ redrive allow lists only its source ARN. Source redrive allow is `denyAll` (not another queue's DLQ). Prod retains on deletion/replacement; dev/ephemeral do not. Renaming never migrates messages.
- Startup reads and validates **both** source and paired DLQ identity, retention, encryption, private/TLS policies, redrive contracts, and source visibility/poll settings. Attribute mismatch or missing access fails startup; do not bypass checks.

The data stack (single stack in ephemeral mode) exports `WorkerQueueAwsRegion`, `WorkerQueueStage`, and, for each table stem:

```text
Worker<Stem>QueueUrl                 Worker<Stem>QueueArn
Worker<Stem>DeadLetterQueueUrl       Worker<Stem>DeadLetterQueueArn
Worker<Stem>PublisherPolicyArn       Worker<Stem>ConsumerPolicyArn
```

For example, `WorkerProductListingOpensearchQueueUrl` is the source URL, not its DLQ. Set `AWS_REGION`, `STAGE`, `AURA_HISTORIA_WORKER_SCOPE`, and `AURA_HISTORIA_WORKER_QUEUE_URL` from the matching outputs. `AWS_DEFAULT_REGION` alone is insufficient. Retain existing `POSTGRES_*` and scope-specific OpenSearch/Vertex settings. EMAIL still needs templates, sender/reply-to, `COMMIT_SHA`, and separate S3/SES permissions.

| Unbound managed policy | Exact source-queue actions | Paired DLQ actions |
|---|---|---|
| `aura-worker-<scope>-publisher-<stage>` | `sqs:SendMessage`, `sqs:GetQueueAttributes` | `sqs:GetQueueAttributes` only |
| `aura-worker-<scope>-consumer-<stage>` | `sqs:ReceiveMessage`, `sqs:DeleteMessage`, `sqs:ChangeMessageVisibility`, `sqs:GetQueueAttributes` | `sqs:GetQueueAttributes` only |

Each statement targets only that scope's exact ARN. The combined ingress/consumer process needs both policies. No runtime `GetQueueUrl`, DLQ message access, purge, queue deletion, redrive, or wildcard-resource permissions are granted. CDK exports managed policies; **the external identity owner binds them** to the existing approved credential/assumed-role arrangement. No runtime IAM user/key/trust binding, bare-metal process deployment, or operator redrive role is defined here. Do not borrow CI/Lambda identities or replace existing S3/SES grants.

`AWS_ENDPOINT_URL_SQS` is allowed only for `ephemeral`, `local`, or `test`, with the same origin as the queue URL. Global `AWS_ENDPOINT_URL` is rejected. Real AWS stages must not override endpoints. Ephemeral CDK names still require `STAGE=ephemeral`; a stack prefix is not runtime isolation.

**Sequin owner action:** scope each subscription to the table/operations above; set delivery timeout **>10s, recommended 15s**, and batch size **<=100**, also respecting the 1 MiB body bound. Check proxy/network deadlines and reduce batches if sequential publication cannot meet 8s. There is **no actual deployment Sequin configuration in this repo**: `src/test-api/src/sequin/` and its Rust builder are test fixtures (batch size 1, no operational timeout setting). Do not mistake them for a completed timeout/subscription rollout.

## Signals and read-only diagnosis

CDK defines prod-only alarms per scope on `cloudwatch-alarms-prod`:

- `prod-worker-<scope>-source-age`: SQS `ApproximateAgeOfOldestMessage >= 900s`.
- `prod-worker-<scope>-dlq-visible`: SQS `ApproximateNumberOfMessagesVisible >= 1`.

Both use **Maximum**, one **300s** period, missing data **not breaching**. Lower stages have no corresponding alarms. Inspect deployed alarm actions/subscriptions; an idle or absent metric is not proof of health. DLQ age metrics reflect time since transfer, not the original retention deadline.

Worker log signals include `scope`, stable job keys, `attempt`, `outcome`; `invalid_wire_job`, `receive_unavailable`, `dependency_circuit_open`, `heartbeat_failed`, `execution_timeout`, `receipt_settlement_failed`, and error-level `worker_drain_deadline` distinguish failure paths. Notification signals include `active_lease_deferred`, `provider_acceptance_unknown`, and `delivery_finalization_unconfirmed`. Attempt completion is not proof of successful receipt deletion. Normalization emits metadata-only events with `metric` fields; these are **logs**, not automatically installed custom metrics/dashboards. No worker dashboard or general freshness/rebuild metric is provisioned here.

Default operator checks are read-only, using an existing approved session:

1. Compare STS `GetCallerIdentity` with the approved account; record stage/region and CloudFormation `DescribeStacks` queue/policy outputs. Never print credentials or full environment.
2. Read SQS `GetQueueAttributes` on source and DLQ for identity/contracts plus visible, not-visible, and delayed counts; inspect source age and deployed alarms. Queue depths are approximate.
3. Read private health/readiness and safe outcome logs; check PostgreSQL connectivity, Sequin delivery failures/age, replication-slot lag and WAL growth. An empty DLQ does not rule out stuck Sequin.
4. For malformed CDC/413/wrong-table failures, diagnose subscription, batch/byte limits, source schema and typed IDs with approved metadata-only tools. Prevalidation publishes nothing; correct upstream/router incompatibility before delivery retry, not by acknowledging or fabricating a queued job.

Never dump queue bodies, source rows, headers, receipts/lease tokens, provider receipts, recipients, or SDK cause chains into terminals/logs/tickets. IDs remain access-controlled metadata. `ReceiveMessage` is **not** a read-only peek: it changes visibility/receive count. Use only an approved inspector that emits allowlisted schema/scope/type, domain IDs/keys, timestamps and counts; unknown bodies must not be echoed.

## Cutover and rollback gates

External owners must supply the actual deployment/change plan; this checklist is not a live AWS mutation procedure.

1. **Pause Sequin delivery first**, retaining its unacknowledged backlog. Inventory all old processes and their **in-memory queues and in-memory DLQs**. Drain them while still running, or capture recoverable compact job identities in an approved durable replay store **before stopping**. There is no universal legacy exporter. If drain/capture cannot be proved, stop the cutover and record the recovery gap. Previously lost jobs cannot be recovered magically by SQS.
2. Review queue creation/retention and identity binding; confirm the deployed business schema includes notification completion receipt columns. Expand OpenSearch mappings and deploy **all readers** before tombstone writers. Retire/fence physical-DELETE writers and their in-flight remote requests before enabling new writers.
3. Start matching SQS-compatible scope binaries with exact outputs/settings; confirm startup checks, health/readiness, subscription timeout/batch handoff, and safe end-to-end handling in the approved environment. Acknowledge only confirmed publication when Sequin resumes. Recover captured legacy work through a separately reviewed replay, preserving domain identity.
4. Watch source age, DLQ, Sequin retries and target state. Keep inventory of retained old queues and the pre-cutover gap; normalizer reconciliation repairs only its authoritative raw backlog.

Rollback is allowed only to binaries compatible with **SQS custody, retained wire schemas, notification lease/completion receipts, and tombstone readers/writers**. Never return to in-memory acknowledgment or physical DELETE writers, drop receipt columns, expire tombstones, reset index versions, purge queues, or delete/recreate an index as rollback. If no compatible binary exists, pause delivery/consumption under the external change plan and repair forward within retention.

## DLQ recovery and optional sandbox smoke

**Safety gate:** default is read-only. Before any AWS mutation exercise below, the operator must explicitly record an **isolated sandbox account and stage**, region, approved role, and **unique source/DLQ names owned only by this exercise**, and verify them against caller identity and outputs. A `dev` label or custom stack prefix alone is not isolation. CDK's fixed scope/stage names must remain runtime-compatible; collisions mean stop/use a fresh isolated account or process-isolated LocalStack, not an ad-hoc rename. Live recovery needs a separate approved change plan. No real AWS credentials or mutations are required in CI.

Sandbox-only recovery exercise, after that assertion:

1. Under a **separate approved operator role**, inspect allowlisted metadata and authoritative source status. Repair dependency/IAM/configuration/schema/corrupt-state causes first; do not redrive blindly or use runtime credentials. The operator role and its native-redrive permissions need external approval; runtime policies above do not supply them.
2. Confirm original enqueue age leaves recovery time. If repair will exceed retention, request an approved encrypted, access-controlled archive with privacy purpose, retention, and deletion policy before expiry. There is no built-in archive or ability to recover expired messages; never copy raw bodies into tickets or use redrive as indefinite retention.
3. Use native SQS redrive (`StartMessageMoveTask`) to the **paired source**, beginning at a low approved velocity (for example 1 message/s) for a short supervised window. It is rate/task controlled, **not** ID-selective or an exact small-message-count operation. Observe `ListMessageMoveTasks`, target results, source age and renewed DLQ arrivals; stop with `CancelMessageMoveTask` on regression. Cancel does not undo messages already moved. Expand only after the small trial is understood. Never purge, manually delete failures, or rewrite/re-send envelopes.
4. Native redrive assigns new SQS message IDs/enqueue timestamps; it leaves application payload/keys unchanged. Business idempotency and ordering identity never derive from that transport metadata. Preserve the original custody record for recovery/privacy accounting.

Optional ordinary-creation smoke uses the same sandbox gate: have the infrastructure owner create the matching unique pair/policies through a normal CloudFormation change set, the identity owner bind them, and the Sequin owner configure the subscription. Start the scoped worker, read both queue contracts/readiness, then create one synthetic source through its **normal application write path**. Observe Sequin 202, durable publication, expected target state and eventual receipt settlement; exercise only approved synthetic email targets. Read-only checks alone do not prove publish/consume or IAM/TLS enforcement. Destructive restart/poison/GC cases belong in the isolated local acceptance suites or a separately approved sandbox test, never against shared/live resources.

## Projection fences and rebuild

Both shared mappings contain boolean `projectionDeleted`. Deletion replaces the full document, removing content, embedding and percolator query:

| Target | Tombstone | External version |
|---|---|---|
| `product-listings` | `{productListingId, projectionDeleted: true}` | Current withdrawn `product_listings.projection_version` |
| `user_search_filters` | `{userSearchFilterId, sourceVersion, projectionDeleted: true}` | Deleted `search_filters.version + 1` from full delete CDC |

Raw OpenSearch document GET now returns **200 with the tombstone**, not 404, after withdrawal/deletion. Check marker and `_version`, not absence. Product search (including BM25/hybrid KNN), similar-listing KNN, filter query and PIT percolation exclude true **before pagination/ranking**. Missing marker means live for old documents. No public REST payload change follows from this private marker.

Physical DELETE forgets its version fence after `index.gc_deletes` (default 60s); a delayed already-prepared write can then resurrect content. Durable tombstones retain the external fence past GC; equal/older writes conflict as stale, only a strictly newer authoritative version can replace it. **Never TTL/delete-by-query tombstones**, use unversioned writes, or reuse IDs/versions. A stopped/timed-out client is not proof that its remote writes or DELETEs have ended.

Deployment/rebuild needs an explicit writer fence, not an unsafe index reset:

- Expand mappings and **all** consuming binaries/readers first. Retire old physical-DELETE writers and drain or externally fence their in-flight requests before tombstone writes. Existing physical deletions need backfill; installing a new writer cannot restore a forgotten fence by itself.
- Rebuild a fresh generation from current PostgreSQL with old writers unable to target it, including through an alias after cutover. Catch up committed changes under the same source-version rules, verify visible IDs/versions, tombstone invisibility on every reader and restore/stale-delete ordering, then atomically activate via a separately reviewed alias/routing plan. No automated general rebuild/cutover command is provided here.
- Withdrawn ProductListings retain source rows and projection versions; backfill their tombstones and rebuild active listings from current listing/translation/required FX state. Physical listing deletion can remove rows/events too; absent source history cannot supply a made-up version.
- Hard-deleted filters have **no current row/history of the deletion version**. Online deletion backfill needs retained full delete facts (owner/ID/old version). Without them, use an externally fenced fresh-generation rebuild from surviving rows. Do not infer missing deletion versions from an old index or claim a live-row scan repairs historical fences.

## Notification recovery limits

The PostgreSQL lease is **five minutes**; the attempt budget is **four minutes**, spanning claim, provider work, finalization and backoff. Service operations have 30s limits; finalization backoff is 100ms–5s. SQS visibility is 360s. A competing active claim defers to its **actual persisted expiry + 5s**, not a new five-minute delay. A failed claim followed by `PENDING`/expired `PROCESSING` defers **1s**. Neither permits receipt deletion. Current runtime also retries `DeliveryMissing`; terminal deletion is limited to delivered/already-delivered/permanently-failed or successfully finalized missing-source outcomes.

The initial business schema defines `completed_lease_token` and `completed_at`. Together with status, provider receipt/error and delivered timestamp, they recognize an **exact original finalization tuple** after a lost commit response. First finalization requires the matching unexpired lease; reclaim clears the completion receipt and fences old attempts. Terminal rows without a completion remain terminal without invented receipt tokens. Retry only that captured finalization tuple, never send again inside the attempt or replace its timestamp.

SES SDK `max_attempts = 1`. Known throttling/transient pre-send failures may finalize back to `PENDING`; permanent rejection/source failure must finalize `FAILED` before completion. Timeout, transport loss, unknown/5xx response or missing usable provider receipt is **ambiguous acceptance**: keep `PROCESSING`, no premature lease release, no acknowledgment. Lease loss, exhausted/unconfirmed finalization and cancellation also give no acknowledgment. A crash after SES accepted but before durable completion can still cause a duplicate email after lease reclaim. Neither SQS nor completion receipts make email exactly-once; repair before redrive and never manually clear a lease to force an immediate resend.

## Source map

- Runtime: `src/aura-historia-worker/src/{cdc,http,wire}.rs`, `queue/{config,consumer,sqs}.rs`, `main.rs`, `notification_delivery.rs`.
- Infrastructure: `infra/src/worker-queue-config.ts`, `constructs/worker-queues.ts`, `constructs/observability.ts`.
- Fences/readers: `src/product-listing-opensearch/`, `src/search-filter-opensearch/`, `opensearch/mappings/`.
- Claims/receipts: `src/notification-service/`, `src/notification-postgres/`, `src/notification-email-aws/`, root migration above.
