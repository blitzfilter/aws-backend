# DOX

## Purpose

- Own `shopify-lambda` crate.

## Core Design

- Worker Lambda for Shopify product ingestion from EventBridge through SQS.
- Root modules: `types`.
- Main neighbors: `application`, `listing-source-core`, `listing-source-service`, `listing-source-postgres`, `platform-observability`, `platform-postgres`, `product-listing-normalization`, `product-listing-service`, and `product-listing-postgres`.
- Event/runtime edge crate. It parses provider-native SQS/EventBridge payloads, retains the complete semantic Shopify product object, maps it into the generic raw-input contract, and captures it through ProductListing service. It never writes canonical ProductListings or ProductListing events. The normalization worker performs canonical mutation later.
- Shopify `active` create/update maps tracked positive inventory to generic `InStock` intent, tracked non-positive inventory to generic `OutOfStock` intent, and missing or untracked inventory to explicit clear. It emits raw-values V2 with `MACHINE_DECIMAL`; nonblank provider prices require ListingSource currency, provider price strings stay unchanged in source payload, and blank values map to generic price clear. `archived`, `draft`, and delete capture `DELETE`; missing ListingSource is acknowledged for idempotency. Missing or unsupported status is acknowledged without capture. Provenance retains topic and delivery IDs but raw input hashing excludes them. `X-Shopify-Triggered-At` is the sole Shopify source-order clock for every captured product topic, including ID-only delete. Missing header means no general source-order guarantee, but a post-DELETE UPSERT without proof is a recoverable source-order ambiguity; `updated_at` is retained only in source JSON and never compared with trigger time. Receipts scope by product topic; their delivery identity is `shopify-webhook:<X-Shopify-Webhook-Id>` or, when absent, `eventbridge:<EventBridge ID>`, so identity systems cannot collide. Receipt digest is canonical source-payload SHA only; source-order digest additionally binds effective `UPSERT` or `DELETE`; `X-Shopify-Event-Id` stays provenance/source event. Deployment clears old Shopify mutable `updated_at` watermarks without rewriting raw revisions or normalization progress. Invalid receipt-ID evidence is acknowledged: reuse has no safe redrive meaning. A distinct delivery that conflicts at equal source order is returned as its own SQS partial failure; SQS retries then redrives it to the configured DLQ for operator inspection/redrive. No payload is logged.

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

## PostgreSQL startup

- Bootstrap uses `platform-postgres::PostgresPoolConfig::from_lookup`. Required: `STAGE`, `POSTGRES_SSL_MODE`, `POSTGRES_HOST`, `POSTGRES_DATABASE`, `POSTGRES_USERNAME`, and exactly one of `POSTGRES_PASSWORD` / `POSTGRES_PASSWORD_FILE`.
- `dev`/`prod` require `verify-full` plus `POSTGRES_SSL_ROOT_CERT` (PEM CA file). Only explicit `local`/`test`/`ephemeral` may use `disable`; no stage or TLS default. Password files require Unix mode `0400` or `0600` and no final symlink.
- Port defaults to `5432`; positive max connections defaults to `2`. Shared parsing rejects malformed/non-Unicode inputs and unsupported ambient `PGSSLCERT`, `PGSSLKEY`, `PGSSLROOTCERT`, `PGOPTIONS`; lookup must forward them. Application name is `shopify-lambda`; config/connect errors retain safe typed causes.
- Binary `postgres_config_tests` need no Lambda runtime, queue, or database. `src/postgres-test-ca.crt` is public test-only CA material; never deploy it. Runtime fixtures need explicit local stage/TLS. Infra rollout remains out of this slice; default-off stays off.

## Verification

- `cargo check -p shopify-lambda`
- `cargo test -p shopify-lambda --all-features`

## Child DOX Index

- None.
