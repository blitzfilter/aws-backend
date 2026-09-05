# DOX

## Purpose

- Own `shopify-lambda` crate.

## Core Design

- Worker Lambda for Shopify product ingestion from EventBridge through SQS.
- Root modules: `types`.
- Main neighbors: `application`, `listing-source-core`, `listing-source-service`, `listing-source-postgres`, `platform-observability`, `platform-postgres`, `product-listing-normalization`, `product-listing-service`, and `product-listing-postgres`.
- Event/runtime edge crate. It parses provider-native SQS/EventBridge payloads, retains the complete semantic Shopify product object, maps it into the generic raw-input contract, and captures it through ProductListing service. It never writes canonical ProductListings or ProductListing events. The normalization worker performs canonical mutation later.
- Shopify `active` create/update maps tracked positive inventory to generic `InStock` intent, tracked non-positive inventory to generic `OutOfStock` intent, and missing or untracked inventory to explicit clear. `archived`, `draft`, and delete capture `DELETE`; missing ListingSource is acknowledged for idempotency. Missing or unsupported status is acknowledged without capture. Provenance retains topic and delivery IDs but raw input hashing excludes them.

## Ownership

- This doc rule `src/shopify-lambda/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.
- If trigger, retry, env var, queue/topic, or side effect change, update `infra/` and test wiring too.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Bootstrap thin. Push reusable work into service or domain crate.
- Be clear about event source, idempotency, and side effects.

## Verification

- `cargo check -p shopify-lambda`
- `cargo test -p shopify-lambda --all-features`

## Child DOX Index

- None.
