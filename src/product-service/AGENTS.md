# DOX

## Purpose

- Own authoritative raw ProductListing revision normalization use case.

## Core Design

- `NormalizeProductListingRawRevisionUseCase` drains immutable raw streams in order; it accepts payload schema `1` and only raw-values schema `1`. Other stored versions fail before canonical work and stay pending for an authorized development reset; no conversion is attempted. A `System` normalizer configuration failure returns a retryable error before transaction/completion; candidate-data invalid input writes terminal `REJECTED`. Malformed or invalid optional auction timing instead writes `APPLIED` or `NO_CHANGE` with stable `AUCTION_TIMING_INVALID`, while its outer auction patch stays `UNCHANGED`. Direct CDC errors stay errors.
- Use generic pure values from `product-listing-normalization` only.
- Own raw head/result ports and a non-durable oldest-pending-time plus stream-ID page cursor contract. Reconciliation isolates one stream failure, returns safe ID/code metadata, and keeps that stream pending; page-list failure stays an overall retryable error.
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
