# DOX

## Purpose

- Own `notification-core` crate.
- Own canonical notification domain and delivery reference types.

## Core Design

- Domain-only crate.
- Root modules: `notification`, `notification_id`, `presentation`, `notification_delivery`, `notification_delivery_id`, `notification_kind`. `NotificationId` and `NotificationDeliveryId` are strict UUIDv7-backed `ntf_` and `nd_` object IDs. ProductListing core has no dependency on notification core, so this keeps notification identity acyclic.
- `notification_delivery` owns channel and opaque logical target-key values. EMAIL is the sole channel. Future channel values belong here, with a matching schema migration, not in producers or adapters.
- `Notification` aggregate has no created/updated, actor, delivery, or runtime metadata.
- Typed `NotificationContent` owns semantic source plus immutable display snapshot; kind is derived. Partnership application decisions carry their application ID and immutable snapshot. Watchlist price changes keep old/new prices as optional factual source-currency values; availability changes keep optional old/new `ListingAvailability` values; no FX conversion belongs here.
- Watchlist/search-filter ProductListing titles are optional.
- View types may carry created/updated timestamps.
- `presentation` owns notification image presentation and reuses the centralized listing content-visibility policy. It preserves the listing-level snapshot assessment while optionally redacting the image URL.
- No transport or runtime glue.

## Ownership

- This doc rule `src/notification-core/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-core`
- `cargo test -p notification-core --all-features`

## Child DOX Index

- None.
