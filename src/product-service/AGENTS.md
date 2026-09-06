# DOX

## Purpose

- Own authoritative raw ProductListing revision normalization use case.

## Core Design

- `NormalizeProductListingRawRevisionUseCase` drains immutable raw streams in order.
- Use generic pure values from `product-listing-normalization` only.
- Own raw head/result ports and pending-stream read contract.
- Use caller-owned PostgreSQL transaction with ProductListing service canonical writer.
- Emit metadata-only normalization outcome and bounded reconciliation backlog metrics; raw JSON never enters logs.
- No SQLx, provider DTO, HTTP, queue, LLM, graph, or runtime config dependency.

## Ownership

- This doc rules `src/product-service/**`.
- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- One public full-record normalization use case only.
- No field normalization use cases or ports.
- No raw payload logs.

## Verification

- `cargo check -p product-service`
- `cargo test -p product-service --all-features`

## Child DOX Index

- None.
