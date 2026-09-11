# DOX

## Purpose

- Own Auction use cases, storage ports, metadata acceptance policy, and admin authorization.

## Core Design

- Service owns Auction write transactions. `auction-core` owns facts; this crate owns application policy.
- Admin create/update/get require administrator authorization. Explicit admin metadata touches protect that field and write restricted audit state. Policy-only work creates no Auction domain event.
- Repositories persist Auction aggregates. Details readers return service views. No ProductListing dependency until iteration 06.

## Verification

- `cargo check -p auction-service --all-targets --all-features`
- `cargo test -p auction-service --all-features`
