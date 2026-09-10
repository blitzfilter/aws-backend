# DOX

## Purpose

- Own `crawler` crate.
- Hold crawler design, operator flow, and durable crate contract.

## Core Design

- Crawler be async, Postgres-backed, LLM-assisted ingest system for antique ListingSource sites.
- Root modules: `llm_runtime`, `local_db`, `logging`, `network`, `review`, `scraper`, `service`, `spider`, `vertex_ai`.
- Main neighbors: `application`, `large-language-model`, `listing-source-core`/`listing-source-service`/`listing-source-postgres`, `localization`, `money`, `platform-postgres`, `product-listing-core`/`product-listing-service`/`product-listing-postgres`.
- Main binaries: `server`, `demo`, `demo-spider`, `demo-scraper`, `fetch-fixture`.
- `service::cron` drives ListingSource sync plus spider/scraper loops. Every spider or scraper pass completes a fresh authoritative ListingSource sync before candidate selection; sync failure fails closed and skips that pass.
- Spider and scraper cron use global slot schedulers. Refill only schedulable work; scraper fetch picks random eligible domains, takes up to 100 due URLs per domain by default, and excludes domains already seen in the pass. Spider caps one crawl at 10,000 pages and 10 minutes; its per-request timeout remains separate.
- ListingSource sync reads a complete canonical ListingSource snapshot with derived `WEB_CRAWL` enablement and optional fallback currency. It receives canonical ID/name/slug, mirrors enablement and fallback currency into local state, preserves local state while disabled, and never derives business identity from a domain or URL.
- Spider crawls configured ListingSource domains, discovers URLs, infers or refreshes domain product regexes, and batch-upserts URL metadata.
- Spider HTTP asks for `gzip, br, deflate` only; avoid zstd decode noise from bad origins. Its fetch graph is restricted to the configured exact host, which is resolved and pinned before crawling. Spider ignores non-owned pages and treats an unrelated root redirect as terminal before persistence. `SPIDER_MAX_SIZE_BYTES` is required and must be 1 MiB through the crawler's 8 MiB ceiling; Spider enforces it while streaming both declared and chunked response bodies. Page count, crawl duration, request timeout, and bounded channel remain additional rails.
- Crawler outbound HTTP accepts only HTTP(S) DNS hosts on 80/443 with no userinfo or IP literals. Spider root, scraper HTML, and image probes resolve immediately before use, reject mixed/non-public or unresolved DNS answers, pin resolved root/peer addresses, and disable automatic redirects. IPv6 destination policy admits only global-unicast `2000::/3` after special-use exclusions; it is an SSRF safety policy, not a general routing claim. Image quality cache never bypasses current target safety. Scraper HTML and image probes bound redirects, time, and bodies. Scraper follows redirects only inside the configured bare/`www` host.
- Scraper consume active and sold product URLs, fetch HTML with short inline retry backoff capped at 2s, detect stored soft-404 removed templates, reuse cached CSS selector schemas, validate plausible raw extraction, and capture changed raw observations. `Retry-After` headers must not sleep domain workers; failed URLs use `listing_source_urls.next_retry_at` after final fetch failure.
- Scraper applies all cached schemas to one parsed page, validates a candidate-local copy including images, ranks by usable completeness, then normalizes richest to least rich. Raw capture keeps the untouched extraction. Candidate-data failures reject only that schema; external/system failures abort and never trigger fresh generation. Fresh generation starts only after cached candidates exhaust, and cached schemas are never modified or generation inputs.
- Source payload preserves exact ordered image candidate groups, duplicates, relative values, and rejected candidates. Generic raw values hold crawler-validated selected image URLs in canonical order and never contain the crawler group delimiter; this derived projection participates in the raw-input hash. Extraction plausibility uses pure normalizers but stores raw strings only. A page fast path requires matching page hash and effective ordered schema-set fingerprint. Otherwise crawler hashes its generic raw-input envelope; equal input skips operational capture and refreshes local scrape metadata. Scraper handoff uses one bounded in-memory channel and one collector per scheduler pass. Producers await capacity. Partial batches flush at size, maximum age, or channel close. The collector never overlaps flushes. Each batch captures every observation through `CaptureProductListingRawObservationUseCase`, preserving enqueue order per `(ListingSourceId, candidate URL)` stream and parallelizing only independent streams. URLs are marked scraped only after `Changed`, `Unchanged`, `Duplicate`, or `Stale`. A typed missing ListingSource capture result is terminally discarded without being marked persisted; the next pass refreshes complete authoritative scope before selecting candidates and disables stale local work. Other persistence failures remain retryable. Structured logs expose enqueue wait, queue depth, oldest item age, capture latency, persistence failures, local mark failures, and successful dormant-disposition transitions without raw input.
- Scraper description text without own language signal inherits title language only when language was detected from the title itself.
- `review` own human-review rail and optional LLM-judge rail for URL patterns and schemas.
- Durable crawler identities use strict UUIDv7-backed TypeIDs: domains `cd`, reviews `cr`, review pages `crp`, and review URLs `cru`. Review routes and JSON expose TypeIDs only; Postgres keeps native UUIDs and every read maps fallibly. Schema matrices use optional `cr` review identity plus tagged persisted `crp` or input-page references. Session cookies remain operational UUIDs. Domain advisory keys fold backing UUID bytes, never TypeID text.
- Postgres is crawler truth. Main durable tables: `listing_sources`, `listing_source_domains`, `listing_source_urls`, `listing_source_product_schemas`, `listing_source_removed_page_schemas`, `crawler_reviews`, `crawler_review_pages`. URL scrape state retains page hash, schema-set fingerprint, latest captured raw-input SHA-256, disposition, and retry/failure metadata; it stores no normalized price, date, image, URL, source-ID, availability, or presence snapshot. `crawler_reviews.candidate_version` increments on candidate payload change and removes the schema-matrix cache; matrix writes carry their captured version/hash and update only `validation_summary.schema_matrix`, so stale live fetches conflict rather than overwrite review state. `crawl_enabled` gates work only; disabled sources keep local domains, configuration, reviews, and history. It defaults false.
- Domain ownership is crawler-local. Register/list/remove only through the guarded review API: `GET`/`POST /api/listing-sources/{listingSourceId}/domains`, `DELETE /api/listing-sources/{listingSourceId}/domains/{domainId}`. Missing ListingSource returns 404; first registration returns `201` with `created: true`, same-source repeat returns `200` with `created: false`. `listing_source_domain` stores canonical ownership (lowercase, no trailing dot, one leading `www.` removed); `crawl_root_host` separately preserves the requested host for exact-host spider navigation and DB checks require it to be the canonical owner or its one `www.` form. Repeated leading `www.` is rejected. Equivalent forms cannot be owned by different ListingSources. Registration rejects IP literals, cannot transfer another source domain, and never enables crawling. All review API mutations require `CRAWLER_REVIEW_AUTH_TOKEN`, even on loopback, except cookie-backed `POST /api/session/logout`; non-loopback startup also requires it. The console exchanges a bearer token for a short-lived `HttpOnly`, `SameSite=Strict` session cookie for browser-native authenticated reads and iframe previews, never ordinary mutations, without putting the token in a URL. Non-loopback use requires TLS at the serving reverse proxy because the cookie is `Secure`.
- `GET /health` is the minimal unauthenticated liveness endpoint and reveals no crawler state; `/api/health` remains authenticated when a review token is configured.
- Every URL references its owning `(listing_source_id, domain_id)` through a composite foreign key. URL hosts must match their configured domain or bare/`www` equivalent, and normal upsert cannot move an exact URL between domains. Exact URLs are globally unique; cross-source URL conflicts fail. URL-pattern state and reviews are domain-scoped through that pair; removing a domain cascades its URL-pattern reviews. Product-schema reviews remain ListingSource-scoped with null `domain_id`.
- Main handoff: ListingSource sync feeds spider; spider feeds scraper through `listing_source_urls`; scraper captures changed immutable raw observations through `product-listing-service`. The normalization worker is sole raw-to-canonical authority. Crawler never queries business ProductListing tables, appends ProductListing events, or calls canonical ProductListing mutation use cases. Crawler creates no Party, seller, or auctioneer attribution; raw marketplace seller names are not canonical attribution.
- Locking be two-layer: process-local locks stop duplicate in one process, DB lock/cooldown metadata stop bad overlap and hot-loop retries across runs after final fetch failure.
- LLM use stay bounded and explicit: URL regex inference, product schema generation, HTML-only fresh page classification, and schema evaluation. Availability normalization is synchronous and deterministic in `product-listing-normalization`; shared regex sets cache compilation failures and return a typed system error before any state resolution. Services stay generic over `large-language-model::LargeLanguageModel`; provider/model selection stays in executable wiring. `vertex_ai` wires Vertex AI Gemini with Google Application Default Credentials, while `llm_runtime` owns crawler retry, concurrency, and pacing.
- Product normalization uses `product-listing-normalization` pure APIs for candidate plausibility. Availability returns resolved, no-assertion, unsupported, or typed fail-closed error outcomes. `listing_source_urls.crawler_disposition` is `ACTIVE` or `DORMANT_SOLD`; both remain eligible for scrape and schema-seed selection. Only resolved `SoldOut` becomes `DORMANT_SOLD`, after durable raw UPSERT capture. Verified removal captures a raw DELETE, refreshes crawler scrape metadata, and returns the URL to `ACTIVE` for later rechecks. Unsupported, missing, ambiguous, invalid, and non-sold availability remain active; transient failures never change disposition. Every automatic scraper-local URL write fences on the candidate's observed raw-input SHA-256 with null-safe equality; a newer raw capture makes an older completion, classifier, or error write a no-op.
- Crawler LLM budgets be explicit: product schema generation/fresh generation and URL classification use 180 seconds. Provider retry be bounded to 3 attempts with rate-limit, outage, transient, and timeout classes. Structured-response correction be bounded to 3 fresh attempts, so one logical call can make at most 9 provider calls. The crawler LLM governor reserves future request-start slots atomically. Reservation is serialized, but waiting for a reserved slot does not hold the start-gate mutex. Provider retry sleeps still release the request permit.
- ListingSource-level LLM spend is budgeted through `listing_sources.llm_calls_count`.
- Review and schema cache be safety rail: generated artifacts can be audited, approved, or superseded.
- Schema generation and fresh single-page generation must use YAML-grounded selectors only. Prefer `null` over guessed optional-field selectors. State selector prompt must choose only availability/cart action nodes and exclude price text.
- Schema prompt DSL strips script/style and layout noise, including header/footer/nav custom elements.
- Product schemas may generate configured raw attribute selectors for review/demo/file inspection only. Missing raw attribute selector matches are skipped. Raw attributes participate in the crawler generic raw-input hash and raw-values attribute bag without crawler registry validation. New raw attribute keys need schema regeneration for existing cached ListingSource schemas.
- Initial multi-page generation accepts product schema responses only. Fresh single-page generation accepts product, removed, and not-product classifications. Removed needs verified selector-bound text or regex evidence, stores ListingSource-scoped `listing_source_removed_page_schemas`, captures a raw DELETE, then refreshes the URL as `ACTIVE` after capture commits. Not-product needs verified reason and only changes that URL class to `other`; never update a domain URL pattern from one page.
- Fresh schema generation creates a brand-new schema from the current page; it never localizes, selector-patches, or mutates a cached schema. Freshly generated schemas are only persisted after they apply and normalize successfully.
- Cached schema scoring lives in `scraper::scraper_service::extraction::schema_candidates`. Each populated prepared logical field counts once; normalized-away values score zero. Source fallback currency and URL-hash fallback IDs do not score. Stored order only breaks score ties.
- Local dev support live here too: `docker-compose.yml`, `scripts/linux/`, `scripts/windows/`, `migrations/`, and test fixtures under `tests/`.
- `fetch-fixture` writes fetched HTML to `tests/fixtures/html`.
- Demo raw-capture file snapshots are display-only, never replay input; they copy source payload, generic raw values, normalization context, and provenance unchanged.
- `server` and `demo` auto-run crawler-local migrations on startup. Migrations be authoritative crawler DB contract. This pre-production branch folds final crawler schema into its authoritative creation migrations instead of retaining temporary remediation migrations.
- `server` needs `BUSINESS_DATABASE_URL` for ListingSource reads and raw ProductListing capture. `SPIDER_MAX_SIZE_BYTES=8388608` is required for all spider-running binaries. LLM-enabled binaries need `VERTEX_AI_PROJECT_ID`, `VERTEX_AI_LOCATION`, and Google Application Default Credentials (for example `GOOGLE_APPLICATION_CREDENTIALS` locally). `VERTEX_AI_MODEL` selects schema generation/repair; `CRAWLER_VERTEX_AI_CHEAP_MODEL` and operation-specific overrides select low-risk models. `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS` and `CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS` bound all crawler LLM calls. Crawler-local state and business capture use separate Postgres transactions; a capture commit followed by a local mark failure remains retryable through `Unchanged`. Server raw-capture tuning is held in `CrawlerCronConfig`: `push_batch_size`, `push_queue_capacity`, `push_max_batch_age`, `push_max_concurrency`, and `business_db_max_connections`. These are code-level settings, not environment variables.

## Ownership

- This doc rule `src/crawler/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.
- No crawler doc rail in `docs/`. Crawler truth live here.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- Update this file when crate contract, binaries, loop flow, DB contract, review flow, env vars, scripts, or tests change.
- Update migrations in same change as durable Postgres contract change. Never patch live schema by hand and forget migration.
- If new table, index, retry field, review field, or query contract appear, document it here and cover it with tests.
- If loop cadence, candidate rules, lock semantics, retry semantics, or LLM budget semantics change, document it here in same change.
- Local DB scripts live in `scripts/linux/` and `scripts/windows/`. Keep both sides honest when workflow change.
- `server` and `demo` auto-run migrations. Keep that startup contract stable unless strong reason.
- Review mode env behavior be durable contract. Changes there need doc and test thought.
- Keep no semantic duplicate drift across code comments, tests, and this file. One crawler truth.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep trait seams clean at external edges: ListingSource source, domain configuration, fetcher, LLM, review, raw capture.
- Keep cron orchestration thin. Put real rule in spider, scraper, review, or service modules.
- Keep retry, cooldown, lock, and budget semantics explicit. Hidden side effect bad.
- Prefer append-or-upsert flows over destructive rewrite when preserving crawler history matters.
- Crawler truth live in Postgres. OpenSearch be a read-side neighbor, not crawler truth.
- Review rail be safety feature, not garnish. Keep audit fields and approval modes meaningful.
- URL classification should stay mostly deterministic after regex inference. A review-approved `NO_PATTERN` state is a completed classification and suppresses fresh inference until an explicit reset returns the domain to `UNKNOWN`. Do not turn every page decision into fresh LLM call.
- Schema repair should grow cache carefully. Bad generated schema should die fast, not poison ListingSource cache.
- Availability recognition stays pure and deterministic in `product-listing-normalization`; it never queries crawler Postgres or an LLM.
- Price normalization de-dupes repeated visible/accessibility price text only when candidates agree or one clean decimal form beats malformed visual cents.
- Keep spider per-site concurrency bounded. `spider::Website` default concurrency can explode HTTP/2 stream churn; crawler pins a conservative per-site limit and scraper-owned reqwest clients stay HTTP/1-only.
- Filter non-actionable `html5ever::tree_builder` warnings at crawler entrypoints.
- Avoid code duplication between `demo` and `server` when shared builder or service can hold it.
- Testcontainers tests be preferred proof for DB behavior. Keep fixtures focused and stable.

## Verification

- `cargo check -p crawler`
- `cargo test -p crawler --all-features`
- `cargo test -p crawler --tests`
- For DB/dev-flow change: check `docker-compose.yml`, `migrations/`, both script folders, and affected tests together.
- For LLM/review change: check candidate selection, budget accounting, and review persistence tests.

## Child DOX Index

- None.
