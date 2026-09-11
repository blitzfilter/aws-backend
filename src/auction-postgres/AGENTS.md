# DOX

## Purpose

- Own PostgreSQL Auction repository, schedule rows, event journal, metadata-protection audit, and details reader.

## Core Design

- Rows and SQL stay private. Rehydration validates all persisted IDs, values, enum codes, localization pairs, schedule precision, and timezones.
- Repository and journal/policy writers bind to caller-owned `SqlxTransaction`. Auction updates use root CAS and replace bounded owned schedule rows in that transaction.
- `auctions.listing_source_id` is restrictive. ListingSource deletion is blocked by retained Auctions.

## Verification

- `cargo check -p auction-postgres --all-targets --all-features`
- `cargo test -p auction-postgres --all-features`
