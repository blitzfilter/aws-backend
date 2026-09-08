# DOX

## Purpose

- Own `search-filter-service` crate.
- Own search-filter use cases and outbound ports.

## Core Design

- Depends on `search-filter-core`, owning core identifiers and values, pure `money`/`localization` values, shared `application` contracts, canonical ProductListing identifiers and availability/orderability query types from `product-listing-core`, canonical `listing-source-core` identifiers, plus canonical `user-service` tier-entitlements contracts.
- Write use cases own transactions.
- Postgres and OpenSearch hidden behind ports. Create/update pass typed semantic query text to `embedding::EmbeddingGenerator::embed_search_query`; the configured provider adapter owns model-specific prompt format. The crate-private shared ProductListing match evaluator owns the enhanced-match prompt, response schema, typed response mapping, retry classification, ordered batch mapping, bounded concurrency, and first-five-image policy; matching use cases call the neutral generic `large-language-model::LargeLanguageModel` capability. Provider/model selection stays in runtime/provider configuration.
- Repository writes return persisted search-filter state. Each aggregate repository capability has its own port file.
- User list reads live in dedicated reader port, not repository.
- Create and update lock the authoritative user tier through transaction-scoped `UserTierEntitlements` before tier checks, active-filter counts, and writes; reactivation rechecks the stored full search and active-filter quota.
- Update generates an external embedding before the short write transaction, then revalidates the derived search state before persisting.
- `RunPeriodicSearchFilterMatching` uses focused ports for run locking, window-end candidate pages, dedupe, source reads, final filter-version/activity and ProductListing current-event guards, idempotent writes, and separate progress; ordinary Search Filter views do not expose progress. Withdrawn ProductListing candidates skip evaluation and match writes, with a report count. Its short final transaction locks and exactly revalidates selected progress before any match insert or checkpoint. Checkpoints only advance; filters already covering a window are no-ops. Transient filter-local reads/writes retry with bounded delay, while malformed persisted state is terminal and isolated.
- CDC projection handlers reread complete Postgres index state and write it with only its authoritative source version. Saved-filter price ranges stay in their requested currency; ProductListing-event percolation supplies FX-valued temporary listing prices.
- Canonical ProductListing-event matching starts with a short service-owned source-read transaction, validates source identity and routed event kind, then skips sources whose current event ID differs from the trigger or whose current lifecycle is `WITHDRAWN`. For a main source price, it loads the exact sale snapshot or latest persisted snapshot at or before immutable origin event time, converts every supported currency with checked HalfUp arithmetic, and passes only an application-owned temporary percolation input to OpenSearch. Price-bearing matches persist `EVENT` or `SALE` snapshot provenance; non-price matches persist none. It reports processed, duplicate, stale, inactive-source, missing-source, and ignored-event outcomes before percolating and evaluating enhanced filters outside PostgreSQL. The final short match transaction locks and rechecks the ProductListing event ID before candidate reads and inserts, so a stale event cannot claim an idempotent match row. Its active-candidate port holds filter locks through match commit and compares the exact evaluated semantic search plus embedding (`expected_search`, `expected_embedding`), not a whole-row version. Changed matching inputs, deactivation, or deletion suppress that candidate; unrelated name/notification edits remain eligible and use the current name. Plain matches and successful enhanced matches persist there even if another enhanced candidate fails. Retryable candidate failures return only after that commit so the worker retry policy can retry them; permanent failures remain explicit in the result count and never create a match.
- `GenerateSearchFilterMatchNotification` uses one service-owned PostgreSQL snapshot to read the exact persisted match, load the ProductListing source, require current `ACTIVE` lifecycle, read its exact current content assessment into the notification snapshot, lock tier entitlements, and calculate monthly quota rank. Missing or mismatched match sources suppress as successful input handling; unrelated later ProductListing events do not suppress the persisted historical match. Each matching filter creates its own idempotent PostgreSQL notification; results distinguish a new notification from deduplication.
- Persisted-match lists compose one tie-safe match page, one factual batched ProductListing-details read that returns canonical `ProductListingUserState` including notification state, and one short PostgreSQL FX snapshot transaction (one current snapshot plus one distinct sale-snapshot batch). ProductListing presentation uses the requested Currency. Returned listing order follows the match page.

## Ownership

- This doc rule `src/search-filter-service/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Verification

- `cargo check -p search-filter-service`
- `cargo test -p search-filter-service --all-features`
