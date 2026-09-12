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

`None` for aggregate availability has one meaning only: Aura has no sufficiently reliable current availability assertion for this active listing. `None` for `ProductListingPricing.price` means Aura has no current asking-price assertion. `Some(ProductListingPrice::Monetary(price))` is an explicit numeric asking price; `Some(ProductListingPrice::OnRequest)` is an explicit seller/source request for price. Neither means unchanged. Application patches use `PatchField::{Unchanged, Set, Clear}` for that separate instruction; a price clear emits the ordinary price-change event with old `Some(price)` and new `None`.

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

Canonical aggregate behaviors are `set_price`, `set_price_on_request`, `clear_price`, `set_availability`, `clear_availability`, `withdraw`, `restore`, `record_sale_observation`, and `retract_sale_observation`. Old `mark_*`, generic state transition, and state-machine methods are removed.

Canonical domain event codes:

```text
PRODUCT_LISTING_DISCOVERED
PRODUCT_LISTING_CHANGED
```

Discovery contains immutable source identity, initial title/description, source pricing, availability, URL, fixed-width image count, and auction. It has no title slug, image URLs, lifecycle, or sale observation. A changed event has a non-empty typed change set. Main price, minimum estimate, maximum estimate, availability, URL, image replacement, auction, lifecycle, and sale observation are separate dimensions. Ordinary value changes retain first `previous` and final `current`; net-zero value changes disappear. Image replacement has separate count fields and may retain equal cardinality. Withdrawal records a lifecycle transition with previous availability. PostgreSQL owns strict v1 DTO decoding and maps directly through an immutable event rehydration boundary; it never reconstructs a ProductListing aggregate. Canonical domain journal rows use group `DOMAIN` and schema version `1`; enrichment journal events are separate from aggregate-core payloads. Event payload enum values persist explicit canonical codes, not Rust debug output.

## Application, API, and search contracts

`auction: None` means no reliable auction-participation assertion. `Some(ProductListingAuction)` asserts auction participation even when its lot number, catalogue position, and timing are all absent. Lot number is an opaque 1–128-byte source label; catalogue position is an optional positive one-based `u32`. `LotAuctionTiming` has independently optional `bidding_opens`, `scheduled_closes`, and exact `reported_closed_at`; the first two retain `INSTANT` or source `DATE` precision, while date-only values never become midnight. Comparable exact bounds and source dates with the same declared timezone reject an opening after its scheduled close. Auction context is not an Auction aggregate membership in this iteration.

Create accepts `Option<ListingAvailability>`; omitted and JSON `null` create an active listing without an assertion. Main price transport is tagged `MONETARY { currency, amount }` or `ON_REQUEST`; only `MONETARY` may be converted, formatted as money, or match numeric filters. Update and upsert use tri-state patches for availability, main price, and each price estimate: omitted is unchanged, `null` clears, and a value sets. The outer typed partner `auction` context is omitted to preserve it or sent as a complete valid replacement; `null` is rejected because correction owns retraction. Raw `auction: CLEAR` preserves an existing context and yields no detach. On creation, omitted or `null` auction means no assertion, while `{}` asserts an empty context. Upsert images are separate: omitted preserves existing images, `[]` clears them, and `null` is invalid. URL is non-clearable: omitted or `null` preserves it; a URL value sets it. Existing withdrawn listings are restored by explicit upsert intent before current facts are applied.

Withdrawal replaces normal deletion. The HTTP partner route may remain `DELETE`, but invokes `WithdrawProductListingUseCase`. Recording a sale observation is a dedicated, authorized PostgreSQL transaction that loads the aggregate and the latest FX snapshot at or before `observed_at`.

`title` and `description` are creation-only upsert inputs. For an existing listing, they preserve current state and emit no current-state history event. Responses always emit `"availability": null` when absent. Requests parse availability tri-state. Aura route and identifier vocabulary uses `product-listings`, `productListingId`, `productListingTitleSlugId`, and `sourceListingId`.

A ProductListing is authoritatively identified for partner writes by `(ListingSourceId, SourceListingId)`. `SourceListingId` is an opaque partner value: Aura trims outer Unicode whitespace, rejects blank values and embedded NUL characters, preserves case, punctuation, and internal whitespace, and accepts at most 512 UTF-8 bytes. The trimmed value is the canonical input and the only value persisted in PostgreSQL, used for the authoritative key and emitted in events; pre-trim input is not retained. It has no seller, auctioneer, Party attribution, address, or location state. The discovery event includes both immutable source identifiers. Actor attribution belongs to #1321; durable raw input to #1646; addresses to #1635.

`ProductListingSlugId` is the immutable Aura-owned public locator, exposed as `productListingTitleSlugId`. Use `raw` for a persisted value or `from_title_and_suffix` for an explicit candidate; no implicit string conversion synthesizes a locator. Aggregate creation requires the selected slug explicitly, so only collision-aware service flows choose production candidates. Aura derives a capped ASCII slug body from the creation title and appends a six-character lowercase hexadecimal suffix from a random UUID; it falls back to `listing` when no body remains and is at most 120 bytes. PostgreSQL globally enforces uniqueness of `product_listing_title_slug_id`. Public detail lookup is `GET /api/v1/product-listings/by-slug/{productListingTitleSlugId}`. There is no source-composite public locator and no source-scoped listing detail route. On a unique-slug collision, creation generates a new locator and retries persistence up to five attempts; exhausting them fails the creation. `PRODUCT_LISTING_DISCOVERED` events identify the aggregate in their envelope and intentionally omit the title slug; event consumers needing the current public locator read current aggregate state.

Public listing discovery contains active listings only. Withdrawn listings are not found by public detail (by ID or title slug returns `404 PRODUCT_LISTING_NOT_FOUND`, never `410`) and are deleted from the OpenSearch projection; restore rebuilds the projection. Public discovery does not expose a lifecycle filter. OpenSearch retains only each raw source `url`. Public search alone resolves its first-page immutable FX snapshot through a process-local cache over the one-statement PostgreSQL reader; continuation pages resolve their cursor-pinned snapshot ID exactly. Exact immutable payloads use FIFO capacity eviction (default 512). Latest selections reuse only a cutoff-compatible snapshot for a fixed monotonic TTL (default 30 seconds); hits never extend that deadline, and a delayed fill is not retained. The latest selection can retain one additional immutable payload after FIFO eviction. Missing/error results and stale-on-error fallbacks are never cached. `PRODUCT_LISTING_SEARCH_FX_CACHE_ENABLED` defaults to `true`; `PRODUCT_LISTING_SEARCH_FX_CACHE_MAX_ENTRIES` accepts 1–8,192; `PRODUCT_LISTING_SEARCH_FX_LATEST_TTL_SECONDS` accepts 0–300, where zero disables only latest-selection reuse. The cache is per API process, restart-cleared, and does not make search database-independent or create an explicit FX read transaction. Transaction-scoped FX repositories remain fresh for detail, similar-listing, administrative, financial-write, and other invariant-critical flows. Public search alone also has a process-local source/referral decorator. It caches the complete `ListingSourceSummaryWithReferral` for a fixed monotonic TTL (default 60 seconds), bounded by 4,096 FIFO entries, 8 MiB accounted payload, and 16 KiB per-entry admission; one coarse fill gate still loads all page misses in one batch. `PRODUCT_LISTING_SEARCH_SOURCE_CACHE_ENABLED` defaults to `true`; `PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_ENTRIES` accepts 1–65,536, `PRODUCT_LISTING_SEARCH_SOURCE_CACHE_MAX_BYTES` accepts 1–536,870,912, and `PRODUCT_LISTING_SEARCH_SOURCE_CACHE_TTL_SECONDS` accepts 1–300 seconds. It has no negative/error cache or stale-if-error fallback. `view_url` is always rebuilt from the cached referral configuration and each hit's current raw URL. Administrative, detail, similar-listing, worker, and write reads use direct fresh source readers; public source edits may remain visible only after the fixed TTL. The endpoint's anonymous HTTP cache can add age beyond service-cache TTLs. Cache disable or rollback is a normal restart/redeploy; it does not clear HTTP caches. Per-request structured cache events carry only component/outcome, aggregate source batch/admission counts, fill-gate wait, and backend duration; they never carry cache keys, IDs, URLs, referral parameters, or payloads, and do not provision dashboards. `PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED` defaults to `false`; it may enable the same source, authenticated user-state, and current-assessment pure assembly reads concurrently only for public search. User state and assessments stay authoritative and uncached.

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

Raw source capture is separate from canonical ProductListing state. `product_listing_raw_streams` holds one mutable change-detection head per `WEB_CRAWL`, `SHOPIFY`, or `WOOCOMMERCE` source record; `product_listing_raw_revisions` holds immutable changed source evidence. A capture hashes action, payload format/version, complete semantic source JSON, provider-neutral raw values, and normalization context. Each raw revision independently persists its optional `source_event_id` and provenance. Shopify and WooCommerce receipt rows are created or reused only for provider observations that map to raw capture and store only a canonical source-evidence digest, never source evidence or JSON, for a 90-day logical window; capture deletes an expired keyed receipt before reuse and asynchronous `pg_ttl_index` cleanup reclaims physical rows. Receipt expiry never changes a raw revision. An authorized ignored WooCommerce create/update status event persists no receipt, even when it carries a delivery ID. No delivery identity or source timestamp is required for accepted intake. The crawler uses its configured candidate URL as its `WEB_CRAWL` stream key, captures selected `RawExtractedProduct` values as `CRAWLER_EXTRACTED_PRODUCT` v1, and writes a verified removal as a raw `DELETE` before dormancy. Shopify uses canonical decimal product ID as its `SHOPIFY` stream key, preserves the complete semantic Shopify product object as `SHOPIFY_PRODUCT` v1, and maps provider status/inventory fields into generic raw intent before durable capture. WooCommerce verifies untouched request bytes before parsing, uses canonical decimal product ID as its `WOOCOMMERCE` stream key, preserves the semantic product object as `WOOCOMMERCE_PRODUCT` v1, and maps topic/status/stock fields into generic raw intent before durable capture. A WooCommerce `204` acknowledges an event that maps to raw capture, including changed, unchanged, deduplicated, and stale outcomes, or an intentional no-op. An authorized ignored create/update status event returns before receipt construction and persists no receipt even when it carries a delivery ID; canonical state updates later through the normalization worker. Capture never writes `product_listings` or `product_listing_events`; partner ProductListing API writes remain canonical/direct.

One current provider-neutral raw-values schema is accepted: discriminator `1` with required `priceFormat`. `DISPLAY_TEXT` parses price patches as display text; main-price on-request markers normalize to the explicit `OnRequest` assertion, while price estimates remain monetary-only. `MACHINE_DECIMAL` requires context `fallbackCurrency` for nonblank values and accepts only full unsigned ASCII decimals. Extra fractional digits may be zero padding only, so it never truncates a nonzero minor-unit value. Crawler UPSERTs emit `DISPLAY_TEXT`; Shopify and WooCommerce UPSERTs emit `MACHINE_DECIMAL`. Provider source payload retains its price string exactly, while a blank provider price maps to a generic `CLEAR` patch. Other raw-values discriminator values are not decoded or upgraded.

`product-service::NormalizeProductListingRawRevisionUseCase` is the sole authoritative raw-to-canonical path. It locks one `product_listing_raw_normalization_heads` stream head, processes only its immediate next revision, writes one immutable `product_listing_raw_normalizations` terminal result, and advances the head in the same PostgreSQL transaction as zero or one canonical ProductListing event. `APPLIED`, `NO_CHANGE`, `IGNORED`, and `REJECTED` advance a stream. Candidate-data normalization failures become terminal `REJECTED`; a `System` normalizer configuration failure, including availability-regex compilation, returns retryable `NORMALIZATION_CONFIGURATION_FAILED` before a transaction or completion and leaves the raw revision and stream head pending. Deployment/schema mismatches and transient persistence failures also leave it pending. The `product-listing-normalization` worker scope accepts only committed `product_listing_raw_revisions` inserts, carries typed stream/revision IDs only, and drains streams in order. CDC wake-ups are durably published to scoped Standard SQS before Sequin acknowledgment. Startup and periodic bounded reconciliation also repair missed wake-ups from authoritative pending raw revisions; only its reconstructible cursor/continuation FIFO remains in memory. Capture and normalization emit metadata-only outcome, count, size, latency, and bounded-backlog signals; the operator queries are in `docs/product-listing-raw-normalization-runbook.md`.

The initial schema uses `product_listings`, listing-owned `product_listing_auction_contexts` and `product_listing_lot_auction_timings`, `product_listing_events`, `product_listing_translations`, and `product_listing_watchlist`; IDs use `product_listing_id`, `product_listing_title_slug_id`, `listing_source_id`, and `source_listing_id`. `product_listings` retains the unique canonical `(listing_source_id, source_listing_id)` key for partner writes, globally enforces unique `product_listing_title_slug_id` for public lookup, and has a cascading foreign key to `listing_sources`. Withdrawal is reversible and retains a watch row unchanged: its state, quota occupancy, and current-interval timestamps survive withdrawal and restore. An explicit physical ProductListing delete cascades listing-owned translations, events, content assessments, watchlist rows, and search-filter matches, but not immutable notification snapshots or their delivery rows. The initial schema rewrite is direct: no outbox, compatibility decoder, migration, backfill, or dual write exists.

Authoritative listing columns are nullable `availability`, non-null `lifecycle`, and the paired nullable `sale_observation_fx_rate_id` / `sale_observed_at`. `version` is aggregate concurrency, `current_event_id` is projection-visible state, `projection_version` is the external projection source version, `content_source_event_id` guards text-derived work, and `embedding_source_event_id` guards title/description/first-image embeddings. Discovery initializes both source markers; only image changes advance the embedding marker and clear the stored vector. PostgreSQL validates exact codes, `Withdrawn => availability IS NULL`, and the sale-observation pair. Listing address/geo and seller columns do not exist. PostgreSQL is authoritative; OpenSearch is rebuildable.

Rows keep persisted enum text as `String` and map using fallible exact canonical parsing. Invalid or noncanonical persisted values are rejected; no mapping defaults or case-normalizes corrupt state.

## Public history

`GET /api/v1/product-listings/{productListingId}/history` returns only committed domain `PRODUCT_LISTING_DISCOVERED` and `PRODUCT_LISTING_CHANGED` entries, ordered by occurrence time then event ID. One changed entry represents one committed revision and contains one deterministically ordered `changes` list. History excludes enrichment rows, storage JSON/core payload wrappers, and source image URLs.
