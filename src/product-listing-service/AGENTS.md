# DOX

## Purpose

- Own `product-listing-service` crate.
- Own canonical ProductListing use-case contracts, handlers, and outbound ports.

## Core Design

- Depends on `product-listing-core`, `listing-source-core` and `listing-source-service` for source identity and provider eligibility, `notification-core`/`notification-service`, direct `application` contracts, and product-neutral `embedding`. Listing text search passes semantic query text to `EmbeddingGenerator::embed_search_query`; embedding failure falls back to BM25.
- Root modules: `ports`, `use_case_bundle`, `use_cases`.
- Write handlers use `application::transaction::UnitOfWork` and transaction-scoped ProductListing repository/event-appender factories. `canonical_product_listing_write` is the narrow caller-owned transaction capability for another service to apply canonical ProductListing aggregate behavior and events; it is not an inbound use case and starts no transaction. `CaptureProductListingRawObservation` uses its own transaction-bound raw-capture writer, recomputes a shared typed input hash, and writes only immutable raw revisions; it never mutates canonical listings or appends ProductListing events. `AuthorizeProductListingRawCapture` is a transactional user/delegated-user preflight: it requires `ProductListingsWrite`, checks the exact ListingSource grant through the same authorizer, and commits only an allowed result; raw capture keeps its own in-transaction write authorization. Its optional provider receipt passes through the service contract; Shopify and WooCommerce adapters store only its canonical source-evidence digest, never source evidence or JSON, for a 90-day logical window, removing an expired keyed receipt in the capture transaction before reuse while `pg_ttl_index` later cleans up physical rows. Raw stream and revision IDs are strict UUIDv7-backed `prs_` and `prr_` object IDs. Raw revisions independently retain optional `source_event_id` and provenance after receipt expiry; no delivery identity or source timestamp is required for accepted intake. Listing availability is `Option<ListingAvailability>` and lifecycle is `ListingLifecycle`; generic writes do not capture FX valuations.
- Create, update, upsert, withdraw, and sale-observation writes drain zero or one core payload, stamp it with `stamp_product_listing_event`, then perform at most one repository write and append the one envelope with its matching current event ID in one transaction. Ordinary mutation results expose semantic outcomes, not event IDs. New upserts preserve absent title and description assertions. Upsert uses `PatchField` for price, estimates, availability, images, and auction bounds: unchanged preserves, set asserts a replacement, clear removes nullable state or empties images. Main-price patches preserve distinct monetary, on-request, clear, and unchanged states; estimates remain monetary. Pricing and auction patches compose one final aggregate replacement, so each emits at most one event. `ProductListing` title-slug generation is service-owned and private to this crate; default handlers use its random implementation, while focused in-file tests may inject deterministic candidates. Candidates are generated only after an authoritative missing-key lookup.
- Partner ProductListing create, update, upsert, and withdraw use cases require `ProductListingsWrite`, then authorize admins or linked partner users inside their listing transaction. Partner-key writes use `(listingSourceId, sourceListingId)`. Withdrawals use canonical ID or this key; no source/URL withdrawal path exists. Generic same-key upserts retry once in a fresh transaction only after `SourceListingAlreadyExists`. Create and generic upsert retry the exact `ProductListingTitleSlugAlreadyExists` outcome up to five times in fresh transactions while retaining the ProductListing and source identities; each attempt supplies one service-generated title-slug candidate to deterministic core construction. Provider intake owns provider mapping and raw capture; canonical mutation happens only in the raw-normalization service. Missing or already-withdrawn provider deletes are committed no-ops.
- Repository writes return persisted ProductListing state; handlers must not read after write for responses.
- OpenSearch-backed ProductListing search captures a valuation instant and loads the required FX snapshot in a short repository read transaction. The handler then compiles one `ProductListingSearchReadRequest`; OpenSearch and embedding stay outside this transaction. Its source, authenticated-user-state, and current-assessment reads use one pure assembly path and may run concurrently only when composition selects `ProductListingSearchReadExecutionPolicy::Concurrent`; default remains sequential. User state and assessments stay authoritative batch reads.
- `ProjectProductListing` rereads the committed current ProductListing source, validates the trigger event against its current event ID, then writes or deletes the rebuildable ProductListing OpenSearch projection with the authoritative monotonic projection version.
- `ProductListingUserStateReader` is an ordinary one-query batch read for relational state of OpenSearch result pages, including ordered unseen notification IDs. Its lookup contains only user and ProductListing IDs; adapters derive image safety from authoritative listing data. ProductListing details readers return the same complete `ProductListingUserState` in their caller transaction; no second notification read, per-listing read, or partial fallback.
- ProductListing detail, embedding, and history lookup support canonical ID and title-slug identity. `GetProductListingHistory` returns service-owned immutable discovery/ordered-change entries; it never exposes storage JSON or core event payload wrappers. Public service views and port models expose the title slug as `product_listing_title_slug_id`; raw source IDs remain internal to read models and responses. ProductListing detail, search, KNN, and watchlist result contracts use `application::personalized::Personalized<Item, ProductListingUserState>`; item views never inline `user_state`. Product read models carry only `ListingSourceSummary` (ListingSource ID, name, and slug) plus `SourceListingId`; they do not expose Party, seller, address, or source type data. OpenSearch search and KNN readers return factual `ProductListingSearchItem` values with `ListingSourceId`, `SourceListingId`, and raw images only. Handlers batch-resolve unique ListingSource IDs through `ListingSourceSummaryReader`, preserve hit order, then hydrate user state, batch-read assessments, and build final `ProductListingSummary` values with presented images and `content_policy`; missing source summaries and invalid source read models are explicit errors. `ProductListingDetailsReader` returns factual detail with source pricing, optional sale observation, availability, and lifecycle. `RecordProductListingSaleObservation` owns the authorized transactional FX lookup and event append; generic writes never capture FX.
- `EmbedProductListingEvent` accepts discovery or image-change sources only, checks `embedding_source_event_id`, calls `EmbeddingGenerator::embed_product`, then atomically stores the vector, appends compact `ENRICHMENT_EMBEDDED`, and advances current/projection revisions without changing aggregate version. `TranslateProductListingEvent` starts from discovery, checks `content_source_event_id`, and independently appends compact `ENRICHMENT_TRANSLATED_TITLES`; unrelated current events do not stale either flow. Completion duplicate detection is by source event, so the first committed translation wins regardless of later LLM output. Stale, duplicate, missing, ignored, and missing-title inputs are explicit outcomes.
- `GenerateWatchlistNotifications` owns ProductListing-event-driven watchlist notifications. It reads the immutable listing event/source, selects recipients by its event-time watchlist intervals, locks current `ACTIVE` lifecycle through notification and delivery-intent commit, then inserts in-app notifications and requested external delivery intents in one short PostgreSQL transaction. Unrelated later ProductListing events do not suppress this historical fact. Search-filter match notifications use the same transaction-scoped ProductListing lock without exact-current event comparison.
- `ProductListingLifecycleGuard` is a focused transaction-scoped lock/read port for authoritative ProductListing lifecycle; it returns typed invalid-persisted-state errors.
- `ProductListingWatchlistDetailsReader` is a transaction-scoped, cursor-paged batch read contract for full localized personalized ProductListing views and relational user state. Its cursor uses watchlist creation time plus ProductListing ID.
- `ProductListingDetailsBatchReader` is an ordinary one-query batch read for full localized personalized ProductListing views and relational user state by ProductListing ID; callers preserve their own source order.
- `ProductListingSearchFilterMatchSourceReader` is a transaction-scoped canonical source for accepted current ProductListing CDC events. Its `find_sources` batch accepts exact `(ProductListingId, EventId)` refs and returns typed listing sources; adapter rows stay private. `ProductListingCurrentEventGuard` batch-locks and verifies authoritative ProductListing event IDs in the final match-write transaction. `ProductListingContentAssessmentSnapshotReader` reads the current assessment by ProductListing ID and its locked content-source revision, not a caller event ID, for durable notification snapshots. `ProductListingPercolationInput` carries only source plus closed-world converted values; no adapter receives an FX snapshot.
- Capture emits metadata-only structured metric events: source method, changed/unchanged/duplicate/stale/failure outcome, latency, and JSON byte sizes. It never logs source record keys or raw JSON.
- Ports are public because adapter crates implement them.
- Port errors carry boxed sources for adapter/read-model failures; do not swallow underlying causes.
- No SQLx, OpenSearch, or transport dependency.

## Ownership

- This doc rule `src/product-listing-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, dependency edge, or use-case boundary changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep orchestration here. Keep rules in `product-listing-core`.
- Keep adapters outside.
- Keep unit tests inside the use-case file that owns the handler. No shared test-support module.

## Verification

- `cargo check -p product-listing-service`
- `cargo test -p product-listing-service --all-features`

## Child DOX Index

- None.
