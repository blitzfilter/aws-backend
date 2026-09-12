# DOX

## Purpose

- Own pure generic ProductListing deterministic normalization.

## Core Design

- Modules: availability, price, date-time, text/language, image URLs, source-listing IDs, raw normalization input, and raw-values normalization. Each raw image value contains one URL; source code selects provider-specific candidates before calling this crate.
- Raw input owns generic action, payload-format/version, source payload, raw-values projection, context, typed SHA-256 input hash, and separate provenance. JSON fields are objects and reject embedded NULs in every key or nested string without rewriting values; caps: source payload 1 MiB, raw values 256 KiB, context/provenance 64 KiB, depth 64. Provenance stays outside input hash.
- `ProductListingRawValues` is the one current provider-neutral UPSERT raw-values JSON shape. Its persisted raw-values discriminator is `1`; it requires `priceFormat`. `DISPLAY_TEXT` uses display-text price parsing: a main asking price uses phrase-level evidence, explicit request-price phrases beat fallback-only/incidental numbers, generic contact wording loses to a credible explicit monetary amount, and contradictory or ambiguous explicit monetary/request assertions fail closed. Display-price parsing binds each amount to its adjacent currency marker; conflicting explicit currency pairs fail closed even with request/contact wording, and fallback applies only to an unmarked selected amount. Estimates remain monetary-only, so explicit request phrases clear them. `MACHINE_DECIMAL` requires fallback currency only for a nonblank `SET` price or price estimate and accepts only full unsigned ASCII decimals with zero-only excess precision. Blank `SET` price and price-estimate values normalize to `CLEAR`. Mutable fields use explicit `SET`, `CLEAR`, or `UNCHANGED` patches; source-selected dynamic attributes use the same patch protocol. Auction context is one strict nested patch: `SET` holds optional lot number, catalogue position, and `biddingOpens`/`scheduledCloses`/`reportedClosedAt` timing; time values explicitly declare `DATE` or `INSTANT`, so source dates remain dates and `reportedClosedAt` requires `INSTANT`. Malformed outer auction data and invalid lot number/catalogue position return typed candidate-data errors. Optional timing decodes separately: malformed or invalid timing maps the outer auction to `UNCHANGED` and emits the stable `AUCTION_TIMING_INVALID` diagnostic. `ProductListingNormalizationContextV1` owns generic base URL plus fallback currency and language. Availability recognition normalizes only case, whitespace, and common separators, then uses exact multilingual status vocabularies plus unambiguous quantity signals; regex sets cache their shared compilation result, and an invalid constant returns a typed fail-closed `System` failure before any state resolution. `ProductListingRawValuesNormalizer` is synchronous and deterministic: it resolves only current UPSERT values, classifies typed invalid outcomes as `CandidateData` or `System`, and passes DELETE through without decoding raw values or context.
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
