# Public ListingSource search

## Status and scope

This is the frozen implementation contract for issues #1634 and #1602. It covers one public ListingSource collection read and the existing public-by-slug URL. It does not implement frontend UI.

Iteration 0 baseline: `c10ea0f44e63f249d398c10a7211933a3c48868f` (September 11, 2026). The checkout matched this baseline and was clean. At this point no `GET /api/v1/listing-sources` collection route exists. `GET /api/v1/listing-sources/by-slug/{listingSourceSlugId}` still calls the protected shared admin detail handler and returns the admin detail shape. That wiring is replaced in later iterations; admin ID detail stays protected.

## Public policy

Every persisted row in authoritative `listing_sources` is eligible for both public reads. There is no publication state in this change. Do not filter by partnership, ingestion method, presentation URL/image, ProductListing count, or Party role/type. Partnership proposals are not ListingSources. Parties are operators, not sellers, and are never public result resources.

Both public routes use optional authentication:

- no `Authorization` header: anonymous success;
- valid supplied credential: same public fields and eligibility;
- malformed, expired, invalid, or suspended-user credential: normal authentication rejection, never anonymous fallback;
- temporary authentication failure: normal authentication failure.

The API uses `OptionalAuthExtractor`; no public use case calls an administrator check or requires a delegated capability.

Each successful public object contains exactly:

```json
{
  "listingSourceId": "ls_<TypeID>",
  "listingSourceSlugId": "immutable-source-slug",
  "name": "Müller Auktionshaus",
  "operator": { "name": "Müller Kunsthandel" },
  "url": "https://example.test/",
  "image": "https://example.test/source-image.jpg"
}
```

`url` and `image` are omitted when absent. They are parsed as canonical URLs at the PostgreSQL public-read boundary and must be `http` or `https` without embedded credentials. An unsafe required stored URL is an invalid read model: fail the whole read with a sanitized `500`; do not omit it, return a partial page, or change admin behavior.

Never expose Party IDs/slugs/contact, ingestion methods/configuration, provider data, webhook/crawler secrets, referral configuration, partnership data, timestamps, versions, normalized names, scores, highlights, or query/cursor internals.

All endpoint-owned successful and error responses use `Cache-Control: no-store`.

## Collection contract

```http
GET /api/v1/listing-sources?query=antik&size=21
```

Only these scalar query parameters are accepted once: `query`, `size`, and `searchAfter`. Unknown or repeated parameters return structured `400 BAD_QUERY_PARAMETER_VALUE` before source search. The raw query string limit is 8 KiB. `size` defaults to 21 and must be an integer from 1 through 50; it is never clamped.

`query` is NFC-normalized, Unicode-outer-whitespace-trimmed, and has internal Unicode whitespace collapsed to one ASCII space. It is limited to 1,024 UTF-8 bytes and 256 Unicode scalar values both before and after canonicalization. NUL and non-whitespace controls reject. Omitted, empty, or whitespace-only query is browse.

Input behavior:

| Canonical query | Result |
| --- | --- |
| browse | ordered full public collection |
| one scalar | terminal empty collection; no source-search transaction |
| punctuation/symbol-only | terminal empty collection; no source-search transaction |
| at least two scalars and a letter/number | normal public search |
| invalid or over limit | structured `400` |

An insufficient-input request with `searchAfter` is invalid, not browse.

A normal query is one escaped literal contiguous string. It searches only ListingSource name and operator Party name, case-insensitively and accent-folded. It does not search slug, URLs, contacts, provider data, or identifiers. `%`, `_`, backslash, quotes, and query-language punctuation are literals. There is no token reordering, full-text syntax, fuzzy fallback, similarity score, autocomplete, or suggestion endpoint.

A normalized query with one run of three consecutive Unicode letters/numbers uses literal contains matching. Other searchable input, including `mu`, `a-b`, and `ab cd`, uses whole-field left-prefix matching. Thus `mu` can match a source named `Müller Auktionshaus`, but `au` does not match that source merely because `Auktionshaus` is later. If database normalization leaves no usable searchable value, return terminal empty rather than `LIKE '%'`.

Text result ordering is:

1. exact source name;
2. source name prefix;
3. source name contains;
4. exact operator name;
5. operator name prefix;
6. operator name contains;
7. source normalized name using `C` collation;
8. ListingSource backing UUID.

Prefix search omits contains tiers. A source matched by both names appears once at its best tier. Browse uses normalized source name then backing UUID.

The response is:

```json
{
  "items": [],
  "size": 21,
  "searchAfter": "opaque-continuation"
}
```

`size` is accepted requested size. `searchAfter` is omitted on the terminal page. There is no `total`, `hasMore`, score, or match kind. The reader obtains `size + 1` globally deduplicated rows after the composite cursor predicate. It must never use offset, window pagination, counts, or a candidate/top-N cap.

The opaque cursor is URL-safe unpadded base64 of one strict current JSON shape. It contains a SHA-256 binding over endpoint, browse/text mode, canonical application query, fixed sort identity, accepted size, and the last returned `(tier, normalized-name, ListingSource ID)` position. It is limited to 4,096 encoded bytes and 3,072 decoded bytes; name key is limited to 2,048 bytes. Invalid encoding/types/fields/ID/tier/binding return `400`. It is unversioned and unsigned: validation prevents bad positioning and mismatch, not client tampering claims. Development redeployments invalidate old cursors.

For an unchanged database, continuation is strictly greater than `(tier, name_search COLLATE "C", listing_source_id)` and reaches every match once. Concurrent writes are live reads, not a multi-request snapshot.

## Slug-detail contract

```http
GET /api/v1/listing-sources/by-slug/{listingSourceSlugId}
```

This existing URL becomes public. It accepts no query parameters; nonempty query input returns structured `400`. Its raw query-string budget is 8 KiB. The decoded path value uses the existing exact `ListingSourceSlugId` parser unchanged. It is immutable navigation identity: do not trim, case-fold, unaccent, regenerate, fuzzy-match, or treat an ID as a slug.

The response is one object using the exact public allowlist above, never a collection envelope. The reader is one parameterized `listing_source_slug_id` equality lookup through its existing unique index, joined once to the operator Party for `operator.name`. A missing authoritative row returns `404 LISTING_SOURCE_NOT_FOUND`. Timeout, unavailable database, capacity exhaustion, and invalid persisted public data must not become `404`; they are `503` or sanitized `500` as applicable.

A source rename leaves its persisted slug resolvable and returns the new names. Operator rename changes `operator.name`. Hard deletion makes the old slug a real `404`. No redirects, aliases, tombstones, or name-derived slug lookup.

## Boundaries and implementation shape

Keep these admin paths and contracts unchanged:

- `GET /api/v1/admin/listing-sources` remains administrative search;
- `GET /api/v1/admin/listing-sources/{listingSourceId}` remains protected admin ID detail;
- Party admin search, ListingSource mutations, and partner ProductListing writes remain protected.

Do not remove `ensure_admin` from `GetListingSourceHandler`. Instead add separate public collection and public slug use cases, reader ports/factories, PostgreSQL readers, state fields, and public REST DTOs. The shared public service view and API DTO are used by both new reads. The old `BySlug` branch, old slug adapter read, and old payload helpers are removed only after all real callers are audited. Controllers authenticate/map/call one inbound use case; services own policy and short read transaction; PostgreSQL readers own SQL, rows, normalization, and URL mapping.

No change is authorized for the existing admin reader's `ILIKE`, aggregation, sorting, cursor, auth, DTO, or limits. Public reads do not call OpenSearch, ProductListing search, external URLs, provider readers, or per-card hydration.

No FTS, `tsvector`, stemming, token search, similarity/fuzzy matching, suggestion/autocomplete endpoint, cache, projection, CDC job, Redis, or generic search framework is part of this feature.

## Schema and PostgreSQL plan

Development only: edit `migrations/20260725090000_initial_business_schema.sql` directly. Do not add a numbered migration, backfill, dual write, compatibility helper/cursor, new API version, or online index workflow. Recreate only approved disposable databases from this final schema.

`migrations/20260725090000_initial_business_schema.sql` installs `pg_trgm` and `unaccent`; defines one schema-qualified current `aura_search_name(text)` helper; adds non-null `name_search text COLLATE "C"` to `parties` and `listing_sources`; and installs narrow insert/name/direct-derived-update maintenance triggers. The helper normalizes NFC, explicitly trims/collapses the Unicode White_Space set, applies the selected `public.unaccent` dictionary, then applies tested lowercase rules while retaining punctuation. It is `STABLE`, not falsely marked immutable. Fresh-schema tests cover extensions, decomposed accents, tabs/newlines, NBSP, narrow NBSP, ideographic space, CJK/Cyrillic, direct drift attempts, and direct/repository writes.

Iteration 3 implements PostgreSQL public reader factories. They query stored normalized columns with B-tree browse/prefix/order indexes and GIN trigram indexes for contains matching; existing `listing_sources.operator_party_id` and unique slug indexes remain. The public text reader has separate browse, prefix, and contains query families. It combines source-name and operator-name branches with `UNION ALL`, deduplicates to minimum tier before cursor filtering, then gets `size + 1`. It does not aggregate ingestion methods. The detail reader is one exact slug-index lookup with one operator join. Both readers set the 150 ms statement timeout transaction-locally and map unsafe persisted public URLs to invalid read models. REST wiring remains Iteration 4.

Iteration 0 local image check used the pinned image `ghcr.io/aura-historia/test-postgres:pg16-pgttl-3.0.0-r1` (digest `sha256:1ef4f65fa354b5771def2872dc765c5cafb5c3f1e56ce1d394a6ad4af33279be`). A new throwaway container reported PostgreSQL 16.15, UTF8, `en_US.utf8` database collation/ctype, available `pg_trgm` 1.6 and `unaccent` 1.1. Both installed successfully into `public`. The failed `SHOW lc_collate` was corrected with `pg_database.datcollate/datctype`; deployment verification must use the documented catalog query, not assume this local locale.

The test harness is the established disposable recreation path: `Postgres::new("migrations")` starts the pinned process-local Docker database, applies the schema once, and truncates application tables after each test. Run targeted real-reader tests with:

```sh
cargo test -p listing-source-postgres --tests --all-features
```

No checked-in command for recreating a shared development database was found in Iteration 0. The shared-dev owner must approve and perform its destructive recreate/reseed after this schema rewrite; do not reset an arbitrary remote/shared database.

## Limits, failure, and edge notes

Final defaults are per API process and shared by collection and slug reads: maximum four in-flight reads, 150 ms PostgreSQL statement timeout, and 500 ms route deadline. Exhaustion returns `503` with `Retry-After: 1`; no unbounded queue. Database timeout/unavailability returns `503`, never empty search or missing detail. Invalid persisted public data returns sanitized `500`.

Use `PUBLIC_LISTING_SOURCE_READ_MAX_IN_FLIGHT`, `PUBLIC_LISTING_SOURCE_READ_STATEMENT_TIMEOUT_MS`, and `PUBLIC_LISTING_SOURCE_READ_REQUEST_TIMEOUT_MS`; validate values at startup. Apply statement timeout with transaction-local PostgreSQL settings. Add bounded telemetry without raw query, slug, cursor, URL, credentials, source list, or SQL parameters.

Current infrastructure has only stage-wide API Gateway throttles: 20 requests/s with burst 50 outside production observability, and 2,000 requests/s with burst 5,000 with it. CloudFront WAF rules cover managed reputation/common/bad-input rules; Iteration 0 found no route-specific rate rule. Deployment owners must confirm anonymous edge-rate coverage for both public paths; do not invent an in-process IP limiter.

## Frontend handoff

The frontend uses only the collection endpoint for debounced typing, Enter, and continuation. Start with 180–200 ms debounce; skip IME composition; use `AbortController` and monotonically increasing request generations; reset the cursor on every query/size change; append only same-generation continuation. One-character input is an editing state. The UI must distinguish empty success from failure and honor `Retry-After`.

Use returned exact slug for a detail navigation request without auth. Do not prefetch detail per card. Required race test: send `mu`, `mul`, `muller`; let `mu` return last; only `muller` may replace results. Repeat for stale continuation, backspace, IME, and clear.

Performance targets are not measurements. Later work must add a runnable real-PostgreSQL harness and report plans, prepared generic/custom behavior, data shape, hardware, latency, cancellations, mixed load, and any unmet targets honestly.
