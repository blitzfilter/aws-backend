# DOX

## Purpose

- Own `product-listing-core` crate.
- Own canonical ProductListing domain types.

## Core Design

- Domain-only crate.
- Root modules: `content_policy`, `description`, `listing_availability`, `listing_lifecycle`, `listing_orderability`, `product_listing`, `product_listing_event`, `product_listing_price`, `product_listing_id`, `product_listing_image`, `product_listing_search`, `product_listing_slug_id`, `sanitize`, `source_listing_id`, `sort_product_listing_field`, `title`.
- `content_policy` owns pure listing text assessment and visibility rules. `ProductListingImage` is a URL-only source fact; assessment is listing-level enrichment outside the aggregate.
- `product_listing::ProductListing` is canonical aggregate. Fields private. Rehydrate boundary public for adapter crates. Creation, rehydration, and auction mutation reject auctions whose start is after end.
- ProductListing translations, embeddings, user-state read models, read joins, and FX snapshots stay outside this aggregate. `ProductListingPricing.price` is `None`, monetary, or explicit `OnRequest`; estimates remain monetary-only. `ListingSaleObservation` retains an explicit observed-at timestamp plus immutable FX snapshot ID; it is separate from availability and lifecycle. `ProductListingPriceValuationBasis` persists canonical `CURRENT`, `EVENT`, and `SALE_OBSERVATION` values. ProductListing user-state read models live in `product-listing-service`.
- `product_listing_event` owns pure event payloads. Event types map only to `PRODUCT_LISTING_DISCOVERED` and `PRODUCT_LISTING_CHANGED`. A listing has zero or one pending payload: creation emits discovery; later semantic mutations coalesce typed first-previous/final-current facts. Image replacement is dedicated, uses fixed-width `ProductListingImageCount(u64)`, and may remain when counts match. Sale observation uses a semantic `ValueChange<Option<ListingSaleObservation>>`: `None -> Some` observes, `Some -> None` retracts, and corrections are rejected. Initial discovery rejects lifecycle and sale observation transitions. Changed rehydration rejects lifecycle/availability composites that violate the final withdrawn-state invariant. The hidden event rehydration boundary rebuilds immutable payload values only; it never creates an aggregate or emits an event. Drain only with `take_pending_event_payload`.
- `ProductListingKey` owns only the semantic `(ListingSourceId, SourceListingId)` pair; labeled storage and transport codecs live at their owning boundaries. `SourceListingId` is opaque: outer Unicode whitespace trims, blank values reject, case/punctuation/internal whitespace stay, and UTF-8 values cap at 512 bytes. The trimmed value is canonical. `ProductListingSlugId` is the title-derived public route locator: ASCII body plus an application-selected six-lower-hex suffix, capped at 120 bytes. Empty slug bodies fall back to `listing`; raw values validate that strict form. The core slug constructor is deterministic and owns no randomness. `ProductListing` exposes it as `title_slug_id`.
- `EnhancedSearchDescription` canonicalizes outer Unicode whitespace, rejects blank values, and caps stored text at 1000 bytes; raw-text construction is fallible. `ProductListingSearch` uses optional `ListingAvailabilityQuery`: exact availability and derived orderability sets intersect; `include_unspecified` separately includes absent assertions.
- `ProductListingId` is a strict UUIDv7-backed `pl_` object ID. Uses `listing-source-core` identifiers plus `money` and `localization` values; `domain-primitives` owns neutral ID/event/outcome facilities.
- No dependency on `product-listing-service` or adapters.

## Ownership

- This doc rule `src/product-listing-core/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, persisted value, or dependency edge changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep business rules here.
- No persistence, transport, runtime glue, or event serialization.

## Verification

- `cargo check -p product-listing-core`
- `cargo test -p product-listing-core --all-features`

## Child DOX Index

- None.
