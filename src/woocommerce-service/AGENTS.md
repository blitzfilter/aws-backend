# DOX

## Purpose

- Own WooCommerce product webhook intake use case.

## Core Design

- `WoocommerceWebhookIntakeUseCase` checks `ProductListingsWrite` before source reads or provider work.
- It verifies untouched signed bytes, then maps WooCommerce product JSON into generic raw input.
- Captured events invoke `CaptureProductListingRawObservationUseCase`; ignored create/update statuses invoke `AuthorizeProductListingRawCaptureUseCase` only. No intake transaction or nested transaction exists.
- Delivery receipts stay topic-scoped and use canonical semantic source evidence. `date_modified_gmt` maps to source order only for captured events.
- No SQLx, HTTP, concrete adapter, or raw-payload logging dependency.

## Ownership

- This doc rules `src/woocommerce-service/**`.
- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep provider vocabulary here. Keep API route transport-only.
- Keep unit tests beside the intake use case.

## Verification

- `cargo check -p woocommerce-service`
- `cargo test -p woocommerce-service --all-features`

## Child DOX Index

- None.
