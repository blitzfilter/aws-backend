# DOX

## Purpose

- Own `notification-service` crate.
- Own canonical notification use cases and service-owned ports.

## Core Design

- Depends on `notification-core` plus pure `money` and `localization` values; never depend on runtime adapters.
- Root modules:
  - `use_cases/commands` — creation, leased external delivery, and owner-scoped mutation handlers/contracts.
  - `use_cases/queries` — list handler/contracts and dedicated views.
  - `ports` — creator, list reader, seen writer, deleter, delivery repository, and channel sender capabilities.
- Notification creation accepts producer-selected external-delivery requests, then the service coordinator plans channel/target intents and persists them atomically for newly inserted notifications only. IDs are generated at the application boundary and outcomes stay input-aligned. The initial planner emits EMAIL/PRIMARY only; producers never select a channel or target. Delivery claims load the persisted channel/target source by delivery ID; the delivery handler calls `NotificationDeliveryDispatcher` once through one registered channel sender, captures one completion value, then retries only the matching lease finalization with the original lease token, completion timestamp, and provider receipt/error code. Each channel sender resolves its own target outside generic service code. Duplicate channel registrations and unregistered channels are errors. Seen and delete mutations derive owner only from `OperationContext`; single missing or cross-owner rows return `NotFound`.
- `presentation::NotificationPresentationPreferences` carries language and current `show_unassessed_or_sensitive_content`. The list read model carries canonical `NotificationKind` beside localized content; localization does not own or derive the kind. Watchlist price changes preserve immutable source-currency values, and availability changes preserve optional old/new values; REST and email consume them without FX conversion or changing notification snapshots.
## Delivery Safety Contract (#1558)

- `AlreadyClaimed { lease_expires_at }` carries the actual persisted active PROCESSING expiry. Never ack it. Runtime defers until that expiry, with a small minimum if it has already passed; never invent another full five-minute lease.
- A failed claim followed by PENDING or expired PROCESSING returns repository `Reclaimable`, mapped to service `ClaimDeferred { retry_after: std::time::Duration::from_secs(1) }`. Short defer, never ack. This closes the claim/status-read race without a spin loop.
- Ack only `Delivered`, `AlreadyDelivered`, `DeliveryMissing`, `SourceMissing`, or `PermanentlyFailed`. Source/permanent outcomes require successful finalization first. Every error stays unacknowledged, including repository failure, lease loss, exhausted/ambiguous completion, and unregistered-channel error (the next delivery observes its persisted FAILED state).
- Claim stays five minutes. `DeliverNotificationTiming::default()` gives one monotonic four-minute attempt, 30-second operation limits (claim/source load, channel/target/provider work, each finalize call), and 100ms–5s exponential finalize backoff. `DeliverNotificationTiming::new(attempt, operation, initial_retry, max_retry)` validates nonzero durations, attempt <= four minutes, operation < attempt, and initial <= max < attempt. `DeliverNotificationHandler::with_timing(repository, dispatcher, timing)` injects it; no env reads.
- Deadline starts before claim, includes waits/backoff, and is capped by the returned lease. Timeout returns `AttemptTimedOut { phase: DeliveryAttemptPhase::{Claim, Provider, Finalization} }`; repeated finalize failures return `FinalizationExhausted { source }`. No error confirms completion.
- Capture one provider result, original timestamp, and original token; retry only finalize, never provider again in the same attempt. Finalize false means `LeaseLost`. Adapter must recognize an exact committed replay after response loss without another write. The initial business schema supplies receipt columns for production and all PostgreSQL fixtures; no separate fixture rail.
- `NotificationChannelSendError::Ambiguous { code, source }` maps to `DeliverNotificationError::AmbiguousSend`; provider timeout does likewise semantically. Keep PROCESSING lease; no premature PENDING release. `Retryable` means a known transient failure. EMAIL adapter classifies SES timeouts/transport/unknown responses/5xx/missing receipt as Ambiguous and disables SDK send retries; known throttling and transient pre-send lookups stay retryable.
- Runtime shutdown drops `execute`; all work is awaited inline, with no detached tasks or background finalization. Cancellation gives no ack and no extra send. Provider may have accepted before cancellation: this is not exactly-once email. Redelivery after lease expiry can duplicate externally accepted sends.
- SQL/SDK types stay outside service. Log IDs, retry count/delay, and safe categories only; retained error sources, provider receipts, recipient/content, and lease tokens are not log payloads.
- No pending republisher, processed table, or generic outbox. No new dependency required.

## Boundaries

- No compatibility re-export modules or noop adapters.
- Keep runtime and HTTP glue outside.

## Ownership

- This doc rule `src/notification-service/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-service`
- `cargo test -p notification-service --all-features`

## Child DOX Index

- None.
