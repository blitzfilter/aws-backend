---
name: aura-rust-projection
description: Use when adding or changing Aura Historia Rust CDC, Sequin routing, projection jobs, OpenSearch or key-value projections, replay/rebuild paths, projection mappings, or CDC tests.
---

# Aura Rust Projection

Use for CDC and rebuildable read projections.

## Must read

- `backend/AGENTS.md` and path `AGENTS.md` files.
- `docs/arch.md` §10.4, §12, §14, §17, §20.6, §21-23.
- `docs/durable-worker-runbook.md` for effective limits, cutover, DLQ, and deletion-fence recovery.

## Before coding

- Identify operational owner of every dataset.
- Classify each store: authoritative storage, rebuildable projection, external source, or cache.
- Identify source version/idempotency strategy.
- Identify replay/rebuild path and verification.
- Update projection docs when behavior, ownership, rebuild, or delivery guarantee changes.

## Hard rules

- Every dataset has one documented operational owner.
- PostgreSQL owns business truth unless a bounded context documents another owner.
- OpenSearch contains rebuildable search projections only.
- Projection stores are never part of PostgreSQL transactions.
- Only committed PostgreSQL changes are propagated.
- Domain invariants must not depend on projections being current.
- Fully prevalidate the batch, typed jobs, stable keys, destinations, and serialized bounds before any publication.
- Acknowledge Sequin only after every required job has confirmed Standard SQS publication. Failure, timeout, or ambiguity means no acknowledgment; partial publication may duplicate on redelivery.
- Keep one source/DLQ pair per deployed scope, no tier dimension. No production in-memory fallback; only the normalizer's reconstructible continuation scheduling stays local.
- Delete SQS receipts only for complete handling. Nonterminal claims, poison, panics, timeouts, and unconfirmed effects remain for retry/native DLQ.
- Guarantee durable at-least-once within source7d/DLQ14d retention, never exactly-once. Standard transfer to DLQ retains original enqueue age; it grants no fresh 14-day window.
- Handlers tolerate duplicates/reordering; target writes enforce idempotency. External provider acceptance and database finalization are not atomic.
- Projection records should store latest applied source version.
- Older or equal source version must not overwrite newer projection state.
- Use content-free external-versioned tombstones for deletion fences; never rely on physical DELETE version GC or expire fences. Missing deletion markers remain live. Deploy mappings/all readers before writers; retire old physical-delete writers and fence their in-flight requests.
- Rebuild from authoritative state with old writers fenced from the new generation. Withdrawn listings retain source versions; hard-deleted filters need retained delete facts or a fenced rebuild, not invented history.
- Prefer target-side conditional updates, unique constraints, or version checks over in-memory duplicate checks.
- Treat incomplete CDC payloads as invalidation signals: read current committed authoritative state, build full projection, conditionally update target.
- Joined/hydrated projections should reread authoritative state instead of incrementally merging unrelated partial changes.
- Projection mapping belongs to target adapter.
- Search documents/items must not escape their adapter.
- Existing projections are not recovery source for authoritative data.
- Poison changes are never silently discarded. Malformed CDC stays in Sequin; invalid queued jobs reach native DLQ. Repair before controlled redrive under separate approved operator authorization; never purge.
- Keep runtime/identity/Sequin deployment handoffs honest. Verify live code, not stale defaults. Document real alarms separately from log-only signals; no invented metrics or completed rollouts.

## Observability

- Monitor replication lag, WAL growth, Sequin lag/retries, unacknowledged age, router failures, queue depth, handler failures/latency, duplicate/stale rejections, projection freshness, and rebuild status.
- Logs include safe identifiers, source/table/op, version, job type, idempotency key, attempt, outcome, and correlation id.
- Never log complete source rows, credentials, tokens, or sensitive payloads.

## Tests

- Cover insert/update/delete mapping, full prevalidation, partial/ambiguous publication then redelivery, ingress bounds, duplicate/concurrent/stale delivery, complete-only deletion, heartbeat failure, DLQ, schema compatibility, replay, and fenced rebuild.
- Every production worker route MUST have a black-box acceptance suite in its runtime crate `tests/`. Use real Postgres, real Sequin webhook delivery, the running worker HTTP server, SQS through LocalStack, and every real target store it writes (for example OpenSearch). Do not replace this flow with mocked ports.
- Worker acceptance cases MUST cover committed happy paths, source rollback, ignored/unrouted changes, redelivery/target idempotency, recipient or projection filtering, and persisted target payload shape. Test retryable queue/backpressure and malformed CDC behavior at the narrowest suitable layer when a real Sequin setup cannot deterministically induce them.
- Test restart after acknowledgment, response-loss duplicates, and stale remote writes beyond delete GC. Real AWS smoke stays opt-in, outside credential-required CI; mutation steps require asserted isolated sandbox account/stage and unique names.
