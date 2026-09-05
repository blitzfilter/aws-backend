# DOX

## Purpose

- Own pure generic ProductListing deterministic normalization.

## Core Design

- Modules: availability, price, date-time, text/language, image URLs, source-listing IDs, raw normalization input, and raw-values normalization.
- Raw input owns generic action, payload-format/version, source payload, raw-values projection, context, typed SHA-256 input hash, and separate provenance. JSON fields are objects; caps: source payload 1 MiB, raw values 256 KiB, context/provenance 64 KiB, depth 64. Provenance stays outside input hash.
- `ProductListingRawValuesV1` is the provider-neutral UPSERT raw-values JSON contract. Mutable fields use explicit `SET`, `CLEAR`, or `UNCHANGED` patches; source-selected dynamic attributes use the same patch protocol. `ProductListingNormalizationContextV1` owns generic base URL plus fallback currency and language. `ProductListingRawValuesNormalizer` is synchronous and deterministic: it resolves V1 UPSERT values, returns typed invalid outcomes, and passes DELETE through without decoding raw values or context.
- Depends only on pure value crates. No SQLx, HTTP client, LLM, queue, runtime config, provider DTO, or logging.
- Source code maps provider payloads before calling this crate.

## Ownership

- This doc rules `src/product-listing-normalization/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read root, `src/AGENTS.md`, then here before edit.
- Update this doc when API, dependency, normalizer, or limit changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep functions synchronous, typed, deterministic.
- Do not add application ports or use cases here.
- Never log raw values.

## Verification

- `cargo check -p product-listing-normalization`
- `cargo test -p product-listing-normalization --all-features`

## Child DOX Index

- None.
