# DOX

## Purpose

- Own `product-listing-postgres` crate.
- Own canonical Product Listing SQLx adapters for Postgres.

## Core Design

- Depends on `product-listing-core`, `product-listing-service`, `listing-source-core`, `notification-core`, `domain-primitives` versioning, `money`/`localization` canonical values, and shared `platform-postgres` UoW primitives.
- Exports public SQLx Product Listing repository, event-appender, raw-capture writer, factual details, history, embedding, user-state, ListingSource-summary batch reader, batch details, batch watchlist-details, search-filter match-source reader, current-event guard, lifecycle guard, and exact content-assessment snapshot reader factories only. Factual detail, batch-detail, watchlist-detail, and match-source readers return source pricing plus optional immutable sale observation; service owns exact-FX lookup and final pricing presentation. The match-source reader loads exact `(ProductListingId, EventId)` refs with one set-based query and exposes immutable `product_listing_events.event_time`, source event kind, and current Product Listing event ID for stale-safe percolation. The current-event guard batch-locks requested `product_listings` rows `FOR SHARE` through final match commit. The lifecycle guard locks one row and reads only its strict canonical lifecycle through the caller transaction. Embedding source reader/writer classify discovery or image changes, lock/revalidate `embedding_source_event_id`, then atomically store vectors plus compact `ENRICHMENT_EMBEDDED` provenance. Translation source/writer use `content_source_event_id` and append compact translated-title provenance; unrelated current events do not stale either flow.
- The ordinary ListingSource-summary reader resolves unique ListingSource IDs to source ID, name, and slug in one set-based query for ProductListing search/KNN hydration. The ordinary Product Listing user-state reader resolves an OpenSearch result page in one set-based query: profile consent/tier, watchlist, selected search-filter match, Free-tier monthly hide state, and all unseen notification IDs ordered newest-first. Factual detail, batch-detail, and watchlist-detail readers return the same complete user state from their own SQL query.
- Keeps SQL rows, SQL, mappings, repositories, event appenders, codecs, and reader internals private. The ProductListing v1 codec maps strict Serde DTOs directly into immutable core event payloads; it never reconstructs ProductListing aggregates or allocates images from persisted counts. All ProductListing source readers share this classification and return corrupt/unsupported contracts as invalid persisted state. Content-assessment source reads normalize canonical empty title or description text to absence.
- Product Listing row, `product_listing_events` append, raw capture, and raw-normalization head/result writes bind to caller-owned transactions through service factory ports. Raw normalization locks one stream head, reads exactly its next immutable revision, records one terminal result, and advances progress atomically with canonical ProductListing mutation/event. Its pool-backed pending reader pages by `(oldest pending capture time, stream ID)` with a non-durable keyset cursor; it never persists reconciliation progress. Raw capture locks one source stream, compares only its latest canonical input hash, and inserts an immutable revision only on change; it never writes canonical ProductListing tables/events. Shopify and WooCommerce receipt rows store only a canonical source-evidence digest, never source evidence or JSON, for a 90-day logical window; capture transactionally deletes an expired keyed receipt before lookup/reuse, while `pg_ttl_index` asynchronously reclaims physical rows. Provider ordering guards store exact epoch-second/nanosecond pairs and operation-aware evidence in one final state with a NULL-safe PostgreSQL shape constraint: `NO_ORDERING` (none), `KNOWN` (ordered), or `UNKNOWN_DELETE` (timestamp-free DELETE). A timestamp-free DELETE enters `UNKNOWN_DELETE` and blocks every later UPSERT, including timestamped and redelivered UPSERTs; blocked UPSERTs write neither a receipt nor a raw revision. Only an explicit developer/operator reset or correction, after independently verifying provider state, may clear or correct that state. Future reconciliation may resolve it. Raw revisions independently retain optional `source_event_id` and provenance after receipt expiry. No delivery identity or source timestamp is required for accepted intake. `product_listings.version` is aggregate optimistic concurrency, `current_event_id` is the latest Product Listing event, and `projection_version` advances for projection-visible writes. Immutable `content_source_event_id` is initialized from `PRODUCT_LISTING_DISCOVERED` and guards text assessment/translation. `embedding_source_event_id` initializes from discovery and advances only with image changes, which clear the stored embedding atomically. The event appender binds `DOMAIN` and schema version `1`; its private codec owns strict version-1 JSON for canonical `PRODUCT_LISTING_DISCOVERED` and `PRODUCT_LISTING_CHANGED`. The history reader accepts only those domain type/version pairs and maps them to service-owned domain-history entries only; storage JSON and core event payload wrappers do not leave the adapter.
- Product Listing rows retain the authoritative `listing_source_id` plus canonical `source_listing_id` key and a globally unique, title-derived `product_listing_title_slug_id` route locator. Canonical ProductListing URLs have no `(listing_source_id, url)` B-tree, because valid canonical URLs can exceed PostgreSQL B-tree tuple limits. PostgreSQL stores only the trimmed canonical source ID, never pre-trim input; rows do not store seller or address/geo data. Material Product Listing reads join `listing_sources` for source ID, name, slug, and referral configuration in the same query; they retain raw URLs and derive outbound view URLs with `listing-source-core::outbound_url`. Product Listing `availability` is nullable canonical text; `lifecycle` is `ACTIVE` or `WITHDRAWN`, and withdrawn rows must have null availability. Source price columns contain no FX ID; paired `sale_observation_fx_rate_id` and `sale_observed_at` persist `ListingSaleObservation`. Canonical FX storage and transactional latest-snapshot reads are owned by `fxrate-postgres`. The initial event schema constrains type/group compatibility and has no delivery state or outbox copy.
- Batch watchlist details use a tie-safe `created DESC, product_listing_id ASC` cursor page with one joined query and retain watched withdrawn ProductListings. Explicit physical ProductListing deletion cascades listing-owned translations, events, assessments, watchlist rows, and search-filter matches; notification snapshots and their delivery rows remain independent.
- Real Postgres integration tests live under `tests/` by implementation file, with helpers inline per file.

## Ownership

- This doc rule `src/product-listing-postgres/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- Update this file when crate contract, dependency edge, SQL shape, or factory exports change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep adapter types private unless composition root needs factories.
- Map rows with `TryFrom`; never leak SQLx row types.
- Preserve SQLx and row-mapping failures as error sources in service port errors.

## Verification

- `cargo check -p product-listing-postgres`
- `cargo test -p product-listing-postgres --all-features`
- `cargo test -p product-listing-postgres --tests` runs real Postgres integration tests split by implementation file.

## Child DOX Index

- None.
