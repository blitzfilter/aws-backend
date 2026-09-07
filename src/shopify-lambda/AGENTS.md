# DOX

## Purpose

- Own `shopify-lambda` crate.

## Core Design

- Worker Lambda for Shopify product ingestion from EventBridge through SQS.
- Root modules: `types`.
- Main neighbors: `application`, `listing-source-core`, `listing-source-service`, `listing-source-postgres`, `platform-observability`, `platform-postgres`, `product-listing-normalization`, `product-listing-service`, and `product-listing-postgres`.
- Event/runtime edge crate. It parses provider-native SQS/EventBridge payloads, retains the complete semantic Shopify product object, maps it into the generic raw-input contract, and captures it through ProductListing service. It never writes canonical ProductListings or ProductListing events. The normalization worker performs canonical mutation later.
- Shopify `active` create/update maps tracked positive inventory to generic `InStock` intent, tracked non-positive inventory to generic `OutOfStock` intent, and missing or untracked inventory to explicit clear. It emits raw-values V2 with `MACHINE_DECIMAL`; nonblank provider prices require ListingSource currency, provider price strings stay unchanged in source payload, and blank values map to generic price clear. `archived`, `draft`, and delete capture `DELETE`; missing ListingSource is acknowledged for idempotency. Missing or unsupported status is acknowledged without capture. Provenance retains topic and delivery IDs but raw input hashing excludes them. Receipts scope by product topic; their delivery identity is `shopify-webhook:<X-Shopify-Webhook-Id>` or, when absent, `eventbridge:<EventBridge ID>`, so identity systems cannot collide. Receipt digest is canonical source-payload SHA only; `X-Shopify-Event-Id` stays provenance/source event. Invalid receipt-ID evidence is acknowledged: reuse has no safe redrive meaning. A distinct delivery that conflicts at equal source order is returned as its own SQS partial failure; SQS retries then redrives it to the configured DLQ for operator inspection/redrive. No payload is logged.

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
