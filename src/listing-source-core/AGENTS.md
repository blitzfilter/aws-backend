# DOX

## Purpose

- Own ListingSource domain state and values.

## Core Design

- `ListingSource` owns stable ID/slug, name, Party operator, ingestion methods, presentation, and referral behavior. `ListingSourceId` is a strict UUIDv7-backed `ls_` object ID; derived slugs retain their legacy raw-UUID suffix. `outbound_url` is the canonical pure Partnerize-or-Aura-UTM URL derivation.
- Names trim Unicode outer whitespace, reject blank values, and allow at most 255 UTF-8 bytes without truncation. Partnerize `camref` is a preserved, nonblank ASCII alphanumeric path component (`[A-Za-z0-9]+`) of at most 128 bytes; it rejects trimming, delimiters, percent encoding, controls, and Unicode. Creation derives an immutable slug once; empty slugification falls back to `listing-source-<listingSourceId>`.
- It has no provider secrets, lifecycle, address, or search-policy state. `ListingSourceSearch` only carries bounded query filters for the service-owned read use case.
- Rehydrate preserves valid stored slugs and rejects invalid persisted name or slug.
- `lib.rs` exports the stable public API. Focused modules own the aggregate (`listing_source`), identifiers (`listing_source_id`, `listing_source_slug_id`), values (`domain`, `listing_ingestion_method`), search vocabulary (`listing_source_search`, `sort_listing_source_field`), and referral behavior (`referral_configuration`).

## Verification

- `cargo check -p listing-source-core`
- `cargo test -p listing-source-core --all-features`
