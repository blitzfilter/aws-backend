# DOX

## Purpose

- Own canonical Notification PostgreSQL adapter.

## Core Design

- PostgreSQL owns notification and external-delivery state.
- Private rows and JSON payload mapping reconstruct typed Notification content. Product-listing V1 snapshots persist `listing_source_id`, `source_listing_id`, `listing_source_slug_id`, and `listing_source_name` with canonical types. Partnership application decisions use their own source column and immutable Party/ListingSource snapshot. V1 watchlist availability payloads use `AVAILABILITY_CHANGE` plus nullable `old_availability`/`new_availability` keys with exact canonical availability codes.
- Low-level notification and delivery-intent repositories share the caller transaction. The notification list reader joins the owner’s current `show_unassessed_or_sensitive_content` preference to the notification page, including empty pages. Generic delivery claim loads channel, target key, content, and current language/visibility preferences only. Watchlist price payloads serialize explicit source-currency prices. Channel-specific runtime adapters resolve targets after claim. Channel selection belongs to the notification-service planner, never this adapter.
- Implements the EMAIL target lookup contract with PostgreSQL. Worker only composes this adapter.
- Invalid persisted rows fail; never skip them.

## Delivery Safety Contract (#1558)

- Claim returns the persisted PROCESSING expiry in `AlreadyClaimed { lease_expires_at }`; never echo the losing caller's proposed lease. Fresh status read reports PENDING/expired PROCESSING as `Reclaimable`, not an active lease. Service maps it to one-second `ClaimDeferred`; runtime never acknowledges either deferral.
- Claim's compare-and-set, source mapping, and commit stay one short adapter operation under the existing pool-backed port. Mapping failure rolls claim back. No transaction spans provider I/O. This retains the existing narrow atomic-claim exception; no multi-repository transaction or runtime orchestration added.
- First finalization requires PROCESSING, matching active token, and lease newer than both original completion time and current database clock. Late old completion cannot bypass fencing. No match checks an exact persisted receipt in a fresh statement snapshot; concurrent identical finalizations and lost commit responses therefore replay read-only.
- Receipt uses `completed_lease_token`, `completed_at`, status, provider message ID/error code, and delivered timestamp. Match every field with original bind values (Postgres microsecond precision). Replay returns true; differing token/result/time or missing row returns false. Reclaim clears completion token/time before issuing a newer lease; old attempt cannot finalize or confirm the newer result.
- Active lease columns still clear on completion. Existing terminal rows have no replay receipt and remain terminal to claim; never backfill invented tokens. Database/commit failures retain SQLx source behind safe service errors. Never log source payloads, receipt, token, or recipient.
- The initial business schema owns the receipt columns/constraint. Production and tests use the root migration rail; adapter never auto-migrates. No separate delivery-schema fixture.
- No exactly-once provider guarantee; PostgreSQL cannot atomically commit an email send. No processed table, republisher, or generic outbox.

## Verification

- `cargo check -p notification-postgres`
- `cargo test -p notification-postgres --all-features`
- Focused real PostgreSQL: `cargo test --locked -p notification-postgres --lib --all-features delivery_repository::tests -- --test-threads=1`.
- Tests use only `test-api::Postgres::new("migrations")`, including initial-schema receipt columns. Cover concurrent claims, exact persisted expiry, before/at/after expiry, status-read races, rollback, stale-token fencing, exact completion replay, concurrent finalization, and safe retained DB errors.

## Child DOX Index

- None.
