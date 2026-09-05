# ProductListing domain contract

## Scope

Aura owns `ProductListing`, not `Product`. `Product` remains reserved for a future intrinsic/catalog identity. This rewrite is breaking and pre-production: no aliases, forwarding crates, compatibility routes, dual writes, migration/backfill machinery, or legacy OpenSearch aliases.

Provider-facing types may retain provider vocabulary, such as `ShopifyProductPayload`, WooCommerce topics, schema.org `Product` and `ItemAvailability`. Each provider boundary maps explicitly to Aura listing commands and values. Provider terms do not enter `product-listing-core`.

## Ubiquitous language

| Concept | Canonical name |
| --- | --- |
| Aggregate | `ProductListing` |
| IDs | `ProductListingId`, `ProductListingSlugId` (REST: `productListingTitleSlugId`), `ListingSourceId`, `SourceListingId`, `ProductListingKey` |
| Pricing, auction, image | `ProductListingPricing`, `ProductListingAuction`, `ProductListingImage` |
| Availability | `ListingAvailability` |
| Derived availability class | `ListingOrderability` |
| Catalog membership | `ListingLifecycle` |
| Explicit sold evidence | `ListingSaleObservation` |
| Search/read types | `ProductListingSearch`, `ProductListingSummary`, `ProductListingDetails*` |
| Events/repository/document | `ProductListingEvent*`, `ProductListingRepository`, `ProductListingDocument` |

Canonical crate family:

```text
product-listing-core
product-listing-service
product-listing-postgres
product-listing-opensearch
product-listing-translation-llm
```

Dependency direction remains:

```text
product-listing-core
        ▲
product-listing-service
        ▲
product-listing-postgres / product-listing-opensearch
        ▲
runtime, API, worker, crawler, provider adapters
```

## Aggregate and invariants

`ProductListing` has private fields. Its durable state includes listing identity and mutable listing facts, plus:

```rust
availability: Option<ListingAvailability>,
lifecycle: ListingLifecycle,
sale_observation: Option<ListingSaleObservation>,
pending_event: Option<ProductListingEventPayload>,
```

New listings are explicitly `Active`; `NewProductListing` does not accept lifecycle. `RehydratedProductListingState` is a `#[doc(hidden)] pub` adapter boundary that validates state and emits no events.

Required invariants:

1. `Withdrawn` implies `availability == None`.
2. `Active` may have a concrete availability or no assertion.
3. A sale observation is complete or absent.
4. A sale observation neither implies nor requires a lifecycle or availability value.
5. `SoldOut` neither implies nor requires a sale observation.
6. Ordinary listing-data mutation requires `Active`.
7. Withdrawal clears availability and retains sale observation.
8. Restore produces `Active` with no availability assertion.
9. Only explicitly named restore/upsert intent can restore a withdrawn listing.
10. Idempotent no-ops emit no event.

Aggregate mutations collect zero or one payload without creating event IDs or reading the clock. Creation emits one `PRODUCT_LISTING_DISCOVERED`; rehydrated mutation coalesces into one non-empty `PRODUCT_LISTING_CHANGED`. Service stamps that one payload with `EventId` and occurrence time before transactional persistence. Public slug generation is separate and uses a random UUID suffix. Initial discovery cannot be paired with a lifecycle transition or sale observation; those transitions are rejected rather than silently omitted. Durable image counts use fixed-width `u64` semantics, and image replacement remains a dedicated change that can retain equal counts when image identity or order changed.

## Availability and orderability

`ListingAvailability` is an optional current source assertion. It is a canonical core enum with explicit exhaustive `as_str()` and exact `from_code()`. It has no `Default`, Serde, or SQLx derive.

| Variant | Code | Meaning |
| --- | --- | --- |
| `Available` | `AVAILABLE` | Source says available without finer stock detail. |
| `InStock` | `IN_STOCK` | Ordinary current stock is available. |
| `LimitedAvailability` | `LIMITED_AVAILABILITY` | Available with explicitly limited quantity/capacity. |
| `BackOrder` | `BACK_ORDER` | Order accepted for later fulfillment. |
| `MadeToOrder` | `MADE_TO_ORDER` | Prepared or produced after order. |
| `PreOrder` | `PRE_ORDER` | Order accepted before ordinary release. |
| `PreSale` | `PRE_SALE` | Explicit pre-sale semantics distinct from pre-order. |
| `Unavailable` | `UNAVAILABLE` | Source says unavailable without a precise reason. |
| `Reserved` | `RESERVED` | Temporarily held for another buyer. |
| `OutOfStock` | `OUT_OF_STOCK` | No current stock; it may return. |
| `SoldOut` | `SOLD_OUT` | Source says sold or permanently exhausted. |

`ListingOrderability` is derived only; it is never independently persisted or mutable.

| Values | Orderability code |
| --- | --- |
| `Available`, `InStock`, `LimitedAvailability` | `ORDERABLE_NOW` |
| `BackOrder`, `MadeToOrder`, `PreOrder`, `PreSale` | `ORDERABLE_CONDITIONALLY` |
| `Unavailable`, `Reserved`, `OutOfStock`, `SoldOut` | `NOT_ORDERABLE` |

## Lifecycle and absence semantics

```rust
pub enum ListingLifecycle {
    Active,
    Withdrawn,
}
```

Codes are `ACTIVE` and `WITHDRAWN`. Withdrawal means retained history for a listing no longer offered/published by its authoritative source. It is reversible. It is not physical purge, legal deletion, or retention cleanup.

`None` for aggregate availability has one meaning only: Aura has no sufficiently reliable current availability assertion for this active listing. `None` for `ProductListingPricing.price` means Aura has no current numeric source price recorded. Neither means unchanged. Application patches use `PatchField::{Unchanged, Set, Clear}` for that separate instruction; a price clear emits the ordinary price-change event with old `Some(price)` and new `None`.

The old canonical `ProductState` vocabulary is deleted. In particular, `LISTED`, `UNKNOWN`, `REMOVED`, and `SOLD` are not Aura listing availability values. Boundary uncertainty remains adapter-local.

## Sale observation

```rust
pub struct ListingSaleObservation {
    observed_at: OffsetDateTime,
    fx_rate_id: FxRateId,
}
```

It records when Aura first recorded an explicit sold assertion and pins the FX snapshot used to value the last advertised source price. It does not claim a completed transaction or transaction amount.

Availability writes never create, overwrite, or clear an observation. Recording an equal observation is a no-op; a different observation conflicts and requires correction. Retraction is a dedicated correction operation. `SoldOut` without an observation is valid, and a retained observation may remain after withdrawal or relisting.

Use observation FX for presentation only while currently `SoldOut`, or for a deliberately historical/withdrawn presentation. An active relisted listing uses current FX.

## Behaviors and events

Canonical aggregate behaviors are `set_price`, `clear_price`, `set_availability`, `clear_availability`, `withdraw`, `restore`, `record_sale_observation`, and `retract_sale_observation`. Old `mark_*`, generic state transition, and state-machine methods are removed.

Canonical domain event codes:

```text
PRODUCT_LISTING_DISCOVERED
PRODUCT_LISTING_CHANGED
```

Discovery contains immutable source identity, initial title/description, source pricing, availability, URL, fixed-width image count, and auction. It has no title slug, image URLs, lifecycle, or sale observation. A changed event has a non-empty typed change set. Main price, minimum estimate, maximum estimate, availability, URL, image replacement, auction, lifecycle, and sale observation are separate dimensions. Ordinary value changes retain first `previous` and final `current`; net-zero value changes disappear. Image replacement has separate count fields and may retain equal cardinality. Withdrawal records a lifecycle transition with previous availability. PostgreSQL owns strict v1 DTO decoding and maps directly through an immutable event rehydration boundary; it never reconstructs a ProductListing aggregate. Canonical domain journal rows use group `DOMAIN` and schema version `1`; enrichment journal events are separate from aggregate-core payloads. Event payload enum values persist explicit canonical codes, not Rust debug output.

## Application, API, and search contracts

Create accepts `Option<ListingAvailability>`; omitted and JSON `null` create an active listing without an assertion. Update and upsert use tri-state patches for availability, main price, each price estimate, and each auction timestamp: omitted is unchanged, `null` clears, and a value sets. On creation, omitted and `null` both create no value. Upsert images are separate: omitted preserves existing images, `[]` clears them, and `null` is invalid. URL is non-clearable: omitted or `null` preserves it; a URL value sets it. Existing withdrawn listings are restored by explicit upsert intent before current facts are applied.

Withdrawal replaces normal deletion. The HTTP partner route may remain `DELETE`, but invokes `WithdrawProductListingUseCase`. Recording a sale observation is a dedicated, authorized PostgreSQL transaction that loads the aggregate and the latest FX snapshot at or before `observed_at`.

`title` and `description` are creation-only upsert inputs. For an existing listing, they preserve current state and emit no current-state history event. Responses always emit `"availability": null` when absent. Requests parse availability tri-state. Aura route and identifier vocabulary uses `product-listings`, `productListingId`, `productListingTitleSlugId`, and `sourceListingId`.

A ProductListing is authoritatively identified for partner writes by `(ListingSourceId, SourceListingId)`. `SourceListingId` is an opaque partner value: Aura trims outer Unicode whitespace, rejects blank values and embedded NUL characters, preserves case, punctuation, and internal whitespace, and accepts at most 512 UTF-8 bytes. The trimmed value is the canonical input and the only value persisted in PostgreSQL, used for the authoritative key and emitted in events; pre-trim input is not retained. It has no seller, auctioneer, Party attribution, address, or location state. The discovery event includes both immutable source identifiers. Actor attribution belongs to #1321; durable raw input to #1646; addresses to #1635.

`ProductListingSlugId` is the immutable Aura-owned public locator, exposed as `productListingTitleSlugId`. Use `raw` for a persisted value or `from_title_and_suffix` for an explicit candidate; no implicit string conversion synthesizes a locator. Aggregate creation requires the selected slug explicitly, so only collision-aware service flows choose production candidates. Aura derives a capped ASCII slug body from the creation title and appends a six-character lowercase hexadecimal suffix from a random UUID; it falls back to `listing` when no body remains and is at most 120 bytes. PostgreSQL globally enforces uniqueness of `product_listing_title_slug_id`. Public detail lookup is `GET /api/v1/product-listings/by-slug/{productListingTitleSlugId}`. There is no source-composite public locator and no source-scoped listing detail route. On a unique-slug collision, creation generates a new locator and retries persistence up to five attempts; exhausting them fails the creation. `PRODUCT_LISTING_DISCOVERED` events identify the aggregate in their envelope and intentionally omit the title slug; event consumers needing the current public locator read current aggregate state.

Public listing discovery contains active listings only. Withdrawn listings are not found by public detail and are deleted from the OpenSearch projection; restore rebuilds the projection. Public discovery does not expose a lifecycle filter. OpenSearch retains only each raw source `url`. Search and KNN batch-hydrate current ListingSource referral configuration from PostgreSQL, then derive `view_url`: Partnerize when configured, otherwise Aura UTM parameters.

`ListingAvailabilityQuery` supports exact availability values, derived orderability values, and `include_unspecified`. Exact values OR together; orderability expands to detailed values; supplying both intersects them; unspecified values only match the missing field and are optionally ORed in. Contradictory exact/orderability filters yield no concrete matches.

OpenSearch stores an active listing document with optional availability. Concrete availability serializes as its canonical code; absent availability omits the field. Missing availability queries use `must_not exists`; `UNKNOWN` is never indexed.

## Content assessment and image visibility

`ProductListingImage` is a URL-only source fact. It carries no classification, consent, or assessment lifecycle.

Listing text is assessed asynchronously after each committed `PRODUCT_LISTING_DISCOVERED` event, the sole current text source. PostgreSQL stores the optional listing-level result in `product_listing_content_assessments`, guarded by its `source_event_id`: a row is current only when it equals `product_listings.content_source_event_id`. Price, availability, URL, images, lifecycle, and enrichment revisions do not invalidate it. A future title/description event must advance `content_source_event_id` and route content assessment. Missing or stale rows mean unassessed.

`ContentPolicyDecision` is either `ALLOWED` or `REQUIRES_CONSENT(NAZI_GERMANY)`. There is no `UNKNOWN` or `NONE` policy/category value. The pure visibility rule is centralized in `product-listing-core`: callers without the stored `show_unassessed_or_sensitive_content` preference see image URLs only for a current `ALLOWED` assessment. Opted-in users see URLs for allowed, sensitive, and unassessed listings. Presentation retains image order/cardinality and redacts a hidden URL as `null`.

Assessment is enrichment, not aggregate state: it does not block source ingestion, append ProductListing events, modify the listing revision, or enter OpenSearch. Crawler, provider, and partner boundaries submit URLs only. OpenSearch keeps raw URLs for internal matching/search and has no content-policy fields.

## Source anti-corruption rules

Crawler normalization is boundary-local:

```text
Availability(value) — reliable assertion; set it
NoAssertion          — successful full page has no assertion; clear it
Ignore               — ambiguous/failed extraction; preserve current value
```

Reusable mappings persist `AVAILABILITY` with a non-null valid value or `NO_ASSERTION` with a null value. `Ignore` is not persisted. Presence is independent: reliable source removal becomes `Withdrawn`; timeouts, 5xx, parsing failure, blocking, and ambiguity never withdraw.

- schema.org directly maps supported availability meanings; `OnlineOnly`, `InStoreOnly`, and `Discontinued` remain adapter diagnostics/raw attributes and map to `NoAssertion` or `Ignore` by confidence.
- Shopify active with tracked inventory above zero sets `InStock`; all known tracked inventory at or below zero sets `OutOfStock`; missing/untracked inventory clears availability. Archived, draft, and delete evidence withdraw existing listings; draft does not create. Missing inventory is never zero and zero inventory is never `SoldOut`.
- WooCommerce published `instock`, `outofstock`, and `onbackorder` map to `InStock`, `OutOfStock`, and `BackOrder`. Trash/delete and nonpublished draft/pending/private evidence withdraw existing listings and do not create. Unsupported/missing status is non-destructive.
- Explicit crawler sold evidence may set `SoldOut`, but creates a sale observation only through the dedicated observation use case when that feature is required.

## Persistence contract

Raw source capture is separate from canonical ProductListing state. `product_listing_raw_streams` holds one mutable change-detection head per `WEB_CRAWL`, `SHOPIFY`, or `WOOCOMMERCE` source record; `product_listing_raw_revisions` holds immutable changed source evidence. A capture hashes action, payload format/version, complete semantic source JSON, provider-neutral raw values, and normalization context. Provenance and delivery IDs are retained separately and do not create a revision by themselves. The crawler uses its configured candidate URL as its `WEB_CRAWL` stream key, captures selected `RawExtractedProduct` values as `CRAWLER_EXTRACTED_PRODUCT` v1, and writes a verified removal as a raw `DELETE` before dormancy. Shopify uses canonical decimal product ID as its `SHOPIFY` stream key, preserves the complete semantic Shopify product object as `SHOPIFY_PRODUCT` v1, and maps provider status/inventory fields into generic raw intent before durable capture. Capture never writes `product_listings` or `product_listing_events`; partner ProductListing API writes remain canonical/direct.

`product-service::NormalizeProductListingRawRevisionUseCase` is the sole authoritative raw-to-canonical path. It locks one `product_listing_raw_normalization_heads` stream head, processes only its immediate next revision, writes one immutable `product_listing_raw_normalizations` terminal result, and advances the head in the same PostgreSQL transaction as zero or one canonical ProductListing event. `APPLIED`, `NO_CHANGE`, `IGNORED`, and `REJECTED` advance a stream; deployment/schema mismatches and transient persistence failures do not. The `product-listing-normalization` worker scope accepts only committed `product_listing_raw_revisions` inserts, carries typed stream/revision IDs only, and drains streams in order. Startup and periodic bounded reconciliation repair missed in-memory CDC wake-ups without delaying readiness.

The initial schema uses `product_listings`, `product_listing_events`, `product_listing_translations`, and `product_listing_watchlist`; IDs use `product_listing_id`, `product_listing_title_slug_id`, `listing_source_id`, and `source_listing_id`. `product_listings` retains the unique canonical `(listing_source_id, source_listing_id)` key for partner writes, globally enforces unique `product_listing_title_slug_id` for public lookup, and has a cascading foreign key to `listing_sources`. The initial schema rewrite is direct: no outbox, compatibility decoder, migration, backfill, or dual write exists.

Authoritative listing columns are nullable `availability`, non-null `lifecycle`, and the paired nullable `sale_observation_fx_rate_id` / `sale_observed_at`. `version` is aggregate concurrency, `current_event_id` is projection-visible state, `projection_version` is the external projection source version, `content_source_event_id` guards text-derived work, and `embedding_source_event_id` guards title/description/first-image embeddings. Discovery initializes both source markers; only image changes advance the embedding marker and clear the stored vector. PostgreSQL validates exact codes, `Withdrawn => availability IS NULL`, and the sale-observation pair. Listing address/geo and seller columns do not exist. PostgreSQL is authoritative; OpenSearch is rebuildable.

Rows keep persisted enum text as `String` and map using fallible exact canonical parsing. Invalid or noncanonical persisted values are rejected; no mapping defaults or case-normalizes corrupt state.

## Public history

`GET /api/v1/product-listings/{productListingId}/history` returns only committed domain `PRODUCT_LISTING_DISCOVERED` and `PRODUCT_LISTING_CHANGED` entries, ordered by occurrence time then event ID. One changed entry represents one committed revision and contains one deterministically ordered `changes` list. History excludes enrichment rows, storage JSON/core payload wrappers, and source image URLs.
