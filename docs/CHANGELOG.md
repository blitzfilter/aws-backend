# Changelog

All notable changes to the Aura-Historia Backend API will be documented in this file.

This changelog is for internal communication between frontend and backend teams. It documents what is new, what has changed, and what has been removed for each API update.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## 2026-08-19 - Product User-State Unseen Notification IDs

### Changed

- **Breaking:** Canonical Product user-state responses now replace `notification.seen` and `notification.originEventId` with required `notification.unseenNotificationIds`: unseen notification UUIDs ordered newest first, or `[]` when none exist.

## 2026-08-11 - Wire Canonical Google Geocoding

### Changed

- Canonical shop and partner-shop application writes now geocode supplied structured addresses through the shared Google Maps adapter. Valid addresses return the documented `geoAddress`; unavailable geocoding returns the existing temporary-service failure.

## Unreleased

### Changed

- **Breaking (#1362):** Aura-owned object IDs emitted by and accepted across REST now use strict canonical TypeID v0.3 strings with their type prefix (for example `pl_`, `ls_`, `usr_`, `psh_`, and `ntf_`); bare UUIDs, malformed or non-v7 IDs, and wrong-prefix IDs return `INVALID_OBJECT_ID`. Cognito `sub` remains an opaque provider identity resolved to an independently generated `usr_` ID. The private asynchronous worker boundary accepts schema version 2 only; development rollout requires PostgreSQL and crawler database, OpenSearch index, and queue resets, plus Cognito identity remapping after database reset.
- **Breaking:** ProductListing main asking prices now use a tagged value: `{ "type": "MONETARY", "currency": "EUR", "amount": 120000 }` or `{ "type": "ON_REQUEST" }`. This replaces the untagged monetary main-price shape in partner create/update/upsert requests and ProductListing detail, summary, history, and watchlist-notification responses. Omitted or `null` main prices still mean no asking-price assertion; price estimates remain untagged monetary `PriceData` values.
- Admin Party hard deletion is available at `DELETE /api/v1/admin/parties/{partyId}`. It hard-deletes only a Party with no `ListingSources` and no retained Partnership; both `ACTIVE` and `DISSOLVED` Partnerships block. It never cascades, rewrites, archives, or detaches business/history rows. Committed success returns bodyless `204 No Content` with `Cache-Control: no-store`; dependencies or concurrency return `409 CONFLICT`; missing and repeated deletion return `404 PARTY_NOT_FOUND`.
- Admin ListingSource hard deletion is available at `DELETE /api/v1/admin/listing-sources/{listingSourceId}`. It requires administrator authority and returns a bodyless `204 No Content` with `Cache-Control: no-store` only after its PostgreSQL transaction commits. It accepts no body or cascade switch. ProductListings (including withdrawn), raw-ingestion streams, approved application references, and retained `EXISTING_LISTING_SOURCE` proposals block with `409 CONFLICT`; repeated deletion returns `404 LISTING_SOURCE_NOT_FOUND`. Permitted deletion explicitly removes target Partnership grants and source-owned ingestion/provider configuration and secrets, but preserves Parties, Partnerships, memberships, applications, ProductListing/raw history, and unrelated records. `WEB_CRAWL` enablement is not a blocker: every new spider or scraper pass requires a successful complete authoritative ListingSource refresh before candidate selection; refresh failure skips the pass, and a deleted source is disabled locally before work. In-flight stale raw capture remains fenced by the business missing-source outcome and cannot recreate business state.
- `POST /api/v1/me/partnership-applications` returns `404 PARTNERSHIP_APPLICATION_LISTING_SOURCE_NOT_FOUND` when an `EXISTING_LISTING_SOURCE` proposal names a missing ListingSource. This protects the proposal-reference deletion race; no application is persisted.

- `WEB_CRAWL` ListingSource ingestion configuration accepts optional `fallbackCurrency` (including `ZAR`). The crawler uses it only for price strings with no currency hint; without a configured fallback it records the raw observation but omits that unresolved price assertion instead of failing the scrape.

- WooCommerce webhook capture now preserves nanosecond provider ordering and blocks an UPSERT that cannot prove it is newer than a captured DELETE. The recoverable `409 WOOCOMMERCE_PROVIDER_SOURCE_ORDER_AMBIGUOUS` creates neither a provider receipt nor raw revision, so the original request can be reconciled and retried.
- Watchlist `POST /api/v1/me/watchlist` and `PATCH /api/v1/me/watchlist/{productListingId}` now return `404 PRODUCT_LISTING_NOT_FOUND` when the ProductListing is missing and the externally visible `409 PRODUCT_LISTING_UNAVAILABLE` conflict when a withdrawn ProductListing would be watched or reactivated. Notification-only updates and deactivation remain allowed for withdrawn ProductListings; withdrawal retains existing watch state, quota occupancy, and interval timestamps. Public ProductListing detail by ID or title slug returns `404 PRODUCT_LISTING_NOT_FOUND` for withdrawn listings, never `410`.
- Admin Cognito session revocation is available at `POST /api/v1/admin/users/{userId}/sessions/revoke`. It requires the persisted `ADMIN` role; delegated Aura Historia access tokens also require `users:write`. The target user must exist, Cognito sessions are invalidated with global sign-out, and repeat requests are safe when no active sessions remain. The operation does not change the Aura user, role, tier, profile, suspension state, Aura access tokens, or partnerships. It returns `204 No Content` with `Cache-Control: no-store`; invalid object IDs use `INVALID_OBJECT_ID`, missing User/Cognito identities use `USER_NOT_FOUND`, and temporary Cognito failures use `USER_TEMPORARILY_UNAVAILABLE`. Structured logs include actor, target, request, correlation, and outcome without tokens.
- Admin user suspension is available at `PUT /api/v1/admin/users/{userId}/suspension`. It requires the persisted `ADMIN` role; delegated Aura Historia access tokens also require `users:write`. The required `{ reason }` payload is trimmed, limited to 1,000 bytes, and rejects missing, empty, whitespace-only, and common secret-bearing values with `400 BAD_BODY_VALUE`; it is included in structured operational logs, so callers must not supply tokens, passwords, credentials, or other secrets. Invalid object IDs use `INVALID_OBJECT_ID`; missing targets use `USER_NOT_FOUND`; and the final active administrator is protected with `409 CONFLICT`. The operation is idempotent, returns `200 { userId, suspended: true }`, and always sends `Cache-Control: no-store`.
- Admin user reactivation is available at `DELETE /api/v1/admin/users/{userId}/suspension`. It requires the persisted `ADMIN` role; delegated Aura Historia access tokens also require `users:write`. It accepts no body, is idempotent, returns `200 { userId, suspended: false }`, and always sends `Cache-Control: no-store`. Invalid object IDs use `INVALID_OBJECT_ID`; missing targets use `USER_NOT_FOUND`. Reactivation changes only durable suspension state: profile, tier, role, tokens, and partnership relationships remain unchanged, and valid existing credentials can authenticate normally again after commit. Successful operations are structured-logged with actor, target, request, correlation, changed, and resulting-state fields.
- Authentication now verifies the resolved User after successful Cognito JWT or Aura Historia access-token authentication. A missing or suspended User makes the supplied credential return `401 INVALID_CREDENTIALS`; temporary and internal status checks return the existing `503 AUTH_TEMPORARILY_UNAVAILABLE` and `500 AUTH_INTERNAL_ERROR` problems. Suspension therefore rejects existing Aura tokens and Cognito JWTs on later requests.
- Admin access-token metadata listing is available at `GET /api/v1/admin/users/{userId}/access-tokens`. It requires the persisted `ADMIN` role; delegated Aura Historia access tokens also require `access-tokens:read`. The explicit target user must exist, results include expired and current secret-free metadata, pages are bounded to 1–100 entries (default 21) with a `[created RFC3339 timestamp, AccessToken ID]` cursor, and every response uses `Cache-Control: no-store`; invalid object IDs use `INVALID_OBJECT_ID`, missing users use `USER_NOT_FOUND`, and raw tokens/hashes are never returned.
- Admin access-token revocation is available at `DELETE /api/v1/admin/users/{userId}/access-tokens/{accessTokenId}`. It requires the persisted `ADMIN` role; delegated Aura Historia access tokens also require `access-tokens:write`. Both typed object-ID path parameters are validated, deletion is scoped to the target user and token ID, and `204 No Content` is idempotently returned when the token was removed or already absent (including a cross-user token ID). Raw token values and hashes are never returned or logged; successful and failed operations carry structured actor, target, token, request, correlation, and outcome context.
- Admin bulk access-token revocation is available at `DELETE /api/v1/admin/users/{userId}/access-tokens`. It requires the persisted `ADMIN` role; delegated Aura Historia access tokens also require `access-tokens:write`. The typed User ID target is explicit, all target-user tokens are deleted atomically, unrelated users are untouched, and `204 No Content` is returned for an existing user with zero tokens or a repeated request. A missing target user returns `404 USER_NOT_FOUND`. Committed deletions immediately invalidate the revoked credentials. Raw token values and hashes are never returned or logged; structured logs include actor, target, affected count, request, correlation, and outcome.
- Admin overview is available at `GET /api/v1/admin/overview`. It requires the persisted `ADMIN` role, returns a bounded versioned counter-only payload (`schemaVersion: 1`), and always sends `Cache-Control: no-store`. All counts come from one authoritative PostgreSQL aggregate statement: users by tier/role, PartnershipApplications by state, Party, ListingSource and ingestion-method assignment counts, Partnership, and ProductListing lifecycle plus active-only availability counts. It exposes no PII or secrets and writes no durable admin audit event.
- Admin Partnership collection is available at `GET /api/v1/admin/partnerships`. It requires an administrator, always sends `Cache-Control: no-store`, supports exact `partyId`, `memberUserId`, and `listingSourceId` filters, and returns safe Partnership/Party summaries with member and ListingSource-grant counts. Results use bounded keyset pagination with page sizes clamped to 1–100 (default 21) in fixed `created DESC, Partnership ID DESC` order; `searchAfter` is `[created RFC3339 timestamp, Partnership ID]` and is omitted on terminal pages. Member/grant identities, Party contact data, persistence versions, provider credentials, webhook secrets, crawler-local configuration, and other secrets are not exposed.
- Admin Partnership detail is available at `GET /api/v1/admin/partnerships/{partnershipId}`. It requires an administrator, returns the Partnership identity, Party reference, current member user IDs, current ListingSource grant IDs, complete association counts, and timestamps, and always sends `Cache-Control: no-store`. Reference arrays use deterministic typed-ID ascending order and are capped at 100 entries; `PARTNERSHIP_NOT_FOUND` is returned for a missing Partnership, while full counts remain available when the cap is reached.
- Admin Partnership dissolution is available at `DELETE /api/v1/admin/partnerships/{partnershipId}`. It requires the persisted `ADMIN` role and semantically deletes the relationship: it atomically marks the Partnership `DISSOLVED` and removes all current membership and ListingSource-grant rows, so authorization ceases immediately. The Partnership row remains as a historical approved-application reference; Parties, ListingSources, users, ProductListings, and PartnershipApplications remain. Active-only Partnership collection reads exclude dissolved rows, detail reads retain history, and a future approved application for the Party reactivates the Partnership. It returns idempotent `204 No Content` for changed or already-dissolved rows, maps invalid object IDs to `INVALID_OBJECT_ID`, missing rows to `PARTNERSHIP_NOT_FOUND`, stale writes to `409 CONFLICT`, and uses `Cache-Control: no-store`.
- Admin Partnership membership grants are available at `PUT /api/v1/admin/partnerships/{partnershipId}/members/{userId}`. The operation requires the persisted `ADMIN` role, validates that both target records exist, idempotently grants membership in one PostgreSQL transaction, returns `204 No Content` for both a new membership and an existing membership, uses `Cache-Control: no-store`, and maps missing targets to `PARTNERSHIP_NOT_FOUND` or `USER_NOT_FOUND`.
- Admin Partnership membership revocation is available at `DELETE /api/v1/admin/partnerships/{partnershipId}/members/{userId}`. It requires the persisted `ADMIN` role, validates that both target records exist, removes only the targeted membership in one PostgreSQL transaction, and treats an absent membership as a successful no-op. It returns `204 No Content`, uses `Cache-Control: no-store`, preserves the user, Partnership, ListingSource, and historical PartnershipApplication records, and maps missing targets to `PARTNERSHIP_NOT_FOUND` or `USER_NOT_FOUND`.
- Admin ListingSource grants are available at `PUT /api/v1/admin/partnerships/{partnershipId}/listing-source-grants/{listingSourceId}`. The operation requires the persisted `ADMIN` role, validates that the Partnership and ListingSource exist and belong to the same Party, idempotently grants access in one PostgreSQL transaction, returns `204 No Content` for both a new and existing grant, and uses `Cache-Control: no-store`. A different Party returns `409 CONFLICT`; missing targets return `PARTNERSHIP_NOT_FOUND` or `LISTING_SOURCE_NOT_FOUND`.
- Admin ListingSource grant revocation is available at `DELETE /api/v1/admin/partnerships/{partnershipId}/listing-source-grants/{listingSourceId}`. It requires the persisted `ADMIN` role, validates that the Partnership and ListingSource exist, removes only the targeted grant in one PostgreSQL transaction, and treats an absent grant as a successful no-op. Both changed and no-op requests return `204 No Content` with `Cache-Control: no-store`; the Partnership, ListingSource, members, and historical PartnershipApplication records remain.
- **Breaking:** Admin PartnershipApplication collection search is now `GET /api/v1/admin/partnership-applications`; the old `GET /api/v1/partnership-applications` collection route is removed. The new admin collection supports exact repeated `state` and `proposalType` filters, `applicantUserId`, `listingSourceId`, inclusive RFC3339 `created` / `updated` ranges, deterministic `created` / `updated` sorting, and bounded cursor pagination with a default size of 21 and maximum size of 100. Results are review-queue summaries without persistence version values and always send `Cache-Control: no-store`.
- **Breaking:** Admin PartnershipApplication detail is now `GET /api/v1/admin/partnership-applications/{partnershipApplicationId}`; the old admin-only `GET /api/v1/partnership-applications/{partnershipApplicationId}` route is removed. The response includes the applicant, state, proposal, and nullable `approvedPartnershipId` / `approvedListingSourceId` references, and all responses send `Cache-Control: no-store`.
- **Breaking:** PartnershipApplication mark-in-review moved from `PATCH /api/v1/partnership-applications/{partnershipApplicationId}` to `PATCH /api/v1/admin/partnership-applications/{partnershipApplicationId}`; the old route is removed. The operation accepts no arbitrary state assignment and only performs the domain-authoritative `SUBMITTED` → `IN_REVIEW` transition, returns the admin application representation with `Cache-Control: no-store`, and maps invalid transitions and concurrency conflicts to `409 CONFLICT`.
- **Breaking:** PartnershipApplication decisions moved from `POST /api/v1/partnership-applications/{partnershipApplicationId}/decision` to `POST /api/v1/admin/partnership-applications/{partnershipApplicationId}/decision`; the old admin-only route is removed. Requests accept only `APPROVE` or `REJECT`, and the existing lifecycle and atomic PostgreSQL side effects are preserved.
- **Breaking:** ListingSource creation moved from `POST /api/v1/listing-sources` to `POST /api/v1/admin/listing-sources`; the legacy route is removed. The admin create response preserves the existing identity payload and returns a `Location` for `/api/v1/admin/listing-sources/{listingSourceId}`. Party operator inputs, ingestion validation, provider uniqueness conflicts, and secret-handling rules are unchanged.
- Admin ListingSource search is available at `GET /api/v1/admin/listing-sources`. It requires an administrator, supports bounded deterministic cursor pagination with page sizes clamped to 1–100 (default 21), text/name, operator Party ID, ingestion-method, and exact ListingSource ID/slug filters, and accepts `name`, `slug`, `created`, or `updated` sorting. Responses contain stable source identity, operator reference, ingestion methods, presentation, and safe referral summary only; provider/webhook secrets and crawler-local configuration are excluded, and responses always send `Cache-Control: no-store`.
- **Breaking:** ListingSource ID detail moved from `GET /api/v1/listing-sources/{listingSourceId}` to `GET /api/v1/admin/listing-sources/{listingSourceId}`. The old GET method is removed; strict ListingSource object-ID validation, not-found behavior, administrator authorization, the canonical detail representation, and provider/webhook secret exclusion remain unchanged.
- **Breaking:** ListingSource update moved from `PATCH /api/v1/listing-sources/{listingSourceId}` to `PATCH /api/v1/admin/listing-sources/{listingSourceId}`. The old admin-only route is removed; patch omission/clear semantics, immutable slugs, ingestion validation, provider uniqueness conflicts, optimistic-concurrency conflicts, and webhook-secret handling remain unchanged.
- Admin Party search is available at `GET /api/v1/admin/parties`. It supports case-insensitive `query`, `name`, `phone`, and `email` substring filters, inclusive RFC3339 `created` / `updated` ranges, deterministic `sort` / `order` values, and opaque Party ID cursor pagination. Page sizes clamp to 1–100 with a default of 21; responses contain Party identity, immutable slug, name, contact summary, and timestamps, and always send `Cache-Control: no-store`.
- Admin Party creation is available at `POST /api/v1/admin/parties`. It accepts only a Party name plus optional phone/email contact, returns the created Party representation with its immutable slug, sends `Location: /api/v1/admin/parties/{partyId}`, and uses `Cache-Control: no-store`.
- Admin Party detail is available at `GET /api/v1/admin/parties/{partyId}`. It requires an administrator, validates `partyId` as a Party object ID, returns the Party identity, immutable slug, name, contact, and timestamps, uses `Cache-Control: no-store`, and reports missing Parties with `PARTY_NOT_FOUND`.
- Admin Party updates are available at `PATCH /api/v1/admin/parties/{partyId}`. The admin-only PATCH accepts only `name`, `phone`, and `email`; omitted members remain unchanged, `null` clears optional contact members, and `name: null` is invalid. It returns the resulting Party representation with its immutable slug and `Cache-Control: no-store`; invalid values use `BAD_BODY_VALUE`, missing Parties use `PARTY_NOT_FOUND`, and stale updates use `409 CONFLICT`.
- **Breaking:** Canonical admin user search is now `GET /api/v1/admin/users`; it accepts case-insensitive `query`, `email`, `firstName`, and `lastName` filters, exact repeated `tier` / `role` filters, inclusive RFC3339 `created` / `updated` ranges, paired `sort` / `order` values, and User ID `searchAfter` pagination. The default sort is `name` ascending, page sizes are clamped to 1–100 with a default of 21, results contain compact user summaries, and responses always send `Cache-Control: no-store`.
- **Breaking:** Canonical admin user detail is now `GET /api/v1/admin/users/{userId}`. It preserves service-layer administrator authorization and the admin user representation, including `role`, `tier`, and an optional `stripeCustomerId`; responses always send `Cache-Control: no-store`.
- **Breaking:** Admin user mutation is now `PATCH /api/v1/admin/users/{userId}`; the legacy `PATCH /api/v1/users/{userId}` route is removed. The endpoint preserves profile/preferences, role, and tier updates, accepts only one change category per request, returns `Cache-Control: no-store`, and protects the last active administrator from demotion with `409 CONFLICT`.
- **Breaking:** Canonical admin user deletion is now `DELETE /api/v1/admin/users/{userId}`; the legacy `DELETE /api/v1/users/{userId}` route is removed. It requires the persisted `ADMIN` role, returns `204 No Content`, protects the final active administrator, and synchronously cascades user-owned access tokens, OAuth authorization codes, watchlist entries, saved-search filters and matches, notifications, partnership memberships, and partnership applications.
- **Breaking:** ProductListing event v1 now has strict `DOMAIN`/`PRODUCT_LISTING_DISCOVERED`, `DOMAIN`/`PRODUCT_LISTING_CHANGED`, `ENRICHMENT`/`ENRICHMENT_EMBEDDED`, and `ENRICHMENT`/`ENRICHMENT_TRANSLATED_TITLES` contracts. The initial schema, codec, CDC router, and direct fixtures reject unsupported headers and malformed payload shapes; there is no compatibility layer or outbox.
- **Breaking:** ProductListing image counts use fixed-width `int64`/`u64` semantics. Image replacement is a dedicated change and remains meaningful when previous and current counts are equal.
- **Breaking:** ProductListing event decoding now maps immutable payload values directly and never reconstructs an aggregate. Initial discovery cannot silently represent lifecycle or sale-observation transitions.
- **Changed:** Historical watchlist and search-filter notifications remain valid after unrelated later ProductListing events, while their current ProductListing lifecycle is locked through notification and delivery-intent commit. Translation and embedding redelivery are source-event idempotent; the first committed completion wins.
- **Breaking:** `GET /api/v1/product-listings/{productListingId}/history` now returns `ProductListingHistoryEntryData`, an application-owned history contract. Each `PRODUCT_LISTING_CHANGED` entry contains one deterministic ordered `payload.changes` array instead of persistence-shaped fields; discovery exposes `imageCount` only. Enrichment rows and source image URLs are excluded.
- **Breaking:** ListingSource ingestion vocabulary replaces acquisition vocabulary everywhere: `ingestionMethods`, `ingestionConfiguration`, and `requestedIngestionMethods` replace the former REST members; canonical types are `ListingIngestionMethod`, `ListingIngestionConfiguration`, and `ListingSourceIngestionConfigurations`; PostgreSQL uses `listing_source_ingestion_methods` plus provider-specific `*_ingestion_configurations` tables. The initial schema is rewritten directly; no aliases, compatibility reads, forward migration, or backfill exists. Persisted ingestion codes remain `WEB_CRAWL`, `SHOPIFY`, `WOOCOMMERCE`, and `PARTNER_API`.
- **Breaking:** ProductListing title-slug detail lookup is `GET /api/v1/product-listings/by-slug/{productListingTitleSlugId}`. The source-scoped listing route is removed. ProductListing responses expose `productListingTitleSlugId` instead of `productListingSlugId` and no longer expose `sourceListingSlugId`.
- **Breaking:** Aura-owned listing routes use `/api/v1/product-listings`, `/api/v1/product-listings/{productListingId}`, `/api/v1/listing-sources/{listingSourceId}/product-listings`, and `/api/v1/me/watchlist/{productListingId}`. Listing IDs are `productListingId`, `productListingTitleSlugId`, `listingSourceId`, and `sourceListingId`; storage and CDC use `product_listings`, `product_listing_events`, `product_listing_translations`, and `product_listing_watchlist`; errors use `PRODUCT_LISTING_*` and `LISTING_SOURCE_*`. No aliases remain.
- **Breaking:** ProductListing `availability` is optional; absent means no current source assertion. `ProductState` and lifecycle filters are removed. Discovery and saved searches filter exact `availability`, derived `orderability`, and explicitly requested unspecified availability; exact availability and orderability intersect.
- ProductListing summary and detail payloads may omit `productListingTitleSlugId` when a listing is redacted as hidden. When present, it is an 8–120 byte lowercase ASCII slug with a six-character lowercase hexadecimal suffix.
- **Breaking:** OAuth client administration collection is now `GET /api/v1/admin/oauth-clients`; the old `GET /api/v1/oauth/clients` route is removed. User and delegated-user callers require the persisted `ADMIN` role, and delegated callers also require `access-tokens:read`. The collection supports exact `clientId` and case-insensitive name filters, bounded page sizes clamped to 1–100 (default 21), fixed `created ASC, OAuthClient ID ASC` keyset pagination with a JSON `[created RFC3339 timestamp, OAuthClient ID]` `searchAfter` cursor, secret-free items, and `Cache-Control: no-store` on every response. OAuth protocol routes remain under `/api/v1/oauth`. OAuth client `client_id_issued_at` uses persisted creation metadata consistently across create, update, get, and admin-list responses.
- **Breaking:** OAuth client detail moved from `GET /api/v1/oauth/clients/{clientId}` to `GET /api/v1/admin/oauth-clients/{clientId}`. The detail response is secret-free, requires the persisted `ADMIN` role for user and delegated-user principals, requires delegated `access-tokens:read`, and always uses `Cache-Control: no-store`; invalid object IDs and missing clients use the canonical `INVALID_OBJECT_ID` and `OAUTH_CLIENT_NOT_FOUND` problems.
- **Breaking:** OAuth client creation is now `POST /api/v1/admin/oauth-clients`; the old `POST /api/v1/oauth/clients` route is removed. User and delegated-user callers require the persisted `ADMIN` role, and delegated callers also require `access-tokens:write`. The raw client secret is returned only at creation time, responses use `Cache-Control: no-store`, and `Location` points to `/api/v1/admin/oauth-clients/{clientId}`.
- **Breaking:** OAuth client metadata update is now `PATCH /api/v1/admin/oauth-clients/{clientId}`; the old `/api/v1/oauth/clients/{clientId}` PATCH route is removed. User and delegated-user callers require the persisted `ADMIN` role, delegated callers require `access-tokens:write`, and update responses are secret-free with `Cache-Control: no-store`. Patch omission/no-op behavior, HTTPS fragment-free redirect validation, supported-scope validation, and client-secret preservation remain unchanged.
- **Breaking:** OAuth client deletion is now `DELETE /api/v1/admin/oauth-clients/{clientId}`; the old `/api/v1/oauth/clients/{clientId}` DELETE route is removed. User and delegated-user callers require the persisted `ADMIN` role, delegated callers require `access-tokens:write`, and successful deletion returns `204 No Content` with `Cache-Control: no-store`. Deletion atomically removes pending authorization codes, OAuth-issued Aura access tokens, and their one-time third-party exchange codes, so the deleted client cannot authenticate or authorize future flows and already-issued credentials are revoked. Repeated or unknown client IDs return `404 OAUTH_CLIENT_NOT_FOUND`.
- OAuth redirect URI invariants are validated during domain construction and persisted-state rehydration; only non-empty HTTPS URIs without fragments are accepted.
- Party names now trim outer Unicode whitespace, reject blank values, and reject values over 255 UTF-8 bytes without truncation. New Party slugs remain immutable and use `party-<partyId>` when slugification would be empty; nested ListingSource and PartnershipApplication Party inputs return `400 BAD_BODY_VALUE` for invalid names.
- ListingSource names now trim outer Unicode whitespace, reject blank values, and reject values over 255 UTF-8 bytes without truncation. New ListingSource slugs remain immutable and use `listing-source-<listingSourceId>` when slugification would be empty; ListingSource create/update and proposed PartnershipApplication inputs return `400 BAD_BODY_VALUE` for invalid names.

### Removed

- **Breaking:** The legacy OAuth client collection route `GET /api/v1/oauth/clients` is removed; use `GET /api/v1/admin/oauth-clients`. OAuth protocol routes remain under `/api/v1/oauth`; client metadata management uses `/api/v1/admin/oauth-clients`.
- **Breaking:** The legacy OAuth client detail route `GET /api/v1/oauth/clients/{clientId}` is removed; use `GET /api/v1/admin/oauth-clients/{clientId}`.
- **Breaking:** The legacy OAuth client creation route `POST /api/v1/oauth/clients` is removed; use `POST /api/v1/admin/oauth-clients`.
- **Breaking:** The legacy OAuth client metadata update route `PATCH /api/v1/oauth/clients/{clientId}` is removed; use `PATCH /api/v1/admin/oauth-clients/{clientId}`.
- **Breaking:** The legacy OAuth client deletion route `DELETE /api/v1/oauth/clients/{clientId}` is removed; use `DELETE /api/v1/admin/oauth-clients/{clientId}`.
- **Breaking:** The admin user collection route `GET /api/v1/users` is removed and is not an alias for `GET /api/v1/admin/users`.
- **Breaking:** The legacy admin user detail route `GET /api/v1/users/{userId}` is removed; use `GET /api/v1/admin/users/{userId}`. Legacy admin PATCH and DELETE item routes are removed; use the canonical `/api/v1/admin/users/{userId}` item routes.
- **Breaking:** User account responses no longer expose address or coordinate values, and user account PATCH requests no longer accept address fields. Structured-address-backed user country, continent, and distance filters are removed.
- **Breaking:** The prior notification REST shape is replaced in `aura-historia-api`. Item paths now use canonical `{notificationId}` rather than `{eventId}`; list responses omit origin-event, actor, external-delivery, and total fields. The former root `PATCH /api/v1/me/notifications` all-notifications behavior is removed.

- Removed the obsolete key-value table, stream bridge, runtime grants, and LocalStack test support. PostgreSQL and OpenSearch are now the only application data stores in the canonical stack.
- Public ProductListing search no longer accepts `sort=price`. `GET /api/v1/product-listings` rejects it with `400 BAD_SORT_VALUE`; use `score`, `updated`, or `created`.
- ProductListing detail responses no longer emit `ETag` or `Last-Modified`. Anonymous detail display values can change when their current persisted FX snapshot changes, so they use cache freshness directives without entity validators.

### Changed

- **Breaking:** Partner ProductListing upsert applies tri-state patch semantics to `price`, `priceEstimateMin`, `priceEstimateMax`, `auctionStart`, and `auctionEnd`: omitted preserves an existing value, `null` clears it, and a value sets it. On creation, omitted and `null` both mean no value. `images` omits to preserve, uses `[]` to clear, and rejects `null`; `url` is non-clearable, so omitted or `null` preserves it. `title` and `description` are creation-only; existing-listing upserts preserve them and emit no current-state history event.
- **Breaking:** A sale observation is the explicit `SALE_OBSERVATION` fact with `observedAt`. It is recorded only by a dedicated write and is never inferred from `SOLD_OUT`; availability updates do not create, alter, or clear it. There are no sold transitions. Active relisted listings use current FX; `SOLD_OUT` and intentional withdrawn history may use the observed snapshot.
- **Breaking:** Canonical PATCH endpoints now distinguish omitted members from explicit `null`. Omitted members remain unchanged; `null` clears only documented nullable members and returns `400 BAD_BODY_VALUE` for all other members. Clients must omit members they do not intend to modify, and use empty arrays rather than `null` to clear non-null collections.
- **Breaking:** Empty HTTP bodies are invalid for object PATCH endpoints; `{}` remains a valid no-op. Partner ProductListing PATCH continues to accept `[]` as an empty batch.
- **Breaking:** OAuth client PATCH metadata no longer accepts `null`; OAuth metadata is non-nullable and clients must omit unchanged members.
- Saved-search `enhancedSearchDescription` input now trims outer whitespace and rejects blank values before embedding or persistence. Canonical values remain capped at 1000 bytes.
- **Breaking:** Watchlist price-change notifications now preserve the event’s immutable source currency in both REST and email output. The notification list no longer accepts a currency preference, and no FX conversion is applied.
- **Breaking:** Canonical notification list payloads now include their immutable, localized rendering snapshot. Watchlist and saved-search items include ProductListing/ListingSource identifiers, names, optional localized title/image, URLs, and their reason-specific change data; PartnershipApplication items include decision, Party name, ListingSource name, and optional image. `kind` remains the top-level discriminator. Origin-event and delivery/provider provenance remain hidden. Timestamps now serialize as RFC 3339.
- Canonical notification routes now use PostgreSQL-backed notification use cases: `GET`, selected-ID `PATCH`, and `DELETE /api/v1/me/notifications`; `PATCH /api/v1/me/notifications/all`; and item `PATCH`/`DELETE /api/v1/me/notifications/{notificationId}`. All mutations return `204`; list responses are `no-store` and paginate with an opaque JSON `[created RFC3339 timestamp, Notification ID]` `searchAfter` cursor, emitted only when another page exists.
- Watchlist aggregate writes now use hidden PostgreSQL optimistic-concurrency versions. Stale `PATCH` and `DELETE /api/v1/me/watchlist/{productListingId}` requests return `409 CONFLICT` instead of overwriting or deleting newer user intent; versions are not exposed in REST payloads.
- **Breaking (#1642):** ProductListing presentation now uses nullable listing-level `contentPolicy`; image entries contain only nullable URLs. Personalized listing state now exposes `contentVisibility.showUnassessedOrSensitiveContent`, the stored preference that controls presentation of unassessed or sensitive content.
- ProductListing search and similar-listing summaries expose `displayPrice` and `priceValuation` instead of ambiguous `price`. Active listings use a persisted current snapshot; a `SALE_OBSERVATION` may supply historical valuation only for current `SOLD_OUT` or withdrawn history. Listings without a main source price have no display price and never match price ranges.
- ProductListing detail, watchlist, and saved-search match reads accept optional `currency` (default `EUR`). Their `pricing` response is `{ source, display, valuation }`; valuation is `CURRENT` or `SALE_OBSERVATION` and uses `observedAt` when applicable. Missing, invalid, or mismatched FX data fails the whole read; no provider fallback occurs.
- ProductListing pricing and immutable ProductListing event pricing snapshots contain source amounts only. Sale observation is separate, explicit, and never created by a state transition or generic update.
- FX ownership now lives in the canonical `fxrate-core`, `fxrate-service`, `fxrate-postgres`, and `fxrate-fxratesapi` crate family. Immutable snapshots have a database-assigned generation and complete EUR-base `units_per_eur` quotes, including EUR at the fixed scale. Capture remains idempotent by EventBridge event ID; the public ProductListing API is unchanged in this foundation step.
- Canonical ProductListing search first pages pin the latest persisted FX snapshot and compile display price ranges into exact native active-listing intervals. Its `searchAfter` value is an opaque JSON object containing `fxRateId` and the OpenSearch token; continuation pages use that snapshot. ProductListing OpenSearch projections carry no converted top-level prices or estimates. Saved filters retain their requested price range and compile it against temporary `priceByCurrency.<currency>` fields; they store no FX ID or generation, and FX capture alone does not rewrite saved filters or alter matches.
- Saved-filter percolation values accepted current ProductListing events at their event time using the newest persisted snapshot at or before the immutable event timestamp. A `SALE_OBSERVATION` uses its observed snapshot. The temporary percolation ProductListing expands its main source price into every supported currency; stored saved-filter documents remain FX-independent. Price-bearing matches persist `EVENT` or `SALE_OBSERVATION` FX provenance; non-price matches persist no valuation provenance.
- Canonical billing route implementations are now available in `aura-historia-api` for `POST /api/v1/me/billing/checkout`, `/portal`, and `/manage`. The canonical routes use PostgreSQL User state and Stripe billing use cases. Cognito JWTs and Aura Historia access tokens are supported; delegated access tokens require `users:read`. Checkout customer association is committed before its Stripe session is created, so a later session failure leaves the customer association for safe management retry.
- `aura-historia-api` now serves `PUT /api/v1/newsletter-subscriptions` through the canonical User bounded context. It accepts anonymous, Cognito JWT, or Aura Historia access-token callers; valid authenticated callers retain user-profile fallback for omitted `firstName`, `lastName`, `language`, and `currency` fields. Invalid supplied bearer credentials return `401 INVALID_CREDENTIALS`; Zoho invalid-email responses return `400 INVALID_EMAIL`; temporary provider failures return `503 NEWSLETTER_TEMPORARILY_UNAVAILABLE`; unexpected provider failures return `500 NEWSLETTER_INTERNAL_ERROR`.
- **Breaking:** ListingSource and PartnershipApplication replace the prior listing-source and partnership-application contracts. ListingSource details/create/update, caller listing-source lists, partner ProductListing batches, and WooCommerce intake use `listingSourceId`; proposed applications carry a Party and ListingSource payload or an existing `listingSourceId`.
- PartnershipApplication decisions use the canonical PostgreSQL state machine. `APPROVE` creates any proposed Party/ListingSource, creates or finds the Party partnership, grants the applicant source access, and completes the application. `REJECT` and withdrawal do not create a proposed Party or ListingSource. Admin review and decisions accept no task token; decisions accept only `APPROVE` or `REJECT`.
- PartnershipApplication decision notifications and EMAIL delivery intents commit atomically with the decision, use the application ID for semantic identity, and render the canonical `partnership-application` templates. Sequin routes delivery inserts through the worker's durable Standard SQS queue (#1558); retention-bounded retry and external-send ambiguity are documented in the [durable-worker runbook](durable-worker-runbook.md).
- **Breaking:** WooCommerce webhook intake now verifies the untouched signed request body, retains only semantic product JSON, and maps WooCommerce topic/status/stock fields into generic raw values. A successful `204 No Content` acknowledges a durable changed, unchanged, duplicate, or stale capture outcome, or an authorized ignored create/update event with missing or unsupported status; it does not guarantee a new raw ProductListing revision. Canonical ProductListing mutation/event processing is asynchronous in the `product-listing-normalization` worker. Partner ProductListing batch endpoints remain synchronous/direct.
- WooCommerce intake treats an optional `x-wc-webhook-delivery-id` as a topic-scoped provider receipt only for an event that maps to raw capture, using the canonical semantic source-payload digest. Captured-observation retries are idempotent; reused receipt IDs with different evidence return `409 WOOCOMMERCE_PROVIDER_RECEIPT_DIGEST_CONFLICT`, while distinct evidence at the same `date_modified_gmt` source timestamp returns `409 WOOCOMMERCE_PROVIDER_SOURCE_ORDER_CONFLICT`. Without a delivery ID, there is no provider receipt or receipt-based retry-deduplication guarantee. A valid `date_modified_gmt` establishes source order only for an event that maps to raw capture, when it is a no-offset GMT value interpreted as UTC, RFC3339 `Z`, or RFC3339 `+00:00`; omitted timestamps provide no source-ordering guarantee, and malformed or nonzero-offset timestamps return `400 BAD_BODY_VALUE` only for captured events.
- An authorized ignored `product.created` or `product.updated` event with a missing or unsupported status returns before receipt construction. It persists no provider receipt and has no receipt-based retry-deduplication even when `x-wc-webhook-delivery-id` is supplied; denial returns `403 FORBIDDEN` without parsing `date_modified_gmt`.
- WooCommerce authentication failures return `401 INVALID_CREDENTIALS`. This route's only `503` outcomes are `AUTH_TEMPORARILY_UNAVAILABLE`, `LISTING_SOURCE_TEMPORARILY_UNAVAILABLE`, and `PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE`; its only `500` outcomes are `AUTH_INTERNAL_ERROR`, `LISTING_SOURCE_INTERNAL_ERROR`, and `PRODUCT_LISTING_INTERNAL_ERROR`.
- CORS preflight accepts the optional `x-wc-webhook-delivery-id` WooCommerce header.
- WooCommerce `price` uses raw-values V2 `MACHINE_DECIMAL`: a nonblank `SET` is an untrimmed unsigned ASCII `digits` or `digits.digits` value with no whitespace, signs, grouping separators, or currency symbols. Fractional digits beyond the configured currency minor-unit scale are valid only when every excess digit is `0`; a nonblank `SET` requires configured `woocommerceCurrency`.

## 2026-08-12 - Harden Canonical API Transport

### Changed

- Canonical API responses now include server-generated `X-Request-Id` and `X-Correlation-Id` headers. Clients may supply `X-Correlation-Id` only when it is a non-empty, maximum-128-character ASCII identifier using letters, digits, `.`, `_`, or `-`; invalid values are replaced by the request ID.
- The API now applies CORS, a 1 MiB request-body limit, a 30-second request timeout, and request tracing with credential headers redacted. `X-Request-Id` and `X-Correlation-Id` are exposed to browser clients.
- `GET /ready` now returns `204 No Content` only when PostgreSQL, DynamoDB, and OpenSearch required by the configured API feature set are reachable; otherwise it returns `503 Service Unavailable`. `GET /health` remains the liveness endpoint.

## 2026-08-12 - Restore Hybrid Product Search

### Changed

- `GET /api/v1/products` now uses native OpenSearch hybrid BM25 plus KNN retrieval for text queries with relevance ordering. Query-embedding failures transparently fall back to BM25; explicit price, creation, and update sorts remain BM25-only.

## 2026-08-11 - Hide Non-Published Shops from Public Reads

### Changed

- Public `GET /api/v1/shops`, `GET /api/v1/shops/{shopId}`, and `GET /api/v1/by-slug/shops/{shopSlugId}` now return only published shops. Drafted, rejected, archived, and deleted records are hidden as absent. Approving a partner-shop application publishes its linked shop atomically with the approval.

## 2026-08-11 - Enforce Cognito Access-Token Authentication

### Changed

- Canonical API runtime now verifies Cognito access JWTs with issuer, app-client, signature, and expiry checks. Cognito ID tokens are rejected; temporary JWKS retrieval failures return the documented `503 AUTH_TEMPORARILY_UNAVAILABLE` response. Aura Historia access tokens remain supported.

## 2026-08-06 - Align Search Filter and Watchlist API Contracts

### Changed

- `GET /api/v1/me/search-filters` and `GET /api/v1/me/search-filters/{userSearchFilterId}/matches` no longer expose client-controlled `sort` or `order`; both use fixed service ordering. The match cursor is supplied as a JSON-encoded `[RFC3339 timestamp, product UUID]` `searchAfter` query string.
- All migrated saved-search filter routes now document Cognito JWT and Aura access-token authentication. Aura access tokens require `search-filters:write`; missing or invalid credentials return `INVALID_CREDENTIALS`, and insufficient scope returns `FORBIDDEN`.
- `POST /api/v1/me/watchlist` now documents its shipped `{ productId, notifications? }` request and `WatchlistEntryData` response. It no longer documents `Location`, `language`, or `currency`; Aura access tokens require `watchlist:write`.
- Free saved-search filters retain legacy support for `excludeProductId` and `lifecycle` alongside `productQuery`, `price`, and `state`.

## 2026-08-06 - Enforce Canonical Watchlist Tier Quotas

### Changed

- Canonical `POST /api/v1/me/watchlist` and inactive-to-active `PATCH /api/v1/me/watchlist/{productId}` now enforce the legacy active-entry quotas: Free 20, Pro 100, Ultimate unlimited.
- Quota exhaustion returns `422` with `WATCHLIST_QUOTA_EXCEEDED`. `PATCH` again accepts `state` as documented; reactivation is subject to the same tier quota.

## 2026-08-05 - Migrate Saved Search Filters to `aura-historia-api` (`backend#1341`)

### Changed

- `aura-historia-api` now serves saved-search filter CRUD and persisted match-feedback routes through canonical search-filter service use cases and Postgres adapters:
  - `GET` and `POST /api/v1/me/search-filters`
  - `GET`, `PATCH`, and `DELETE /api/v1/me/search-filters/{userSearchFilterId}`
  - `GET /api/v1/me/search-filters/{userSearchFilterId}/matches`
  - `PATCH /api/v1/me/search-filters/{userSearchFilterId}/matches/{productId}`
- These routes accept Cognito JWTs or Aura access tokens. Delegated access tokens require `search-filters:write`.
- Filter and match reads return `Cache-Control: no-store`; creation returns relative `Location` and `Content-Language` headers.
- Match-list responses are fully hydrated localized `PersonalizedData<ProductDetailsData, ProductUserStateData>` values. They use `searchAfter` as a JSON `[match creation RFC3339 timestamp, product UUID]` cursor and default `language` to `en`; the feedback route uses canonical `productId`. The legacy `/products` preview route is intentionally not migrated to `aura-historia-api`.
- Saved-search filter create and text-search updates now create normalized 768-dimensional Gemini embeddings through Vertex AI. Provider failures return the existing temporary-service error response rather than persisting a missing embedding.

## 2026-08-05 - Synchronous Partner Product Batches (`backend#1341`)

### Changed

- Partner product `POST`, `PATCH`, and `PUT /api/v1/shops/{shopId}/products` now execute synchronously against canonical Product PostgreSQL storage. Each batch entry has its own transaction and no longer returns `202 Accepted` or forwards to an ingestion queue.
- These routes return `200 OK` with `[]` when all entries succeed. If one or more entries succeed and others fail, the `200` body lists failed `{ shopId, shopsProductId, error }` entries; `error` is the stable API error key for that item. If every non-empty batch entry fails, the first failure returns as the normal problem response.
- Product deletion is now batch `DELETE /api/v1/shops/{shopId}/products`, with an array body of `{ shopsProductId }` items. The former item delete route `DELETE /api/v1/shops/{shopId}/products/{shopsProductId}` is removed.
- Partner product writes retain admin-or-linked-partner authorization; Aura Historia access tokens require `products:write`. Batches allow at most 100 entries.


## 2026-08-04 - Personalize Canonical Watchlist Products

### Changed

- `GET /api/v1/me/watchlist` now returns the authenticated user's full canonical Product data with `userState`, not bare watchlist rows. It accepts optional `language` (default `en`) plus `size` and stable JSON-array `searchAfter` infinite-scroll pagination; it preserves watchlist creation order.
- The read uses one joined PostgreSQL query for product, localization, and relational user state, then one DynamoDB read for all user notifications. The newest notification per product supplies `userState.notification`; DynamoDB failure returns `503` rather than a partial response.

## 2026-08-04 - Document Deferred Product Currency Conversion (`backend#1466`)

### Changed

- Canonical Product detail, history, and similarity routes use either `productId` or the shop/product slug pair.
- Product prices and event-price snapshots remain stored source amounts and currencies. This historical entry predates the current detail, watchlist, and saved-match display-pricing contract documented under Unreleased.
- Similar-product KNN retrieval returns ready matches when an embedding exists; only a missing embedding returns `202 Accepted` with a polling location.
- Canonical Product detail and similar reads accept optional `language` (default `en`). Detail resolves current title/description from `product_translations`; similar reads resolve projected titles; immutable history stays in its recorded language.

## 2026-07-31 - Migrate REST APIs to `aura-historia-api` (`backend#1341`)

### Changed

- **REST API migration epic** now has `aura-historia-api` route implementations backed by canonical service use cases and Postgres/DynamoDB adapters for:
  - Product routes:
    - `GET /api/v1/products/{productId}`
    - `GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}`
    - `GET /api/v1/products/{productId}/history`
    - `GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}/history`
    - `GET /api/v1/products/{productId}/similar`
    - `GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}/similar`
    - `GET /api/v1/products`
    - `POST /api/v1/products/search`
  - Shop routes:
    - `GET /api/v1/shops/{shopId}`
    - `GET /api/v1/by-slug/shops/{shopSlugId}`
    - `GET /api/v1/shops`
    - `POST /api/v1/shops`
    - `PATCH /api/v1/shops/{shopId}`
    - `GET /api/v1/me/partner-shops`
  - User routes:
    - `GET /api/v1/me/account`
    - `PATCH /api/v1/me/account`
    - `DELETE /api/v1/me`
    - `GET /api/v1/users`
    - `GET /api/v1/users/{userId}`
    - `PATCH /api/v1/users/{userId}`
    - `DELETE /api/v1/users/{userId}`
    - `POST /api/v1/me/access-tokens`
    - `GET /api/v1/me/access-tokens`
    - `GET /api/v1/me/access-tokens/{accessTokenId}`
    - `PATCH /api/v1/me/access-tokens`
    - `DELETE /api/v1/me/access-tokens/{accessTokenId}`
  - Watchlist routes:
    - `GET /api/v1/me/watchlist`
    - `POST /api/v1/me/watchlist`
    - `PATCH /api/v1/me/watchlist/{productId}`
    - `DELETE /api/v1/me/watchlist/{productId}`
  - Partner shop application routes:
    - `GET /api/v1/me/partner-applications`
    - `POST /api/v1/me/partner-applications`
    - `GET /api/v1/me/partner-applications/{partnerApplicationId}`
    - `PATCH /api/v1/me/partner-applications/{partnerApplicationId}`
    - `DELETE /api/v1/me/partner-applications/{partnerApplicationId}`
    - `GET /api/v1/partner-applications`
    - `GET /api/v1/partner-applications/{partnerApplicationId}`
    - `PATCH /api/v1/partner-applications/{partnerApplicationId}`
    - `POST /api/v1/partner-applications/{partnerApplicationId}/decision`
- Optional Aura Historia access tokens are accepted on public read routes; invalid supplied bearer tokens return `401`.
- Admin partner-application routes now accept Cognito JWTs and Aura Historia access tokens. User callers must have stored `ADMIN` role; Aura access tokens also need `partner-shop-applications:write`.
- Admin user PATCH now accepts one logical change category per request: profile/preferences fields, `role`, or `tier`. It no longer documents `stripeCustomerId` in that payload.
- `/api/v1/me/account` now uses own-user DTOs for writes while still returning read-only `role`, `tier`, and `stripeCustomerId`; admin user routes use separate admin DTOs.
- Watchlist list reads with Aura Historia access tokens now require `watchlist:read`; watchlist writes still require `watchlist:write`.
- Partner-application payload responses now use camelCase `shopId`, matching `swagger.yaml`.
- `DELETE /api/v1/me/partner-applications/{partnerApplicationId}` now withdraws the application instead of physically deleting it.
- `GetShopData` no longer includes `createdBy` or `updatedBy` because canonical shop storage has no audit actor columns.
- Canonical product detail and search responses omit `createdBy` and `updatedBy`. Product user-state personalization remains on legacy routes.
- Product history by ID or slug uses canonical Product service/Postgres reads. It returns ordered immutable event snapshots with source `price`, estimate bounds, and `fxRateId`; it has no currency-conversion parameter.
- Canonical Product pricing fields are `price`, `priceEstimateMin`, and `priceEstimateMax`; the old public `native*` names are removed. Similar-product discovery returns KNN matches when an embedding is ready and `202 Accepted` only while it is pending.
- Migrated API errors use `application/problem+json` and stable `ApiErrorCode` constants.

## 2026-07-15 - Add User Measurement Unit Preference

### Added

- **User account preference payloads**
  - `GetUserAccountData`, `PatchUserAccountData`, and `PatchAdminUserData` now support optional `measurementUnit`.
  - Accepted values are `METRIC` and `IMPERIAL`.

## 2026-07-09 - Add Partner Product Delete

### Added

- **`DELETE /api/v1/shops/{shopId}/products/{shopsProductId}`**
  - Allows admins and users partnered with the shop to soft-delete a product.
  - Returns `200 OK` after appending a lifecycle delete event and marking the
    materialized product record as `DELETED`.
- **Product lifecycle fields**
  - Product read response schemas now expose `lifecycle` (`ACTIVE`, `DELETED`).
  - Product search keeps deleted products hidden by default; lifecycle is not a
    user-facing search input.

## 2026-06-28 - Add Product ID Exclusion to Product Search

### Added

- **`ProductSearchData.excludeProductId`**
  - New optional array of product IDs for excluding specific products from search results.
  - Supported by product search requests and saved search filter create/response payloads.

## 2026-06-17 - Move Enhanced Search Description Into Product Search & Product Search Accepts Multiple Text Queries

### Changed

- **Saved search filter DTOs**
  - `enhancedSearchDescription` moved from `PostUserSearchFilterData`,
    `PatchUserSearchFilterData`, and `UserSearchFilterData` into their nested
    `search` / `ProductSearchData` payload.
  - PATCH requests now update the enhanced search description via
    `search.enhancedSearchDescription`.
- **`ProductSearchData.productQuery`**
  - Changed from a single optional string to an array of strings.
  - Empty arrays are allowed and mean no text query.
  - When multiple queries are provided, product search and saved-search-filter
    matching OR them together while preserving the existing matching logic for
    each individual query.
- **Affected endpoints**
  - `GET /api/v1/products`
  - `POST /api/v1/products/search`
  - `POST /api/v1/me/search-filters`
  - `PATCH /api/v1/me/search-filters/{userSearchFilterId}`
  - Search-filter response payloads

## 2026-06-13 - Search Filter Product Preview Uses Percolator Matching

### Changed

- **`GET /api/v1/me/search-filters/{userSearchFilterId}/products`**
  - Now uses the same percolator-style product matching query as search-filter
    product-match processing instead of normal hybrid product search.
  - No longer supports client pagination. Supplying `size` or `searchAfter`
    now returns `400 Bad Request` with `BAD_QUERY_PARAMETER_VALUE`.
  - Always returns a fixed preview of up to 10 products.
  - For filters with `enhancedSearchDescription`, products rejected by enhanced
    matching are omitted from the preview.

## 2026-06-09 - Hydrate Existing Partner Shop Applications

### Changed

- **Partner shop application responses**
  - `GetPartnerShopApplicationPayloadData` for `type: "EXISTING"` now returns
    `shop: GetShopData` instead of only `shopId`.
  - Create requests for existing shops still submit `shopId` via
    `PostPartnerShopApplicationPayloadData`.

## 2026-05-31 - Shop Lookup by Domain and OpenAPI Drift Repair

This update realigns the internal OpenAPI spec with the backend's current shop lookup, saved-search, and notification contracts. It adds the missing domain-based shop lookup endpoint, documents cache behavior for slug-based shop lookups, and corrects request/response schema details for saved-search creation, saved-search match feedback, and notification pagination metadata.

### Added

- **New shop lookup endpoint**
  - `GET /api/v1/by-domain/shops/{shopDomain}`
  - Path parameter: `shopDomain` (configured shop domain; primary domains and subdomains are accepted)
  - Authentication: optional
    - unauthenticated requests return cacheable shared responses
    - authenticated Cognito JWT or Aura Historia access-token requests return `Cache-Control: no-store`
  - Response: `200 OK` with `GetShopData`
  - Response headers: `Last-Modified`, `Cache-Control`
  - Error responses:
    - `400 Bad Request` for missing or invalid `shopDomain`
    - `404 Not Found` / `SHOP_NOT_FOUND`

### Changed

- **`GET /api/v1/by-slug/shops/{shopSlugId}`**
  - The `200 OK` response now explicitly documents the `Cache-Control` header.
  - Cache behavior is now described as:
    - `public, max-age=600, s-maxage=3600` for unauthenticated requests
    - `no-store` for authenticated requests

- **`POST /api/v1/me/search-filters`**
  - `PostUserSearchFilterData.enhancedSearchDescription` is now documented as nullable.
  - Clients may send explicit JSON `null` for this optional field.

- **`PATCH /api/v1/me/search-filters/{userSearchFilterId}/matches/{shopId}/{shopsProductId}`**
  - `PatchUserSearchFilterMatchData.feedback` is now documented as nullable.
  - Clients may send explicit JSON `null` in addition to boolean feedback values.

- **Notification collection pagination metadata**
  - `NotificationCollectionData.size` description is corrected.
  - `size` now explicitly documents the requested page size echoed from the cursor payload; it is not guaranteed to equal `items.length`.
  - This affects:
    - `GET /api/v1/me/notifications`
    - `PATCH /api/v1/me/notifications`

### Removed

- No documented endpoints or schemas were removed in this update.

## 2026-05-31 - Audit Actor Metadata on Response DTOs (`backend#1113`)

Backend PR `#1113` adds actor-based audit metadata to multiple REST response models. The backend now exposes both the creator and the last updater on affected resources as plain strings: either the literal `SYSTEM` or the UUID of the acting user.

### Added

- **New shared schema**
  - `ActorData`

- **New response fields**
  - `createdBy`
  - `updatedBy`

  These fields are now documented on:
  - `GetProductData`
  - `GetProductSummaryData`
  - `UserSearchFilterData`
  - `SearchFilterProductMatchData`
  - `GetNotificationData`
  - `GetPartnerShopApplicationData`
  - `GetShopData`
  - `GetAccessTokenData`
  - `GetUserAccountData`

### Changed

- **Product responses now include audit actors**
  - Product detail and product-summary payloads now expose `createdBy` and `updatedBy`.
  - This affects product data returned by:
    - `GET /api/v1/shops/{shopId}/products/{shopsProductId}`
    - `GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}`
    - `GET /api/v1/shops/{shopId}/products/{shopsProductId}/similar`
    - `GET /api/v1/products`
    - `POST /api/v1/products/search`
    - `GET /api/v1/me/watchlist`
    - `POST /api/v1/me/watchlist`
    - `PATCH /api/v1/me/watchlist/{shopId}/{shopsProductId}`
    - `GET /api/v1/me/search-filters/{userSearchFilterId}/products`
    - `GET /api/v1/me/search-filters/{userSearchFilterId}/matches`

- **Saved-search and match responses now include audit actors**
  - `UserSearchFilterData` and `SearchFilterProductMatchData` now return `createdBy` and `updatedBy`.
  - This affects:
    - `POST /api/v1/me/search-filters`
    - `GET /api/v1/me/search-filters/{userSearchFilterId}`
    - `PATCH /api/v1/me/search-filters/{userSearchFilterId}`
    - `PATCH /api/v1/me/search-filters/{userSearchFilterId}/matches/{shopId}/{shopsProductId}`

- **Notification responses now include audit actors**
  - `GetNotificationData` now returns `createdBy` and `updatedBy`.
  - This affects:
    - `GET /api/v1/me/notifications`
    - `PATCH /api/v1/me/notifications`
    - `PATCH /api/v1/me/notifications/{eventId}`

- **Partner shop application responses now include audit actors**
  - `GetPartnerShopApplicationData` now returns `createdBy` and `updatedBy`.
  - This affects:
    - `GET /api/v1/me/partner-applications`
    - `POST /api/v1/me/partner-applications`
    - `GET /api/v1/me/partner-applications/{partnerApplicationId}`
    - `PATCH /api/v1/me/partner-applications/{partnerApplicationId}`
    - `GET /api/v1/partner-applications`
    - `GET /api/v1/partner-applications/{partnerApplicationId}`
    - `PATCH /api/v1/partner-applications/{partnerApplicationId}`
    - `POST /api/v1/partner-applications/{partnerApplicationId}/decision`

- **Shop responses now include audit actors**
  - `GetShopData` now returns `createdBy` and `updatedBy`.
  - This affects every endpoint that returns shop data, including:
    - `GET /api/v1/shops`
    - `POST /api/v1/shops`
    - `GET /api/v1/shops/{shopId}`
    - `PATCH /api/v1/shops/{shopId}`
    - `POST /api/v1/shops/search`
    - `GET /api/v1/me/partner-shops`

- **Access token responses now include audit actors**
  - `GetAccessTokenData` now returns `createdBy` and `updatedBy`.
  - This affects:
    - `GET /api/v1/me/access-tokens`
    - `POST /api/v1/me/access-tokens`
    - `GET /api/v1/me/access-tokens/{accessTokenId}`
    - `PATCH /api/v1/me/access-tokens/{accessTokenId}`

- **User account responses now include audit actors**
  - `GetUserAccountData` now returns `createdBy` and `updatedBy`.
  - This affects:
    - `GET /api/v1/me`
    - `PATCH /api/v1/me`
    - `GET /api/v1/users/{userId}` (admin)

### Removed

- No documented endpoints or schemas were removed in this update.

## 2026-05-31 - OAuth Client URI Metadata and Redirect URI Validation (`backend#1117`, `backend#1116`)

Backend PR `#1117` adds the RFC 7591-style URI metadata fields to OAuth client payloads, and backend PR `#1116` tightens OAuth redirect URI handling so malformed URI values are rejected at the API boundary. This update realigns the internal OpenAPI spec with both backend changes.

### Added

- **New OAuth client metadata URI fields**
  - `tos_uri`
  - `policy_uri`
  - `client_uri`
  - `logo_uri`

  These fields are now documented on:
  - `POST /api/v1/oauth/clients`
  - `GET /api/v1/oauth/clients`
  - `GET /api/v1/oauth/clients/{clientId}`
  - `PATCH /api/v1/oauth/clients/{clientId}`
  - `OAuthClientMetadataRequestData`
  - `OAuthClientMetadataPatchData`
  - `OAuthClientMetadataResponseData`

### Changed

- **OAuth client metadata payload shapes**
  - OAuth client create requests now require `tos_uri`, `policy_uri`, `client_uri`, and `logo_uri` in addition to `client_name` and `redirect_uris`.
  - OAuth client patch requests now support updating those four URI metadata fields individually.
  - OAuth client create/list/get/update response examples now expose the stored URI metadata fields.

- **OAuth redirect URI validation on authorization and token exchange**
  - `GET /api/v1/oauth/authorize` now documents `400 BAD_QUERY_PARAMETER_VALUE` for malformed `redirect_uri` query values.
  - `POST /api/v1/oauth/token` now documents `400 BAD_BODY_VALUE` for malformed `redirect_uri` form values.

### Removed

- No endpoints or documented schemas were removed in this update.

## 2026-05-29 - OAuth Client Metadata Management (`backend#1110`)

Backend PR `#1110` adds OAuth client metadata management endpoints so backend admins can create, inspect, update, and delete OAuth clients through the API. This update realigns the internal OpenAPI spec with that backend contract and also documents the tightened UUID-based `client_id` handling on the existing OAuth authorization, token, revocation, and introspection endpoints.

### Added

- **New OAuth client metadata management endpoints**

  | Endpoint | Purpose |
  |---|---|
  | `POST /api/v1/oauth/clients` | Create a new OAuth client metadata record and return the plaintext client secret once. |
  | `GET /api/v1/oauth/clients` | List registered OAuth client metadata records. |
  | `GET /api/v1/oauth/clients/{clientId}` | Get one OAuth client metadata record by client ID. |
  | `PATCH /api/v1/oauth/clients/{clientId}` | Update OAuth client metadata (`client_name`, `redirect_uris`, `scope`). |
  | `DELETE /api/v1/oauth/clients/{clientId}` | Delete an OAuth client metadata record. |

- **New schemas**
  - `OAuthClientMetadataRequestData`
  - `OAuthClientMetadataPatchData`
  - `OAuthClientMetadataResponseData`

### Changed

- **OAuth client identifiers are now UUIDs across the OAuth flow**
  - `client_id` is now documented as a UUID/UUIDv7-style identifier on:
    - `GET /api/v1/oauth/authorize`
    - `POST /api/v1/oauth/token`
    - `POST /api/v1/oauth/revoke`
    - `POST /api/v1/oauth/introspect`
  - The OpenAPI examples and validation/error documentation now reflect `INVALID_UUID` responses when an OAuth `client_id` cannot be parsed.
  - `OAuthIntrospectionResponseData.client_id` is now documented as the UUID of the issuing OAuth client.

- **OAuth client secret exposure differs by operation**
  - `POST /api/v1/oauth/clients` returns the raw `client_secret` once at creation time.
  - List/get/update responses document the masked stored secret display value instead (for example `aurahistoria_oauth_client_secret_abcdefghijk_****`).

### Removed

- Nothing was removed from the API in this update.

## 2026-05-28 - OAuth2 Authorization Code Flow (`backend#1103`)

Backend PR `#1103` introduces a full OAuth2 authorization code flow with PKCE, allowing third-party OAuth clients to obtain Aura Historia access tokens on behalf of authenticated users. This update adds all four new OAuth endpoints and two new response schemas to the OpenAPI spec.

### Added

- **New OAuth2 endpoints**

  | Endpoint | Purpose |
  |---|---|
  | `GET /api/v1/oauth/authorize` | Start the authorization code flow. The authenticated user (resource owner) grants the OAuth client access. Returns a `302` redirect to `redirect_uri?code=<code>&state=<state>`. |
  | `POST /api/v1/oauth/token` | Exchange a single-use authorization code for an Aura Historia access token. Body is `application/x-www-form-urlencoded`. |
  | `POST /api/v1/oauth/revoke` | Revoke an access token issued via the OAuth flow. Body is `application/x-www-form-urlencoded`. |
  | `POST /api/v1/oauth/introspect` | Return metadata about an access token. Body is `application/x-www-form-urlencoded`. |

  **Authorization endpoint** (`GET /api/v1/oauth/authorize`) query parameters:
  - `response_type` (required): must be `code`
  - `client_id` (required): registered OAuth client ID
  - `redirect_uri` (required): must exactly match a registered redirect URI for the client
  - `scope` (optional): space-separated scope string (e.g. `products:write`)
  - `state` (optional): opaque CSRF protection value — echoed back in the redirect
  - `code_challenge` (required): PKCE S256 code challenge (base64url-encoded SHA256 of `code_verifier`)
  - `code_challenge_method` (required): must be `S256`

  Requires a valid Cognito bearer token (the authenticated user authorizes the client).

  **Token endpoint** (`POST /api/v1/oauth/token`) form fields:
  - `grant_type` (required): must be `authorization_code`
  - `code` (required): authorization code from the authorize redirect
  - `redirect_uri` (required): must match the redirect URI used in the authorization request
  - `client_id` (required)
  - `client_secret` (required)
  - `code_verifier` (required): PKCE verifier whose SHA256 hash matches the `code_challenge`

  No bearer token required — client authenticates via `client_id` / `client_secret` form fields.

  **Revoke endpoint** (`POST /api/v1/oauth/revoke`) and **introspect endpoint** (`POST /api/v1/oauth/introspect`) form fields:
  - `token` (required): the access token to revoke/introspect
  - `client_id` (required)
  - `client_secret` (required)

  No bearer token required for these endpoints either.

- **New schemas**
  - `OAuthTokenResponseData` — response body of `POST /api/v1/oauth/token`:
    - `access_token` (string): the issued Aura Historia bearer token
    - `token_type` (AccessTokenTypeData): always `BEARER`
    - `expires_in` (integer, nullable): seconds until expiry; `null` for non-expiring tokens
    - `scope` (string): space-separated scopes granted to the token
  - `OAuthIntrospectionResponseData` — response body of `POST /api/v1/oauth/introspect`:
    - `active` (boolean, **required**): `true` if the token is valid and not revoked; `false` otherwise
    - `scope` (string, optional): space-separated scopes (present when `active` is `true`)
    - `client_id` (string, optional): OAuth client that issued the token (present when `active` is `true` and token was issued via OAuth)
    - `sub` (string/uuid, optional): Aura Historia user ID of the resource owner (present when `active` is `true`)
    - `token_type` (string, optional): always `Bearer` when `active` is `true`
    - `exp` (integer, optional): Unix timestamp expiry (present when `active` is `true` and the token has an expiry)
    - `iat` (integer, optional): Unix timestamp issued-at (present when `active` is `true` and the issue time is known)

### Changed

- **`AccessToken` domain type** now carries an `origin` field (`User` or `OAuth { client_id }`). This is an internal domain change; the `GetAccessTokenData` REST response schema is **not** changed — the `origin` field is not exposed to API consumers.

### Removed

- Nothing was removed from the API in this update.

## 2026-05-28 - Access Token Deletion Path Parameter (`backend#1105`)

Backend PR `#1105` changes access-token deletion to use a resource path parameter instead of a query parameter. This update realigns the internal OpenAPI spec with that backend contract and documents the updated request path and validation behavior for frontend consumers.

### Added

- No new endpoints or documented schemas were added in this update.

### Changed

- **`DELETE /api/v1/me/access-tokens/{accessTokenId}`**
  - The access token to delete is now identified by the required `{accessTokenId}` path parameter instead of the old `accessTokenId` query parameter.
  - `400` responses now document path-parameter validation errors (`BAD_PATH_PARAMETER_VALUE` / `INVALID_UUID`) consistent with the backend's shared access-token path parsing.

### Removed

- **Removed obsolete delete route documentation**
  - `DELETE /api/v1/me/access-tokens?accessTokenId=...`

## 2026-05-28 - Scoped Access Tokens for Partner APIs (`backend#1095`)

Backend PR `#1095` refactors partner API keys into user-owned Aura Historia access tokens, adds self-service access-token management endpoints, and realigns partner/shop authorization flows around bearer authentication. This update realigns the internal OpenAPI spec with the merged backend contract by documenting the new access-token CRUD routes, the renamed/self-scoped partner-shop lookup, the bearer-authenticated partner ingestion flows, and the removal of obsolete API-key-specific documentation.

### Added

- **New self-service access-token management endpoints**

  | Endpoint | Purpose |
  |---|---|
  | `POST /api/v1/me/access-tokens` | Create a new Aura Historia access token for the authenticated user. |
  | `GET /api/v1/me/access-tokens` | List the authenticated user's non-expired access tokens. |
  | `GET /api/v1/me/access-tokens/{accessTokenId}` | Get one non-expired access token by ID. |
  | `PATCH /api/v1/me/access-tokens` | Update access-token metadata (`name`, `scope`, `expiresAt`). |
  | `DELETE /api/v1/me/access-tokens?accessTokenId=...` | Delete one access token by query parameter. |

  Newly documented schemas:
  - `AccessTokenScopeData`
  - `AccessTokenTypeData`
  - `GetAccessTokenData`
  - `PostAccessTokenData`
  - `PatchAccessTokenData`

- **New bearer security scheme for Aura Historia access tokens**
  - `AccessTokenAuth` documents bearer tokens created through `/api/v1/me/access-tokens`.
  - Aura Historia access tokens are sent as standard bearer authorization headers.

- **New self-scoped partner-shop listing endpoint**
  - `GET /api/v1/me/partner-shops` replaces the old partner-ID-based shop lookup for non-admin callers.
  - The response remains an array of `GetShopData` and returns `[]` when the authenticated user has no linked partner shops.

### Changed

- **Partner batch product ingestion now uses bearer authentication**
  - `POST /api/v1/shops/{shopId}/products`
  - `PATCH /api/v1/shops/{shopId}/products`
  - `PUT /api/v1/shops/{shopId}/products`

  These endpoints no longer use `x-api-key`. They now require:
  - a Cognito bearer token for the partner user linked to the shop, or
  - an Aura Historia access token owned by that partner user.

  Additionally:
  - Aura Historia access tokens on these three endpoints must include the `products:write` scope.
  - `401` responses now cover missing/invalid bearer tokens and unknown access tokens instead of partner API-key mismatches.
  - `403` responses now cover authenticated callers that are not linked to the requested partner shop.

- **`POST /api/v1/webhooks/woocommerce/{shopId}` authentication**
  - The webhook route now authenticates the caller with the `Authorization` bearer header instead of `x-api-key`.
  - The WooCommerce signature requirements (`x-wc-webhook-topic`, `x-wc-webhook-signature`) are unchanged.

- **`PATCH /api/v1/shops/{shopId}` authentication**
  - The shop-update route now accepts Aura Historia bearer access tokens for the partner user linked to the shop.
  - The obsolete partner API-key authentication alternative is no longer documented.

### Removed

- **Removed obsolete API-key-only documentation**
  - `PUT /api/v1/shops/{shopId}/api-key`
  - `GET /api/v1/partner/{partnerId}/shops`
  - `PartnerApiKeyAuth`
  - `PartnerShopApiKeyResponse`

## 2026-05-25 - Partner Application Notification Shop Logos (`backend#1087`)

Backend PR `#1087` extends partner-application notifications so the REST notification payload can now expose an optional shop logo URL. This update realigns the internal OpenAPI spec with that backend contract and documents where frontend consumers can expect the new field.

### Added

- **New optional shop-logo field on partner-application notification payloads**

  | Schema / field | Type | Required | Description |
  |---|---|---|---|
  | `PartnerApplicationNotificationPayloadData.image` | `string (uri) \| null` | No | Optional shop logo URL sourced from the partner application itself for `NEW` applications or from the linked existing shop for `EXISTING` applications. |

  Exposed anywhere `GetNotificationData` / `NotificationCollectionData` returns a `PARTNER_APPLICATION` payload, including:
  - `GET /api/v1/me/notifications`
  - `PATCH /api/v1/me/notifications`
  - `PATCH /api/v1/me/notifications/{eventId}`

### Changed

- **`PARTNER_APPLICATION` notification payload shape**
  - `NotificationPayloadData` now documents that partner-application notification payloads may include an optional `image` field containing the shop logo URL.
  - The field is omitted when no logo URL is available.

### Removed

- No endpoints or documented schemas were removed in this update.

## 2026-05-22 - Partner Product Upsert Update Semantics (`backend#1079`)

Backend PR `#1079` deprecates removal of stored product prices on the upsert-update path. While verifying that change against the `develop` branch, the internal OpenAPI spec also corrected older stale `PUT /api/v1/shops/{shopId}/products` documentation so it now matches the fields the backend actually applies when an upsert targets an existing product.

### Added

- No new endpoints or documented schemas were added in this update.

### Changed

- **`PUT /api/v1/shops/{shopId}/products` existing-product semantics**
  - The update path now documents the backend's actual field coverage for existing products:
    - `price`
    - `priceEstimateMin`
    - `priceEstimateMax`
    - `state`
    - `url`
    - `images`
    - `auctionStart`
    - `auctionEnd`
  - The update path still ignores create-only fields:
    - `title`
    - `description`
    - `sellerName`
    - `structuredAddress`
    - `geoAddress`

- **`PutProductData.price`**
  - The spec now documents the backend `#1079` behavior: omitting `price` or sending `null` no longer removes an existing stored price during upsert updates.
  - A non-null `price` value still updates the stored price on existing products.

- **Other `PutProductData` update-path field semantics**
  - `priceEstimateMin`, `priceEstimateMax`, `url`, `auctionStart`, and `auctionEnd` are now documented as updateable for existing products.
  - For those fields, omitting the property or sending `null` leaves the stored value unchanged.
  - `images` is now documented as replacing the stored image set on update; omitting it or sending `null` is treated as an empty list and therefore clears stored images.

- **Examples**
  - The `PUT /api/v1/shops/{shopId}/products` request examples now include:
    - an existing-product update that leaves the current price unchanged while updating other supported fields
    - an explicit image-clearing example via `images: null`

### Removed

- No endpoints or documented schemas were removed in this update.

## 2026-05-20 - Async Partner Product Ingestion Queue (`backend#1070`)

Backend PR `#1070` moves partner product write requests and WooCommerce product webhooks behind an asynchronous ingestion queue. This update realigns the internal OpenAPI spec with the merged backend contract by documenting the new `202 Accepted` behavior, the new queue-forwarding failure semantics, and the removal of obsolete synchronous response types.

### Added

- **Queue-forwarding failure documentation for existing write endpoints**

  | Endpoint | New status | Error code | Meaning |
  |---|---|---|---|
  | `POST /api/v1/shops/{shopId}/products` | `503 Service Unavailable` | `SERVICE_UNAVAILABLE` | All product-create commands failed to be forwarded to the asynchronous ingestion queue. |
  | `PATCH /api/v1/shops/{shopId}/products` | `503 Service Unavailable` | `SERVICE_UNAVAILABLE` | All product-update commands failed to be forwarded to the asynchronous ingestion queue. |
  | `PUT /api/v1/shops/{shopId}/products` | `503 Service Unavailable` | `SERVICE_UNAVAILABLE` | All product-upsert commands failed to be forwarded to the asynchronous ingestion queue. |
  | `POST /api/v1/webhooks/woocommerce/{shopId}` | `503 Service Unavailable` | `SERVICE_UNAVAILABLE` | The WooCommerce webhook event could not be forwarded to the asynchronous ingestion queue. |

- **Shared async enqueue response schema**

  | Schema | Type | Description |
  |---|---|---|
  | `PartnerProductEnqueueFailuresResponse` | `array[string]` | Lists only the `shopsProductId` values that failed to be forwarded to the asynchronous ingestion queue. An empty array means full acceptance for asynchronous processing. |

### Changed

- **Partner batch product write endpoints now return `202 Accepted`**
  - `POST /api/v1/shops/{shopId}/products`
  - `PATCH /api/v1/shops/{shopId}/products`
  - `PUT /api/v1/shops/{shopId}/products`

  All three endpoints now document asynchronous acceptance instead of synchronous persistence:
  - successful queue forwarding returns `202 Accepted`
  - the success body is now a JSON array of failed `shopsProductId` queue-forwarding attempts
  - a fully successful request now returns `[]` instead of an `errors` object

- **`PATCH /api/v1/shops/{shopId}/products` semantics**
  - The endpoint documentation now clarifies that product updates are only accepted for asynchronous processing during the HTTP request.
  - Product existence is no longer documented as a synchronous request-time guarantee; it is evaluated later during ingestion.

- **`PUT /api/v1/shops/{shopId}/products` semantics**
  - The endpoint documentation now clarifies that upsert commands are accepted asynchronously.
  - The create-vs-update decision is documented as happening during later ingestion rather than during the HTTP request itself.

- **`POST /api/v1/webhooks/woocommerce/{shopId}` success response**
  - Successful queue forwarding now returns `202 Accepted`.
  - The success response body was removed; the endpoint now returns an empty body on success.

### Removed

- **Obsolete synchronous response schemas**
  - `PostProductsResponse`
  - `PatchProductsResponse`
  - `PutProductsResponse`
  - `WoocommerceWebhookResponse`

## 2026-05-19 - Affiliate View URLs for Shops, Products, and Notifications (`backend#1066`)

Backend PR `#1066` separates raw destination URLs from user-facing tracked/affiliate links across shop, product, and notification read models. This update realigns the internal OpenAPI spec with the merged backend contract by documenting the new `viewUrl` fields, correcting the semantics of existing `url` fields, and refreshing every affected response example.

### Added

- **New `viewUrl` field on shop read DTOs**

  | Schema / field | Type | Required | Description |
  |---|---|---|---|
  | `GetShopData.viewUrl` | `string (uri) \| null` | No | Optional tracked or affiliate URL derived from `GetShopData.url` for user-facing navigation. |

  Exposed anywhere `GetShopData` is returned, including:
  - `POST /api/v1/shops`
  - `GET /api/v1/shops/{shopId}`
  - `PATCH /api/v1/shops/{shopId}`
  - `GET /api/v1/by-slug/shops/{shopSlugId}`
  - `POST /api/v1/shops/search`
  - `GET /api/v1/partner/{partnerId}/shops`

- **New `viewUrl` field on product read DTOs**

  | Schema / field | Type | Required | Description |
  |---|---|---|---|
  | `GetProductData.viewUrl` | `string (uri)` | Yes | Tracked or affiliate URL generated by the backend for user-facing navigation to a product detail. |
  | `GetProductSummaryData.viewUrl` | `string (uri)` | Yes | Tracked or affiliate URL generated by the backend for user-facing navigation in product list/summary responses. |

  Exposed anywhere personalized or non-personalized product read models are returned, including:
  - `GET /api/v1/shops/{shopId}/products/{shopsProductId}`
  - `GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}`
  - `GET /api/v1/shops/{shopId}/products/{shopsProductId}/similar`
  - `POST /api/v1/products/search`
  - `GET /api/v1/me/watchlist`
  - `POST /api/v1/me/watchlist`
  - `PATCH /api/v1/me/watchlist/{shopId}/{shopsProductId}`
  - `GET /api/v1/me/search-filters/{userSearchFilterId}/products`
  - `GET /api/v1/me/search-filters/{userSearchFilterId}/matches`

- **New URL fields on notification product payloads**

  | Schema / field | Type | Required | Description |
  |---|---|---|---|
  | `WatchlistNotificationPayloadData.url` | `string (uri)` | Yes | Raw product URL on the shop website. |
  | `WatchlistNotificationPayloadData.viewUrl` | `string (uri)` | Yes | Tracked or affiliate product URL for user-facing navigation. |
  | `SearchFilterNotificationPayloadData.url` | `string (uri)` | Yes | Raw product URL on the shop website. |
  | `SearchFilterNotificationPayloadData.viewUrl` | `string (uri)` | Yes | Tracked or affiliate product URL for user-facing navigation. |

  Exposed on notification payloads returned by:
  - `GET /api/v1/me/notifications`
  - `PATCH /api/v1/me/notifications`
  - `PATCH /api/v1/me/notifications/{eventId}`

### Changed

- **Shop read URL semantics**
  - `GetShopData.url` is now documented as the raw primary shop URL.
  - The tracked or affiliate navigation target previously shown on `GetShopData.url` is now documented on `GetShopData.viewUrl`.

- **Product and notification URL semantics**
  - `GetProductData.url`, `GetProductSummaryData.url`, `WatchlistNotificationPayloadData.url`, and `SearchFilterNotificationPayloadData.url` are now documented as raw destination URLs.
  - The backend-generated tracked or affiliate navigation target is now documented separately on the corresponding required `viewUrl` field.

- **Examples**
  - All affected shop, product, watchlist, search-filter, and notification response examples now show both `url` and `viewUrl` with values matching the backend contract.

### Removed

- No endpoints or previously documented schemas were removed in this update.

## 2026-05-16 - Shop Default Ingestion Languages (`backend#1052`)

Backend PR `#1052` replaces runtime Shopify/WooCommerce language inference with explicit per-shop default language settings. This update realigns the internal OpenAPI spec with that backend contract by documenting the new optional shop language fields anywhere shop create, update, and read DTOs are exposed.

### Added

- **`PostShopData.shopifyLanguage`**
  - Optional Shopify default language accepted by `POST /api/v1/shops`
  - Uses the shared `LanguageData` enum (`de`, `en`, `fr`, `es`, `it`, `zh`, `pt`, `pl`, `tr`, `nl`, `cs`, `ja`, `ru`, `ar`)
  - Relevant for shops using Shopify partner-shop ingestion; when configured, product lifecycle events for the shop are ingested using this language

- **`PatchShopData.shopifyLanguage`**
  - Optional Shopify default language accepted by `PATCH /api/v1/shops/{shopId}`
  - When omitted or sent as `null`, the stored Shopify language remains unchanged

- **`GetShopData.shopifyLanguage`**
  - Optional Shopify default language returned when present on shop read models
  - Exposed anywhere `GetShopData` is returned, including:
    - `POST /api/v1/shops`
    - `GET /api/v1/shops/{shopId}`
    - `PATCH /api/v1/shops/{shopId}`
    - `GET /api/v1/by-slug/shops/{shopSlugId}`
    - `POST /api/v1/shops/search`
    - `GET /api/v1/partner/{partnerId}/shops`

- **`PostShopData.woocommerceLanguage`**
  - Optional WooCommerce default language accepted by `POST /api/v1/shops`
  - Uses the shared `LanguageData` enum (`de`, `en`, `fr`, `es`, `it`, `zh`, `pt`, `pl`, `tr`, `nl`, `cs`, `ja`, `ru`, `ar`)
  - Relevant for shops using `POST /api/v1/webhooks/woocommerce/{shopId}`; when configured, webhook-ingested products for the shop are ingested using this language

- **`PatchShopData.woocommerceLanguage`**
  - Optional WooCommerce default language accepted by `PATCH /api/v1/shops/{shopId}`
  - When omitted or sent as `null`, the stored WooCommerce language remains unchanged

- **`GetShopData.woocommerceLanguage`**
  - Optional WooCommerce default language returned when present on shop read models
  - Exposed anywhere `GetShopData` is returned, including:
    - `POST /api/v1/shops`
    - `GET /api/v1/shops/{shopId}`
    - `PATCH /api/v1/shops/{shopId}`
    - `GET /api/v1/by-slug/shops/{shopSlugId}`
    - `POST /api/v1/shops/search`
    - `GET /api/v1/partner/{partnerId}/shops`

### Changed

- **Shop request/response documentation**
  - Shop create and patch examples now include `shopifyLanguage` and `woocommerceLanguage` alongside the existing partner-shop Shopify/WooCommerce configuration fields.
  - Shop read examples now include the stored default ingestion languages wherever `GetShopData` is returned.

- **Product ingestion language behavior**
  - Shopify and WooCommerce partner-shop product ingestion now require the corresponding shop language to be configured.
  - The backend no longer infers a language from product text and no longer falls back to `en` when the shop-specific language is missing.
  - `POST /api/v1/webhooks/woocommerce/{shopId}` can therefore now fail with `400 Bad Request` if the addressed shop has no configured `woocommerceLanguage` (and, as before, if `price` is non-empty and `woocommerceCurrency` is missing).

### Removed

- No endpoints or previously documented schemas were removed in this update.

## 2026-05-14 - WooCommerce Partner-Shop Webhooks (`backend#1036`)

Backend PR `#1036` adds WooCommerce as a partner-shop ingestion source via signed webhooks, extends shop DTOs with WooCommerce webhook configuration fields, and broadens partner-shop patch authorization to support API-key-authenticated updates when no Cognito identity is present. This update realigns the internal OpenAPI spec with that backend contract and documents the new inbound webhook route, the changed shop schemas, and the revised authentication behavior.

### Added

- **New endpoint: `POST /api/v1/webhooks/woocommerce/{shopId}`**
  - Uses partner `x-api-key` authentication and requires the additional headers `x-wc-webhook-topic` and `x-wc-webhook-signature`.
  - Accepts exactly these WooCommerce topic values in `x-wc-webhook-topic`:
    - `product.created`
    - `product.updated`
    - `product.deleted`
  - Documents topic-specific request payloads via:
    - `WoocommerceProductWebhookUpsertData`
    - `WoocommerceProductWebhookDeleteData`
    - `WoocommerceProductWebhookImageData`
  - Returns `WoocommerceWebhookResponse` with an `errors` count of `0` or `1`.
  - Documents the relevant error cases:
    - `400` for invalid path/body/topic/payload data
    - `401` for missing or invalid `x-api-key` / `x-wc-webhook-signature`
    - `403` when the addressed shop is not partnered
    - `404` when the shop does not exist
    - `500` when the shop has no configured WooCommerce webhook secret or another internal failure occurs

- **New WooCommerce fields on shop DTOs**

  | Schema / field | Type | Required | Description |
  |---|---|---|---|
  | `PostShopData.woocommerceWebhookSecret` | `string \| null` | No | Optional write-only secret stored for validating `x-wc-webhook-signature` on `POST /api/v1/webhooks/woocommerce/{shopId}`. |
  | `PatchShopData.woocommerceWebhookSecret` | `string \| null` | No | Optional write-only replacement secret used for WooCommerce webhook signature validation. |
  | `PostShopData.woocommerceCurrency` | `CurrencyData \| null` | No | Optional WooCommerce currency configured when creating a shop. Used when WooCommerce webhook payloads provide a non-empty `price`. |
  | `PatchShopData.woocommerceCurrency` | `CurrencyData \| null` | No | Optional replacement WooCommerce currency used when webhook payloads provide a non-empty `price`. |
  | `GetShopData.woocommerceCurrency` | `CurrencyData \| null` | No | Optional WooCommerce currency returned on shop read DTOs. |

### Changed

- **`PATCH /api/v1/shops/{shopId}` authentication**
  - The endpoint now documents both supported authorization modes:
    - Cognito bearer authentication for the assigned partner user or an `ADMIN`
    - partner `x-api-key` authentication when no Cognito identity is present

- **Shop create/update/read documentation**
  - `POST /api/v1/shops` and `PATCH /api/v1/shops/{shopId}` request examples now include the new optional `woocommerceWebhookSecret` and `woocommerceCurrency` fields.
  - Shop read examples for `GET /api/v1/shops/{shopId}`, `GET /api/v1/by-slug/shops/{shopSlugId}`, `POST /api/v1/shops/search`, and `GET /api/v1/partner/{partnerId}/shops` now include `woocommerceCurrency`.
  - `woocommerceWebhookSecret` is explicitly documented as write-only and therefore remains absent from all shop read responses.

### Removed

- No endpoints or previously documented schemas were removed in this update.

## 2026-05-13 - Shopify Currency on Shop DTOs (`backend#1035`)

Backend PR `#1035` adds an optional Shopify currency to the shop REST DTOs so each shop can carry the ISO-4217 currency code associated with its Shopify storefront. This update realigns the internal OpenAPI spec with that backend contract and documents where the new field can be sent and returned.

### Added

- **`PostShopData.shopifyCurrency`**
  - Optional Shopify currency accepted by `POST /api/v1/shops`
  - Uses the shared `CurrencyData` enum (`EUR`, `GBP`, `USD`, `AUD`, `CAD`, `NZD`, `CNY`, `BRL`, `PLN`, `TRY`, `JPY`, `CZK`, `RUB`, `AED`, `SAR`, `HKD`, `SGD`, `CHF`)

- **`PatchShopData.shopifyCurrency`**
  - Optional Shopify currency accepted by `PATCH /api/v1/shops/{shopId}`
  - When omitted or sent as `null`, the existing stored Shopify currency remains unchanged

- **`GetShopData.shopifyCurrency`**
  - Optional Shopify currency returned when present on shop read models
  - Exposed anywhere `GetShopData` is returned, including:
    - `POST /api/v1/shops`
    - `GET /api/v1/shops/{shopId}`
    - `PATCH /api/v1/shops/{shopId}`
    - `GET /api/v1/by-slug/shops/{shopSlugId}`
    - `GET /api/v1/shops`
    - `POST /api/v1/shops/search`
    - `GET /api/v1/partner/{partnerId}/shops`

### Changed

- Updated shop request/response examples across the affected shop endpoints to show the new optional `shopifyCurrency` field together with `shopifyDomain` where relevant.

### Removed

- No endpoints or previously documented shop fields were removed in this update.

## 2026-05-12 - Shopify Partner-Shop Domain on Shop DTOs (`backend#1022`)

Backend PR `#1022` adds an optional Shopify storefront domain to the shop REST DTOs so partner shops can be matched against incoming Shopify product lifecycle events. This update realigns the internal OpenAPI spec with that backend contract and documents where the new field can be sent and returned.

### Added

- **`PostShopData.shopifyDomain`**
  - Optional normalized Shopify storefront domain accepted by `POST /api/v1/shops`
  - Uses the same normalization rules as `domains` (lowercase, no scheme, no `www.` prefix, no path/query/fragment)

- **`PatchShopData.shopifyDomain`**
  - Optional normalized Shopify storefront domain accepted by `PATCH /api/v1/shops/{shopId}`
  - When omitted or sent as `null`, the existing stored Shopify domain remains unchanged

- **`GetShopData.shopifyDomain`**
  - Optional normalized Shopify storefront domain returned when present on shop read models
  - Exposed anywhere `GetShopData` is returned, including:
    - `POST /api/v1/shops`
    - `GET /api/v1/shops/{shopId}`
    - `PATCH /api/v1/shops/{shopId}`
    - `GET /api/v1/by-slug/shops/{shopSlugId}`
    - `GET /api/v1/shops`
    - `POST /api/v1/shops/search`
    - `GET /api/v1/partner/{partnerId}/shops`

### Changed

- Updated shop request/response examples across the affected shop endpoints to show the new optional `shopifyDomain` field where relevant.

### Removed

- No endpoints or previously documented shop fields were removed in this update.

## 2026-05-11 - Re-added Disabled Category and Period Documentation

This update restores the previously removed category and period endpoint/type documentation in a disabled state so the internal contract remains visible for frontend/backend coordination, without reintroducing the earlier category/period fields on unrelated product, search-filter, or other resource contracts.

### Added

- Re-added the disabled category documentation:
  - `GET /api/v1/categories`
  - `GET /api/v1/categories/{categoryId}`
  - `POST /api/v1/categories/search`
  - `CategorySearchData`
  - `SortCategoryFieldData`
  - `GetCategorySummaryData`
  - `GetCategoryData`

- Re-added the disabled period documentation:
  - `GET /api/v1/periods`
  - `GET /api/v1/periods/{periodId}`
  - `POST /api/v1/periods/search`
  - `PeriodSearchData`
  - `SortPeriodFieldData`
  - `GetPeriodSummaryData`
  - `GetPeriodData`

### Changed

- All restored category and period operations are now explicitly marked as disabled in the OpenAPI spec for internal reference.
- All restored category and period schemas are marked as deprecated/disabled reference types.

### Removed

- No unrelated category/period fields were reintroduced on other endpoints or schemas. Previously removed partial fields such as `categoryId` / `periodId` references on other resource contracts remain removed.

## 2026-05-10 - Search-Filter Live Product Search and Match Route Rename (`backend#1014`)

Backend PR `#1014` splits the old saved-search-filter product endpoints into two distinct concerns: persisted match management now lives under `/matches`, while `/products` now performs a live search against the current product index. This update realigns the internal OpenAPI spec with that backend contract and documents the new live-search behavior precisely.

### Added

- **New endpoint: `GET /api/v1/me/search-filters/{userSearchFilterId}/products`**
  - Requires authentication.
  - Runs a live search using the saved filter's stored search criteria and returns `PersonalizedProductSearchResultData`.
  - Query parameters:

  | Parameter | Type | Required | Description |
  |---|---|---|---|
  | `userSearchFilterId` | `string (uuid)` | Yes | Saved search-filter identifier whose stored search criteria are executed live. |
  | `currency` | `CurrencyData` | No | Currency used to select indexed product prices; no request-time conversion occurs. |
  | `language` | `LanguageData` | No | Preferred language for localized summary fields. Defaults to `en` when omitted. |
  | `size` | `integer` | No | Maximum number of results to return. Values above `10` are capped to `10`; default is `10`. |
  | `searchAfter` | `array` | No | Must not be provided. Any non-null value is rejected with `BAD_QUERY_PARAMETER_VALUE` because this live-preview endpoint does not allow client-driven pagination. |

  - Response item schema: `PersonalizedGetProductSummaryData`
  - Response headers: `Cache-Control`, `Access-Control-Allow-Origin`
  - When the saved filter has an `enhancedSearchDescription`, the backend re-evaluates each returned product for that specific filter and overwrites `userState.searchFilter` so the live result exposes:
    - always-present `matched`
    - always-present `hidden` (`false` in this flow)
    - optional `matchReason`
    - omitted `userSearchFilterId`, `userSearchFilterName`, and `matchFeedback`

### Changed

- **Persisted saved-search match listing renamed**
  - `GET /api/v1/me/search-filters/{userSearchFilterId}/products`
  - → `GET /api/v1/me/search-filters/{userSearchFilterId}/matches`
  - The renamed endpoint keeps the existing contract for stored matches:
    - authentication required
    - query parameters `currency`, `language`, `sort`, `order`, `searchAfter`, `size`
    - response schema `SearchFilterMatchProductCollectionData`
    - time-based cursor pagination sorted by match creation timestamp

- **Persisted saved-search match feedback endpoint renamed**
  - `PATCH /api/v1/me/search-filters/{userSearchFilterId}/products/{shopId}/{shopsProductId}`
  - → `PATCH /api/v1/me/search-filters/{userSearchFilterId}/matches/{shopId}/{shopsProductId}`
  - Request body and response schemas are unchanged:
    - request: `PatchUserSearchFilterMatchData`
    - success response: `SearchFilterProductMatchData`
    - request body still accepts optional `feedback: boolean`

### Removed

- No standalone documented schemas or endpoint capabilities were removed in this update; the persisted-match functionality was renamed and the former `GET .../products` behavior was replaced by the new live-search endpoint above.

## 2026-05-09 - Completely removed Category, Period, Authenticity, Condition, Provenance, Restoration and Origin Year

## 2026-05-03 - Shared Resource State for Search Filters and Watchlist (`backend#963`)

Backend PR `#963` evolved beyond the initial saved-search response change. The current backend contract now uses a shared resource-state enum for persisted user-owned resources, returns that state on saved search filters, and allows clients to manually activate/deactivate both saved search filters and watchlist entries through PATCH payloads. This update realigns the internal OpenAPI spec with that broader backend contract.

### Added

- **New schema: `ResourceStateData`**
  - Shared serialized enum with the values `ACTIVE`, `INACTIVE_BY_USER`, and `INACTIVE_BY_RESTRICTED_PLAN`.
  - `INACTIVE_BY_RESTRICTED_PLAN` is backend-managed and indicates a stored resource that is temporarily inactive because the current user tier does not allow it to remain active.

- **New schema: `PatchResourceStateData`**
  - Client-settable PATCH enum with the values `ACTIVE` and `INACTIVE_BY_USER`.
  - Used when manually reactivating or deactivating supported user-owned resources.

- **New response field on saved search filters: `UserSearchFilterData.state`**

  | Field | Type | Always present | Description |
  |---|---|---|---|
  | `state` | `ResourceStateData` | Yes | Current activation state of the saved search filter. `INACTIVE_BY_RESTRICTED_PLAN` means the filter is still stored but inactive because the user's current tier does not allow it to remain active. |

### Changed

- **Saved search-filter responses**
  - `GET /api/v1/me/search-filters`
  - `GET /api/v1/me/search-filters/{userSearchFilterId}`
  - `POST /api/v1/me/search-filters`
  - `PATCH /api/v1/me/search-filters/{userSearchFilterId}`
  - All now return `UserSearchFilterData.state` in addition to the existing metadata fields.

- **`PATCH /api/v1/me/search-filters/{userSearchFilterId}`**
  - Request body `PatchUserSearchFilterData` now accepts optional `state: PatchResourceStateData`.
  - Supported values are:
    - `ACTIVE` — reactivate the saved search filter
    - `INACTIVE_BY_USER` — manually deactivate the saved search filter
  - Reactivating a filter can now fail with:
    - `SEARCH_FILTER_QUOTA_EXCEEDED` when the tier's active-filter quota is already full
    - `SEARCH_FILTER_RESTRICTED_FEATURE` when the current tier no longer permits the stored search criteria or enhanced-search description

- **`PATCH /api/v1/me/watchlist/{shopId}/{shopsProductId}`**
  - Request body `WatchlistProductPatch` now accepts optional `state: PatchResourceStateData`.
  - Supported values are:
    - `ACTIVE` — reactivate the watchlist entry
    - `INACTIVE_BY_USER` — manually deactivate the watchlist entry
  - Reactivating a watchlist entry can now fail with `WATCHLIST_QUOTA_EXCEEDED` when the current tier's active-watchlist quota is already full.

### Removed

- The search-filter-specific enum name `UserSearchFilterStateData` is no longer used in the documented contract; the backend now exposes the shared `ResourceStateData` / `PatchResourceStateData` types instead.

## 2026-04-30 - Search-Filter Match Feedback (`backend#942`)

Backend PR `#942` adds persisted user feedback for saved-search product matches. This update realigns the internal OpenAPI spec with the backend contract by documenting the new authenticated PATCH endpoint for individual matches, the dedicated match-feedback request/response schemas, and the new `matchFeedback` field now exposed in personalized product user state.

### Added

- **New endpoint: `PATCH /api/v1/me/search-filters/{userSearchFilterId}/products/{shopId}/{shopsProductId}`**
  - Requires authentication.
  - Request body schema: `PatchUserSearchFilterMatchData`
  - Success response schema: `SearchFilterProductMatchData`

  | Contract element | Type | Required | Description |
  |---|---|---|---|
  | Path `userSearchFilterId` | `string (uuid)` | Yes | Saved search-filter identifier that produced the match. |
  | Path `shopId` | `string (uuid)` | Yes | Shop identifier of the matched product. |
  | Path `shopsProductId` | `string` | Yes | Shop-scoped product identifier of the matched product. |
  | Body `feedback` | `boolean` | No | Optional persisted relevance feedback. `true` marks the match as relevant, `false` marks it as not relevant. Omitting the field performs a no-op and returns the existing match unchanged. |
  | Response `feedback` | `boolean` | No | Stored feedback value, returned only after feedback has been explicitly recorded. |
  | Response headers | `Last-Modified`, `Access-Control-Allow-Origin` | — | Returns the updated match timestamp and standard CORS header. |

- **New schema: `PatchUserSearchFilterMatchData`**
  - Partial patch payload for search-filter product match feedback with optional `feedback: boolean`.

- **New schema: `SearchFilterProductMatchData`**
  - Match record returned by the new PATCH endpoint with fields `userId`, `userSearchFilterId`, `shopId`, `shopsProductId`, `productId`, optional `feedback`, `created`, and `updated`.

### Changed

- **`SearchFilterUserStateData`**
  - Added optional `matchFeedback: boolean` to the personalized product user-state object.
  - The field is returned only when the authenticated user has a saved-search match for the product and has already submitted feedback for that match.

- **Personalized product responses**
  - The shared `ProductUserStateData.searchFilter` contract now allows `matchFeedback` anywhere personalized product user state is returned, including product detail, product listings/search, similar products, watchlist products, and search-filter matched products.

### Removed

- No endpoints or documented schemas were removed in this update.

## 2026-04-29 - Shop Primary URL Fields (`backend#940`)

Backend PR `#940` adds an optional primary shop URL to shop REST DTOs and to the `NEW` partner shop application payloads used by user and admin flows. This update realigns the internal OpenAPI spec with the backend contract and documents the tracked shop URL now returned by shop read responses.

### Added

- **New shop URL field on shop DTOs**

  | Schema / field | Type | Required | Description |
  |---|---|---|---|
  | `PostShopData.url` | `string (uri) \| null` | No | Optional primary URL of the shop website accepted when creating a shop. |
  | `PatchShopData.url` | `string (uri) \| null` | No | Optional replacement primary URL of the shop website accepted when updating a shop. |
  | `GetShopData.url` | `string (uri) \| null` | No | Optional primary URL returned for the shop. |

- **New partner shop application URL field for `type: "NEW"` payloads**

  | Schema / field | Type | Required | Description |
  |---|---|---|---|
  | `PostPartnerShopApplicationPayloadData.NEW.shopUrl` | `string (uri) \| null` | No | Optional primary URL of the requested new shop website when creating an application. |
  | `GetPartnerShopApplicationPayloadData.NEW.shopUrl` | `string (uri) \| null` | No | Optional primary URL returned for the requested new shop website. |
  | `PatchPartnerShopApplicationData.shopUrl` | `string (uri) \| null` | No | Optional replacement primary URL for `NEW` applications in the user patch endpoint. |
  | `AdminPatchPartnerShopApplicationData.shopUrl` | `string (uri) \| null` | No | Optional replacement primary URL for `NEW` applications in the admin patch endpoint. |

### Changed

- **Shop endpoint documentation**
  - `POST /api/v1/shops`, `PATCH /api/v1/shops/{shopId}`, and shop response examples now include the optional `url` field.
  - Shop read examples now show the tracked URL representation returned by the backend, including `utm_source=aura_historia` and `utm_medium=referral` on `GetShopData.url`.

- **Partner shop application documentation**
  - User and admin examples for listing, creating, retrieving, and updating partner shop applications now include `shopUrl` whenever the payload type is `NEW`.

### Removed

- No endpoints or documented schemas were removed in this update.

## 2026-04-28 - Shared Geo Data for Users, Products, and Search Filters (`backend#929`)

Backend PR `#929` extends REST contracts with shared structured-address and coordinate data for users and partner product ingestion, and adds geo-aware search filters for products, saved search filters, and admin user search. This update realigns the internal OpenAPI spec with the PR contract by documenting the new request/response fields, shared geo-distance schema, and the new query/body filters.

### Added

- **New schemas**
  - **`DistanceUnitData`** — Geo-distance unit enum with serialized values `MILES`, `YARDS`, `FEET`, `INCHES`, `KILOMETERS`, `METERS`, `CENTIMETERS`, `MILLIMETERS`, and `NAUTICAL_MILES`.
  - **`DistanceData`** — Distance object with required `amount` and `unit`.
  - **`GeoDistanceQueryData`** — Geo-distance filter object with required `lat`, `lon`, and `distance`.

- **User account address fields**
  - **`GetUserAccountData.structuredAddress`** — Optional structured postal address stored on the user account.
  - **`GetUserAccountData.geoAddress`** — Optional latitude/longitude coordinates derived by the backend from `structuredAddress`.
  - **`PatchUserAccountData.structuredAddress`** — Optional self-service patch field for persisting a user address.
  - **`PatchAdminUserData.structuredAddress`** — Optional admin patch field for persisting a user address.

- **New admin user search filters on `GET /api/v1/users` and `UserSearchData`**

  | Field / query parameter | Type | Required | Allowed values / format | Description |
  |---|---|---|---|---|
  | `country` | `CountryCodeData[]` | No | ISO 3166-1 alpha-2 country codes | Filters users by structured-address country. |
  | `continent` | `ContinentData[]` | No | `AFRICA`, `ANTARCTICA`, `ASIA`, `EUROPE`, `NORTH_AMERICA`, `OCEANIA`, `SOUTH_AMERICA` | Filters users by structured-address continent. |
  | `geoAddress` | `GeoDistanceQueryData` | No | deep-object query: `geoAddress[lat]`, `geoAddress[lon]`, `geoAddress[distance][amount]`, `geoAddress[distance][unit]` | Filters users by proximity to indexed coordinates. |

- **Product geo/address fields**
  - **`GetProductData.structuredAddress`** — Optional structured address propagated from the selling shop.
  - **`GetProductData.geoAddress`** — Optional latitude/longitude coordinates propagated from the selling shop.
  - **`PostProductData.structuredAddress`** / **`PostProductData.geoAddress`** — Optional partner-ingestion fields for geo-aware product indexing.
  - **`PutProductData.structuredAddress`** / **`PutProductData.geoAddress`** — Optional partner upsert fields for geo-aware product indexing.

- **New product geo filters on `ProductSearchData`, `PatchProductSearchData`, `GET /api/v1/products`, `POST /api/v1/products/search`, and saved search-filter contracts**

  | Field / query parameter | Type | Required | Allowed values / format | Description |
  |---|---|---|---|---|
  | `country` | `CountryCodeData[]` | No | ISO 3166-1 alpha-2 country codes | Filters products by the selling shop's structured-address country. |
  | `continent` | `ContinentData[]` | No | `AFRICA`, `ANTARCTICA`, `ASIA`, `EUROPE`, `NORTH_AMERICA`, `OCEANIA`, `SOUTH_AMERICA` | Filters products by the selling shop's structured-address continent. |
  | `geoAddress` | `GeoDistanceQueryData` | No | `{ lat, lon, distance }` | Filters products by proximity to the selling shop's indexed coordinates. |

### Changed

- **`GET /api/v1/users`**
  - The admin search documentation now includes `country`, `continent`, and deep-object `geoAddress` query parameters in addition to the existing text, tier, role, and date-range filters.
  - Response examples now show the newly returned `structuredAddress` and `geoAddress` fields on user account items.

- **Product search documentation**
  - `ProductSearchData`, `PatchProductSearchData`, and both product-search endpoints now document the backend's geo-aware `country`, `continent`, and `geoAddress` filters.
  - Saved search-filter contracts automatically inherit the same geo-aware product-search fields because they embed `ProductSearchData`.

- **Partner product ingestion documentation**
  - `PostProductData` and `PutProductData` now document the optional structured-address and coordinate fields accepted by the backend for geo-aware indexing.

### Removed

- No endpoints or documented schemas were removed in this update.

## 2026-04-27 - Structured Address Normalization (`backend#923`)

Backend PR `#923` normalizes the structured-address REST DTO shared by shop and partner-application payloads and extends shop search with structured-address geography filters. This update realigns the internal OpenAPI spec with the backend contract by replacing the old multi-line address field, documenting the new continent enum, correcting the country-code format, and adding the missing search filters.

### Added

- **New schema: `ContinentData`**
  - Structured-address continent enum with these serialized values:
    - `AFRICA`
    - `ANTARCTICA`
    - `ASIA`
    - `EUROPE`
    - `NORTH_AMERICA`
    - `OCEANIA`
    - `SOUTH_AMERICA`

- **`StructuredAddressData.continent`** — Optional continent field returned alongside structured-address data.

- **New schema: `CountryCodeData`**
  - Explicit ISO 3166-1 alpha-2 country-code enum used for structured addresses and shop-search country filters.

- **New shop search filters on `ShopSearchData`, `POST /api/v1/shops/search`, and `GET /api/v1/shops`**

  | Field / Query parameter | Type | Required | Allowed values / format | Description |
  |---|---|---|---|---|
  | `countries` | `CountryCodeData[]` | No | ISO 3166-1 alpha-2 country codes | Filters shops by one or more structured-address countries. |
  | `continents` | `ContinentData[]` | No | `AFRICA`, `ANTARCTICA`, `ASIA`, `EUROPE`, `NORTH_AMERICA`, `OCEANIA`, `SOUTH_AMERICA` | Filters shops by one or more structured-address continents. |

### Changed

- **`StructuredAddressData`**
  - `country` is now documented as an ISO 3166-1 alpha-2 country code string (for example `DE`) instead of a free-form country name.
  - The schema examples now use the normalized address shape and include `continent` where a country is present.
  - `continent` is now documented as derivable from `country` when omitted in client payloads.

- **All request/response schemas using `StructuredAddressData`**
  - The updated address contract now propagates through:
    - `GetShopData`
    - `PostShopData`
    - `PatchShopData`
    - `GetPartnerShopApplicationPayloadData` (`NEW` variant)
    - `PostPartnerShopApplicationPayloadData` (`NEW` variant)
    - `PatchPartnerShopApplicationData`
    - `AdminPatchPartnerShopApplicationData`

- **Shop search documentation**
  - `ShopSearchData`, `POST /api/v1/shops/search`, and `GET /api/v1/shops` now document the backend's `countries` and `continents` filters.
  - Structured-address `country` references now use the shared finite `CountryCodeData` enum so generated clients can treat country codes as a closed set.

### Removed

- **`StructuredAddressData.addressLines`** — Removed because the backend no longer exposes a list-based structured-address field.

- **Free-form country-name documentation for `StructuredAddressData.country`** — Removed in favor of the backend’s ISO alpha-2 country-code contract.

## 2026-04-26 - Rich Shop Metadata and Shop Speciality Filters (`backend#915`)

Backend PR `#915` extends shop-facing REST contracts with optional structured-address, geocoded coordinates, contact, and speciality metadata. It also adds speciality-category and speciality-period filters to shop search and propagates the same optional new-shop metadata through partner shop application payloads.

### Added

- **New shop metadata fields on `GetShopData`**

  | Field | Type | Always present | Allowed values / format | Description |
  |---|---|---|---|---|
  | `structuredAddress` | `StructuredAddressData` | No | object | Optional structured postal address for the shop. |
  | `geoAddress` | `GeoAddressData` | No | object | Optional geocoded coordinates derived from `structuredAddress`. |
  | `phone` | `string` | No | any string | Optional public contact phone number. |
  | `email` | `string` | No | email | Optional public contact email address. |
  | `specialitiesCategories` | `string[]` | No | lowercase kebab-case category keys | Optional speciality category keys. Omitted when empty. |
  | `specialitiesPeriods` | `string[]` | No | lowercase kebab-case period keys | Optional speciality period keys. Omitted when empty. |

- **New component schemas**
  - **`StructuredAddressData`** — Structured postal address object with optional `addressLines`, `locality`, `region`, `postalCode`, and `country`.
  - **`GeoAddressData`** — Geocoded `{ lat, lon }` coordinate object returned by the backend when a shop has a structured address.

- **New shop search filters on `ShopSearchData` and `GET /api/v1/shops`**

  | Field / Query parameter | Type | Required | Allowed values / format | Description |
  |---|---|---|---|---|
  | `specialitiesCategories` | `string[]` | No | lowercase kebab-case category keys | Filters shops by one or more speciality categories. |
  | `specialitiesPeriods` | `string[]` | No | lowercase kebab-case period keys | Filters shops by one or more speciality periods. |

### Changed

- **`POST /api/v1/shops`**
  - `PostShopData` now also accepts optional `structuredAddress`, `phone`, `email`, `specialitiesCategories`, and `specialitiesPeriods`.
  - When `structuredAddress` is supplied, the backend geocodes it and returns the resulting `geoAddress` on the created `GetShopData` response.
  - A structurally empty `structuredAddress` is rejected with `400 Bad Request` / `BAD_BODY_VALUE`.

- **`PATCH /api/v1/shops/{shopId}`**
  - `PatchShopData` now also accepts optional `structuredAddress`, `phone`, `email`, `specialitiesCategories`, and `specialitiesPeriods`.
  - Updating `structuredAddress` causes the backend to refresh the returned `geoAddress`.
  - `null` or omitted patch fields remain no-ops; the backend does not expose a separate field-clearing contract for these new optional metadata fields.
  - A structurally empty `structuredAddress` is rejected with `400 Bad Request` / `BAD_BODY_VALUE`.

- **`POST /api/v1/shops/search`** and **`GET /api/v1/shops`**
  - Shop search now supports `specialitiesCategories` and `specialitiesPeriods` in addition to the existing name, type, partner-status, and date-range filters.

- **Partner shop application payloads**
  - The `NEW` variant of `PostPartnerShopApplicationPayloadData` and `GetPartnerShopApplicationPayloadData` now includes optional `shopStructuredAddress`, `shopPhone`, `shopEmail`, `shopSpecialitiesCategories`, and `shopSpecialitiesPeriods`.
  - `PatchPartnerShopApplicationData` and `AdminPatchPartnerShopApplicationData` now allow updating the same optional new-shop metadata fields for `NEW` applications.

### Removed

- No endpoints or documented schemas were removed in this update.

## 2026-04-25 - Partner Shop Application Applicant User ID (`backend#913`)

Backend PR `#913` extends the partner shop application response DTO with the submitting user's identifier. This update documents the new required response field on `GetPartnerShopApplicationData` across every partner-application endpoint that returns that schema.

### Added

- **`GetPartnerShopApplicationData.applicantUserId`** — New required response field exposing the user who submitted the partner shop application.

  | Field | Type | Always present | Allowed values / format | Description |
  |---|---|---|---|---|
  | `applicantUserId` | `string` | Yes | UUID | Unique identifier of the applicant user. |

  **Affected endpoints:**
  - `GET /api/v1/me/partner-applications`
  - `POST /api/v1/me/partner-applications`
  - `GET /api/v1/me/partner-applications/{partnerApplicationId}`
  - `PATCH /api/v1/me/partner-applications/{partnerApplicationId}`
  - `GET /api/v1/partner-applications`
  - `GET /api/v1/partner-applications/{partnerApplicationId}`
  - `PATCH /api/v1/partner-applications/{partnerApplicationId}`
  - `POST /api/v1/partner-applications/{partnerApplicationId}/decision`

### Changed

- No previously documented endpoints or schemas changed in this update.

### Removed

- No endpoints or documented schemas were removed in this update.

## 2026-04-25 - Admin Shop Creation REST API (`backend#911`)

Backend PR `#911` adds an admin-only shop-creation endpoint to the existing shop administration surface. This update documents the new authenticated path, the create payload, and the new conflict case when the slug derived from the requested shop name already exists.

### Added

- **New endpoint: `POST /api/v1/shops`**
  - **Authentication**: Required Bearer JWT with the `ADMIN` role.
  - **Request body**: `PostShopData`

    | Field | Type | Required | Allowed values / format | Description |
    |---|---|---|---|---|
    | `name` | `string` | Yes | max length `255` | Display name of the shop. The backend derives `shopSlugId` from this value. |
    | `shopType` | `ShopTypeData` | Yes | `AUCTION_HOUSE`, `AUCTION_PLATFORM`, `COMMERCIAL_DEALER`, `MARKETPLACE` | Shop classification. |
    | `domains` | `string[]` | Yes | domain-like strings; normalized to lowercase hostnames | Unique set of shop domains. The backend strips any `http://` / `https://` scheme, optional `www.` prefix, and any port, path, query, or fragment before storing the value. |
    | `image` | `string \| null` | No | URI | Optional shop logo / image URL. |

  - **Behavior**:
    - The HTTP request body itself must be present and non-empty.
    - The created shop is returned using the existing `GetShopData` schema.
    - Newly created shops start with `partnerStatus = SCRAPED`.
  - **Response**: `201 Created` with `GetShopData` and `Last-Modified`.
  - **Errors**:
    - `400 Bad Request` — missing, empty, malformed, or field-invalid JSON body (`BAD_BODY_VALUE`)
    - `401 Unauthorized` — missing or invalid JWT (`UNAUTHORIZED`)
    - `403 Forbidden` — authenticated requester is not an admin (`FORBIDDEN`)
    - `409 Conflict` — a shop with the same derived slug already exists (`SHOP_EXISTS_ALREADY`)

- **New schema**
  - **`PostShopData`** — Shop creation payload with required `name`, `shopType`, and `domains`, plus optional `image`.

### Changed

- No previously documented endpoints or schemas changed in this update.

### Removed

- No endpoints or documented schemas were removed in this update.

## 2026-04-25 - Admin User Management REST API (`backend#906`)

Backend PR `#906` adds admin-only user-management endpoints backed by the shared user account DTOs and a new OpenSearch-powered search contract. This update documents the new admin paths, their query interface, and the admin-only patch payload used to manage tier, role, and Stripe customer mappings.

### Added

- **New endpoint: `GET /api/v1/users`**
  - **Authentication**: Required Bearer JWT with persisted user account and `ADMIN` role.
  - **Query parameters**:

    | Parameter | Type | Required | Allowed values / format | Description |
    |---|---|---|---|---|
    | `query` | `string` | No | any string | Fuzzy full-text query across `email`, `firstName`, `lastName`, and `stripeCustomerId`. |
    | `email` | `string` | No | any string | Fuzzy email-only search term. |
    | `firstName` | `string` | No | any string | Fuzzy first-name search term. |
    | `lastName` | `string` | No | any string | Fuzzy last-name search term. |
    | `tier` | `UserTierData[]` | No | `FREE`, `PRO`, `ULTIMATE` | Repeated query parameter filtering users by one or more tiers. |
    | `role` | `UserRoleData[]` | No | `USER`, `ADMIN` | Repeated query parameter filtering users by one or more roles. |
    | `created[min]`, `created[max]` | `string` | No | RFC3339 date-time | Inclusive created-at range filters. |
    | `updated[min]`, `updated[max]` | `string` | No | RFC3339 date-time | Inclusive updated-at range filters. |
    | `sort` | `SortUserFieldData` | No | `score`, `email`, `firstName`, `lastName`, `tier`, `role`, `updated`, `created` | Sort field. Only applied when `order` is also supplied. |
    | `order` | `string` | No | `asc`, `desc` | Sort direction. Only applied when `sort` is also supplied. |
    | `searchAfter` | heterogeneous JSON array | No | OpenSearch sort cursor | Cursor returned by the previous response. |
    | `size` | `integer` | No | `0`–`100` | Maximum number of results requested from OpenSearch. Defaults to `21`. |

  - **Query encoding note**: `created[min]`, `created[max]`, `updated[min]`, and `updated[max]` are the literal query parameter names expected by the API.
  - **Behavior**:
    - Omitting every filter lists users.
    - Without explicit `sort` + `order`, the backend defaults to `email` ascending for list-all queries and `score` descending when any text query is present.
  - **Response**: `200 OK` with `UserCollectionData`.
  - **Errors**:
    - `400 Bad Request` — invalid query payload, sort/order, cursor, or page size (`BAD_BODY_VALUE`, `BAD_SORT_VALUE`, `BAD_ORDER_VALUE`, `INVALID_JSON`, `BAD_PAGE_SIZE_VALUE`)
    - `401 Unauthorized` — missing or invalid JWT (`UNAUTHORIZED`)
    - `403 Forbidden` — authenticated requester is not an admin (`FORBIDDEN`)
    - `404 Not Found` — authenticated requester has no persisted user record (`USER_NOT_FOUND`)

- **New endpoint: `GET /api/v1/users/{userId}`**
  - **Authentication**: Required Bearer JWT with `ADMIN` role.
  - **Path parameters**:

    | Parameter | Type | Required | Description |
    |---|---|---|---|
    | `userId` | `UUID` | Yes | Target user identifier. |

  - **Response**: `200 OK` with `GetUserAccountData`, `Last-Modified`, and `Cache-Control: no-store`.
  - **Errors**:
    - `401 Unauthorized` — missing or invalid JWT (`UNAUTHORIZED`)
    - `403 Forbidden` — requester is not an admin (`FORBIDDEN`)
    - `404 Not Found` — requester or target user record does not exist (`USER_NOT_FOUND`)

- **New endpoint: `PATCH /api/v1/users/{userId}`**
  - **Authentication**: Required Bearer JWT with `ADMIN` role.
  - **Request body**: `PatchAdminUserData`

    | Field | Type | Required | Allowed values / format | Description |
    |---|---|---|---|---|
    | `firstName` | `string \| null` | No | max length `64` | Updates the user's first name. |
    | `lastName` | `string \| null` | No | max length `64` | Updates the user's last name. |
    | `language` | `LanguageData \| null` | No | documented `LanguageData` enum | Updates the preferred language. |
    | `currency` | `CurrencyData \| null` | No | documented `CurrencyData` enum | Updates the preferred currency. |
    | `prohibitedContentConsent` | `boolean \| null` | No | `true`, `false` | Updates the prohibited-content consent flag. |
    | `tier` | `UserTierData \| null` | No | `FREE`, `PRO`, `ULTIMATE` | Updates the subscription tier. |
    | `role` | `UserRoleData \| null` | No | `USER`, `ADMIN` | Updates the user role. |
    | `stripeCustomerId` | `string \| null` | No | any string | Persists a Stripe customer identifier for the user. |

  - **Behavior**:
    - The body itself must be present and non-empty.
    - An empty JSON object (`{}`) is accepted and returns the existing user unchanged.
  - **Response**: `200 OK` with updated `GetUserAccountData` and `Last-Modified`.
  - **Errors**:
    - `400 Bad Request` — missing or invalid JSON body (`BAD_BODY_VALUE`)
    - `401 Unauthorized` — missing or invalid JWT (`UNAUTHORIZED`)
    - `403 Forbidden` — requester is not an admin (`FORBIDDEN`)
    - `404 Not Found` — requester or target user record does not exist (`USER_NOT_FOUND`)

- **New endpoint: `DELETE /api/v1/users/{userId}`**
  - **Authentication**: Required Bearer JWT with `ADMIN` role.
  - **Response**: `204 No Content`.
  - **Errors**:
    - `401 Unauthorized` — missing or invalid JWT (`UNAUTHORIZED`)
    - `403 Forbidden` — requester is not an admin (`FORBIDDEN`)
    - `404 Not Found` — requester or target user record does not exist (`USER_NOT_FOUND`)

- **New schemas**
  - **`UserCollectionData`** — Cursor-paginated admin user-search response with `items`, `size`, optional `total`, and optional heterogeneous `searchAfter` cursor.
  - **`UserSearchData`** — Query model covering full-text filters, per-field filters (`email`, `firstName`, `lastName`), repeated `tier` / `role` filters, and RFC3339 `created` / `updated` ranges.
  - **`PatchAdminUserData`** — Admin-only partial update payload for profile fields, preferences, consent, tier, role, and `stripeCustomerId`.
  - **`SortUserFieldData`** — Sort enum for admin user search: `score`, `email`, `firstName`, `lastName`, `tier`, `role`, `updated`, `created`.

### Changed

- No previously documented endpoints or schemas changed in this update.

### Removed

- No endpoints or documented schemas were removed in this update.

## 2026-04-22 - Product DTO Drift Repair

The backend `develop` branch currently exposes additional product period and seller identifier fields, while several previously documented product-summary and product-history fields no longer exist in the Rust REST DTOs. This update realigns the internal OpenAPI spec with the implemented backend contract.

### Added

- **`GetProductData.periodId`** — Optional kebab-case identifier for the product's classified period.
- **`GetProductData.period`** — Optional localized display name for the product's classified period.
- **`GetProductEventData.sellerId`** — Required UUID of the seller associated with each product history event.

### Changed

- **`GET /api/v1/shops/{shopId}/products/{shopsProductId}/history`** — Response examples now include `sellerId` on every `GetProductEventData` item.
- **`GetProductSummaryData` examples** — Similar-product and product-search examples now match the actual summary DTO shape and no longer show category fields that are absent from the backend response.

### Removed

- **`GetProductSummaryData.categoryId`** — Removed from the schema because the backend summary DTO no longer serializes a category identifier.
- **`GetProductSummaryData.category`** — Removed from the schema because the backend summary DTO no longer serializes a localized category label.
- **`ProductCreatedEventPayloadData.priceEstimateMin`** — Removed from the schema because the backend creation-event payload only includes `price` and `state`.
- **`ProductCreatedEventPayloadData.priceEstimateMax`** — Removed from the schema because the backend creation-event payload only includes `price` and `state`.

## 2026-04-22 - Shop Search `shopType` Filter Documentation

The backend `develop` branch already exposes `shopType` as part of `ShopSearchData`. This update brings the internal API documentation in line with the implemented shop-search contract so frontend and backend teams can rely on the same documented filter set.

### Added

- **`GET /api/v1/shops`** — The simple shop-search query interface now explicitly documents the repeated `shopType` query parameter.

  | Parameter | Type | Allowed values | Description |
  |---|---|---|---|
  | `shopType` | `ShopTypeData[]` | `AUCTION_HOUSE`, `AUCTION_PLATFORM`, `COMMERCIAL_DEALER`, `MARKETPLACE` | Filter returned shops to one or more shop types. Repeated query parameter. |

### Changed

- **`ShopSearchData`** — The schema description and examples now explicitly show `shopType` alongside the existing name, partner-status, and date-range filters.

- **`POST /api/v1/shops/search`** — The request-body documentation and example now explicitly include the optional `shopType` filter.

### Removed

- No endpoints or documented fields were removed in this update.

## 2026-04-21 - Smart Stripe Billing Management Endpoint (`backend#876`)

Backend PR `#876` adds a single authenticated Stripe billing endpoint that chooses between Checkout and Customer Portal based on the persisted user tier. The existing `/api/v1/me/billing/checkout` and `/api/v1/me/billing/portal` endpoints remain unchanged.

### Added

- **New endpoint: `POST /api/v1/me/billing/manage`**
  - **Authentication**: Required Bearer JWT.
  - **Request body**: `PostBillingCheckoutData`

    | Field | Type | Required | Allowed values | Description |
    |---|---|---|---|---|
    | `plan` | `BillingPlanData` | Yes | `PRO`, `ULTIMATE` | Requested subscription plan. Required for all callers. |
    | `cycle` | `BillingCycleData` | Yes | `MONTHLY`, `YEARLY` | Requested billing interval. Required for all callers. |

  - **Behavior**:
    - Stored tier `FREE` → returns a Stripe Checkout session URL for the requested `plan` + `cycle`.
    - Stored tier `PRO` / `ULTIMATE` → returns a Stripe Customer Portal session URL instead.
    - For free users, an existing persisted Stripe customer is reused; otherwise the backend creates and stores one before creating the checkout session.
    - For paid users, a valid request body is still required even though the response is a portal session.

  - **Responses**:
    - `201 Created` — Returns `BillingSessionUrlData`.
      - Checkout example:
        ```json
        {
          "url": "https://checkout.stripe.com/c/pay/cs_test_manage_free"
        }
        ```
      - Portal example:
        ```json
        {
          "url": "https://billing.stripe.com/p/session/manage_paid"
        }
        ```
    - `400 Bad Request` — Missing, empty, malformed, or enum-invalid request body. Error code: `BAD_BODY_VALUE`.
    - `401 Unauthorized` — Missing or invalid Cognito JWT. Error code: `UNAUTHORIZED`.
    - `404 Not Found` — Authenticated user record does not exist. Error code: `USER_NOT_FOUND`.
    - `422 Unprocessable Content` — Paid user has no persisted Stripe customer record. Error code: `STRIPE_CUSTOMER_DOES_NOT_EXIST`.
    - `500 Internal Server Error` — Unexpected Stripe/session creation or missing server-side Stripe price configuration. Error code: `INTERNAL_SERVER_ERROR`.

## 2026-04-20 - Stripe Billing Checkout and Portal Endpoints (`backend#871`)

Backend PR `#871` adds authenticated Stripe billing endpoints for starting a subscription checkout flow and for opening the Stripe customer portal. The update also introduces dedicated billing request/response schemas and two new Stripe-specific error codes.

### Added

- **New endpoint: `POST /api/v1/me/billing/checkout`**
  - **Authentication**: Required Bearer JWT.
  - **Request body**: `PostBillingCheckoutData`

    | Field | Type | Required | Allowed values | Description |
    |---|---|---|---|---|
    | `plan` | `BillingPlanData` | Yes | `PRO`, `ULTIMATE` | Subscription tier to purchase. |
    | `cycle` | `BillingCycleData` | Yes | `MONTHLY`, `YEARLY` | Billing interval for the subscription. |

  - **Responses**:
    - `201 Created` — Returns `BillingSessionUrlData` with the hosted Stripe Checkout URL.
    - `400 Bad Request` — Missing, empty, malformed, or enum-invalid request body. Error code: `BAD_BODY_VALUE`.
    - `401 Unauthorized` — Missing or invalid Cognito JWT. Error code: `UNAUTHORIZED`.
    - `404 Not Found` — Authenticated user record does not exist. Error code: `USER_NOT_FOUND`.
    - `409 Conflict` — User already has a persisted Stripe customer record. Error code: `STRIPE_CUSTOMER_ALREADY_EXISTS`.
    - `500 Internal Server Error` — Unexpected Stripe/session creation or server-side configuration failure. Error code: `INTERNAL_SERVER_ERROR`.

  - **Example request**:

    ```json
    {
      "plan": "PRO",
      "cycle": "MONTHLY"
    }
    ```

  - **Example response**:

    ```json
    {
      "url": "https://checkout.stripe.com/c/pay/cs_test_123"
    }
    ```

- **New endpoint: `POST /api/v1/me/billing/portal`**
  - **Authentication**: Required Bearer JWT.
  - **Request body**: None.
  - **Responses**:
    - `201 Created` — Returns `BillingSessionUrlData` with the hosted Stripe customer-portal URL.
    - `401 Unauthorized` — Missing or invalid Cognito JWT. Error code: `UNAUTHORIZED`.
    - `404 Not Found` — Authenticated user record does not exist. Error code: `USER_NOT_FOUND`.
    - `422 Unprocessable Content` — User has no persisted Stripe customer record yet. Error code: `STRIPE_CUSTOMER_DOES_NOT_EXIST`.
    - `500 Internal Server Error` — Unexpected Stripe/session creation failure. Error code: `INTERNAL_SERVER_ERROR`.

  - **Example response**:

    ```json
    {
      "url": "https://billing.stripe.com/p/session/test_123"
    }
    ```

- **New schemas**
  - **`PostBillingCheckoutData`** — Request body for Stripe checkout-session creation.

    | Field | Type | Required | Description |
    |---|---|---|---|
    | `plan` | `BillingPlanData` | Yes | Requested subscription plan. |
    | `cycle` | `BillingCycleData` | Yes | Requested billing interval. |

  - **`BillingPlanData`** — Enum of supported subscription plans.

    | Value |
    |---|
    | `PRO` |
    | `ULTIMATE` |

  - **`BillingCycleData`** — Enum of supported billing intervals.

    | Value |
    |---|
    | `MONTHLY` |
    | `YEARLY` |

  - **`BillingSessionUrlData`** — Response body returned by both billing endpoints.

    | Field | Type | Required | Description |
    |---|---|---|---|
    | `url` | `string (uri)` | Yes | Hosted Stripe URL the frontend should redirect the authenticated user to. |

- **New error codes**
  - `STRIPE_CUSTOMER_ALREADY_EXISTS` — Returned by `POST /api/v1/me/billing/checkout` when the authenticated user already has a `stripe_customer_id` and must use the portal endpoint instead.
  - `STRIPE_CUSTOMER_DOES_NOT_EXIST` — Returned by `POST /api/v1/me/billing/portal` when the authenticated user has never had a Stripe customer record persisted.

### Changed

- No existing documented endpoints or schemas changed in this update.

### Removed

- No endpoints or documented schemas were removed in this update.

## 2026-04-19 - Newsletter Invalid Email Error Mapping (`backend#869`)

Backend PR `#869` changes how newsletter subscription failures from Zoho Campaigns are exposed through the REST API. The endpoint path, request body, authentication model, and success response remain unchanged, but specific upstream email validation failures are now reported as client errors instead of internal server errors.

### Added

- **New error code: `INVALID_EMAIL`**
  - Returned by `PUT /api/v1/newsletter-subscriptions` when Zoho Campaigns rejects the submitted address with one of the documented provider error codes below:
    - `2004` — invalid contact email address
    - `2005` — group email address

### Changed

- **`PUT /api/v1/newsletter-subscriptions`**
  - **Response behavior**:
    - `204 No Content` remains unchanged for successful newsletter subscription syncs
    - `400 Bad Request` now also covers provider-level invalid email rejections and returns error code `INVALID_EMAIL`
    - `500 Internal Server Error` is now reserved for unexpected newsletter sync failures, including Zoho Campaigns error responses other than codes `2004` and `2005`
  - **400 error variants**:
    - `BAD_BODY_VALUE` — request body missing, empty, malformed JSON, or invalid request-field value
    - `INVALID_EMAIL` — submitted address was rejected by the newsletter provider as invalid or as a group/distribution email address

### Removed

- No endpoints or documented schemas were removed in this update.

## 2026-04-17 - Newsletter Subscription Sync Endpoint (`backend#854`)

Backend PR `#854` adds a new public newsletter subscription endpoint. The endpoint accepts anonymous requests or optional Cognito-authenticated requests and syncs newsletter contacts using the shared language and currency contracts already documented elsewhere in the API.

### Added

- **New endpoint: `PUT /api/v1/newsletter-subscriptions`**
  - **Authentication**: Optional Bearer JWT. Anonymous requests are allowed. When a valid authenticated user calls the endpoint and omits optional profile fields, the backend falls back to the user's stored `firstName`, `lastName`, `language`, and `currency`. Explicit request values take precedence over fallback values.
  - **Request body**: `PutNewsletterSubscriptionData`

    | Field | Type | Required | Description |
    |---|---|---|---|
    | `email` | `string (email)` | Yes | Email address to subscribe. |
    | `firstName` | `string \| null` | No | Optional first name to sync with the subscription. |
    | `lastName` | `string \| null` | No | Optional last name to sync with the subscription. |
    | `language` | `LanguageData \| null` | No | Optional preferred language to sync with the subscription. |
    | `currency` | `CurrencyData \| null` | No | Optional preferred currency to sync with the subscription. |

  - **Responses**:
    - `204 No Content` — Newsletter subscription synced successfully.
    - `400 Bad Request` — Invalid request body, including empty body, malformed JSON, or invalid field values. Error code: `BAD_BODY_VALUE`.
    - `500 Internal Server Error` — Unexpected failure while syncing the newsletter subscription. Error code: `INTERNAL_SERVER_ERROR`.

  - **Example requests**:

    ```json
    {
      "email": "collector@example.com"
    }
    ```

    ```json
    {
      "email": "collector@example.com",
      "firstName": "Ada",
      "lastName": "Lovelace",
      "language": "en",
      "currency": "EUR"
    }
    ```

- **New schema: `PutNewsletterSubscriptionData`**
  - Introduced a dedicated request-body contract for newsletter subscription upserts.
  - Reuses existing `LanguageData` and `CurrencyData` enums for optional preference fields.

### Changed

- No existing documented endpoints or schemas changed in this update.

### Removed

- No endpoints or documented fields were removed in this update.

## 2026-04-14 - Product Search Slug-ID Filters and Shop Partner-Status Filter (`backend#844`)

Backend PR `#844` extends the shared search-filter contracts used by product search, saved user search filters, and shop search. No endpoint paths changed, but affected request/query schemas now support additional include/exclude filters based on slug identifiers and shop partner status.

### Added

- **`ProductSearchData`** — Added four new optional slug-based filter fields:

  | Field | Type | Description |
  |---|---|---|
  | `shopSlugId` | `string[]` | Include only products whose `shopSlugId` matches one of the provided kebab-case shop slug IDs |
  | `excludeShopSlugId` | `string[]` | Exclude products whose `shopSlugId` matches one of the provided kebab-case shop slug IDs |
  | `sellerSlugId` | `string[]` | Include only products whose seller slug ID matches one of the provided kebab-case slug IDs |
  | `excludeSellerSlugId` | `string[]` | Exclude products whose seller slug ID matches one of the provided kebab-case slug IDs |

- **`PatchProductSearchData`** — Added the same four optional nullable slug-based filter fields for partial search-filter updates.

- **`ShopSearchData`** — Added a new optional `partnerStatus` filter field.

  | Field | Type | Allowed values | Description |
  |---|---|---|---|
  | `partnerStatus` | `ShopPartnerStatusData[]` | `SCRAPED`, `PARTNERED` | Restrict shop search results to one or more partner relationship statuses |

### Changed

- **`GET /api/v1/products`** — The simple product-search query interface now documents the new slug-based include/exclude filters:
  - `shopSlugId`
  - `excludeShopSlugId`
  - `sellerSlugId`
  - `excludeSellerSlugId`

- **`POST /api/v1/products/search`** — The complex product-search request body now supports the same four slug-based filter fields through `ProductSearchData`.

- **Saved search-filter payloads** — Because they embed `ProductSearchData` / `PatchProductSearchData`, the following request/response bodies now support the new slug-based product filters:
  - `PostUserSearchFilterData`
  - `PatchUserSearchFilterData`
  - `UserSearchFilterData`

- **`GET /api/v1/shops`** and **`POST /api/v1/shops/search`** — Shop search now supports filtering by `partnerStatus` with the allowed values `SCRAPED` and `PARTNERED`.

### Removed

- No endpoints or documented fields were removed in this update.

## 2026-04-13 - Expanded Currency Support (`backend#822`)

Backend PR `#822` expands the shared `CurrencyData` contract from 6 to 18 ISO 4217 currencies. No endpoint paths changed, but every query parameter and schema field typed as `CurrencyData`, plus every `PriceData.amount` and price-range filter interpreted in minor units, is affected by this update.

### Added

- **`CurrencyData`** — Added 12 new canonical enum values:

  | Value | Currency |
  |---|---|
  | `CNY` | Chinese Yuan |
  | `BRL` | Brazilian Real |
  | `PLN` | Polish Złoty |
  | `TRY` | Turkish Lira |
  | `JPY` | Japanese Yen |
  | `CZK` | Czech Koruna |
  | `RUB` | Russian Ruble |
  | `AED` | UAE Dirham |
  | `SAR` | Saudi Riyal |
  | `HKD` | Hong Kong Dollar |
  | `SGD` | Singapore Dollar |
  | `CHF` | Swiss Franc |

### Changed

- **`CurrencyData`** — The full supported value set is now:
  - `EUR`, `GBP`, `USD`, `AUD`, `CAD`, `NZD`, `CNY`, `BRL`, `PLN`, `TRY`, `JPY`, `CZK`, `RUB`, `AED`, `SAR`, `HKD`, `SGD`, `CHF`

- **`PriceData.amount` and price-range filter semantics** — Monetary values remain expressed in minor currency units, but the supported set now includes a zero-decimal currency:
  - `JPY` amounts are whole yen (for example, `{"currency":"JPY","amount":1234}` means `¥1234`)
  - all other currently supported currencies use 2 decimal minor units

- **Endpoints with `currency` query parameters** — The `currency` query parameter now accepts all 18 supported `CurrencyData` values on:
  - `GET /api/v1/shops/{shopId}/products/{shopsProductId}`
  - `GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}`
  - `GET /api/v1/shops/{shopId}/products/{shopsProductId}/history`
  - `GET /api/v1/shops/{shopId}/products/{shopsProductId}/similar`
  - `GET /api/v1/products`
  - `GET /api/v1/me/search-filters/{userSearchFilterId}/products`
  - `GET /api/v1/me/watchlist`
  - `POST /api/v1/me/watchlist/{shopId}/{shopsProductId}`
  - `PATCH /api/v1/me/watchlist/{shopId}/{shopsProductId}`
  - `GET /api/v1/me/notifications`
  - `PATCH /api/v1/me/notifications`
  - `PATCH /api/v1/me/notifications/{eventId}`

- **Schemas with `currency: CurrencyData` fields** — These request/response bodies now accept or return the expanded currency set:
  - `PriceData`
  - `ProductSearchData`
  - `PatchProductSearchData`
  - `GetUserAccountData`
  - `PatchUserAccountData`

### Removed

- No endpoints or documented fields were removed in this update.

## 2026-04-13 - Ingestion-Only Languages (`backend#820`)

Backend PR `#820` expands the shared `LanguageData` contract with additional ISO 639-1 language codes for product ingestion and stored/native content. No endpoint paths changed, but every query parameter, request field, and response field typed as `LanguageData` is affected by this schema expansion.

### Added

- **`LanguageData`** — Added new canonical enum values and documented accepted input aliases.

  | Canonical value | Language | Accepted aliases |
  |---|---|---|
  | `zh` | Chinese (Simplified) | `zh-CN`, `zh-Hans` |
  | `pt` | Portuguese | `pt-PT`, `pt-BR` |
  | `pl` | Polish | `pl-PL` |
  | `tr` | Turkish | `tr-TR` |
  | `nl` | Dutch | `nl-NL`, `nl-BE` |
  | `cs` | Czech | `cs-CZ` |
  | `ja` | Japanese | `ja-JP` |
  | `ru` | Russian | `ru-RU` |
  | `ar` | Arabic | `ar-SA`, `ar-EG`, `ar-AE` |

### Changed

- **`LanguageData`** — The full canonical value set is now:
  - `de`, `en`, `fr`, `es`, `it`, `zh`, `pt`, `pl`, `tr`, `nl`, `cs`, `ja`, `ru`, `ar`

- **English alias documentation** — Corrected the documented accepted English regional alias to match the backend implementation exactly:
  - accepted aliases are `en-US`, `en-GB`, `en-AU`, `en-CA`, `en-NZ`, and the backend-specific accepted alias `en_IE`

- **Localization semantics for the new languages** — The newly added values are ingestion-only languages.
  - They may be accepted anywhere `LanguageData` is used and may appear in stored/native content returned by the API.
  - Backend-generated translation targets remain limited to `de`, `en`, `fr`, `es`, and `it`.
  - Where the backend has no dedicated localized fallback string for one of the new language values, it falls back to English.

### Removed

- No endpoints or documented fields were removed in this update.

## 2026-04-12 - Partner Shop Management and Partner Shop Listing (`backend#816`)

Backend PR `#816` adds Cognito-authenticated partner/admin shop-management endpoints, a partner-scoped shop listing endpoint, and updates shop response semantics so `partnerStatus` reflects partner ownership independently of partner API-key creation.

### Added

- **`PATCH /api/v1/shops/{shopId}`** — Update mutable shop metadata as the shop's partner user or as an admin.

  **Authentication:** Requires a valid Cognito JWT. Authorized for:
  - the shop's assigned partner user, or
  - any user with the `ADMIN` role.

  | Path parameter | Type | Description |
  |---|---|---|
  | `shopId` | `string (uuid)` | Unique identifier of the shop to update |

  **Request body:** `PatchShopData` (required)

  | Field | Type | Required | Description |
  |---|---|---|---|
  | `shopType` | `ShopTypeData \| null` | No | Replace the shop type when provided |
  | `domains` | `string[] \| null` | No | Replace the complete shop-domain set when provided |
  | `image` | `string (uri) \| null` | No | Replace the shop image URL when provided |

  Notes:
  - The HTTP body itself must not be empty.
  - Omitted or `null` fields leave the current value unchanged.
  - `{}` is accepted as a no-op update.

  **Response headers:**
  | Header | Description |
  |---|---|
  | `Last-Modified` | RFC 7231 HTTP-date of when the shop was last updated |

  **Response `200`:** `GetShopData`

  **Error responses:** `400 BAD_PATH_PARAMETER_VALUE`, `400 INVALID_UUID`, `400 BAD_BODY_VALUE`, `401 UNAUTHORIZED`, `403 PARTNER_SHOP_NOT_PARTNERED`, `404 SHOP_NOT_FOUND`, `500 INTERNAL_SERVER_ERROR`

---

- **`PUT /api/v1/shops/{shopId}/api-key`** — Create or overwrite the partner API key for a shop.

  **Authentication:** Requires a valid Cognito JWT. Authorized for:
  - the shop's assigned partner user, or
  - any user with the `ADMIN` role.

  | Path parameter | Type | Description |
  |---|---|---|
  | `shopId` | `string (uuid)` | Unique identifier of the shop whose partner API key should be generated |

  **Request body:** None

  **Response `200`:** `PartnerShopApiKeyResponse`

  | Field | Type | Always present | Description |
  |---|---|---|---|
  | `apiKey` | `string` | Yes | Newly generated plaintext partner API key. Returned only in this response |

  **Error responses:** `400 BAD_PATH_PARAMETER_VALUE`, `400 INVALID_UUID`, `401 UNAUTHORIZED`, `403 PARTNER_SHOP_NOT_PARTNERED`, `404 SHOP_NOT_FOUND`, `500 INTERNAL_SERVER_ERROR`

---

- **`GET /api/v1/partner/{partnerId}/shops`** — List all shops linked to a partner user.

  **Authentication:** Requires a valid Cognito JWT. Authorized for:
  - the partner user referenced by `partnerId`, or
  - any user with the `ADMIN` role.

  | Path parameter | Type | Description |
  |---|---|---|
  | `partnerId` | `string (uuid)` | User ID of the partner whose shops should be returned |

  **Response `200`:** `GetShopData[]`

  Returns an empty array when the partner currently has no shops.

  **Error responses:** `400 BAD_PATH_PARAMETER_VALUE`, `400 INVALID_UUID`, `401 UNAUTHORIZED`, `403 FORBIDDEN`, `500 INTERNAL_SERVER_ERROR`

---

- **`PatchShopData`** — New request schema for partial shop updates.

  | Field | Type | Required | Description |
  |---|---|---|---|
  | `shopType` | `ShopTypeData \| null` | No | Optional new shop type |
  | `domains` | `string[] \| null` | No | Optional full replacement set of normalized shop domains |
  | `image` | `string (uri) \| null` | No | Optional new shop image URL |

---

- **`PartnerShopApiKeyResponse`** — New response schema for partner API-key creation.

  | Field | Type | Always present | Description |
  |---|---|---|---|
  | `apiKey` | `string` | Yes | Newly generated plaintext partner API key |

### Changed

- **`GetShopData.partnerStatus`** — Shop responses now explicitly document the partner relationship status.

  | Field | Type | Always present | Description |
  |---|---|---|---|
  | `partnerStatus` | `ShopPartnerStatusData` | Yes | `SCRAPED` when no partner user is linked to the shop; `PARTNERED` when a partner user is linked |

  This affects every documented endpoint returning `GetShopData`, including:
  - `GET /api/v1/shops/{shopId}`
  - `GET /api/v1/by-slug/shops/{shopSlugId}`
  - `POST /api/v1/shops/search`
  - `PATCH /api/v1/shops/{shopId}`
  - `GET /api/v1/partner/{partnerId}/shops`

---

- **Partner status semantics** — A shop can now be `PARTNERED` before any partner API key has been created.

  Frontend implication:
  - `partnerStatus: PARTNERED` means the shop is linked to a partner user.
  - It does **not** guarantee that a partner API key already exists.
  - API-key-protected partner product endpoints still require a valid `x-api-key` header.

### Removed

- No endpoints or documented fields were removed in this update.

## 2026-04-11 - Partner Application Decision Workflow (`backend#812`)

Backend PR `#812` introduces a dedicated admin decision endpoint for partner shop applications and changes partner-application response schemas to expose separate business and workflow execution states.

### Added

- **`POST /api/v1/partner-applications/{partnerApplicationId}/decision`** — Submit an admin review decision for a partner shop application.

  This endpoint requires a valid Cognito JWT **and** the caller's stored user role to be `ADMIN`.

  | Path parameter | Type | Description |
  |---|---|---|
  | `partnerApplicationId` | `string (uuid)` | Unique identifier of the partner shop application |

  **Request body:** `PostPartnerShopApplicationDecisionData` (required)

  | Field | Type | Required | Description |
  |---|---|---|---|
  | `decision` | `PartnerShopApplicationDecisionData` | Yes | Review decision to apply. Allowed values: `APPROVE`, `REJECT` |

  **Response headers:**
  | Header | Description |
  |---|---|
  | `Last-Modified` | RFC 7231 HTTP-date of when the application was last updated |

  **Response `200`:** `GetPartnerShopApplicationData`

  The response keeps `businessState` at `IN_REVIEW` and sets `executionState` to `PROCESSING` while the asynchronous approval/rejection workflow continues.

  **Example request / response:**

  ```json
  {
    "decision": "APPROVE"
  }
  ```

  ```json
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "businessState": "IN_REVIEW",
    "executionState": "PROCESSING",
    "payload": {
      "type": "NEW",
      "shopName": "My Antique Store",
      "shopType": "COMMERCIAL_DEALER",
      "shopDomains": ["my-antique-store.com"],
      "shopImage": "https://my-antique-store.com/logo.png"
    },
    "created": "2026-04-09T10:00:00Z",
    "updated": "2026-04-11T12:00:00Z"
  }
  ```

  **Error responses:** `400 BAD_PATH_PARAMETER_VALUE`, `400 INVALID_UUID`, `400 BAD_BODY_VALUE`, `401 UNAUTHORIZED`, `403 FORBIDDEN`, `404 PARTNER_SHOP_APPLICATION_NOT_FOUND`, `409 CONFLICT`, `500 INTERNAL_SERVER_ERROR`

---

- **`ExecutionStateData`** — New enum schema describing the workflow progress of a partner shop application.

  | Value | Description |
  |---|---|
  | `PROCESSING` | The backend workflow is actively processing the application |
  | `WAITING` | The workflow is paused and waiting for an admin decision |
  | `COMPLETED` | The workflow has finished processing the application |

---

- **`PartnerShopApplicationDecisionData`** — New enum schema used by admin decision requests.

  | Value | Description |
  |---|---|
  | `APPROVE` | Resume the review workflow with an approval decision |
  | `REJECT` | Resume the review workflow with a rejection decision |

### Changed

- **`GetPartnerShopApplicationData`** — Partner application responses now expose separate business and execution states.

  | Field | Type | Always present | Description |
  |---|---|---|---|
  | `businessState` | `PartnerShopApplicationStateData` | Yes | Review/business state of the application (`SUBMITTED`, `IN_REVIEW`, `REJECTED`, `APPROVED`) |
  | `executionState` | `ExecutionStateData` | Yes | Workflow execution state (`PROCESSING`, `WAITING`, `COMPLETED`) |

  This schema change affects every endpoint returning partner applications:
  - `GET /api/v1/me/partner-applications`
  - `POST /api/v1/me/partner-applications`
  - `GET /api/v1/me/partner-applications/{partnerApplicationId}`
  - `PATCH /api/v1/me/partner-applications/{partnerApplicationId}`
  - `GET /api/v1/partner-applications`
  - `GET /api/v1/partner-applications/{partnerApplicationId}`
  - `PATCH /api/v1/partner-applications/{partnerApplicationId}`
  - `POST /api/v1/partner-applications/{partnerApplicationId}/decision`

---

- **`POST /api/v1/me/partner-applications`** — Creation responses now return `businessState: SUBMITTED` together with `executionState: PROCESSING`.

  Newly created applications immediately enter backend workflow processing instead of exposing only a single review-state field.

### Removed

- **`AdminPatchPartnerShopApplicationData.state`** — Admin patch requests can no longer update review state directly.

  Review transitions must now be triggered through `POST /api/v1/partner-applications/{partnerApplicationId}/decision`. The admin patch schema still supports payload-only updates such as `shopName`, `shopType`, `shopDomains`, and `shopImage`.

---

- **`GetPartnerShopApplicationData.state`** — Removed from partner-application responses.

  Clients must now read `businessState` for the review outcome and `executionState` for workflow progress.

## 2026-04-10 - Partner Application Notification Payloads (`backend#807`)

Backend PR `#807` extends notification responses with a new partner-application notification variant so clients can distinguish partner application approval and rejection events from watchlist and search-filter notifications.

### Added

- **`PartnerApplicationNotificationPayloadData`** — New notification payload schema for partner application review events.

  | Field | Type | Always present | Description |
  |---|---|---|---|
  | `type` | `"PARTNER_APPLICATION"` | Yes | Discriminator for partner application notifications |
  | `shopName` | `string` | Yes | Display name of the shop referenced by the partner application |
  | `partnerApplicationPayload` | `PartnerApplicationPayloadData` | Yes | Partner-application-specific review result payload |

---

- **`PartnerApplicationPayloadData`** — New nested discriminated union for partner application notification payloads.

  | Variant | Fields | Description |
  |---|---|---|
  | `APPROVED` | `partnerApplicationId` (`string (uuid)`) | The referenced partner application was approved |
  | `REJECTED` | `partnerApplicationId` (`string (uuid)`) | The referenced partner application was rejected |

### Changed

- **`NotificationPayloadData`** — Extended discriminated union; new variant added:
  - `PARTNER_APPLICATION` → `PartnerApplicationNotificationPayloadData`

  This variant is returned by notification endpoints alongside the existing `WATCHLIST` and `SEARCH_FILTER` payloads.

---

- **Affected endpoints returning notification payloads**:
  - `GET /api/v1/me/notifications`
  - `PATCH /api/v1/me/notifications`
  - `PATCH /api/v1/me/notifications/{eventId}`

  These responses may now include partner application review notifications with payloads such as:

  ```json
  {
    "type": "PARTNER_APPLICATION",
    "shopName": "Tech Store",
    "partnerApplicationPayload": {
      "type": "APPROVED",
      "partnerApplicationId": "550e8400-e29b-41d4-a716-446655440002"
    }
  }
  ```

## 2026-04-10 - Admin Partner Shop Application REST API (`backend#803`)

Backend PR `#803` adds admin-only partner shop application management endpoints and extends user account responses with the user's role. These changes are intended for internal frontend/backend coordination and do not introduce a public version bump.

### Added

- **`GET /api/v1/partner-applications`** — List all partner shop applications across all users.

  This endpoint requires a valid Cognito JWT **and** the caller's stored user role to be `ADMIN`.

  **Response headers:**
  | Header | Value |
  |---|---|
  | `Cache-Control` | `no-store` |

  **Response `200`:** `GetPartnerShopApplicationData[]`

  **Error responses:** `401 UNAUTHORIZED`, `403 FORBIDDEN`, `500 INTERNAL_SERVER_ERROR`

---

- **`GET /api/v1/partner-applications/{partnerApplicationId}`** — Get a specific partner shop application by ID as an admin.

  This lookup is global across all users and does not require the applicant user's ID. The endpoint requires a valid Cognito JWT **and** the caller's stored user role to be `ADMIN`.

  | Path parameter | Type | Description |
  |---|---|---|
  | `partnerApplicationId` | `string (uuid)` | Unique identifier of the partner shop application |

  **Response headers:**
  | Header | Description |
  |---|---|
  | `Last-Modified` | RFC 7231 HTTP-date of when the application was last updated |
  | `Cache-Control` | `no-store` |

  **Response `200`:** `GetPartnerShopApplicationData`

  **Error responses:** `400 BAD_PATH_PARAMETER_VALUE`, `400 INVALID_UUID`, `401 UNAUTHORIZED`, `403 FORBIDDEN`, `404 PARTNER_SHOP_APPLICATION_NOT_FOUND`, `500 INTERNAL_SERVER_ERROR`

---

- **`PATCH /api/v1/partner-applications/{partnerApplicationId}`** — Update a specific partner shop application by ID as an admin.

  This endpoint requires a valid Cognito JWT **and** the caller's stored user role to be `ADMIN`. Unlike the user-facing partner application patch endpoint, admins can update the review `state` in addition to payload fields.

  | Path parameter | Type | Description |
  |---|---|---|
  | `partnerApplicationId` | `string (uuid)` | Unique identifier of the partner shop application |

  **Request body:** `AdminPatchPartnerShopApplicationData` (required)

  **Response headers:**
  | Header | Description |
  |---|---|
  | `Last-Modified` | RFC 7231 HTTP-date of when the application was last updated |

  **Response `200`:** `GetPartnerShopApplicationData`

  **Error responses:** `400 BAD_PATH_PARAMETER_VALUE`, `400 INVALID_UUID`, `400 BAD_BODY_VALUE`, `401 UNAUTHORIZED`, `403 FORBIDDEN`, `404 PARTNER_SHOP_APPLICATION_NOT_FOUND`, `500 INTERNAL_SERVER_ERROR`

---

- **`AdminPatchPartnerShopApplicationData`** — New schema for admin-only partial updates to partner shop applications. All fields are optional.

  | Field | Type | Description |
  |---|---|---|
  | `state` | `PartnerShopApplicationStateData` | Updated review state |
  | `shopName` | `string` | Updated display name of the shop |
  | `shopType` | `ShopTypeData` | Updated shop type |
  | `shopDomains` | `string[]` | Updated set of normalized domains (replaces existing) |
  | `shopImage` | `string (uri)` | Updated shop logo URL |

---

### Changed

- **`GetUserAccountData`** — New required field `role` added to user account responses.

  | Field | Type | Always present | Description |
  |---|---|---|---|
  | `role` | `UserRoleData` | Yes | The authenticated user's role used for API authorization |

  This affects responses from endpoints that return `GetUserAccountData`, including:
  - `GET /api/v1/me/account`
  - `PATCH /api/v1/me/account`

  The user account patch request body remains unchanged; clients cannot set `role` through `PATCH /api/v1/me/account`.

---

- **`UserRoleData`** — New enum schema used in user account responses.

  | Value | Description |
  |---|---|
  | `USER` | Standard authenticated user |
  | `ADMIN` | Administrator with access to admin-only endpoints |

## 2026-04-09 - Partner Shop Application REST API (`backend#795`)

Users can now submit, view, update, and delete partner shop applications. A partner shop application represents a request by a user to have a shop (new or existing) granted partner status. Applications progress through a review lifecycle managed by the backend team; the `state` field is read-only from the client side.

### Added

- **`GET /api/v1/me/partner-applications`** — List all partner shop applications for the authenticated user.

  Returns an array of `GetPartnerShopApplicationData` objects. Returns an empty array when no applications exist.

  **Response headers:**
  | Header | Value |
  |---|---|
  | `Cache-Control` | `no-store` |

  **Response `200`:** `GetPartnerShopApplicationData[]`

  **Error responses:** `401 UNAUTHORIZED`, `500 INTERNAL_SERVER_ERROR`

---

- **`POST /api/v1/me/partner-applications`** — Submit a new partner shop application.

  The request body must be a `PostPartnerShopApplicationPayloadData` object specifying either an existing shop or a new shop. The application is created with state `SUBMITTED`.

  **Request body:** `PostPartnerShopApplicationPayloadData` (required)

  **Response headers:**
  | Header | Description |
  |---|---|
  | `Last-Modified` | RFC 7231 HTTP-date of when the application was last updated |

  **Response `201`:** `GetPartnerShopApplicationData`

  **Error responses:** `400 BAD_BODY_VALUE`, `401 UNAUTHORIZED`, `500 INTERNAL_SERVER_ERROR`

---

- **`GET /api/v1/me/partner-applications/{partnerApplicationId}`** — Get a specific partner shop application by ID.

  | Path parameter | Type | Description |
  |---|---|---|
  | `partnerApplicationId` | `string (uuid)` | Unique identifier of the partner shop application |

  **Response headers:**
  | Header | Description |
  |---|---|
  | `Last-Modified` | RFC 7231 HTTP-date of when the application was last updated |
  | `Cache-Control` | `no-store` |

  **Response `200`:** `GetPartnerShopApplicationData`

  **Error responses:** `400 BAD_PATH_PARAMETER_VALUE`, `400 INVALID_UUID`, `401 UNAUTHORIZED`, `404 PARTNER_SHOP_APPLICATION_NOT_FOUND`, `500 INTERNAL_SERVER_ERROR`

---

- **`PATCH /api/v1/me/partner-applications/{partnerApplicationId}`** — Update a partner shop application.

  Only fields present in the request body are applied. The `state` field is **read-only** and cannot be updated.

  | Path parameter | Type | Description |
  |---|---|---|
  | `partnerApplicationId` | `string (uuid)` | Unique identifier of the partner shop application |

  **Request body:** `PatchPartnerShopApplicationData` (required)

  **Response headers:**
  | Header | Description |
  |---|---|
  | `Last-Modified` | RFC 7231 HTTP-date of when the application was last updated |

  **Response `200`:** `GetPartnerShopApplicationData`

  **Error responses:** `400 BAD_PATH_PARAMETER_VALUE`, `400 INVALID_UUID`, `400 BAD_BODY_VALUE`, `401 UNAUTHORIZED`, `404 PARTNER_SHOP_APPLICATION_NOT_FOUND`, `500 INTERNAL_SERVER_ERROR`

---

- **`DELETE /api/v1/me/partner-applications/{partnerApplicationId}`** — Delete a partner shop application.

  | Path parameter | Type | Description |
  |---|---|---|
  | `partnerApplicationId` | `string (uuid)` | Unique identifier of the partner shop application |

  **Response `204`:** No content

  **Error responses:** `400 BAD_PATH_PARAMETER_VALUE`, `400 INVALID_UUID`, `401 UNAUTHORIZED`, `404 PARTNER_SHOP_APPLICATION_NOT_FOUND`, `500 INTERNAL_SERVER_ERROR`

---

- **`PartnerShopApplicationStateData`** — New schema representing the review state of a partner shop application.

  | Value | Description |
  |---|---|
  | `SUBMITTED` | Application has been submitted and is awaiting review |
  | `IN_REVIEW` | Application is currently being reviewed |
  | `REJECTED` | Application was reviewed and rejected |
  | `APPROVED` | Application was reviewed and approved |

  State is **read-only** from the client perspective.

---

- **`GetPartnerShopApplicationPayloadData`** — New schema for the payload portion of a partner shop application response. Discriminated by the `type` field.

  **`type: "EXISTING"`** — Targets an existing shop:
  | Field | Type | Required | Description |
  |---|---|---|---|
  | `type` | `string` | Yes | Always `"EXISTING"` |
  | `shopId` | `string (uuid)` | Yes | Unique identifier of the existing shop |

  **`type: "NEW"`** — Describes a new shop:
  | Field | Type | Required | Description |
  |---|---|---|---|
  | `type` | `string` | Yes | Always `"NEW"` |
  | `shopName` | `string` | Yes | Display name of the shop (max 255 chars) |
  | `shopType` | `ShopTypeData` | Yes | Type of the shop |
  | `shopDomains` | `string[]` | Yes | Unique set of normalized domains |
  | `shopImage` | `string (uri)` | No | Optional URL to the shop logo/image |

---

- **`GetPartnerShopApplicationData`** — New schema for a complete partner shop application.

  | Field | Type | Required | Description |
  |---|---|---|---|
  | `id` | `string (uuid)` | Yes | Unique identifier of the application |
  | `state` | `PartnerShopApplicationStateData` | Yes | Current review state (read-only) |
  | `payload` | `GetPartnerShopApplicationPayloadData` | Yes | Application payload (EXISTING or NEW) |
  | `created` | `string (date-time)` | Yes | When the application was created (RFC3339) |
  | `updated` | `string (date-time)` | Yes | When the application was last updated (RFC3339) |

---

- **`PostPartnerShopApplicationPayloadData`** — New schema for creating a partner shop application. Discriminated by the `type` field.

  **`type: "EXISTING"`** — Apply for an existing shop:
  | Field | Type | Required | Description |
  |---|---|---|---|
  | `type` | `string` | Yes | Always `"EXISTING"` |
  | `shopId` | `string (uuid)` | Yes | Unique identifier of the existing shop |

  **`type: "NEW"`** — Apply for a new shop:
  | Field | Type | Required | Description |
  |---|---|---|---|
  | `type` | `string` | Yes | Always `"NEW"` |
  | `shopName` | `string` | Yes | Display name of the shop (max 255 chars) |
  | `shopType` | `ShopTypeData` | Yes | Type of the shop |
  | `shopDomains` | `string[]` | Yes | Unique set of normalized domains |
  | `shopImage` | `string (uri)` | No | Optional URL to the shop logo/image |

---

- **`PatchPartnerShopApplicationData`** — New schema for partial updates to a partner shop application. All fields are optional.

  | Field | Type | Description |
  |---|---|---|
  | `shopName` | `string` | Updated display name of the shop |
  | `shopType` | `ShopTypeData` | Updated shop type |
  | `shopDomains` | `string[]` | Updated set of normalized domains (replaces existing) |
  | `shopImage` | `string (uri)` | Updated shop logo URL (`null` to remove) |

---

- **New error code `PARTNER_SHOP_APPLICATION_NOT_FOUND`** — Returned as `404` when no partner shop application exists for the authenticated user with the requested ID.

---

## 2026-04-08 - Search-Filter Match Quota Enforcement on Product Views (`backend#785`)

Products matched by a user's saved search filters are now subject to the user's monthly search-filter match quota. Matches beyond the quota are marked as hidden and the product data is anonymized so no identifying information leaks to the client.

### Changed

- **`SearchFilterUserStateData`** — new required field `hidden` added.

  | Field | Type | Always present | Description |
  |---|---|---|---|
  | `matched` | `boolean` | Yes | `true` if any of the user's saved search filters matched this product; `false` otherwise. |
  | `hidden` | `boolean` | Yes | `true` if this match exceeds the user's monthly search-filter match quota and the product is anonymized; `false` otherwise. Defaults to `false`. |
  | `userSearchFilterId` | `string (uuid)` | No | The ID of the matching saved search filter. Present only when `matched` is `true`. |
  | `userSearchFilterName` | `string` | No | The user-defined name of the matching saved search filter. Present only when `matched` is `true`. |
  | `matchReason` | `string` | No | Human-readable explanation of why the filter matched. Only set for AI-enhanced filters. |

  When `hidden` is `true`, the product data in the same response object is anonymized:
  - `productId` → nil UUID (`00000000-0000-0000-0000-000000000000`)
  - `title` → language-specific placeholder string
  - `condition` → `"Unknown"`
  - All other UUID fields → nil UUID
  - All other enum fields → their `Unknown` variant
  - Optional fields → omitted
  - Timestamps → Unix epoch (`1970-01-01T00:00:00Z`)

  Only matches ranked beyond the user's tier quota (ordered by ascending match creation time) are hidden. The first N matches (where N is the quota for the user's tier) remain fully visible.

  Example — product matched within quota (visible):
  ```json
  {
    "matched": true,
    "hidden": false,
    "userSearchFilterId": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
    "userSearchFilterName": "Vintage Art Deco",
    "matchReason": "Product matches the vintage Art Deco style described in your search filter."
  }
  ```

  Example — product matched beyond quota (hidden, anonymized):
  ```json
  {
    "matched": true,
    "hidden": true
  }
  ```

  Example — product not matched (default):
  ```json
  {
    "matched": false,
    "hidden": false
  }
  ```

  This field is present on every personalized product response across:
  - `GET /api/v1/products/{productId}`
  - `GET /api/v1/products/{productId}/similar`
  - `GET /api/v1/products`
  - `GET /api/v1/me/search-filters/{userSearchFilterId}/products`

---

## 2026-04-06 - Search Filter Matching Metadata in Product Views (`backend#775`)

Product views now expose whether and why one of the authenticated user's saved search filters matched a product. This is surfaced as a new `searchFilter` field on `ProductUserStateData`, which is returned as `userState` on every personalized product response.

### Added

- **`SearchFilterUserStateData`** — new schema representing the per-product search-filter match state for the authenticated user.

  | Field | Type | Always present | Description |
  |---|---|---|---|
  | `matched` | `boolean` | Yes | `true` if any of the user's saved search filters matched this product; `false` otherwise. Defaults to `false`. |
  | `userSearchFilterId` | `string (uuid)` | No | The ID of the matching saved search filter. Present only when `matched` is `true`; omitted otherwise. |
  | `userSearchFilterName` | `string` | No | The user-defined name of the matching saved search filter. Present only when `matched` is `true`; omitted otherwise. |
  | `matchReason` | `string` | No | Human-readable explanation of why the filter matched. Only set for AI-enhanced filters (those with `enhancedSearchDescription`). Omitted when not available. |

  Example — product matched by a saved filter:
  ```json
  {
    "matched": true,
    "userSearchFilterId": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
    "userSearchFilterName": "Vintage Art Deco",
    "matchReason": "Product matches the vintage Art Deco style described in your search filter."
  }
  ```

  Example — product not matched (default):
  ```json
  {
    "matched": false
  }
  ```

### Changed

- **`ProductUserStateData`** (the `userState` object on personalized product responses across `GET /api/v1/products/{productId}`, `GET /api/v1/products/{productId}/similar`, `GET /api/v1/products`, `GET /api/v1/me/search-filters/{userSearchFilterId}/products`) — new required field.

  | Field | Type | Required | Description |
  |---|---|---|---|
  | `searchFilter` | `SearchFilterUserStateData` | Yes | The authenticated user's search-filter match state for this product. |

  Full `ProductUserStateData` example with the new field:
  ```json
  {
    "watchlist": { "watching": false, "notifications": false },
    "prohibitedContent": { "consent": false },
    "notification": { "seen": true },
    "searchFilter": { "matched": false }
  }
  ```

---

## 2026-04-06 - AI-Instructed Search Filter Matching (`backend#771`)

Search filters can now carry an optional `enhancedSearchDescription` — a free-text, natural-language description of what the user is actually looking for. When set, only products that are genuinely relevant to this description are surfaced as matches. From the REST API perspective this introduces a single new optional field on all search-filter request and response types.

### Changed

- **`PostUserSearchFilterData`** (request body of `POST /api/v1/me/search-filters`) — new optional field.

  | Field | Type | Required | Description |
  |---|---|---|---|
  | `enhancedSearchDescription` | `string` | No | Natural-language description for AI-enhanced product matching. Whitespace is trimmed; values longer than 500 characters are silently truncated. |

  Example request body with the new field:
  ```json
  {
    "name": "Art Deco cufflinks",
    "enhancedSearchDescription": "Golden cufflinks from the Art Deco period, preferably from French workshops",
    "search": {
      "language": "en",
      "currency": "EUR",
      "categoryId": ["jewelry"]
    }
  }
  ```

- **`PatchUserSearchFilterData`** (request body of `PATCH /api/v1/me/search-filters/{userSearchFilterId}`) — new optional field.

  | Field | Type | Required | Description |
  |---|---|---|---|
  | `enhancedSearchDescription` | `string` | No | Natural-language description for AI-enhanced product matching. Omit to leave the existing value unchanged. Whitespace is trimmed; values longer than 500 characters are silently truncated. |

  Example patch body setting the description:
  ```json
  {
    "enhancedSearchDescription": "Golden cufflinks from the Art Deco period, preferably from French workshops"
  }
  ```

- **`UserSearchFilterData`** (response of `GET /api/v1/me/search-filters`, `GET /api/v1/me/search-filters/{userSearchFilterId}`, `POST /api/v1/me/search-filters`, `PATCH /api/v1/me/search-filters/{userSearchFilterId}`) — new optional field.

  | Field | Type | Always present | Description |
  |---|---|---|---|
  | `enhancedSearchDescription` | `string` | No | The stored natural-language description for AI-enhanced matching. Omitted from the response when not set. |

  Example response with the new field:
  ```json
  {
    "userId": "550e8400-e29b-41d4-a716-446655440000",
    "userSearchFilterId": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
    "name": "Art Deco cufflinks",
    "enhancedSearchDescription": "Golden cufflinks from the Art Deco period, preferably from French workshops",
    "notifications": true,
    "search": { "language": "en", "currency": "EUR" },
    "created": "2026-04-06T10:00:00Z",
    "updated": "2026-04-06T10:00:00Z"
  }
  ```

---

## 2026-04-05 - User Deletion (`backend#770`)

Users can now permanently delete their own account via a dedicated DELETE endpoint. The deletion is synchronous and the access token is immediately invalidated upon success.

### Added

- **`DELETE /api/v1/me`** — permanently deletes the authenticated user's account.

  | Aspect | Detail |
  |---|---|
  | Authentication | Bearer JWT (Cognito) |
  | Request body | None |
  | Success response | `204 No Content` |

  Possible error responses:

  | HTTP Status | Error code | Condition |
  |---|---|---|
  | `401` | `UNAUTHORIZED` | Missing or invalid JWT token. |
  | `404` | `USER_NOT_FOUND` | The authenticated user does not exist. |
  | `500` | `INTERNAL_SERVER_ERROR` | Unexpected server error (e.g. deletion service not available). |

---

## 2026-04-04 - User Tiers (`backend#765`)

The user subscription tier system has been expanded. Previously only `FREE` was available; two new tiers — `PRO` and `ULTIMATE` — are now possible values on any endpoint that returns or accepts `UserTierData`. This also introduces tier-based feature gating on search filters: certain search filter fields are restricted for `FREE` tier users, and the search filter and watchlist quota limits have been updated across all tiers.

### Changed

- **`UserTierData`** — enum extended with two new values.

  | Value | Change | Description |
  |---|---|---|
  | `FREE` | Updated quota | Default tier. Allows up to 20 watchlist entries, up to 1 search filter with a restricted set of filter fields, and up to 10 matched products per filter. |
  | `PRO` | **Added** | Premium tier. Allows up to 100 watchlist entries, up to 5 search filters with access to all filter fields, and unlimited matched products per filter. |
  | `ULTIMATE` | **Added** | Highest tier. Allows unlimited watchlist entries, unlimited search filters with access to all filter fields, and unlimited matched products per filter. |

- **`POST /api/v1/me/search-filters`** — tier-based feature restrictions enforced at creation time.

  `FREE` tier users may only use the following filter fields in `productSearch`: `productQuery`, `categoryId`, `periodId`, `price`, `state`. Providing any other field (e.g. `shopName`, `sellerName`, `originYear`, `authenticity`, `condition`, `provenance`, `restoration`, `created`, `updated`, `auctionStart`, `auctionEnd`, ...) results in `HTTP 422` with error code `SEARCH_FILTER_RESTRICTED_FEATURE`.

  Updated 422 response — now returns either `SEARCH_FILTER_QUOTA_EXCEEDED` or `SEARCH_FILTER_RESTRICTED_FEATURE`:

  | Error code | Condition |
  |---|---|
  | `SEARCH_FILTER_QUOTA_EXCEEDED` | User has reached the maximum number of allowed search filters for their tier. |
  | `SEARCH_FILTER_RESTRICTED_FEATURE` | The request body contains a search filter field that is not available for the user's current tier. |

  Example `SEARCH_FILTER_RESTRICTED_FEATURE` response:
  ```json
  {
    "status": 422,
    "title": "Unprocessable Content",
    "error": "SEARCH_FILTER_RESTRICTED_FEATURE",
    "detail": "Search filter contains forbidden search field 'shopName' which requires a higher user tier."
  }
  ```

- **`PATCH /api/v1/me/search-filters/{userSearchFilterId}`** — tier-based feature restrictions enforced at update time, and new error responses added.

  `FREE` tier users may only use the same restricted set of filter fields described above. Providing a forbidden field results in `HTTP 422` with error code `SEARCH_FILTER_RESTRICTED_FEATURE`.

  New possible responses:

  | HTTP Status | Error code | Condition |
  |---|---|---|
  | `404` | `USER_NOT_FOUND` | The authenticated user no longer exists. |
  | `422` | `SEARCH_FILTER_RESTRICTED_FEATURE` | The request body contains a search filter field that is not available for the user's current tier. |

  Example `SEARCH_FILTER_RESTRICTED_FEATURE` response:
  ```json
  {
    "status": 422,
    "title": "Unprocessable Content",
    "error": "SEARCH_FILTER_RESTRICTED_FEATURE",
    "detail": "Search filter contains forbidden search field 'shopName' which requires a higher user tier."
  }
  ```

## 2026-04-03 - Seller Fields on Product Responses and Search Filters (`backend#764`)

Products now expose a `sellerName` field in all response types. Additionally, the product search API supports two new dedicated filter fields — `sellerName` and `excludeSellerName` — for keyword-exact inclusion and exclusion filtering by seller name, applied independently from the existing `shopName`/`excludeShopName` filters.

### Changed

- **`GetProductData`** — new required `sellerName` field added.

  | Field | Change | Type | Description |
  |---|---|---|---|
  | `sellerName` | **Added** | `string` | Display name of the seller associated with the product |

- **`GetProductSummaryData`** — new required `sellerName` field added.

  | Field | Change | Type | Description |
  |---|---|---|---|
  | `sellerName` | **Added** | `string` | Display name of the seller associated with the product |

- **`ProductSearchData`** — two new optional filter fields added.

  | Field | Change | Type | Description |
  |---|---|---|---|
  | `sellerName` | **Added** | `string[]` | Include only products whose seller name exactly matches one of the given values. Applied independently from `shopName`. Defaults to `[]`. |
  | `excludeSellerName` | **Added** | `string[]` | Exclude products whose seller name exactly matches one of the given values. Applied independently from `excludeShopName`. Defaults to `[]`. |

- **`PatchProductSearchData`** — two new optional patch fields added.

  | Field | Change | Type | Description |
  |---|---|---|---|
  | `sellerName` | **Added** | `string[] \| null` | Replace the seller name inclusion filter. `null` leaves it unchanged. |
  | `excludeSellerName` | **Added** | `string[] \| null` | Replace the seller name exclusion filter. `null` leaves it unchanged. |

## 2026-04-03 - First Product Image in Notifications (`backend#763`)

Notification payloads now include the first image of the relevant product. Both watchlist and search-filter notification variants expose a new optional `image` field. When the product had no images at the time the notification was generated the field is absent.

Note: The image URL is **always** included in notification payloads — prohibited content filtering is intentionally skipped for notifications.

### Changed

- **`WatchlistNotificationPayloadData`** — new optional `image` field added.

  | Field | Change | Type | Description |
  |---|---|---|---|
  | `image` | **Added** | `ProductImageData \| null` | First image of the product at notification time. Absent when the product had no images. |

  Updated response example (`GET /api/v1/me/notifications`):
  ```json
  {
    "type": "WATCHLIST",
    "productId": "550e8400-e29b-41d4-a716-446655440000",
    "shopId": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
    "shopsProductId": "ABC123",
    "shopSlugId": "tech-store",
    "productSlugId": "amazing-product-fa87c4",
    "shopName": "Tech Store",
    "title": { "text": "Amazing Product", "language": "en" },
    "image": {
      "url": "https://my-shop.com/images/product-1.jpg",
      "prohibitedContent": "NONE"
    },
    "watchlistPayload": {
      "type": "STATE_CHANGE",
      "oldState": "LISTED",
      "newState": "AVAILABLE"
    }
  }
  ```

- **`SearchFilterNotificationPayloadData`** — new optional `image` field added.

  | Field | Change | Type | Description |
  |---|---|---|---|
  | `image` | **Added** | `ProductImageData \| null` | First image of the product at notification time. Absent when the product had no images. |

  Updated response example (`GET /api/v1/me/notifications`):
  ```json
  {
    "type": "SEARCH_FILTER",
    "productId": "550e8400-e29b-41d4-a716-446655440000",
    "shopId": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
    "shopsProductId": "ABC123",
    "shopSlugId": "antique-store",
    "productSlugId": "baroque-clock-fa87c4",
    "shopName": "Antique Store",
    "title": { "text": "Baroque Clock", "language": "en" },
    "image": {
      "url": "https://antique-store.com/images/clock-1.jpg",
      "prohibitedContent": "NONE"
    },
    "searchFilterPayload": {
      "userSearchFilterId": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
      "userSearchFilterName": "My Baroque Clock Search"
    }
  }
  ```

## 2026-04-03 - Secondary Shops (`backend#758`)

Partner endpoints for creating and upserting products now support an optional `sellerName` field. This enables `AUCTION_PLATFORM` and `MARKETPLACE` shop types to attribute a product to a secondary seller shop rather than the partner shop itself.

### Changed

- **`PostProductData`** — new optional `sellerName` field added.

  | Field | Change | Type | Description |
  |---|---|---|---|
  | `sellerName` | **Added** | `string \| null` | Optional raw name of the secondary seller for this product. Only effective for `AUCTION_PLATFORM` and `MARKETPLACE` shop types. |

  When `sellerName` is provided and the authenticating shop's type is `AUCTION_PLATFORM` or `MARKETPLACE`, the backend resolves the seller shop by name and records the product under that seller's identity.
  When omitted — or when the shop type is anything other than `AUCTION_PLATFORM` or `MARKETPLACE` — the partner shop itself remains the seller (identical to previous behaviour).

  Updated request body example (`POST /api/v1/shops/{shopId}/products`):
  ```json
  [
    {
      "shopsProductId": "lot-4711",
      "title": { "text": "Baroque Violin", "language": "en" },
      "description": { "text": "A fine 18th-century violin.", "language": "en" },
      "state": "AVAILABLE",
      "url": "https://auction-platform.com/lots/4711",
      "images": ["https://auction-platform.com/images/lot-4711.jpg"],
      "sellerName": "Christie's London"
    }
  ]
  ```

- **`PutProductData`** — new optional `sellerName` field added.

  | Field | Change | Type | Description |
  |---|---|---|---|
  | `sellerName` | **Added** | `string \| null` | Optional raw name of the secondary seller for this product. Only effective for `AUCTION_PLATFORM` and `MARKETPLACE` shop types. Only used when creating a new product. |

  Same resolution logic as for `PostProductData` — only active for `AUCTION_PLATFORM` and `MARKETPLACE` shop types.
  On the upsert path the field is only taken into account when the product is being **created**, not when an existing product is updated.

  Updated request body example (`PUT /api/v1/shops/{shopId}/products`):
  ```json
  [
    {
      "shopsProductId": "lot-4711",
      "state": "LISTED",
      "sellerName": "Christie's London"
    }
  ]
  ```

## 2026-04-02 - Remove display-description & add product-counts for category and period (`backend#751`)

The `description` (localized display description) field has been removed from the category and period detail responses. In its place, all category and period responses — both detail and summary — now include a `products` field that indicates the total number of products associated with that category or period.

### Changed

- **`GetCategoryData`** — `description` field removed; `products` field added.

  | Field | Change | Type | Description |
  |---|---|---|---|
  | `description` | **Removed** | — | Localized display description is no longer part of the response |
  | `products` | **Added** | `integer` | Total number of products in this category |

  Updated response example (`GET /api/v1/categories/{categoryId}`):
  ```json
  {
    "categoryId": "musical-instruments",
    "categoryKey": "musical-instruments",
    "name": { "text": "Musical Instruments", "language": "en" },
    "products": 42,
    "created": "2024-01-01T00:00:00Z",
    "updated": "2024-06-01T00:00:00Z"
  }
  ```

- **`GetCategorySummaryData`** — `products` field added.

  | Field | Change | Type | Description |
  |---|---|---|---|
  | `products` | **Added** | `integer` | Total number of products in this category |

  Updated response example (`GET /api/v1/categories`, `POST /api/v1/categories/search`):
  ```json
  {
    "categoryId": "musical-instruments",
    "categoryKey": "musical-instruments",
    "name": { "text": "Musical Instruments", "language": "en" },
    "products": 42,
    "created": "2024-01-01T00:00:00Z",
    "updated": "2024-06-01T00:00:00Z"
  }
  ```

- **`GetPeriodData`** — `description` field removed; `products` field added.

  | Field | Change | Type | Description |
  |---|---|---|---|
  | `description` | **Removed** | — | Localized display description is no longer part of the response |
  | `products` | **Added** | `integer` | Total number of products in this period |

  Updated response example (`GET /api/v1/periods/{periodId}`):
  ```json
  {
    "periodId": "renaissance",
    "periodKey": "RENAISSANCE",
    "name": { "text": "Renaissance", "language": "en" },
    "products": 42,
    "created": "2024-01-01T00:00:00Z",
    "updated": "2024-06-01T00:00:00Z"
  }
  ```

- **`GetPeriodSummaryData`** — `products` field added.

  | Field | Change | Type | Description |
  |---|---|---|---|
  | `products` | **Added** | `integer` | Total number of products in this period |

  Updated response example (`GET /api/v1/periods`, `POST /api/v1/periods/search`):
  ```json
  {
    "periodId": "renaissance",
    "periodKey": "RENAISSANCE",
    "name": { "text": "Renaissance", "language": "en" },
    "products": 42,
    "created": "2024-01-01T00:00:00Z",
    "updated": "2024-06-01T00:00:00Z"
  }
  ```

## 2026-04-01 - Extend Batch Product Update: Fine-grained field updates (`backend#745`)

The partner batch-update endpoint (`PATCH /api/v1/shops/{shopId}/products`) now accepts 11 additional optional fields per product entry. Previously only `price` and `state` could be updated; any other fields were silently ignored. With this change, partners can independently update `priceEstimateMin`, `priceEstimateMax`, `url`, `images`, `auctionStart`, `auctionEnd`, `originYear`, `authenticity`, `condition`, `provenance`, and `restoration`. Each field remains optional and is only changed when explicitly provided.

Each updated field emits its own dedicated product domain event, which is now also surfaced through the product event history API.

### Changed

- **`PatchProductData`** — 11 new optional fields added. Each field is independently updatable. When omitted, the corresponding product attribute is left unchanged.

  | Field | Type | Description |
  |---|---|---|
  | `priceEstimateMin` | `PriceData` | Lower bound of the estimated price range |
  | `priceEstimateMax` | `PriceData` | Upper bound of the estimated price range |
  | `url` | `string (uri)` | URL to the product on the shop's website |
  | `images` | `array of string (uri)` | List of image URLs |
  | `auctionStart` | `string (date-time, RFC3339)` | Auction start timestamp |
  | `auctionEnd` | `string (date-time, RFC3339)` | Auction end timestamp |
  | `originYear` | `OriginYearData` | Origin year information |
  | `authenticity` | `AuthenticityData` | Authenticity classification |
  | `condition` | `ConditionData` | Condition classification |
  | `provenance` | `ProvenanceData` | Provenance classification |
  | `restoration` | `RestorationData` | Restoration classification |

  Extended example:
  ```json
  {
    "shopsProductId": "baroque-violin-001",
    "priceEstimateMin": { "currency": "EUR", "amount": 3000 },
    "priceEstimateMax": { "currency": "EUR", "amount": 6000 },
    "url": "https://my-shop.com/products/baroque-violin-updated",
    "images": ["https://my-shop.com/images/violin-new.jpg"],
    "auctionStart": "2026-04-10T10:00:00Z",
    "auctionEnd": "2026-04-10T12:00:00Z",
    "originYear": { "year": 1740 },
    "authenticity": "ORIGINAL",
    "condition": "EXCELLENT",
    "provenance": "COMPLETE",
    "restoration": "MINOR"
  }
  ```

### Added

- **`ProductEventTypeData`** — 9 new enum values for the product event history:

  | Value | Description |
  |---|---|
  | `ESTIMATE_PRICE_CHANGED` | The estimated price range (min/max) was updated |
  | `URL_CHANGED` | The product URL was updated |
  | `IMAGES_CHANGED` | The product image list was updated |
  | `AUCTION_TIME_CHANGED` | The auction start or end time was updated |
  | `ORIGIN_YEAR_CHANGED` | The product origin year was updated |
  | `AUTHENTICITY_CHANGED` | The authenticity classification was updated |
  | `CONDITION_CHANGED` | The condition classification was updated |
  | `PROVENANCE_CHANGED` | The provenance classification was updated |
  | `RESTORATION_CHANGED` | The restoration classification was updated |

- **`ProductEventEstimatePriceChangedPayloadData`** (new schema) — Payload for `ESTIMATE_PRICE_CHANGED` events. Only the fields that changed are present.

  | Field | Type | Description |
  |---|---|---|
  | `priceEstimateMin` | `PriceData` | Updated lower bound of the estimated price range. Absent if not changed. |
  | `priceEstimateMax` | `PriceData` | Updated upper bound of the estimated price range. Absent if not changed. |

- **`ProductEventUrlChangedPayloadData`** (new schema) — Payload for `URL_CHANGED` events.

  | Field | Type | Description |
  |---|---|---|
  | `url` | `string (uri)` | The new product URL |

- **`ProductEventImagesChangedPayloadData`** (new schema) — Payload for `IMAGES_CHANGED` events.

  | Field | Type | Description |
  |---|---|---|
  | `images` | `array of ProductImageData` | The complete updated image list |

- **`ProductEventAuctionTimeChangedPayloadData`** (new schema) — Payload for `AUCTION_TIME_CHANGED` events. Only the fields that changed are present.

  | Field | Type | Description |
  |---|---|---|
  | `auctionStart` | `string (date-time, RFC3339)` | Updated auction start timestamp. Absent if not changed. |
  | `auctionEnd` | `string (date-time, RFC3339)` | Updated auction end timestamp. Absent if not changed. |

- **`ProductEventOriginYearChangedPayloadData`** (new schema) — Payload for `ORIGIN_YEAR_CHANGED` events.

  | Field | Type | Description |
  |---|---|---|
  | `originYear` | `OriginYearData` | The new origin year |

- **`ProductEventAuthenticityChangedPayloadData`** (new schema) — Payload for `AUTHENTICITY_CHANGED` events.

  | Field | Type | Description |
  |---|---|---|
  | `authenticity` | `AuthenticityData` | The new authenticity classification |

- **`ProductEventConditionChangedPayloadData`** (new schema) — Payload for `CONDITION_CHANGED` events.

  | Field | Type | Description |
  |---|---|---|
  | `condition` | `ConditionData` | The new condition classification |

- **`ProductEventProvenanceChangedPayloadData`** (new schema) — Payload for `PROVENANCE_CHANGED` events.

  | Field | Type | Description |
  |---|---|---|
  | `provenance` | `ProvenanceData` | The new provenance classification |

- **`ProductEventRestorationChangedPayloadData`** (new schema) — Payload for `RESTORATION_CHANGED` events.

  | Field | Type | Description |
  |---|---|---|
  | `restoration` | `RestorationData` | The new restoration classification |

---

## 2026-03-30 - Add Partner API: Batch Product Upsert (`backend#733`)

Partner shops can now upsert products programmatically via a single batch endpoint that intelligently handles both create and update in one call. The endpoint uses API key authentication, requires partner status, and returns HTTP 200 with a partial-failure map even if some upserts fail.

For each item in the request, the backend checks whether the product already exists:
- **New products** are created using all provided fields.
- **Existing products** have only their `state` and `price` updated; all other fields are ignored.

### Added

- **`PUT /api/v1/shops/{shopId}/products`** — New partner batch product-upsert endpoint.

  **Authentication**: `x-api-key` header (partner API key, no Cognito JWT required).

  **Path parameter**:
  | Parameter | Type | Description |
  |---|---|---|
  | `shopId` | `string (uuid)` | UUID of the partner shop |

  **Request body**: `application/json` — array of [`PutProductData`](#PutProductData) objects. Must not be empty.

  **Response `200`**: [`PutProductsResponse`](#PutProductsResponse) — map of `shopsProductId → errorKey` for products that failed to upsert. An empty `errors` map indicates full success.

  ```json
  { "errors": {} }
  ```

  **Error responses**:
  | Status | Error code | Condition |
  |---|---|---|
  | `400` | `BAD_BODY_VALUE` | Request body is absent or empty |
  | `400` | `INVALID_JSON` | Request body is not valid JSON |
  | `401` | `BAD_HEADER_VALUE` | `x-api-key` header is missing or malformed |
  | `401` | `PARTNER_SHOP_API_KEY_MISMATCH` | API key does not match the shop's stored key |
  | `403` | `PARTNER_SHOP_NOT_PARTNERED` | Shop exists but has not been granted partner status |
  | `404` | `SHOP_NOT_FOUND` | Shop with the given `shopId` does not exist |
  | `500` | `INTERNAL_SERVER_ERROR` | Unexpected server error |

- **`PutProductData`** (new schema) — Object describing a single product upsert via the partner endpoint. Only `shopsProductId` is required. All other fields are optional.

  | Field | Type | Required | Default | Description |
  |---|---|---|---|---|
  | `shopsProductId` | `string` | ✓ | — | Shop's own identifier for the product |
  | `title` | `LocalizedTextData` | — | — | Localized title. Used only on create. |
  | `description` | `LocalizedTextData` | — | — | Localized description. Used only on create. |
  | `price` | `PriceData` | — | — | Asking price. Applied on both create and update. |
  | `priceEstimateMin` | `PriceData` | — | — | Lower bound of estimated price range. Used only on create. |
  | `priceEstimateMax` | `PriceData` | — | — | Upper bound of estimated price range. Used only on create. |
  | `state` | `ProductStateData` | — | — | Product state. Applied on both create and update. |
  | `url` | `string (uri)` | — | — | URL to the product on the shop's website. Used only on create. |
  | `images` | `array of string (uri)` | — | — | Image URLs. Used only on create. |
  | `auctionStart` | `string (date-time, RFC3339)` | — | — | Auction start timestamp. Used only on create. |
  | `auctionEnd` | `string (date-time, RFC3339)` | — | — | Auction end timestamp. Used only on create. |
  | `originYear` | `OriginYearData` | — | — | Origin year information. Used only on create. |
  | `authenticity` | `AuthenticityData` | — | `UNKNOWN` | Authenticity classification. Used only on create. |
  | `condition` | `ConditionData` | — | `UNKNOWN` | Condition classification. Used only on create. |
  | `provenance` | `ProvenanceData` | — | `UNKNOWN` | Provenance classification. Used only on create. |
  | `restoration` | `RestorationData` | — | `UNKNOWN` | Restoration classification. Used only on create. |

  Minimal example (update state of existing product):
  ```json
  { "shopsProductId": "baroque-violin-001", "state": "SOLD" }
  ```

  Full example (create a new product):
  ```json
  {
    "shopsProductId": "baroque-violin-001",
    "title": { "text": "Baroque Violin", "language": "en" },
    "description": { "text": "A beautiful 18th-century baroque violin.", "language": "en" },
    "price": { "currency": "EUR", "amount": 4500 },
    "state": "AVAILABLE",
    "url": "https://my-shop.com/products/baroque-violin",
    "images": ["https://my-shop.com/images/violin-1.jpg"],
    "originYear": { "year": 1740 },
    "authenticity": "ORIGINAL",
    "condition": "EXCELLENT",
    "provenance": "COMPLETE",
    "restoration": "MINOR"
  }
  ```

- **`PutProductsResponse`** (new schema) — Response object for the batch-upsert endpoint.

  | Field | Type | Description |
  |---|---|---|
  | `errors` | `object (string → string)` | Map of `shopsProductId` to error key for products that failed to upsert. The only possible error value is `UPSERT_FAILED`. Empty on full success. |

---

## 2026-03-29 - Add Partner API: Batch Product Update (`backend#730`)

Partner shops can now update existing products programmatically via a dedicated batch endpoint that mirrors the existing batch-create endpoint. The endpoint uses API key authentication, requires partner status, and returns HTTP 200 with a partial-failure map even if some updates fail.

### Added

- **`PATCH /api/v1/shops/{shopId}/products`** — New partner batch product-update endpoint.

  **Authentication**: `x-api-key` header (partner API key, no Cognito JWT required).

  **Path parameter**:
  | Parameter | Type | Description |
  |---|---|---|
  | `shopId` | `string (uuid)` | UUID of the partner shop |

  **Request body**: `application/json` — array of [`PatchProductData`](#PatchProductData) objects. Must not be empty.

  **Response `200`**: [`PatchProductsResponse`](#PatchProductsResponse) — map of `shopsProductId → errorKey` for products that failed to update. An empty `errors` map indicates full success.

  ```json
  { "errors": {} }
  ```

  **Error responses**:
  | Status | Error code | Condition |
  |---|---|---|
  | `400` | `BAD_BODY_VALUE` | Request body is absent or empty |
  | `400` | `INVALID_JSON` | Request body is not valid JSON |
  | `401` | `BAD_HEADER_VALUE` | `x-api-key` header is missing or malformed |
  | `401` | `PARTNER_SHOP_API_KEY_MISMATCH` | API key does not match the shop's stored key |
  | `403` | `PARTNER_SHOP_NOT_PARTNERED` | Shop exists but has not been granted partner status |
  | `404` | `SHOP_NOT_FOUND` | Shop with the given `shopId` does not exist |
  | `500` | `INTERNAL_SERVER_ERROR` | Unexpected server error |

- **`PatchProductData`** (new schema) — Object describing a single product update via the partner endpoint. Only `shopsProductId` is required; all other fields are optional and leave the current value unchanged when omitted.

  | Field | Type | Required | Description |
  |---|---|---|---|
  | `shopsProductId` | `string` | ✓ | Shop's own identifier for the product to update |
  | `price` | `PriceData` | — | Optional updated asking price |
  | `state` | `ProductStateData` | — | Optional updated product state |

  Minimal example:
  ```json
  { "shopsProductId": "baroque-violin-001", "state": "SOLD" }
  ```

  Full example:
  ```json
  {
    "shopsProductId": "baroque-violin-001",
    "price": { "currency": "EUR", "amount": 5000 },
    "state": "AVAILABLE"
  }
  ```

- **`PatchProductsResponse`** (new schema) — Response object for the batch-update endpoint.

  | Field | Type | Description |
  |---|---|---|
  | `errors` | `object (string → string)` | Map of `shopsProductId` to error key for products that failed to update. The only possible error value is `UPDATE_FAILED`. Empty on full success. |

---

## 2026-03-29 - Add Partner API: Batch Product Creation (`backend#729`)

Partner shops can now create products programmatically via a dedicated batch endpoint that uses API key authentication instead of Cognito JWTs. A shop must have been granted partner status (with an API key configured) before it can use this endpoint. Products are processed individually — a partial failure does not prevent successful products from being created, and the response always returns HTTP 200 with a breakdown of any errors.

### Added

- **`POST /api/v1/shops/{shopId}/products`** — New partner batch product-creation endpoint.

  **Authentication**: `x-api-key` header (partner API key, no Cognito JWT required).

  **Path parameter**:
  | Parameter | Type | Description |
  |---|---|---|
  | `shopId` | `string (uuid)` | UUID of the partner shop |

  **Request body**: `application/json` — array of [`PostProductData`](#PostProductData) objects. Must not be empty.

  **Response `200`**: [`PostProductsResponse`](#PostProductsResponse) — map of `shopsProductId → errorKey` for products that failed to create. An empty `errors` map indicates full success.

  ```json
  { "errors": {} }
  ```

  **Error responses**:
  | Status | Error code | Condition |
  |---|---|---|
  | `400` | `BAD_BODY_VALUE` | Request body is absent or empty |
  | `400` | `INVALID_JSON` | Request body is not valid JSON |
  | `401` | `BAD_HEADER_VALUE` | `x-api-key` header is missing or malformed |
  | `401` | `PARTNER_SHOP_API_KEY_MISMATCH` | API key does not match the shop's stored key |
  | `403` | `PARTNER_SHOP_NOT_PARTNERED` | Shop exists but has not been granted partner status |
  | `404` | `SHOP_NOT_FOUND` | Shop with the given `shopId` does not exist |
  | `500` | `INTERNAL_SERVER_ERROR` | Unexpected server error |

- **`PostProductData`** (new schema) — Object describing a single product to create via the partner endpoint.

  | Field | Type | Required | Default | Description |
  |---|---|---|---|---|
  | `shopsProductId` | `string` | ✓ | — | Shop's own identifier for the product (unique within the shop) |
  | `title` | `LocalizedTextData` | ✓ | — | Localized product title |
  | `description` | `LocalizedTextData` | ✓ | — | Localized product description |
  | `price` | `PriceData` | — | absent | Optional asking price |
  | `priceEstimateMin` | `PriceData` | — | absent | Optional lower bound of estimated price range |
  | `priceEstimateMax` | `PriceData` | — | absent | Optional upper bound of estimated price range |
  | `state` | `ProductStateData` | ✓ | — | Current product state |
  | `url` | `string (uri)` | ✓ | — | URL to the product on the shop's website |
  | `images` | `string[] (uri)` | ✓ | — | List of image URLs (may be empty) |
  | `auctionStart` | `string (date-time)` | — | absent | RFC3339 auction start timestamp (auction houses only) |
  | `auctionEnd` | `string (date-time)` | — | absent | RFC3339 auction end timestamp (auction houses only) |
  | `originYear` | `OriginYearData` | — | absent | Origin year information for the antique |
  | `authenticity` | `AuthenticityData` | — | `UNKNOWN` | Authenticity classification |
  | `condition` | `ConditionData` | — | `UNKNOWN` | Condition classification |
  | `provenance` | `ProvenanceData` | — | `UNKNOWN` | Provenance classification |
  | `restoration` | `RestorationData` | — | `UNKNOWN` | Restoration classification |

  Minimal example:
  ```json
  {
    "shopsProductId": "baroque-violin-001",
    "title": { "text": "Baroque Violin", "language": "en" },
    "description": { "text": "18th-century baroque violin.", "language": "en" },
    "state": "AVAILABLE",
    "url": "https://my-shop.com/products/baroque-violin",
    "images": ["https://my-shop.com/images/violin-1.jpg"]
  }
  ```

- **`PostProductsResponse`** (new schema) — Response object for the batch-create endpoint.

  | Field | Type | Description |
  |---|---|---|
  | `errors` | `object (string → string)` | Map of `shopsProductId` to error key for products that failed to create. Empty on full success. |

- **`PARTNER_SHOP_NOT_PARTNERED`** (new error code) — Returned as `403 Forbidden` when the shop exists but has not been granted partner status.

- **`PARTNER_SHOP_API_KEY_MISMATCH`** (new error code) — Returned as `401 Unauthorized` when the provided API key does not match the shop's stored key.

- **`PartnerApiKeyAuth`** (new security scheme) — API key authentication via the `x-api-key` request header, used exclusively by the partner product-creation endpoint.

---

## 2026-03-28 - Introduce User Tiers, Limits & Quotas (`backend#719`)

User accounts now carry a `tier` field that governs the limits applied to a user's watchlist and search filters. The initial release introduces a single tier, `FREE`, which caps both watchlist entries and search filters at 5. Quota enforcement has been tightened: the `POST /api/v1/me/search-filters` endpoint now checks the user's quota before creating a new filter (and surfaces `USER_NOT_FOUND` when the user does not exist), and the `POST /api/v1/me/watchlist` endpoint now correctly returns `404` (instead of `500`) when the requesting user cannot be found.

### Added

- **`UserTierData`** (new schema) — String enum representing the user's subscription tier, which determines limits and quotas.

  | Value | Watchlist limit | Search filter limit |
  |---|---|---|
  | `FREE` | 5 | 5 |

- **`tier` field on `GET /api/v1/me/account` response** (`GetUserAccountData`) — The user's current tier is now always present in the account response.

  ```json
  {
    "userId": "550e8400-e29b-41d4-a716-446655440000",
    "email": "user@example.com",
    "prohibitedContentConsent": false,
    "tier": "FREE",
    "created": "2024-01-01T10:00:00Z",
    "updated": "2024-01-01T12:00:00Z"
  }
  ```

- **`tier` field on `PATCH /api/v1/me/account` response** (`GetUserAccountData`) — The updated account response now also includes the `tier` field.

- **`404 USER_NOT_FOUND`** on **`POST /api/v1/me/search-filters`** — Returned when the authenticated user does not exist in the system at the time of search filter creation (quota check reads user record first).

  ```json
  { "status": 404, "title": "Not Found", "error": "USER_NOT_FOUND" }
  ```

- **`422 SEARCH_FILTER_QUOTA_EXCEEDED`** on **`POST /api/v1/me/search-filters`** — Returned when the user has already reached the maximum number of search filters allowed by their tier.

  ```json
  {
    "status": 422,
    "title": "Unprocessable Content",
    "error": "SEARCH_FILTER_QUOTA_EXCEEDED",
    "detail": "Exceeded the maximum amount of search filters. There are already 5/5 search filters occupied."
  }
  ```

### Changed

- **`404 USER_NOT_FOUND`** on **`POST /api/v1/me/watchlist`** — The user-not-found case on watchlist creation is now returned as `404 Not Found` instead of the previous `500 Internal Server Error`.

  ```json
  { "status": 404, "title": "Not Found", "error": "USER_NOT_FOUND" }
  ```

---

## 2026-03-27 - Ditch Complex Shop-Domain Logic (`backend#714`)

The shop lookup-by-domain feature has been removed. Shops can no longer be retrieved by their associated domain name — only by shop ID (UUID) or slug. Alongside this, the error code for exceeding the domain limit has been replaced by a new error code for providing no domains at all.

### Removed

- **`GET /api/v1/by-domain/shops/{shopDomain}`** — Endpoint removed entirely. Shops can no longer be looked up by domain name. Use `GET /api/v1/shops/{shopId}` (by UUID) or `GET /api/v1/by-slug/shops/{shopSlugId}` (by slug) instead.

- **`SHOP_TOO_MANY_DOMAINS`** error code — Previously returned as `400 Bad Request` when a shop creation request supplied more than 100 domains. This validation and its error code no longer exist.

### Added

- **`NO_DOMAIN`** error code — Returned as `400 Bad Request` when a shop creation or update request provides an empty domains list. A shop must have at least one domain.

---

## 2026-03-26 - Unify Product Domain Events: Created, StateChanged, PriceChanged (`backend#707`)

The product event type system has been unified. The previous 11 brittle variants encoding state/price-change semantics in the event type name have been collapsed into 3 generalized event types. This affects the `GET /api/v1/products/{productId}/events` endpoint and any consumer of `GetProductEventData`.

### Changed

- **`ProductEventTypeData`** — Enum reduced from 11 values to 3:

  | Old values (removed) | New value |
  |---|---|
  | `STATE_LISTED`, `STATE_AVAILABLE`, `STATE_RESERVED`, `STATE_SOLD`, `STATE_REMOVED`, `STATE_UNKNOWN` | `STATE_CHANGED` |
  | `PRICE_DISCOVERED`, `PRICE_DROPPED`, `PRICE_INCREASED`, `PRICE_REMOVED` | `PRICE_CHANGED` |

  The 3 remaining values are: `CREATED`, `STATE_CHANGED`, `PRICE_CHANGED`.

- **`ProductEventPriceChangedPayloadData`** — Both `oldPrice` and `newPrice` are now **optional** (nullable). The combination of presence/absence encodes the price change semantic:

  | `oldPrice` | `newPrice` | Meaning |
  |---|---|---|
  | absent | present | Price first discovered (initial detection) |
  | present | present | Price changed (dropped or increased) |
  | present | absent | Price removed |

### Removed schemas

- **`ProductEventPriceDiscoveredPayloadData`** — Replaced by `ProductEventPriceChangedPayloadData` with `oldPrice: null`.
- **`ProductEventPriceRemovedPayloadData`** — Replaced by `ProductEventPriceChangedPayloadData` with `newPrice: null`.

---

## 2026-03-26 - Remove Unused Write Endpoints for Products and Shops (`backend#705`)

Removes four write endpoints that are no longer needed in production:
`PUT /api/v1/products`, `POST /api/v1/shops`, `PATCH /api/v1/shops/{shopId}`, and `PATCH /api/v1/by-domain/shops/{shopDomain}`.

### Removed

- **`PUT /api/v1/products`** — Bulk create or update products endpoint has been removed. This endpoint accepted a collection of product data and processed them asynchronously with automatic shop enrichment based on the product URL's domain.

- **`POST /api/v1/shops`** — Create a new shop endpoint has been removed. This endpoint created a new shop with name, type, domains, and optional image.

- **`PATCH /api/v1/shops/{shopId}`** — Update shop details by ID endpoint has been removed. This endpoint allowed partial updates (type, domains, image) to an existing shop by its UUID.

- **`PATCH /api/v1/by-domain/shops/{shopDomain}`** — Update shop details by domain endpoint has been removed. This endpoint allowed partial updates (type, domains, image) to an existing shop by one of its domains.

### Removed schemas

The following request/response schemas have been removed as they were exclusively used by the deleted endpoints:

- **`PutProductsCollectionData`** — Request body for `PUT /api/v1/products`. Wrapped an array of `PutProductData` items.
- **`PutProductData`** — Individual product entry for bulk upsert requests.
- **`PutProductsResponse`** — Response from bulk product upsert with `skipped`, `unprocessed`, and `failed` fields.
- **`PutProductError`** — Enum of error codes for failed product processing: `SHOP_NOT_FOUND`, `MONETARY_AMOUNT_OVERFLOW`, `PRODUCT_ENRICHMENT_FAILED`, `NO_DOMAIN`.
- **`PostShopData`** — Request body for `POST /api/v1/shops`.
- **`PatchShopData`** — Request body for `PATCH /api/v1/shops/{shopId}` and `PATCH /api/v1/by-domain/shops/{shopDomain}`.

---

## 2026-03-24 - User Search Filter Match Products Endpoint (`backend#680`)

Adds a new endpoint `GET /api/v1/me/search-filters/{userSearchFilterId}/products` that returns the actual products matched by a user's saved search filter. Results are localized, personalized, and paginated using the same search-after cursor pattern as `GET /api/v1/me/watchlist`.

### Added

- **`GET /api/v1/me/search-filters/{userSearchFilterId}/products`** — Returns a paginated, personalized list of products that have been matched by the given search filter.

  **Path parameters:**

  | Parameter | Type | Required | Description |
  |---|---|---|---|
  | `userSearchFilterId` | `string (uuid)` | Yes | Unique identifier of the search filter |

  **Query parameters:**

  | Parameter | Type | Required | Description |
  |---|---|---|---|
  | `language` | `LanguageData` | No | Preferred language for localized content. Defaults to `en`. |
  | `currency` | `CurrencyData` | No | Currency for price display. |
  | `sort` | `SortSearchFilterMatchFieldData` | No | Field to sort results by. Defaults to `created`. |
  | `order` | `string` (`asc` \| `desc`) | No | Sort order. Only valid when `sort` is specified. |
  | `searchAfter` | `string (date-time)` | No | RFC3339 timestamp cursor for the next page. For `asc` order, returns matches created after this timestamp; for `desc` order, returns matches created before it. |
  | `size` | `integer` (1–100) | No | Number of products per page. Defaults to 21. |

  **Responses:**

  | Status | Description |
  |---|---|
  | `200` | Matched products retrieved successfully. Body: `SearchFilterMatchProductCollectionData`. Response always includes `Cache-Control: no-store`. |
  | `400` | Bad request — invalid path or query parameter (invalid UUID, unsupported sort field, invalid RFC3339 timestamp, etc.). |
  | `401` | Unauthorized — missing or invalid JWT token. |
  | `404` | Search filter not found. |
  | `500` | Internal server error. |

  **200 Response body (`SearchFilterMatchProductCollectionData`):**

  | Property | Type | Required | Description |
  |---|---|---|---|
  | `items` | `PersonalizedGetProductData[]` | Yes | Personalized matched products for the current page |
  | `size` | `integer` | Yes | Number of products in the current page |
  | `searchAfter` | `string (date-time)` | No | RFC3339 cursor for the next page. Omitted when there are no further results. |
  | `total` | `integer` | No | Total number of matched products |

### New schemas

- **`SortSearchFilterMatchFieldData`** — Enum of fields available for sorting search filter matched products.
  - `created`: Sort by when the product was matched by the search filter (the only supported value).

- **`SearchFilterMatchProductCollectionData`** — Paginated collection of personalized products matched by a search filter.

  | Property | Type | Required | Description |
  |---|---|---|---|
  | `items` | `PersonalizedGetProductData[]` | Yes | Personalized matched products for the current page |
  | `size` | `integer` | Yes | Number of products in the current page |
  | `searchAfter` | `string (date-time)` | No | RFC3339 cursor for the next page. Omitted when there are no further results. |
  | `total` | `integer` | No | Total number of matched products |

---

## 2026-03-24 - Watchlist Endpoints Return Personalized Product Data (`backend#678`)

All three mutable watchlist endpoints (GET, POST, PATCH) now return the same `PersonalizedGetProductData` response shape used by every other product endpoint. The previous bespoke watchlist response types (`WatchlistProductData` / `WatchlistProductPatchResponse`) have been removed. Watchlist entry timestamps (`created` / `updated`) are intentionally no longer included in any watchlist response.

### Changed

- **`GET /api/v1/me/watchlist`** — Response items change from `WatchlistProductData` to `PersonalizedGetProductData`.
  - Previously each item contained `product` (full product data), `notifications` (bool), `created` (RFC3339), and `updated` (RFC3339).
  - Now each item is a `PersonalizedGetProductData` with an `item` (full `GetProductData`) and an optional `userState` (`ProductUserStateData`). Watchlist entry timestamps are no longer present.

  Updated `WatchlistCollectionData` schema:

  | Property | Type | Required | Description |
  |---|---|---|---|
  | `items` | `PersonalizedGetProductData[]` | Yes | Array of personalized watchlist products in the current page |
  | `size` | `integer` | Yes | Number of products in the current page |
  | `searchAfter` | `string (date-time)` | No | Cursor for the next page. Present when more results exist. |
  | `total` | `integer` | No | Total number of watchlist products |

- **`POST /api/v1/me/watchlist`** — Two new optional query parameters added; response type changed.
  - New query parameters `language` and `currency` control localization and currency of the returned product data.
  - Response body changes from `WatchlistProductPatchResponse` to `PersonalizedGetProductData`. Watchlist entry timestamps are no longer present.

  New query parameters:

  | Parameter | Type | Required | Description |
  |---|---|---|---|
  | `language` | `LanguageData` | No | Preferred language for localized content. Defaults to `en`. |
  | `currency` | `CurrencyData` | No | Currency for price display. |

- **`PATCH /api/v1/me/watchlist/{shopId}/{shopsProductId}`** — Two new optional query parameters added; response type and headers changed.
  - New query parameters `language` and `currency` control localization and currency of the returned product data.
  - The `Last-Modified` response header is no longer present.
  - Response body changes from `WatchlistProductPatchResponse` to `PersonalizedGetProductData`. Watchlist entry timestamps are no longer present.

  New query parameters:

  | Parameter | Type | Required | Description |
  |---|---|---|---|
  | `language` | `LanguageData` | No | Preferred language for localized content. Defaults to `en`. |
  | `currency` | `CurrencyData` | No | Currency for price display. |

### Removed

- **`WatchlistProductData`** schema — replaced by `PersonalizedGetProductData`.
- **`WatchlistProductPatchResponse`** schema — replaced by `PersonalizedGetProductData`.

---

## 2026-03-19 - Notification User State `originEventId` (`backend#647`)

This update extends `NotificationUserStateData` with a reference to the event that triggered the latest notification for a product. Clients can use this ID to directly patch a specific notification (e.g. mark it as seen) without an additional lookup.

### Changed

- **`NotificationUserStateData`** — New optional field **`originEventId`** added:

  | Property | Type | Required | Description |
  |---|---|---|---|
  | `seen` | `boolean` | Yes | Unchanged. `false` if the latest notification is unseen; `true` otherwise. Defaults to `true` when no notifications exist. |
  | `originEventId` | `string (uuid)` | **No (new)** | ID of the domain event that triggered the latest notification for this product. Present whenever at least one notification exists (seen or unseen); **absent** (field omitted entirely) when there are no notifications for this product. |

  **Presence rules for `originEventId`:**
  - **Present** — one or more notifications exist for this user/product (regardless of whether `seen` is `true` or `false`).
  - **Absent** — no notifications exist for this user/product (`seen` will be `true` in this case).

  Affected endpoints (all endpoints that return `userState.notification`):
  - `GET /api/v1/shops/{shopId}/products/{productId}`
  - `GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}`
  - `GET /api/v1/shops/{shopId}/products/{productId}/similar`
  - `POST /api/v1/products/search`
  - `GET /api/v1/products`

---

## 2026-03-18 - Personalized Product Notification State (`backend#638`)

This update extends `ProductUserStateData` with notification awareness. When the authenticated user has an unseen notification for a product, the product response now includes `notification.seen: false` in the `userState`, enabling the frontend to surface an unread indicator without an additional round-trip.

### Changed

- **`GET /api/v1/shops/{shopId}/products/{productId}`** — `userState` (when present) is extended with a new required field **`notification`**.
- **`GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}`** — same `userState` extension.
- **`GET /api/v1/shops/{shopId}/products/{productId}/similar`** — `userState` (when present) is extended with **`notification`**.
- **`POST /api/v1/products/search`** and **`GET /api/v1/products`** — `userState` (when present) is extended with **`notification`**.

- **`ProductUserStateData`** — New required field added:

  | Property | Type | Required | Description |
  |---|---|---|---|
  | `watchlist` | `WatchlistUserStateData` | Yes | Unchanged. |
  | `prohibitedContent` | `ProhibitedContentUserStateData` | Yes | Unchanged. |
  | `notification` | `NotificationUserStateData` | **Yes (new)** | Notification seen-state for this product. |

### New schemas

- **`NotificationUserStateData`** — Indicates whether the authenticated user has unseen notifications for a product. Derived from the latest notification (by creation time) associated with the product.

  | Property | Type | Required | Description |
  |---|---|---|---|
  | `seen` | `boolean` | Yes | `false` if the latest notification for this product is unseen; `true` otherwise. Defaults to `true` when the user has no notifications for this product. |

---

## 2026-03-17 - Search Filter Match Notifications (`backend#630`)

This update introduces a new notification type for search filter matches. When a product event (price change, state change, or enrichment) results in a product matching a user's saved search filter, a `SEARCH_FILTER` notification is now created and optionally emailed to the user. Additionally, all search filter objects now expose a `notifications` flag that users can toggle to control whether they receive such notifications.

### Added

- **`SEARCH_FILTER` notification type** — New variant in `NotificationPayloadData`.
  - When a product matches a user's saved search filter, a notification with `type: "SEARCH_FILTER"` is created.
  - Notifications are sent externally (via email) only if the matched search filter has `notifications: true`.

### Changed

- **`UserSearchFilterData`** (returned by `GET /api/v1/me/search-filters`, `GET /api/v1/me/search-filters/{userSearchFilterId}`, `POST /api/v1/me/search-filters`, `PATCH /api/v1/me/search-filters/{userSearchFilterId}`)
  - Added required boolean field **`notifications`**: whether notifications are enabled for this search filter.
  - Defaults to `true` for all newly created filters. Existing filters without an explicit stored value also default to `true`.

  | Property | Type | Required | Description |
  |---|---|---|---|
  | `notifications` | `boolean` | **Yes (new)** | `true` if the user receives notifications when products match this filter; `false` to suppress all match notifications. |

- **`PatchUserSearchFilterData`** (request body for `PATCH /api/v1/me/search-filters/{userSearchFilterId}`)
  - Added optional boolean field **`notifications`**: set to `true` or `false` to enable or disable match notifications for this filter. Omit to leave unchanged.

  | Property | Type | Required | Description |
  |---|---|---|---|
  | `notifications` | `boolean \| null` | No | Toggle notifications for this search filter. `true` enables, `false` disables. Omit to leave unchanged. |

### New schemas

- **`SearchFilterNotificationPayloadData`** — Payload for a search filter match notification (`type: "SEARCH_FILTER"`).

  | Property | Type | Required | Description |
  |---|---|---|---|
  | `type` | `"SEARCH_FILTER"` | Yes | Discriminator. |
  | `productId` | `string (uuid)` | Yes | Internal product identifier. |
  | `shopId` | `string (uuid)` | Yes | Shop identifier. |
  | `shopsProductId` | `string` | Yes | Shop's own identifier for the product. |
  | `shopSlugId` | `string` | Yes | URL-friendly slug for the shop. |
  | `productSlugId` | `string` | Yes | URL-friendly slug for the product (6-character suffix). |
  | `shopName` | `string` | Yes | Display name of the shop. |
  | `title` | `LocalizedTextData` | Yes | Localized product title. |
  | `searchFilterPayload` | `SearchFilterPayloadData` | Yes | Sub-payload identifying the matched search filter. |

- **`SearchFilterPayloadData`** — Sub-payload for search filter match notifications.

  | Property | Type | Required | Description |
  |---|---|---|---|
  | `userSearchFilterId` | `string (uuid)` | Yes | Unique identifier of the saved search filter that matched the product. |
  | `userSearchFilterName` | `string` | Yes | User-defined name of the matched search filter. |

- **`NotificationPayloadData`** — Extended discriminated union; new variant added:
  - `SEARCH_FILTER` → `SearchFilterNotificationPayloadData`

---



This update personalizes product image responses based on the authenticated user's prohibited-content consent flag. Image URLs for images that carry prohibited content are now withheld unless the user has explicitly consented to view them. The user's consent state is also surfaced in the `userState` response so the frontend can reflect the current consent setting without an extra round-trip.

### Changed

- **`GET /api/v1/shops/{shopId}/products/{productId}`** (`PersonalizedGetProductData` response)
  - `item.images[].url` is now **optional** (`string | absent`). The field is omitted for any image whose `prohibitedContent != "NONE"` when the authenticated user has not consented, and always omitted for unauthenticated users for such images.
  - `userState` (when present): extended with a new required field `prohibitedContent` — see **`ProductUserStateData`** changes below.

- **`GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}`** (`PersonalizedGetProductData` response)
  - Same image URL behaviour and `userState` extension as the endpoint above.

- **`GET /api/v1/shops/{shopId}/products/{productId}/similar`** (`PersonalizedGetProductSummaryData[]` response)
  - `item.images[].url` is now **optional** (`string | absent`) following the same consent-based logic.
  - `userState` (when present): extended with `prohibitedContent`.

- **`POST /api/v1/products/search`** and **`GET /api/v1/products`** (`CursoredResult<PersonalizedGetProductSummaryData>` response)
  - `item.images[].url` is now **optional** following the same consent-based logic.
  - `userState` (when present): extended with `prohibitedContent`.

- **`GET /api/v1/me/watchlist`** (`PersonalizedGetProductSummaryData[]` response)
  - `item.images[].url` is now **optional** following the same consent-based logic.

### Changed schemas

- **`ProductImageData`**
  | Property | Type | Required | Description |
  |---|---|---|---|
  | `url` | `string (uri)` | **No** (was Yes) | URL to the product image. Omitted when `prohibitedContent != "NONE"` and the user has not consented (or the request is unauthenticated). |
  | `prohibitedContent` | `ProhibitedContentData` | Yes | Unchanged. |

- **`ProductUserStateData`**
  | Property | Type | Required | Description |
  |---|---|---|---|
  | `watchlist` | `WatchlistUserStateData` | Yes | Unchanged. |
  | `prohibitedContent` | `ProhibitedContentUserStateData` | **Yes (new)** | User's prohibited-content consent state. |

### New schemas

- **`ProhibitedContentUserStateData`** – Reflects the authenticated user's consent for viewing prohibited content.
  | Property | Type | Required | Description |
  |---|---|---|---|
  | `consent` | `boolean` | Yes | `true` if the user has consented to see prohibited-content images; `false` otherwise. When `false`, image URLs for non-safe images are omitted from the response. |

---

## 2026-03-14 - User Notifications REST API (`backend#622`)

This update exposes user notifications via a new REST API. Users can list, update (mark as seen), and delete their notifications — individually or in bulk.

### Added

- **`GET /api/v1/me/notifications`** – Retrieve the authenticated user's notifications.
  - Sorted latest-first.
  - Cursor-based pagination using an event ID (`searchAfter` / `size` query parameters).
  - Query parameters:
    | Parameter | Type | Required | Description |
    |---|---|---|---|
    | `language` | `LanguageData` | No | Language for localized content. Defaults to `en`. |
    | `currency` | `CurrencyData` | No | Currency for price display in payloads. Defaults to `EUR`. |
    | `searchAfter` | `string (uuid)` | No | Event ID cursor from the previous response for pagination. |
    | `size` | `integer` | No | Page size; capped at 100. |
  - Response: `NotificationCollectionData` (200); `Cache-Control: no-store` response header always set.
  - Error codes: `BAD_QUERY_PARAMETER_VALUE` (400), `INVALID_UUID` (400), `UNAUTHORIZED` (401).

- **`PATCH /api/v1/me/notifications/{eventId}`** – Update a single notification.
  - Path parameter `eventId` (UUID): identifies the notification.
  - Query parameters: `language` (optional), `currency` (optional) – control localization of the response.
  - Request body: **required** `PatchNotificationData` JSON object.
  - Response: updated `GetNotificationData` (200).
  - Error codes: `BAD_PATH_PARAMETER_VALUE` (400), `INVALID_UUID` (400), `BAD_BODY_VALUE` (400), `UNAUTHORIZED` (401), `NOTIFICATION_NOT_FOUND` (404).

- **`PATCH /api/v1/me/notifications`** – Update all notifications at once.
  - Query parameters: `language` (optional), `currency` (optional) – control localization of the response.
  - Request body: **optional** `PatchNotificationData` JSON object. If the body is omitted or empty the patch is a no-op.
  - Response: `NotificationCollectionData` (200) — first page of the updated notifications; `Cache-Control: no-store` response header always set.
  - Error codes: `BAD_BODY_VALUE` (400), `UNAUTHORIZED` (401).

- **`DELETE /api/v1/me/notifications/{eventId}`** – Delete a single notification.
  - Path parameter `eventId` (UUID): identifies the notification.
  - Response: 204 No Content on success.
  - Error codes: `BAD_PATH_PARAMETER_VALUE` (400), `INVALID_UUID` (400), `UNAUTHORIZED` (401), `NOTIFICATION_NOT_FOUND` (404).

- **`DELETE /api/v1/me/notifications`** – Delete all notifications for the authenticated user.
  - No request body or query parameters.
  - Response: 204 No Content on success.
  - Error codes: `UNAUTHORIZED` (401).

- **New schemas**:

  - **`GetNotificationData`** – Full localized notification response object.
    | Property | Type | Required | Description |
    |---|---|---|---|
    | `originEventId` | `string (uuid)` | Yes | ID of the domain event that triggered the notification. |
    | `notificationId` | `string (uuid)` | Yes | Unique identifier of the notification record. |
    | `payload` | `NotificationPayloadData` | Yes | Notification-type-specific payload. |
    | `seen` | `boolean` | Yes | Whether the user has seen this notification. |
    | `external` | `boolean` | Yes | Whether the notification has been sent externally to the user via a third-party medium (e.g. mail, SMS). `true` means it was sent externally; `false` means it was not. |
    | `created` | `string (date-time)` | Yes | Creation timestamp (RFC3339). |
    | `updated` | `string (date-time)` | Yes | Last-updated timestamp (RFC3339). |

  - **`NotificationPayloadData`** – Discriminated union keyed on `type`. Current variants:
    - `WATCHLIST` → `WatchlistNotificationPayloadData`

  - **`WatchlistNotificationPayloadData`** – Payload for watchlist-triggered notifications.
    | Property | Type | Required | Description |
    |---|---|---|---|
    | `type` | `"WATCHLIST"` | Yes | Discriminator. |
    | `productId` | `string (uuid)` | Yes | Internal product identifier. |
    | `shopId` | `string (uuid)` | Yes | Shop identifier. |
    | `shopsProductId` | `string` | Yes | Shop's own identifier for the product. |
    | `shopSlugId` | `string` | Yes | URL-friendly slug for the shop. |
    | `productSlugId` | `string` | Yes | URL-friendly slug for the product (6-character suffix). |
    | `shopName` | `string` | Yes | Display name of the shop. |
    | `title` | `LocalizedTextData` | Yes | Localized product title. |
    | `watchlistPayload` | `WatchlistPayloadData` | Yes | Watchlist-specific event payload. |

  - **`WatchlistPayloadData`** – Discriminated union keyed on `type`. Current variants:
    - `PRICE_CHANGE` → `PriceChangeWatchlistPayloadData`
    - `STATE_CHANGE` → `StateChangeWatchlistPayloadData`

  - **`PriceChangeWatchlistPayloadData`** – Payload for price-change watchlist notifications.
    | Property | Type | Required | Description |
    |---|---|---|---|
    | `type` | `"PRICE_CHANGE"` | Yes | Discriminator. |
    | `oldPrice` | `PriceData \| null` | No | Previous price in the requested currency. Absent when not available. |
    | `newPrice` | `PriceData \| null` | No | New price in the requested currency. Absent when not available. |

  - **`StateChangeWatchlistPayloadData`** – Payload for product state-change watchlist notifications.
    | Property | Type | Required | Description |
    |---|---|---|---|
    | `type` | `"STATE_CHANGE"` | Yes | Discriminator. |
    | `oldState` | `ProductStateData` | Yes | Previous product state. |
    | `newState` | `ProductStateData` | Yes | New product state. |

  - **`PatchNotificationData`** – Request body for PATCH operations.
    | Property | Type | Required | Description |
    |---|---|---|---|
    | `seen` | `boolean \| null` | No | Set to `true`/`false` to mark the notification seen/unseen. Omit to leave unchanged. |

  - **`NotificationCollectionData`** – Paginated notification list response.
    | Property | Type | Required | Description |
    |---|---|---|---|
    | `items` | `GetNotificationData[]` | Yes | Notifications in the current page. |
    | `size` | `integer` | Yes | Number of items in the current page. |
    | `searchAfter` | `string (uuid) \| null` | No | Cursor for the next page. Present only when more results are available. |
    | `total` | `integer \| null` | No | Total notification count for the user. May be absent. |

- **New error code**:
  - `NOTIFICATION_NOT_FOUND` (404) – No notification with the given event ID exists for this user.

---

## 2026-03-11 - User Account Prohibited Content Consent Flag (`backend#606`)

This update extends the user account API contract with a persisted consent flag for prohibited content display behavior.

### Changed

- **`GET /api/v1/me/account`** response schema **`GetUserAccountData`**
  - Added required boolean property **`prohibitedContentConsent`**
  - Existing users without an explicit stored value are represented with `false` by default
- **`PATCH /api/v1/me/account`** request schema **`PatchUserAccountData`**
  - Added optional boolean property **`prohibitedContentConsent`**
  - When provided, updates the authenticated user's consent state
- **`PATCH /api/v1/me/account`** and **`GET /api/v1/me/account`** response examples
  - Updated examples to include `prohibitedContentConsent`

---

## 2026-03-11 - Language Input Moved to Query Parameter for Slug-Based Product Endpoint

This update aligns the slug-based product read endpoint with the rest of the API by replacing the `Accept-Language` request header with the `language` query parameter.

### Changed

- **`GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}`** – Language input transport changed from header to query parameter:
  - **Removed**: `Accept-Language` request header
  - **Added**: `language` query parameter (`LanguageData`, optional, defaults to `en`)

---

## 2026-03-10 - Simple Product Search – Formal Query Parameter Definitions (`GET /api/v1/products`)

This update formally documents all optional filter query parameters for the simple product search endpoint `GET /api/v1/products`. The parameters were already accepted by the backend (via `ProductSearchData`) but were not formally typed in the OpenAPI spec.

### Changed

- **`GET /api/v1/products`** – Added formal query parameter definitions for all optional search filters:

  | Parameter | Type | Description |
  |---|---|---|
  | `categoryId` | `string[]` | Filter by category identifiers (kebab-case, e.g. `antique-furniture`). Repeated parameter: `categoryId=a&categoryId=b`. |
  | `periodId` | `string[]` | Filter by period identifiers (kebab-case, e.g. `baroque`). Repeated parameter. |
  | `shopName` | `string[]` | Filter products to those from exact-matching shop names. Repeated parameter. |
  | `excludeShopName` | `string[]` | Exclude products from exact-matching shop names. Repeated parameter. |
  | `shopType` | `ShopTypeData[]` | Filter by shop type (`AUCTION_HOUSE`, `AUCTION_PLATFORM`, `COMMERCIAL_DEALER`, `MARKETPLACE`). Repeated parameter. |
  | `price[min]` / `price[max]` | `integer` | Price range filter in minor currency units (e.g. cents). Both bounds are optional and inclusive. |
  | `state` | `ProductStateData[]` | Filter by product state (`LISTED`, `AVAILABLE`, `RESERVED`, `SOLD`, `REMOVED`, `UNKNOWN`). Repeated parameter. |
  | `originYear[min]` / `originYear[max]` | `integer` | Origin year range filter. Both bounds are optional and inclusive. |
  | `authenticity` | `AuthenticityData[]` | Filter by authenticity (`ORIGINAL`, `LATER_COPY`, `REPRODUCTION`, `QUESTIONABLE`, `UNKNOWN`). Repeated parameter. |
  | `condition` | `ConditionData[]` | Filter by condition (`EXCELLENT`, `GREAT`, `GOOD`, `FAIR`, `POOR`, `UNKNOWN`). Repeated parameter. |
  | `provenance` | `ProvenanceData[]` | Filter by provenance (`COMPLETE`, `PARTIAL`, `CLAIMED`, `NONE`, `UNKNOWN`). Repeated parameter. |
  | `restoration` | `RestorationData[]` | Filter by restoration level (`NONE`, `MINOR`, `MAJOR`, `UNKNOWN`). Repeated parameter. |
  | `created[min]` / `created[max]` | `string (date-time)` | Filter by product creation datetime (RFC3339). Both bounds are optional and inclusive. |
  | `updated[min]` / `updated[max]` | `string (date-time)` | Filter by last-updated datetime (RFC3339). Both bounds are optional and inclusive. |
  | `auctionStart[min]` / `auctionStart[max]` | `string (date-time)` | Filter by auction start datetime (RFC3339). Only matches products with an auction start time set. |
  | `auctionEnd[min]` / `auctionEnd[max]` | `string (date-time)` | Filter by auction end datetime (RFC3339). Only matches products with an auction end time set. |

  All previously documented parameters (`language`, `currency`, `productQuery`, `sort`, `order`, `searchAfter`, `size`) remain unchanged.

---

## 2026-03-07 - Product Search Query Minimum-Length Removal (`backend#592`)

This update removes the former minimum length constraint of 3 characters for `productQuery` and aligns request/response contracts with the backend change where `productQuery` is optional and, when provided, accepts 1+ characters.

### Changed

- **`ProductSearchData.productQuery`**
  - Changed from required `string` (min length 3) to optional `string | null` (min length 1 when provided)
  - Applies to request/response payloads and query-based simple product search
- **`PatchProductSearchData.productQuery`**
  - Changed minimum length from 3 to 1 when field is provided
- **Validation/error examples**
  - Removed outdated `productQuery must be at least 3 characters long` examples from affected 400-response documentation

**Affected Endpoints**:
- **GET `/api/v1/products`**
  - Query parameter `productQuery` is now optional and uses min length 1 when provided
- **POST `/api/v1/products/search`**
  - Request body `productQuery` is now optional and uses min length 1 when provided
  - 400 examples no longer include the removed min-length-3 validation message
- **POST `/api/v1/me/search-filters`** (requires authentication)
  - Search payload `productQuery` now uses min length 1 when provided
  - 400 examples no longer include the removed min-length-3 validation message
- **PATCH `/api/v1/me/search-filters/{userSearchFilterId}`** (requires authentication)
  - Search payload `productQuery` now uses min length 1 when provided
  - 400 examples no longer include the removed min-length-3 validation message
- **GET `/api/v1/me/search-filters`** and **GET `/api/v1/me/search-filters/{userSearchFilterId}`** (requires authentication)
  - Returned search payloads now document `productQuery` as optional

## 2026-02-27 - Italian Language Support (`it`)

This update extends API language support by adding Italian as a first-class supported language across language-constrained request and response fields.

### Changed

- **`LanguageData`**
  - Added new allowed value: `it`
  - Updated supported regional variants to include Italian: `it-IT`, `it-CH`
  - Default remains `en`
- **Header/query language inputs and localized response language outputs**
  - Endpoints that accept/return `LanguageData` now support `it` in addition to `de|en|fr|es`
  - Affected read endpoints include:
    - **GET `/api/v1/shops/{shopId}/products/{shopsProductId}`**
    - **GET `/api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}`**
    - **GET `/api/v1/shops/{shopId}/products/{shopsProductId}/history`**
    - **GET `/api/v1/shops/{shopId}/products/{shopsProductId}/similar`**
    - **GET `/api/v1/products`**
    - **GET `/api/v1/me/watchlist`** (requires authentication)
    - **GET `/api/v1/categories`**
    - **GET `/api/v1/categories/{categoryId}`**
    - **GET `/api/v1/periods`**
    - **GET `/api/v1/periods/{periodId}`**
- **Request/response bodies containing language fields typed as `LanguageData`**
  - Contracts now allow/return `it` anywhere `LanguageData` is used (for example in localized text/search-filter related payloads)
  - Common affected endpoints include:
    - **POST `/api/v1/products/search`**
    - **POST `/api/v1/me/search-filters`** (requires authentication)
    - **PATCH `/api/v1/me/search-filters/{userSearchFilterId}`** (requires authentication)
    - **GET `/api/v1/me/search-filters`** (requires authentication)
    - **GET `/api/v1/me/search-filters/{userSearchFilterId}`** (requires authentication)

## 2026-02-24 - Language Input Moved to Query Parameter + Search Body Defaults

This update aligns multiple read endpoints to use the `language` query parameter instead of the `Accept-Language` request header, and makes language/currency fields optional in selected search request bodies through server-side defaults.

### Changed

- **Localization input transport (header → query parameter)**:
  - The following endpoints now read localization from query parameter **`language`** (`LanguageData`, optional, default `en`) instead of header **`Accept-Language`**:
    - **GET `/api/v1/shops/{shopId}/products/{shopsProductId}`**
    - **GET `/api/v1/shops/{shopId}/products/{shopsProductId}/history`**
    - **GET `/api/v1/shops/{shopId}/products/{shopsProductId}/similar`**
    - **GET `/api/v1/me/watchlist`** (requires authentication)
    - **GET `/api/v1/categories`**
    - **GET `/api/v1/categories/{categoryId}`**
    - **GET `/api/v1/periods`**
    - **GET `/api/v1/periods/{periodId}`**
- **Search body defaults**:
  - **`ProductSearchData`**
    - `language` is now optional (defaults to `en`)
    - `currency` is now optional (defaults to `EUR`)
    - `productQuery` remains required
  - **`CategorySearchData`**
    - `language` is now optional (defaults to `en`)
  - **`PeriodSearchData`**
    - `language` is now optional (defaults to `en`)
- **Affected request-body endpoints due updated search schemas**:
  - **POST `/api/v1/products/search`**
  - **POST `/api/v1/me/search-filters`** (requires authentication)
  - **PATCH `/api/v1/me/search-filters/{userSearchFilterId}`** (requires authentication)
  - **POST `/api/v1/categories/search`**
  - **POST `/api/v1/periods/search`**

## 2026-02-23 - Search Filter PATCH Any-Of Category & Period Fields

This update aligns the PATCH search-filter contract with the existing multi-value (any-of) search semantics for category and period filters.

### Changed

- **`PatchProductSearchData.categoryId`**
  - Changed from `string | null` to `string[] | null` (kebab-case category IDs, unique values)
  - PATCH updates now accept any-of category filters
- **`PatchProductSearchData.periodId`**
  - Changed from `string | null` to `string[] | null` (kebab-case period IDs, unique values)
  - PATCH updates now accept any-of period filters

**Affected Endpoint**:
- **PATCH `/api/v1/me/search-filters/{userSearchFilterId}`** (requires authentication)
  - Request body field `productSearch.categoryId` now expects an array when provided
  - Request body field `productSearch.periodId` now expects an array when provided

## 2026-02-23 - Product Search Multi-Value Category & Period Filters

This update changes product-search category and period filters from single-value fields to multi-value fields.

### Changed

- **`ProductSearchData.categoryId`**
  - Changed from `string | null` to `string[]` (kebab-case category IDs, unique values)
  - Products are now matched when they belong to **any** of the provided categories
- **`ProductSearchData.periodId`**
  - Changed from `string | null` to `string[]` (kebab-case period IDs, unique values)
  - Products are now matched when they belong to **any** of the provided periods

**Affected Endpoints**:
- **POST `/api/v1/products/search`**
  - Request body now expects `categoryId` and `periodId` as arrays when provided
- **POST `/api/v1/me/search-filters`** (requires authentication)
  - `productSearch.categoryId` and `productSearch.periodId` now use array shape
- **GET `/api/v1/me/search-filters`** and **GET `/api/v1/me/search-filters/{userSearchFilterId}`** (requires authentication)
  - Returned `productSearch.categoryId` and `productSearch.periodId` now use array shape when present
- **PATCH `/api/v1/me/search-filters/{userSearchFilterId}`** (requires authentication)
  - Returned `productSearch.categoryId` and `productSearch.periodId` now use array shape when present

## 2026-02-22 - Product Search Period Filter

This update adds an optional period filter to product search criteria and to persisted user search filters.

### Added

**New Field in Product Search Criteria**:
- **`periodId`** (string, nullable, pattern: `^[a-z0-9]+(-[a-z0-9]+)*$`):
  - Optional kebab-case identifier for the level-one period to filter products by
  - Example values include `"renaissance"`, `"baroque"`, `"decorative-objects"`
  - When omitted or `null`, no period filtering is applied

**Affected Endpoints**:
- **GET `/api/v1/products`**
  - Supports `periodId` as an additional optional simple-search query parameter
- **POST `/api/v1/products/search`** (optional authentication via `Authorization: Bearer <token>`)
  - Request body (`ProductSearchData`) now accepts `periodId`
- **POST `/api/v1/me/search-filters`** (requires authentication)
  - Request body (`PostUserSearchFilterData.productSearch`) now accepts `periodId`
- **PATCH `/api/v1/me/search-filters/{userSearchFilterId}`** (requires authentication)
  - Request body (`PatchUserSearchFilterData.productSearch`) now accepts `periodId`
- **GET `/api/v1/me/search-filters`** and **GET `/api/v1/me/search-filters/{userSearchFilterId}`** (requires authentication)
  - Responses may include `ProductSearchData.periodId` when stored on the filter

## 2026-02-22 - Read-Only Period API

This update introduces a public, read-only API for retrieving and searching product periods with localized names and descriptions.

### Added

**New Period Endpoints** (no authentication required):
- **GET `/api/v1/periods`**
  - Existing get-all behavior when no query parameters are provided
  - GET simple-search behavior when query parameters are present
  - Search query parameters: `language` (required in search mode), `nameQuery` (optional), `sort` (`score|name|updated|created`, optional), `order` (`asc|desc`, optional)
  - Optional header: `Accept-Language` (de, en, fr, es)
  - Response: `200 OK` with an array of `GetPeriodSummaryData`
  - Cache headers: `Cache-Control: public, max-age=86400, s-maxage=604800`
- **GET `/api/v1/periods/{periodId}`**
  - Path parameter: `periodId` (kebab-case)
  - Optional header: `Accept-Language` (de, en, fr, es)
  - Response: `200 OK` with `GetPeriodData`
  - Response headers: `Last-Modified`, `Cache-Control: public, max-age=3600, s-maxage=86400`
- **POST `/api/v1/periods/search`**
  - Query parameters: `sort` (score|name|updated|created), `order` (asc|desc)
  - Request body: `PeriodSearchData` (required `language`, optional `nameQuery`)
  - Response: `200 OK` with an array of `GetPeriodSummaryData`

**New Data Types**:
- **`PeriodSearchData`**: `{ language: LanguageData, nameQuery?: string }`
- **`SortPeriodFieldData`**: `score | name | updated | created`
- **`GetPeriodSummaryData`**: `periodId`, `periodKey`, `name`, `created`, `updated`
- **`GetPeriodData`**: `periodId`, `periodKey`, `name`, `description`, `created`, `updated`

**New Error Codes**:
- `BAD_PATH_PARAMETER_VALUE` - missing `periodId`
- `BAD_BODY_VALUE` - missing or invalid JSON body
- `BAD_SORT_VALUE` - invalid sort field
- `BAD_ORDER_VALUE` - invalid sort order
- `NOT_FOUND` - period not found
- `INTERNAL_SERVER_ERROR` - unexpected server errors

## 2026-02-20 - Cacheable GET Simple-Search Endpoints

This update adds cache-friendly GET simple-search variants for products, shops, and categories. The new GET behavior mirrors existing POST search contracts while moving search inputs to query parameters.

### Added

**New GET Simple-Search Endpoint - Products**:
- **GET `/api/v1/products`**
  - Optional authentication: `Authorization: Bearer <token>` (same personalization behavior as `POST /api/v1/products/search`)
  - Required query parameters:
    - `language` (`LanguageData`)
    - `currency` (`CurrencyData`)
    - `productQuery` (string, min length 3)
  - Optional query parameters:
    - Existing search controls: `sort`, `order`, `searchAfter`, `size`
    - Additional product filter fields from `ProductSearchData` using query-string encoding (e.g. `categoryId`, `shopName`, `state`, `price[min]`, `created[max]`)
  - Response:
    - `200 OK` with `PersonalizedProductSearchResultData`
    - `Cache-Control: public, max-age=60, s-maxage=300`
    - `Access-Control-Allow-Origin: *`

**New GET Simple-Search Endpoint - Shops**:
- **GET `/api/v1/shops`**
  - Optional query parameters:
    - Existing search controls: `sort`, `order`, `searchAfter`, `size`
    - Additional shop filter fields from `ShopSearchData` using query-string encoding (e.g. `shopNameQuery`, `shopType`, `created[min]`, `updated[max]`)
  - Response:
    - `200 OK` with `ShopSearchResultData`
    - `Cache-Control: public, max-age=3600, s-maxage=86400`
    - `Access-Control-Allow-Origin: *`

**Extended GET Endpoint Behavior - Categories**:
- **GET `/api/v1/categories`**
  - Existing get-all behavior remains unchanged when **no query parameters** are provided
  - New simple-search behavior is used when query parameters are present
  - Query parameters for search mode:
    - `language` (required in search mode)
    - `nameQuery` (optional)
    - `sort` (optional: `score|name|updated|created`)
    - `order` (optional: `asc|desc`)
  - Response:
    - `200 OK` with array of `GetCategorySummaryData`
    - `Cache-Control: public, max-age=86400, s-maxage=604800`
    - `Access-Control-Allow-Origin: *`

### Changed

**Category Caching Policy**:
- **GET `/api/v1/categories`** cache policy was increased to stronger caching:
  - from `public, max-age=3600, s-maxage=86400`
  - to `public, max-age=86400, s-maxage=604800`

**Search Input Transport**:
- Existing POST search endpoints are unchanged and remain supported:
  - `POST /api/v1/products/search`
  - `POST /api/v1/shops/search`
  - `POST /api/v1/categories/search`
- Equivalent GET search variants are now available for cacheable query-param based calls.

## 2026-02-11 - Read-Only Category API

This update introduces a public, read-only API for retrieving and searching product categories with localized names and descriptions.

### Added

**New Category Endpoints** (no authentication required):
- **GET `/api/v1/categories`**
  - Optional header: `Accept-Language` (de, en, fr, es)
  - Response: `200 OK` with an array of `GetCategorySummaryData`
- **GET `/api/v1/categories/{categoryId}`**
  - Path parameter: `categoryId` (kebab-case)
  - Optional header: `Accept-Language` (de, en, fr, es)
  - Response: `200 OK` with `GetCategoryData` and `Last-Modified` header
- **POST `/api/v1/categories/search`**
  - Query parameters: `sort` (score|name|updated|created), `order` (asc|desc)
  - Request body: `CategorySearchData` (required `language`, optional `nameQuery`)
  - Response: `200 OK` with an array of `GetCategorySummaryData`

**New Data Types**:
- **`CategorySearchData`**: `{ language: LanguageData, nameQuery?: string }`
- **`SortCategoryFieldData`**: `score | name | updated | created`
- **`GetCategorySummaryData`**: `categoryId`, `categoryKey`, `name`, `created`, `updated`
- **`GetCategoryData`**: `categoryId`, `categoryKey`, `name`, `description`, `created`, `updated`

**New Error Codes**:
- `BAD_PATH_PARAMETER_VALUE` - missing `categoryId`
- `BAD_BODY_VALUE` - missing or invalid JSON body
- `BAD_SORT_VALUE` - invalid sort field
- `BAD_ORDER_VALUE` - invalid sort order
- `NOT_FOUND` - category not found
- `INTERNAL_SERVER_ERROR` - unexpected server errors

**Example - Search Categories Request**:
```json
{
  "language": "en",
  "nameQuery": "Furniture"
}
```

**Example - Get Category Response**:
```json
{
  "categoryId": "musical-instruments",
  "categoryKey": "musical-instruments",
  "name": { "text": "Antique Musical Instruments", "language": "en" },
  "description": { "text": "Antique instruments and accessories.", "language": "en" },
  "created": "2024-01-01T00:00:00Z",
  "updated": "2024-06-01T00:00:00Z"
}
```

## 2026-02-10 - Category Filter for Product Search

This update introduces an optional category filter for product search and saved search filters. Clients can now restrict results to a single level-one category by providing a `categoryId`.

### Added

**New Field in Product Search Criteria**:
- **`categoryId`** (string, nullable, pattern: `^[a-z0-9]+(-[a-z0-9]+)*$`):
  - Optional kebab-case identifier for the level-one category to filter products by
  - Examples: `"musical-instruments"`, `"antique-furniture"`, `"antique-clocks"`
  - When omitted or `null`, no category filtering is applied

**Affected Endpoints**:
- **POST `/api/v1/products/search`** (optional authentication via `Authorization: Bearer <token>`)
  - Request body (`ProductSearchData`) now accepts `categoryId`
- **POST `/api/v1/me/search-filters`** (requires authentication)
  - Request body (`PostUserSearchFilterData.productSearch`) now accepts `categoryId`
- **PATCH `/api/v1/me/search-filters/{userSearchFilterId}`** (requires authentication)
  - Request body (`PatchUserSearchFilterData.productSearch`) now accepts `categoryId`
- **GET `/api/v1/me/search-filters`** and **GET `/api/v1/me/search-filters/{userSearchFilterId}`** (requires authentication)
  - Responses return `ProductSearchData.categoryId` when a filter stores a category

**Example - Product Search Request**:
```json
{
  "language": "en",
  "currency": "USD",
  "productQuery": "antique clock",
  "categoryId": "antique-clocks"
}
```

**Example - Create Search Filter Request**:
```json
{
  "name": "Category Filtered Search",
  "productSearch": {
    "language": "en",
    "currency": "USD",
    "productQuery": "antique clock",
    "categoryId": "antique-clocks"
  }
}
```

## 2026-02-10 - Product Category Classification

This update adds automatic level-one category classification to product data. Products are now classified into top-level categories (e.g., "musical-instruments", "antique-furniture", "antique-clocks") through an event-driven classification pipeline. Category information is returned in all product responses with localized display names supporting German, English, French, and Spanish.

### Added

**Product Category Fields** - Two new optional fields added to product responses:

**GetProductData Schema**:
- **`categoryId`** (string, nullable, pattern: `^[a-z0-9]+(-[a-z0-9]+)*$`):
  - Kebab-case identifier for the level-one category the product has been classified into
  - Categories are automatically assigned by the backend classification system
  - Examples: `"musical-instruments"`, `"antique-furniture"`, `"antique-clocks"`, `"archaeological-artifacts"`
  - Optional field - will be `null` if product has not yet been classified
  
- **`category`** (LocalizedTextData, nullable):
  - Localized display name for the product's level-one category
  - Language matches the product's primary content language
  - Supports: German (de), English (en), French (fr), Spanish (es)
  - Examples:
    - German: `{"text": "Historische Musikinstrumente", "language": "de"}`
    - English: `{"text": "Antique Musical Instruments", "language": "en"}`
    - French: `{"text": "Instruments de musique anciens", "language": "fr"}`
    - Spanish: `{"text": "Instrumentos musicales antiguos", "language": "es"}`
  - Optional field - will be `null` if product has not yet been classified

**GetProductSummaryData Schema** - Same category fields as above:
- **`categoryId`** (string, nullable)
- **`category`** (LocalizedTextData, nullable)

**Example Response with Category Data**:
```json
{
  "productId": "550e8400-e29b-41d4-a716-446655440000",
  "shopName": "My Shop",
  "shopType": "COMMERCIAL_DEALER",
  "categoryId": "musical-instruments",
  "category": {
    "text": "Antique Musical Instruments",
    "language": "en"
  },
  "title": {
    "text": "Vintage Violin",
    "language": "en"
  }
}
```

### Changed

**All Product Endpoints** now include category fields in their responses:
- `GET /api/v1/shops/{shopId}/products/{shopsProductId}` - Returns `GetProductData` with category fields
- `GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}` - Returns `GetProductData` with category fields
- `GET /api/v1/shops/{shopId}/products/{shopsProductId}/similar` - Returns array of `GetProductSummaryData` with category fields
- `GET /api/v1/products` - Returns array of `GetProductSummaryData` with category fields
- `GET /api/v1/products/search` - Returns array of `GetProductSummaryData` with category fields

**Backward Compatibility**: These changes are fully backward compatible as both new fields are optional (nullable). Existing integrations will continue to work without modification.

**Implementation Notes**:
- Classification is event-driven and asynchronous - newly added products may not have category data immediately
- Categories are assigned at the level-one (top-level) only - no sub-categories at this time
- The classification system uses machine learning and rule-based approaches for automatic categorization
- Category display names are stored and returned in the language that best matches the product's content

## 2026-02-07 - Restructure Product Data Response Format

This update restructures the product data response format to better organize pricing, origin year, and auction information into composite objects. This provides a more cohesive API structure and reduces field proliferation at the top level.

### Added

**New Schema - AuctionData**:
- **Properties**:
  - `start` (string, date-time, nullable): Start datetime of the auction window in RFC3339 format. Optional field indicating when bidding begins or when the item will be auctioned.
  - `end` (string, date-time, nullable): End datetime of the auction window in RFC3339 format. Optional field indicating when bidding ends or when the auction session concludes.
- At least one of the fields (start or end) is present when the AuctionData object is included in a response.
- Both fields use RFC3339 datetime format (e.g., "2025-05-01T12:00:00Z")

**New Schema - PricingData**:
- **Properties**:
  - `offer` (PriceData, nullable): The actual offer or asking price for the product. This is the price at which the product is currently being sold.
  - `estimate` (PriceEstimateData, nullable): Optional price estimate range, common for auction items.
- Combines offer price and estimates into a single cohesive pricing structure.

**New Schema - PriceEstimateData**:
- **Properties**:
  - `min` (PriceData, nullable): Optional minimum estimated price for the product.
  - `max` (PriceData, nullable): Optional maximum estimated price for the product.
- Encapsulates price estimate range information.

**New Schema - OriginYearData**:
- **Properties**:
  - `min` (integer, nullable): Lower end of the year range when the antique is estimated to have originated.
  - `year` (integer, nullable): Exact year the antique is estimated to have originated.
  - `max` (integer, nullable): Upper end of the year range when the antique is estimated to have originated.
- At least one of the fields will be present when this object is included.
- Can represent either an exact year or a year range.

### Changed

**GetProductData Schema** - Major restructuring of product fields:

1. **Price Structure Changed**:
   - **Removed**: `price`, `priceEstimateMin`, `priceEstimateMax` (three separate fields)
   - **Added**: `price` (PricingData, nullable) - Single field containing nested offer and estimate data
   - **Example Before**:
     ```json
     {
       "price": { "currency": "EUR", "amount": 2999 },
       "priceEstimateMin": { "currency": "EUR", "amount": 2500 },
       "priceEstimateMax": { "currency": "EUR", "amount": 3500 }
     }
     ```
   - **Example After**:
     ```json
     {
       "price": {
         "offer": { "currency": "EUR", "amount": 2999 },
         "estimate": {
           "min": { "currency": "EUR", "amount": 2500 },
           "max": { "currency": "EUR", "amount": 3500 }
         }
       }
     }
     ```

2. **Origin Year Structure Changed**:
   - **Removed**: `originYearMin`, `originYear`, `originYearMax` (three separate fields)
   - **Added**: `originYear` (OriginYearData, nullable) - Single field containing nested min/year/max data
   - **Example Before**:
     ```json
     {
       "originYear": 1837
     }
     ```
   - **Example After**:
     ```json
     {
       "originYear": { "year": 1837 }
     }
     ```

3. **Auction Fields Restructured**:
   - **Removed**: `auctionStart`, `auctionEnd` (two separate fields)
   - **Added**: `auction` (AuctionData, nullable) - Single field containing nested start and end data
   - **Example Before**:
     ```json
     {
       "auctionStart": "2025-05-01T12:00:00Z",
       "auctionEnd": "2025-05-10T12:00:00Z"
     }
     ```
   - **Example After**:
     ```json
     {
       "auction": {
         "start": "2025-05-01T12:00:00Z",
         "end": "2025-05-10T12:00:00Z"
       }
     }
     ```

4. **Metadata Fields Now Required**:
   - `authenticity`, `condition`, `provenance`, `restoration` are now required fields (always present with default value "UNKNOWN" when not specified)
   - Previously these fields were nullable and could be absent from responses

**GetProductSummaryData Schema** - Added auction field:
- **New Property**:
  - `auction` (AuctionData, nullable): Optional auction time window information. Only present for products from auction houses with scheduled auction times. Contains start and/or end timestamps for the auction.
- This field is skipped in JSON responses when null
- The GetProductSummaryData description has been updated to reflect that auction times are now included (previously listed as excluded metadata)

**Affected Endpoints**:
All endpoints returning `GetProductData` or `PersonalizedGetProductData` now use the restructured format:
- **GET /api/v1/shops/{shopId}/products/{shopsProductId}** - Returns PersonalizedGetProductData
- **GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}** - Returns PersonalizedGetProductData

All endpoints returning `GetProductSummaryData` or `PersonalizedGetProductSummaryData` now include the auction field:
- **GET /api/v1/shops/{shopId}/products/{shopsProductId}/similar** - Returns array of PersonalizedGetProductSummaryData
- **POST /api/v1/products/search** - Returns PersonalizedProductSearchResultData containing PersonalizedGetProductSummaryData items

## 2026-01-25 - Restructure REST API Resource Paths

This update restructures API resource paths to follow a more hierarchical and RESTful pattern. Product endpoints now consistently use the `/shops/{shopId}/products/...` path structure, and shop identifier endpoints have been split into separate, more explicit endpoints based on the type of identifier (ID, domain, or slug).

### Changed

**Product Endpoints** - Restructured to nest under shops resource:

- **GET /api/v1/products/{shopId}/{shopsProductId}** → **GET /api/v1/shops/{shopId}/products/{shopsProductId}**
  - Path structure now clearly shows the hierarchical relationship between shops and products
  - All parameters, headers, responses, and behavior remain identical
  
- **GET /api/v1/products/by-slug/{shopSlugId}/{productSlugId}** → **GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}**
  - Path structure now clearly shows the hierarchical relationship between shops and products
  - The `/by-slug/` prefix is moved to the beginning to group all slug-based lookups
  - All parameters, headers, responses, and behavior remain identical

- **GET /api/v1/products/{shopId}/{shopsProductId}/similar** → **GET /api/v1/shops/{shopId}/products/{shopsProductId}/similar**
  - Path structure now clearly shows the hierarchical relationship between shops and products
  - All parameters, headers, responses, and behavior remain identical

- **GET /api/v1/products/{shopId}/{shopsProductId}/history** → **GET /api/v1/shops/{shopId}/products/{shopsProductId}/history**
  - Path structure now clearly shows the hierarchical relationship between shops and products
  - All parameters, headers, responses, and behavior remain identical

**Shop Endpoints** - Split polymorphic identifier endpoint into explicit endpoints:

- **GET /api/v1/shops/{shopIdentifier}** has been split into two separate endpoints:
  - **GET /api/v1/shops/{shopId}** - Retrieve shop by UUID
    - **Path Parameters**:
      - `shopId` (string, uuid, required): Unique identifier of the shop
    - Previously this was accessed via `/api/v1/shops/{shopIdentifier}` with a UUID value
    - Response structure, headers, and behavior remain identical
    - **Error Response Changes**:
      - Field name in errors changed from `shopIdentifier` to `shopId`
      - Error code for invalid format changed from `INVALID_SHOP_IDENTIFIER` to `INVALID_UUID`
      
  - **GET /api/v1/by-domain/shops/{shopDomain}** - Retrieve shop by domain (NEW)
    - **Path Parameters**:
      - `shopDomain` (string, required): Domain associated with the shop (e.g., "tech-store.com")
      - Domain is normalized (lowercased, www prefix removed)
      - Pattern: `^[a-z0-9]+([-.][a-z0-9]+)*\.[a-z]{2,}$`
    - Previously this was accessed via `/api/v1/shops/{shopIdentifier}` with a domain value
    - Response structure, headers, and behavior remain identical
    - **Error Response Changes**:
      - Field name in errors changed from `shopIdentifier` to `shopDomain`
      - Error code for invalid format changed from `INVALID_SHOP_IDENTIFIER` to `INVALID_DOMAIN`

- **GET /api/v1/shops/by-slug/{shopSlugId}** → **GET /api/v1/by-slug/shops/{shopSlugId}**
  - The `/by-slug/` prefix is moved to the beginning to group all slug-based lookups
  - All parameters, headers, responses, and behavior remain identical

- **PATCH /api/v1/shops/{shopIdentifier}** has been split into two separate endpoints:
  - **PATCH /api/v1/shops/{shopId}** - Update shop by UUID
    - **Path Parameters**:
      - `shopId` (string, uuid, required): Unique identifier of the shop
    - Previously this was accessed via `/api/v1/shops/{shopIdentifier}` with a UUID value
    - Request body, response structure, headers, and behavior remain identical
    - **Error Response Changes**:
      - Field name in errors changed from `shopIdentifier` to `shopId`
      - Error code for invalid format changed from `INVALID_SHOP_IDENTIFIER` to `INVALID_UUID`
      
  - **PATCH /api/v1/by-domain/shops/{shopDomain}** - Update shop by domain (NEW)
    - **Path Parameters**:
      - `shopDomain` (string, required): Domain associated with the shop (e.g., "tech-store.com")
      - Domain is normalized (lowercased, www prefix removed)
      - Pattern: `^[a-z0-9]+([-.][a-z0-9]+)*\.[a-z]{2,}$`
    - Previously this was accessed via `/api/v1/shops/{shopIdentifier}` with a domain value
    - Request body, response structure, headers, and behavior remain identical
    - **Error Response Changes**:
      - Field name in errors changed from `shopIdentifier` to `shopDomain`
      - Error code for invalid format changed from `INVALID_SHOP_IDENTIFIER` to `INVALID_DOMAIN`

### Removed

- **GET /api/v1/shops/{shopIdentifier}** - Removed polymorphic endpoint
  - Use `GET /api/v1/shops/{shopId}` for UUID-based lookup
  - Use `GET /api/v1/by-domain/shops/{shopDomain}` for domain-based lookup
  
- **PATCH /api/v1/shops/{shopIdentifier}** - Removed polymorphic endpoint
  - Use `PATCH /api/v1/shops/{shopId}` for UUID-based updates
  - Use `PATCH /api/v1/by-domain/shops/{shopDomain}` for domain-based updates

## 2026-01-24 - Split Product History into Dedicated Endpoint

This update separates product history retrieval from the main product endpoint into a dedicated history endpoint. This change provides better separation of concerns and allows the history endpoint to return a cleaner array structure instead of nesting history data within the product response.

### Added

**GET /api/v1/products/{shopId}/{shopsProductId}/history** - New endpoint to retrieve product event history:
- **Path Parameters**:
  - `shopId` (string, uuid, required): Unique identifier of the shop
  - `shopsProductId` (string, required): Shop's unique identifier for the product
- **Query Parameters**:
  - `currency` (CurrencyData, optional): Currency for price display in event payloads (default: EUR)
- **Headers**:
  - `Accept-Language` (string, optional): Preferred language for localized content
- **Response** (200 OK):
  - Returns an array of `GetProductEventData` objects representing the product's event history
  - Events are ordered chronologically
  - Each event contains:
    - `eventType` (ProductEventTypeData): Type of event (CREATED, STATE_LISTED, STATE_AVAILABLE, STATE_RESERVED, STATE_SOLD, STATE_REMOVED, STATE_UNKNOWN, PRICE_DISCOVERED, PRICE_DROPPED, PRICE_INCREASED, PRICE_REMOVED)
    - `productId` (string, uuid): Unique internal product identifier
    - `eventId` (string, uuid): Unique event identifier
    - `shopId` (string, uuid): Shop identifier
    - `shopsProductId` (string): Shop's product identifier
    - `payload` (ProductEventPayloadData): Event-specific payload with details
    - `timestamp` (string, date-time): When the event occurred (RFC3339 format)
- **Response Headers**:
  - `Content-Language`: Language of returned content
  - `Access-Control-Allow-Origin`: CORS header
- **Error Responses**:
  - 400 Bad Request: Invalid parameters (missing shopId or shopsProductId)
  - 404 Not Found: Product not found
  - 500 Internal Server Error: Server error

**Example Response**:
```json
[
  {
    "eventType": "CREATED",
    "productId": "550e8400-e29b-41d4-a716-446655440000",
    "eventId": "650e8400-e29b-41d4-a716-446655440001",
    "shopId": "550e8400-e29b-41d4-a716-446655440000",
    "shopsProductId": "6ba7b810",
    "payload": {
      "price": {
        "currency": "EUR",
        "amount": 3000
      },
      "state": "LISTED"
    },
    "timestamp": "2024-01-01T10:00:00Z"
  },
  {
    "eventType": "STATE_AVAILABLE",
    "productId": "550e8400-e29b-41d4-a716-446655440000",
    "eventId": "650e8400-e29b-41d4-a716-446655440002",
    "shopId": "550e8400-e29b-41d4-a716-446655440000",
    "shopsProductId": "6ba7b810",
    "payload": {
      "oldState": "LISTED",
      "newState": "AVAILABLE"
    },
    "timestamp": "2024-01-02T14:30:00Z"
  },
  {
    "eventType": "PRICE_DROPPED",
    "productId": "550e8400-e29b-41d4-a716-446655440000",
    "eventId": "650e8400-e29b-41d4-a716-446655440003",
    "shopId": "550e8400-e29b-41d4-a716-446655440000",
    "shopsProductId": "6ba7b810",
    "payload": {
      "oldPrice": {
        "currency": "EUR",
        "amount": 3000
      },
      "newPrice": {
        "currency": "EUR",
        "amount": 2999
      }
    },
    "timestamp": "2024-01-15T09:15:00Z"
  }
]
```

### Changed

**GET /api/v1/products/{shopId}/{shopsProductId}** - Removed history query parameter:
- The `history` query parameter has been removed
- Previously: `?history=true` would include history events in the response
- Now: Use the dedicated `/history` endpoint to retrieve product event history
- The response no longer includes a `history` field in the product data

**GET /api/v1/products/by-slug/{shopSlugId}/{productSlugId}** - Removed history query parameter:
- The `history` query parameter has been removed
- Previously: `?history=true` would include history events in the response
- Now: Use the dedicated `/history` endpoint with product IDs to retrieve event history
- The response no longer includes a `history` field in the product data

### Removed

**GetProductData.history** - Field removed from product response:
- Previously: `history` (array of GetProductEventData, optional, nullable)
- Now: Field no longer exists in the schema
- To retrieve product history, use the new dedicated endpoint: `GET /api/v1/products/{shopId}/{shopsProductId}/history`

**Query Parameter `history`** - Removed from product retrieval endpoints:
- Previously accepted on: `GET /api/v1/products/{shopId}/{shopsProductId}` and `GET /api/v1/products/by-slug/{shopSlugId}/{productSlugId}`
- The parameter and its validation error responses have been removed

### Rationale

This change provides:
- **Better Separation of Concerns**: Product details and event history are distinct resources with dedicated endpoints
- **Cleaner Response Structure**: History endpoint returns a direct array instead of nested data
- **Performance Optimization**: Product retrieval no longer carries optional heavy history data
- **API Design Consistency**: Follows RESTful principles with resource-specific endpoints

**Migration Guide**:
- **Before**: `GET /api/v1/products/{shopId}/{shopsProductId}?history=true`
- **After**: First call `GET /api/v1/products/{shopId}/{shopsProductId}` to get product details, then call `GET /api/v1/products/{shopId}/{shopsProductId}/history` to get event history if needed

## 2026-01-24 - Shop Name Update Restriction and Slug Uniqueness Enforcement

This update removes the ability to update a shop's name after creation and adds enforcement of shop slug uniqueness during shop creation. These changes ensure shop slug identifiers remain stable and unique throughout the shop's lifetime.

### Changed

**PATCH /api/v1/shops/{shopIdentifier}** - Shop name can no longer be updated:
- The `name` field has been removed from the request body schema (`PatchShopData`)
- Shop names are now immutable after creation since they determine the shop's slug identifier
- Only `shopType`, `domains`, and `image` fields can be updated
- Attempting to update the name will have no effect (field is ignored if provided)

**POST /api/v1/shops** - Enhanced uniqueness validation:
- Now validates shop slug uniqueness in addition to domain uniqueness
- Returns HTTP 409 Conflict when attempting to create a shop with a name that generates a slug already in use
- Two distinct conflict scenarios:
  - **Slug conflict**: Returns error "Shop with name '{name}' exists already - the shop-slug '{slug}' is already registered."
  - **Domain conflict**: Returns error "Shop with name '{name}' exists already - a domain of the shop is already registered."

### Removed

**PatchShopData.name** - Field removed from shop update request:
- Previously: `name` (string, optional, nullable)
- Now: Field no longer exists in the schema
- Rationale: Shop names determine slug identifiers which are used in URLs and must remain stable

### Rationale

This change ensures:
- **URL Stability**: Shop slugs in URLs (e.g., `/api/v1/shops/by-slug/tech-store-premium`) remain valid and consistent
- **Slug Uniqueness**: Each shop has a unique slug identifier, preventing confusion and routing issues
- **Data Integrity**: Shop name and slug remain aligned throughout the shop's lifetime
- **API Consistency**: The slug-based endpoints work reliably without handling name/slug mismatches

**Migration Note**: Existing shops retain their names and slugs. Only future updates are affected by this restriction. If a shop name change is absolutely necessary, it must be handled as a special administrative operation outside the normal API flow.

## 2026-01-24 - Separate Product DTOs for Search and Similar Products

This update introduces a new lightweight product summary data type (`GetProductSummaryData`) for use in search results and similar products listings. This reduces response payload size by excluding detailed metadata fields that are not needed in list views.

### Added

**GetProductSummaryData** - New lightweight product data type with the following fields:
- `productId` (string, uuid, required): Unique internal identifier for the product
- `productSlugId` (string, required): Human-readable slug identifier (e.g., "amazing-product-fa87c4")
- `shopSlugId` (string, required): Human-readable shop slug (e.g., "tech-store-premium")
- `eventId` (string, uuid, required): Unique identifier for current product state/version
- `shopId` (string, uuid, required): Unique shop identifier
- `shopsProductId` (string, required): Shop's unique product identifier
- `shopName` (string, required): Display name of the shop
- `shopType` (ShopTypeData, required): Type of shop (e.g., DEALER, AUCTION_HOUSE)
- `title` (LocalizedTextData, required): Localized product title
- `price` (PriceData, optional): Product price
- `state` (ProductStateData, required): Current product state
- `url` (string, uri, required): URL to product on shop's website
- `images` (array of ProductImageData, required): Product images with prohibited content classification
- `created` (string, date-time, required): When product was first created (RFC3339)
- `updated` (string, date-time, required): When product was last updated (RFC3339)

**PersonalizedGetProductSummaryData** - Wrapper combining `GetProductSummaryData` with optional user state:
- `item` (GetProductSummaryData, required): The product summary data
- `userState` (ProductUserStateData, optional): User-specific state when authenticated

### Changed

**GET /api/v1/products/search** - Response now uses `PersonalizedGetProductSummaryData`:
- Response type changed from `PersonalizedProductSearchResultData<PersonalizedGetProductData>` to `PersonalizedProductSearchResultData<PersonalizedGetProductSummaryData>`
- Each product in search results now excludes: `description`, `priceEstimateMin`, `priceEstimateMax`, `originYear`, `originYearMin`, `originYearMax`, `authenticity`, `condition`, `provenance`, `restoration`, `auctionStart`, `auctionEnd`, `history`
- All core product information remains available (id, title, price, state, images, timestamps)

**GET /api/v1/products/{shopId}/{shopsProductId}/similar** - Response now uses `PersonalizedGetProductSummaryData`:
- Response type changed from `Array<PersonalizedGetProductData>` to `Array<PersonalizedGetProductSummaryData>`
- Each similar product now excludes the same detailed fields as search results (listed above)
- Reduces payload size for similar product suggestions while maintaining essential product information

### Rationale

The new `GetProductSummaryData` type is specifically designed for list/summary views where detailed metadata is not needed. This:
- **Improves Performance**: Significantly reduces response payload size for endpoints that return multiple products
- **Better Separation**: Distinguishes between detailed product views (single product endpoint) and summary views (search/similar)
- **Maintains Compatibility**: The full `GetProductData` remains unchanged and continues to be used for single product retrieval

To get full product details including description, estimates, origin years, authenticity, condition, provenance, restoration, auction times, and history, use the single product endpoints:
- `GET /api/v1/products/{shopId}/{shopsProductId}`
- `GET /api/v1/products/by-slug/{shopSlugId}/{productSlugId}`

## 2026-01-24 - Add shop-type for auction-platforms

This update adds a new shop-type for auction-platforms. 

### Added

- `ShopType::AUCTION_PLATFORM`

## 2026-01-21 - Human-Readable Slug Identifiers

This update introduces human-readable slug identifiers for products and shops, providing SEO-friendly and user-friendly URLs. The new slug-based endpoints allow accessing resources using kebab-case identifiers instead of UUIDs, while maintaining backward compatibility with existing ID-based endpoints.

### Added

#### New Endpoints

**GET /api/v1/products/by-slug/{shopSlugId}/{productSlugId}**:
- Retrieves a single product using human-readable slug identifiers
- **Path Parameters**:
  - `shopSlugId` (string, required): Kebab-case shop identifier (e.g., "tech-store-premium", "christies")
    - Pattern: `^[a-z0-9]+(-[a-z0-9]+)*$`
    - Derived from shop name, normalized to lowercase with hyphens
  - `productSlugId` (string, required): Kebab-case product identifier with 6-character hex suffix
    - Pattern: `^[a-z0-9]+(-[a-z0-9]+)*-[a-f0-9]{6}$`
    - Format: `{product-title}-{6-char-hex}`
    - Example: "amazing-product-fa87c4"
- **Query Parameters**: Same as existing endpoint (`currency`, `history`)
- **Headers**: Same as existing endpoint (`Accept-Language`, optional `Authorization`)
- **Response**: Returns `PersonalizedGetProductData` (200) with product details including new slug fields
- **Authentication**: Optional (supports both authenticated and anonymous access)
- **Behavior**: Identical to GET /api/v1/products/{shopId}/{shopsProductId} but uses slug identifiers

**GET /api/v1/shops/by-slug/{shopSlugId}**:
- Retrieves shop details using human-readable slug identifier
- **Path Parameters**:
  - `shopSlugId` (string, required): Kebab-case shop identifier
    - Pattern: `^[a-z0-9]+(-[a-z0-9]+)*$`
    - Examples: "tech-store-premium", "christies", "sothebys"
- **Response**: Returns `GetShopData` (200) with complete shop information including new slug field
- **Behavior**: Identical to GET /api/v1/shops/{shopIdentifier} but specifically uses slug identifier

#### Schema Changes

**GetProductData**:
Added two new required fields to the product data response:

```json
{
  "productId": "550e8400-e29b-41d4-a716-446655440000",
  "productSlugId": "amazing-product-fa87c4",
  "shopSlugId": "tech-store-premium",
  "eventId": "550e8400-e29b-41d4-a716-446655440001",
  "shopId": "550e8400-e29b-41d4-a716-446655440000",
  "shopsProductId": "6ba7b810",
  ...
}
```

**New Fields**:
- `productSlugId` (string, required):
  - Human-readable product identifier with unique suffix
  - Pattern: `^[a-z0-9]+(-[a-z0-9]+)*-[a-f0-9]{6}$`
  - Format: Kebab-case product title + 6-character hexadecimal suffix
  - Example: "vintage-watch-3a2f9b"
  - The suffix ensures uniqueness when multiple products have similar titles
  
- `shopSlugId` (string, required):
  - Human-readable shop identifier
  - Pattern: `^[a-z0-9]+(-[a-z0-9]+)*$`
  - Format: Kebab-case shop name (no suffix)
  - Example: "tech-store-premium"
  - Derived from shop name by converting to lowercase and replacing spaces/special characters with hyphens

**GetShopData**:
Added one new required field to the shop data response:

```json
{
  "shopId": "550e8400-e29b-41d4-a716-446655440000",
  "shopSlugId": "tech-store-premium",
  "name": "Tech Store Premium",
  "shopType": "COMMERCIAL_DEALER",
  ...
}
```

**New Field**:
- `shopSlugId` (string, required):
  - Human-readable shop identifier
  - Pattern: `^[a-z0-9]+(-[a-z0-9]+)*$`
  - Format: Kebab-case shop name
  - Example: "tech-store-premium" or "christies"

#### Affected Endpoints (Schema Updates)

All endpoints that return product or shop data now include the new slug fields:

**Product Endpoints**:
- GET /api/v1/products/{shopId}/{shopsProductId}
- GET /api/v1/products/by-slug/{shopSlugId}/{productSlugId} (new)
- GET /api/v1/products/{shopId}/{shopsProductId}/similar
- POST /api/v1/products/search
- GET /api/v1/me/watchlist

**Shop Endpoints**:
- GET /api/v1/shops/{shopIdentifier}
- GET /api/v1/shops/by-slug/{shopSlugId} (new)
- POST /api/v1/shops
- PATCH /api/v1/shops/{shopIdentifier}
- POST /api/v1/shops/search

### Technical Details

**Slug Generation**:
- Shop slugs: Generated from shop name using slug library (kebab-case, no suffix)
  - Example: "Tech Store Premium" → "tech-store-premium"
  - Example: "Christie's" → "christies"
- Product slugs: Generated from product title + UUID-based 6-character hex suffix
  - Example: "Amazing Product" + UUID → "amazing-product-fa87c4"
  - The suffix ensures uniqueness across products with similar titles

**Backward Compatibility**:
- All existing ID-based endpoints remain unchanged and fully functional
- New slug-based endpoints provide alternative access patterns
- Clients can choose to use either UUID-based or slug-based endpoints
- All responses now include both ID and slug fields for maximum flexibility

**Error Codes**:
- Same error codes apply to slug-based endpoints as ID-based endpoints
- 400: Missing or invalid slug parameters (BAD_PATH_PARAMETER_VALUE)
- 404: Product or shop not found (PRODUCT_NOT_FOUND, SHOP_NOT_FOUND)
- 500: Internal server error (INTERNAL_SERVER_ERROR)

**Use Cases**:
- SEO-friendly URLs for web applications
- User-shareable links with readable identifiers
- Improved user experience with meaningful URLs
- Backend lookups by human-readable identifiers

## 2026-01-17 - Shop Name Exclusion Filter

This update adds the ability to exclude products from specific shops when searching. The new `excludeShopName` field allows users to filter out products from unwanted shops by exact shop name matching.

### Added

#### ProductSearchData Schema

A new `excludeShopName` field has been added:

**Field Structure**:
```json
{
  "language": "en",
  "currency": "USD",
  "productQuery": "vintage watch",
  "shopName": ["Sotheby's"],
  "excludeShopName": ["Heritage Auctions", "Christie's"]
}
```

**Field Details**:
- **Field name**: `excludeShopName`
- **Type**: `array of strings` (default: empty array)
- **Behavior**: Exact keyword match exclusion (case-sensitive)
- **Multiple values**: Supported (excludes products from any of the specified shops)
- **Default**: Empty array `[]` (no shops excluded)

**Technical Details**:
- The underlying OpenSearch query uses `must_not` clause with `terms` query
- Empty array `[]` means no shop name exclusion (includes all shops)
- Non-empty array excludes products whose shop name exactly matches one of the provided values
- Shop names are matched as complete strings, not partial matches
- Works in conjunction with `shopName` filter (include/exclude logic combined)

#### PatchProductSearchData Schema

The same `excludeShopName` field is available for partial updates:

**Field Structure**:
```json
{
  "excludeShopName": ["Bonhams", "Phillips"]
}
```

**Field Details**:
- **Field name**: `excludeShopName`
- **Type**: `array of strings` (optional, nullable)
- **Behavior**: Exact keyword match exclusion
- **When updating**: Providing `excludeShopName` array replaces the entire exclusion filter; omitting it leaves existing filter unchanged

#### API Endpoints Affected

**POST /api/v1/products/search**:
- Request body now accepts optional `excludeShopName` field
- Accepts array of exact shop names to exclude from search results
- Example request:
  ```json
  {
    "language": "de",
    "currency": "EUR",
    "productQuery": "classical music",
    "shopName": ["Sotheby's"],
    "excludeShopName": ["Heritage Auctions", "Christie's"],
    "price": {
      "min": 1000,
      "max": 50000
    }
  }
  ```

**POST /api/v1/search-filters**:
- Request body's `productSearch` object supports new `excludeShopName` field
- Shop name exclusion filter can be saved as part of user search filters

**PATCH /api/v1/search-filters/{userSearchFilterId}**:
- Request body's `productSearch` object supports new `excludeShopName` field
- Can update shop name exclusions independently of other filter criteria

**Response Behavior**:
- Products from excluded shops are completely filtered out of search results
- Total count reflects products after exclusion filter is applied
- Works correctly with pagination and sorting

## 2026-01-17 - Shop Name Filter: Text Search to Keyword Filter

This update changes the shop name filter from a fuzzy text search to an exact keyword match filter. The filter now accepts an array of exact shop names instead of a single text query string, enabling filtering by multiple specific shop names simultaneously.

### Changed

#### ProductSearchData Schema

The `shopNameQuery` field has been replaced with `shopName`:

**Before**:
```json
{
  "language": "en",
  "currency": "USD",
  "productQuery": "vintage watch",
  "shopNameQuery": "Christie"
}
```

**After**:
```json
{
  "language": "en",
  "currency": "USD",
  "productQuery": "vintage watch",
  "shopName": ["Christie's", "Sotheby's"]
}
```

**Field Changes**:
- **Field name**: `shopNameQuery` → `shopName`
- **Type**: `string` (optional, nullable) → `array of strings` (default: empty array)
- **Behavior**: Fuzzy text search with AUTO fuzziness → Exact keyword match (case-sensitive)
- **Multiple values**: Not supported → Supported (filters products from any of the specified shops)
- **Constraints**: Minimum 3 characters → No minimum length (accepts any valid shop name)

**Technical Details**:
- The underlying OpenSearch mapping for `shopName` changed from `text` type (analyzed, fuzzy-searchable) to `keyword` type (exact match)
- Empty array `[]` means no shop name filtering (matches all shops)
- Non-empty array filters to products whose shop name exactly matches one of the provided values
- Shop names are matched as complete strings, not partial matches

#### PatchProductSearchData Schema

The same change applies to the PATCH endpoint for updating search filters:

**Before**:
```json
{
  "shopNameQuery": "Updated Store"
}
```

**After**:
```json
{
  "shopName": ["Updated Store Name", "Another Store"]
}
```

**Field Changes**:
- **Field name**: `shopNameQuery` → `shopName`
- **Type**: `string` (optional, nullable) → `array of strings` (optional, nullable)
- **Behavior**: Fuzzy text search → Exact keyword match
- **When updating**: Providing `shopName` array replaces the entire filter; omitting it leaves existing filter unchanged

#### API Endpoints Affected

**POST /api/v1/products/search**:
- Request body field changed from `shopNameQuery` to `shopName`
- Now accepts array of exact shop names instead of fuzzy search string
- Example request:
  ```json
  {
    "language": "de",
    "currency": "EUR",
    "productQuery": "classical music",
    "shopName": ["Heritage Auctions", "Sotheby's", "Christie's"],
    "price": {
      "min": 1000,
      "max": 50000
    }
  }
  ```

**POST /api/v1/search-filters**:
- Request body's `productSearch` object uses new `shopName` field
- Shop name filter now accepts exact shop names as array

**PATCH /api/v1/search-filters/{userSearchFilterId}**:
- Request body's `productSearch` object uses new `shopName` field
- Update filter by providing array of exact shop names

**GET /api/v1/search-filters** and **GET /api/v1/search-filters/{userSearchFilterId}**:
- Response's `productSearch` object now contains `shopName` array instead of `shopNameQuery` string

### Migration Guide

**For Frontend Developers**:

1. **Update request payloads**: Change `shopNameQuery` to `shopName` and use array format:
   ```javascript
   // Before
   const searchRequest = {
     language: "en",
     currency: "USD",
     productQuery: "antique",
     shopNameQuery: "Christie"  // ❌ Old format
   };
   
   // After
   const searchRequest = {
     language: "en",
     currency: "USD",
     productQuery: "antique",
     shopName: ["Christie's"]  // ✅ New format - exact match
   };
   ```

2. **Handle multiple shops**: You can now filter by multiple shop names:
   ```javascript
   const searchRequest = {
     language: "en",
     currency: "USD",
     productQuery: "painting",
     shopName: ["Sotheby's", "Christie's", "Heritage Auctions"]
   };
   ```

3. **Use exact names**: The shop name must match exactly (case-sensitive). Partial matches no longer work:
   ```javascript
   // ❌ Will not match "Christie's"
   shopName: ["Christie"]
   
   // ✅ Will match "Christie's"
   shopName: ["Christie's"]
   ```

4. **Empty array for no filter**: Use empty array instead of null/undefined:
   ```javascript
   // Before (no shop filter)
   shopNameQuery: null
   
   // After (no shop filter)
   shopName: []  // or omit the field entirely
   ```

5. **Update response parsing**: When reading saved search filters, expect `shopName` array:
   ```javascript
   // Before
   const shopFilter = filter.productSearch.shopNameQuery;  // string or null
   
   // After
   const shopFilters = filter.productSearch.shopName;  // array of strings
   ```

### Removed

- **shopNameQuery** field (replaced by **shopName** in ProductSearchData and PatchProductSearchData schemas)
- Fuzzy text search capability for shop names (now requires exact match)
- Single shop name filtering (replaced by array-based filtering supporting multiple shops)

## 2026-01-15 - Product Price Estimates

This update adds price estimate fields to products, allowing antique dealers and auction houses to display estimated price ranges alongside actual selling prices. This is particularly useful for auction items where the final price may differ from the estimated value.

### Added

#### Product Price Estimate Fields

**GetProductData** (extended):
- **priceEstimateMin** (PriceData, optional): Minimum estimated price for the product
  - Type: PriceData object with currency and amount in minor units
  - Format: Same as the `price` field (e.g., `{"currency": "EUR", "amount": 2500}`)
  - Nullable: Yes
  - Only present when the seller provides an estimated minimum price
  - Common for auction items and antiques where valuation ranges are typical
  
  **Example**:
  ```json
  {
    "productId": "550e8400-e29b-41d4-a716-446655440000",
    "price": {
      "currency": "EUR",
      "amount": 3000
    },
    "priceEstimateMin": {
      "currency": "EUR",
      "amount": 2500
    },
    "priceEstimateMax": {
      "currency": "EUR",
      "amount": 3500
    }
  }
  ```

- **priceEstimateMax** (PriceData, optional): Maximum estimated price for the product
  - Type: PriceData object with currency and amount in minor units
  - Format: Same as the `price` field (e.g., `{"currency": "EUR", "amount": 3500}`)
  - Nullable: Yes
  - Only present when the seller provides an estimated maximum price
  - Typically used together with `priceEstimateMin` to indicate a value range

**PutProductData** (extended):
- **priceEstimateMin** (PriceData, optional): Minimum estimated price for creating/updating products
  - Type: PriceData object with currency and amount in minor units
  - Format: `{"currency": "EUR", "amount": 2500}`
  - Nullable: Yes
  - Optional field for creating or updating products
  - Sellers can provide price estimate ranges when listing items
  
  **Example**:
  ```json
  {
    "shopsProductId": "antique-vase-123",
    "title": {
      "text": "Victorian Vase",
      "language": "en"
    },
    "state": "AVAILABLE",
    "url": "https://antique-shop.com/item/123",
    "price": {
      "currency": "EUR",
      "amount": 1200
    },
    "priceEstimateMin": {
      "currency": "EUR",
      "amount": 1000
    },
    "priceEstimateMax": {
      "currency": "EUR",
      "amount": 1500
    }
  }
  ```

- **priceEstimateMax** (PriceData, optional): Maximum estimated price for creating/updating products
  - Type: PriceData object with currency and amount in minor units
  - Format: `{"currency": "EUR", "amount": 3500}`
  - Nullable: Yes
  - Optional field for creating or updating products
  - Used to specify the upper bound of the estimated value range

#### API Endpoints Affected

**GET /api/v1/products/{shopId}/{shopsProductId}**:
- Response now includes `priceEstimateMin` and `priceEstimateMax` fields when present
- Both fields are optional and may be null if no estimates are provided

**GET /api/v1/products/{shopId}/{shopsProductId}/similar**:
- Similar products now include `priceEstimateMin` and `priceEstimateMax` fields when available

**PUT /api/v1/products**:
- Request body can now include `priceEstimateMin` and `priceEstimateMax` fields
- Both fields are optional for each product in the items array

**POST /api/v1/products/search**:
- Search results now include `priceEstimateMin` and `priceEstimateMax` for each product when available

**GET /api/v1/me/watchlist**:
- Watchlist products now display `priceEstimateMin` and `priceEstimateMax` when present

#### Product History Events

**ProductCreatedEventPayloadData** (extended):
- Now includes `priceEstimateMin` and `priceEstimateMax` fields in the CREATED event payload
- These fields capture the initial price estimates when a product is first created
- Both fields are optional and use the PriceData type
- Helps track the complete initial state of a product including estimated values

### Usage Examples

#### Product with price estimates
GET /api/v1/products/{shopId}/{shopsProductId}
```json

{
  "item": {
    "productId": "550e8400-e29b-41d4-a716-446655440000",
    "shopName": "Antique Auctions Ltd",
    "title": {
      "text": "18th Century Mahogany Desk",
      "language": "en"
    },
    "price": {
      "currency": "GBP",
      "amount": 450000
    },
    "priceEstimateMin": {
      "currency": "GBP",
      "amount": 400000
    },
    "priceEstimateMax": {
      "currency": "GBP",
      "amount": 550000
    },
    "state": "AVAILABLE"
  }
}
```

#### Creating a product with price estimates
PUT /api/v1/products
```json
{
  "items": [
    {
      "shopsProductId": "auction-item-456",
      "title": {
        "text": "Rare Ming Dynasty Vase",
        "language": "en"
      },
      "state": "LISTED",
      "url": "https://auction-house.com/item/456",
      "priceEstimateMin": {
        "currency": "USD",
        "amount": 500000
      },
      "priceEstimateMax": {
        "currency": "USD",
        "amount": 800000
      }
    }
  }
}
```

### Implementation Notes

**Price Estimate Behavior**:
- Both `priceEstimateMin` and `priceEstimateMax` are completely independent from the actual `price` field
- Estimates can be present even when `price` is null (e.g., for auction items without a fixed price)
- The actual `price` may fall outside the estimate range (e.g., if a sold price exceeded expectations)
- Both estimate fields are optional and can be present independently
- Currency conversion is handled automatically by the enrichment service, same as for the `price` field

**Common Use Cases**:
1. **Auction Houses**: Display estimated value ranges before bidding
2. **Antique Dealers**: Show professional appraisal values alongside asking prices
3. **Collectibles**: Indicate market value estimates for rare items
4. **Insurance Purposes**: Provide valuation ranges for high-value antiques

**Frontend Impact**:
- Frontend applications should display price estimates when available
- Consider showing estimates differently from actual prices (e.g., "Estimated: £4,000 - £5,500" vs "Price: £4,750")
- For auction items without a fixed price, estimates are the primary pricing information
- Estimates are optional - handle cases where they are null or absent

**Backend Type**: The Rust backend uses:
- `native_price_estimate_min: Option<Price>` - Native currency minimum estimate
- `native_price_estimate_max: Option<Price>` - Native currency maximum estimate
- `other_price_estimate_min: HashMap<Currency, MonetaryAmount>` - Converted estimates for other currencies
- `other_price_estimate_max: HashMap<Currency, MonetaryAmount>` - Converted estimates for other currencies

### Changed

No existing fields or endpoints were modified. This is a purely additive change.

### Removed

No endpoints, fields, or functionality have been removed in this update.

## 2026-01-14 - Auction DateTime Fields

This update adds auction timing information for products, enabling users to see when items will be auctioned and filter searches by auction timing. Auction houses typically list time windows for when items will be sold, and this feature makes that information available through the API.

### Added

#### Product Fields

**GetProductData** (extended):
- **auctionStart** (string, date-time, optional): Start datetime of the auction window
  - Type: ISO8601/RFC3339 datetime string
  - Format: `date-time` (e.g., `"2026-02-15T10:00:00Z"`)
  - Nullable: Yes
  - Only present for products from auction houses with scheduled auction times
  - Indicates when bidding begins or when the item will be auctioned
  
  **Example**:
  ```json
  {
    "productId": "550e8400-e29b-41d4-a716-446655440000",
    "auctionStart": "2026-02-15T10:00:00Z",
    "auctionEnd": "2026-02-15T14:00:00Z"
  }
  ```

- **auctionEnd** (string, date-time, optional): End datetime of the auction window
  - Type: ISO8601/RFC3339 datetime string
  - Format: `date-time` (e.g., `"2026-02-15T14:00:00Z"`)
  - Nullable: Yes
  - Only present for products from auction houses with scheduled auction times
  - Indicates when bidding ends or when the auction session concludes

**PutProductData** (extended):
- **auctionStart** (string, date-time, optional): Start datetime of the auction window
  - Type: ISO8601/RFC3339 datetime string
  - Format: `date-time`
  - Nullable: Yes
  - Optional field for creating or updating products
  - Only applicable for products from auction houses
  
  **Example**:
  ```json
  {
    "shopsProductId": "auction-item-123",
    "title": {
      "text": "Antique Vase",
      "language": "en"
    },
    "state": "LISTED",
    "url": "https://auction-house.com/item/123",
    "auctionStart": "2026-02-15T10:00:00Z",
    "auctionEnd": "2026-02-15T14:00:00Z"
  }
  ```

- **auctionEnd** (string, date-time, optional): End datetime of the auction window
  - Type: ISO8601/RFC3339 datetime string
  - Format: `date-time`
  - Nullable: Yes
  - Optional field for creating or updating products
  - Only applicable for products from auction houses

#### Search Filter Fields

**ProductSearchData** (extended):
- **auctionStart** (RangeQueryDateTime, optional): Filter by auction start datetime range
  - Type: Object with `min` and/or `max` ISO8601/RFC3339 datetime strings
  - Nullable: Yes
  - Filters products by when their auction windows begin
  - Only matches products that have auction start times set
  - Both `min` and `max` are optional within the range
  
  **Example**:
  ```json
  {
    "language": "de",
    "currency": "EUR",
    "productQuery": "antique furniture",
    "auctionStart": {
      "min": "2026-01-01T00:00:00Z",
      "max": "2026-03-31T23:59:59Z"
    }
  }
  ```

- **auctionEnd** (RangeQueryDateTime, optional): Filter by auction end datetime range
  - Type: Object with `min` and/or `max` ISO8601/RFC3339 datetime strings
  - Nullable: Yes
  - Filters products by when their auction windows end
  - Only matches products that have auction end times set
  - Both `min` and `max` are optional within the range
  
  **Example**:
  ```json
  {
    "language": "en",
    "currency": "USD",
    "productQuery": "vintage watch",
    "auctionEnd": {
      "max": "2026-02-28T23:59:59Z"
    }
  }
  ```

**PatchProductSearchData** (extended):
- **auctionStart** (RangeQueryDateTime, optional): Filter by auction start datetime range
  - Type: Object with `min` and/or `max` ISO8601/RFC3339 datetime strings
  - Nullable: Yes
  - Can be updated independently when patching a search filter
  - Filters products by when their auction windows begin
  - Only matches products that have auction start times set

- **auctionEnd** (RangeQueryDateTime, optional): Filter by auction end datetime range
  - Type: Object with `min` and/or `max` ISO8601/RFC3339 datetime strings
  - Nullable: Yes
  - Can be updated independently when patching a search filter
  - Filters products by when their auction windows end
  - Only matches products that have auction end times set

#### API Endpoints Affected

**GET /api/v1/products/{shopId}/{shopsProductId}**:
- Response now includes `auctionStart` and `auctionEnd` fields when present

**PUT /api/v1/products**:
- Request body can now include `auctionStart` and `auctionEnd` fields

**POST /api/v1/products/search**:
- Request body can now include `auctionStart` and `auctionEnd` query filters

**POST /api/v1/users/{userId}/search-filters**:
- Can create search filters with `auctionStart` and `auctionEnd` query filters

**PATCH /api/v1/users/{userId}/search-filters/{searchFilterId}**:
- Can update search filters with `auctionStart` and `auctionEnd` query filters

**GET /api/v1/users/{userId}/search-filters**:
- Returns search filters that may include `auctionStart` and `auctionEnd` query filters

**GET /api/v1/users/{userId}/search-filters/{searchFilterId}**:
- Returns search filter that may include `auctionStart` and `auctionEnd` query filters

### Usage Examples

#### Filtering products by auction timing

Find products being auctioned in February 2026:
```json
POST /api/v1/products/search
{
  "language": "de",
  "currency": "EUR",
  "productQuery": "antique",
  "auctionStart": {
    "min": "2026-02-01T00:00:00Z",
    "max": "2026-02-28T23:59:59Z"
  }
}
```

Find products with auctions ending before a specific date:
```json
POST /api/v1/products/search
{
  "language": "en",
  "currency": "GBP",
  "productQuery": "vintage",
  "auctionEnd": {
    "max": "2026-01-31T23:59:59Z"
  }
}
```

Find products with auctions starting after a specific date:
```json
POST /api/v1/products/search
{
  "language": "fr",
  "currency": "EUR",
  "productQuery": "furniture",
  "auctionStart": {
    "min": "2026-03-01T00:00:00Z"
  }
}
```

## 2026-01-14 - Shop Type Classification

This update introduces shop type classification to distinguish between different types of vendors (auction houses, commercial dealers, and marketplaces), enabling users to filter products and shops by vendor type.

### Added

#### New Data Types

**ShopTypeData** (enum):
- Classification of shop/vendor type
- Possible values:
  - `AUCTION_HOUSE`: Auction house selling items through auctions
  - `COMMERCIAL_DEALER`: Commercial dealer or shop selling items directly
  - `MARKETPLACE`: Marketplace platform connecting buyers and sellers
- No default value (field is mandatory)
- Example: `"COMMERCIAL_DEALER"`

#### New Filter Fields

**ProductSearchData and PatchProductSearchData**:
- **shopType** (array of ShopTypeData, optional): Filter products by shop type
  - Type: Array of enum strings
  - Values: `AUCTION_HOUSE`, `COMMERCIAL_DEALER`, `MARKETPLACE`
  - Default: Empty array `[]`
  - Nullable: Yes
  - Unique items: Yes
  
  **Example**:
  ```json
  {
    "shopType": ["COMMERCIAL_DEALER", "AUCTION_HOUSE"]
  }
  ```

**ShopSearchData**:
- **shopType** (array of ShopTypeData, optional): Filter shops by shop type
  - Type: Array of enum strings
  - Values: `AUCTION_HOUSE`, `COMMERCIAL_DEALER`, `MARKETPLACE`
  - Default: Empty array `[]`
  - Nullable: Yes
  - Unique items: Yes
  
  **Example**:
  ```json
  {
    "shopType": ["MARKETPLACE"]
  }
  ```

### Changed

#### Shop Data Types

All shop-related data types now include the mandatory `shopType` field:

**GetShopData**:
- **Added field**: `shopType` (ShopTypeData, required): Type classification of the shop
- This field is now required in all shop responses

**Example**:
```json
{
  "shopId": "550e8400-e29b-41d4-a716-446655440000",
  "name": "Tech Store Premium",
  "shopType": "COMMERCIAL_DEALER",
  "domains": ["tech-store-premium.com"],
  "created": "2024-01-01T10:00:00Z",
  "updated": "2024-01-01T12:00:00Z"
}
```

**PostShopData**:
- **Added field**: `shopType` (ShopTypeData, required): Type classification of the shop
- This field is now mandatory when creating a new shop

**Example**:
```json
{
  "name": "Tech Store Premium",
  "shopType": "COMMERCIAL_DEALER",
  "domains": ["tech-store-premium.com"]
}
```

**PatchShopData**:
- **Added field**: `shopType` (ShopTypeData, optional): New type classification for the shop
- This field is optional and can be updated independently

**Example**:
```json
{
  "shopType": "AUCTION_HOUSE"
}
```

#### Product Data Types

**GetProductData**:
- **Added field**: `shopType` (ShopTypeData, required): Type of the shop selling this product
- This field is now included in all product responses, providing shop type information at the product level

**Example**:
```json
{
  "productId": "550e8400-e29b-41d4-a716-446655440000",
  "shopId": "660e8400-e29b-41d4-a716-446655440001",
  "shopName": "Antique Auctions Ltd",
  "shopType": "AUCTION_HOUSE",
  "title": {
    "text": "Victorian Writing Desk",
    "language": "en"
  }
}
```

#### Affected Endpoints

All endpoints returning shop or product data now include the `shopType` field:

**Shop Endpoints**:
- `POST /api/v1/shops` - Now requires `shopType` in request body
- `GET /api/v1/shops/{shopIdentifier}` - Returns `shopType` in response
- `PATCH /api/v1/shops/{shopIdentifier}` - Accepts optional `shopType` in request body, returns `shopType` in response
- `POST /api/v1/shops/search` - Returns `shopType` for each shop, accepts `shopType` filter in request body

**Product Endpoints**:
- `GET /api/v1/products/{shopId}/{shopsProductId}` - Returns `shopType` in product data
- `POST /api/v1/products/search` - Returns `shopType` for each product, accepts `shopType` filter in request body
- `GET /api/v1/products/{shopId}/{shopsProductId}/similar` - Returns `shopType` for each similar product
- `GET /api/v1/me/watchlist` - Returns `shopType` for each watchlist product

**Search Filter Endpoints**:
- `POST /api/v1/me/search-filters` - Accepts `shopType` in `productSearch` criteria
- `GET /api/v1/me/search-filters` - Returns `shopType` in saved filter criteria
- `GET /api/v1/me/search-filters/{userSearchFilterId}` - Returns `shopType` in filter criteria
- `PATCH /api/v1/me/search-filters/{userSearchFilterId}` - Accepts `shopType` in partial `productSearch` update

#### Search Behavior

- The `shopType` filter works as a disjunctive (OR) filter: products/shops matching ANY of the specified types will be returned
- Empty arrays or null values for the `shopType` filter will not apply any filtering for this criterion
- The filter combines with other search criteria using AND logic

**Example Product Search Request**:
```json
POST /api/v1/products/search
{
  "language": "en",
  "currency": "USD",
  "productQuery": "antique furniture",
  "shopType": ["AUCTION_HOUSE", "COMMERCIAL_DEALER"]
}
```

This request will return products from shops that are either auction houses OR commercial dealers, matching the search query "antique furniture".

**Example Shop Search Request**:
```json
POST /api/v1/shops/search
{
  "shopNameQuery": "antique",
  "shopType": ["AUCTION_HOUSE"]
}
```

This request will return shops that match "antique" in their name AND are auction houses.

### Frontend Impact

**Breaking Changes**:
1. **Shop Creation**: Frontend must now provide `shopType` when creating shops via `POST /api/v1/shops`
2. **Data Models**: Update TypeScript/data models to include the mandatory `shopType` field in:
   - `GetShopData`
   - `PostShopData`
   - `GetProductData`
3. **Optional Fields**: Update models to include optional `shopType` field in:
   - `PatchShopData`
   - `ProductSearchData`
   - `PatchProductSearchData`
   - `ShopSearchData`

**New Features**:
1. **Shop Type Display**: Display shop type badges or indicators in:
   - Shop listings and search results
   - Product details (showing the type of shop selling the product)
   - Shop detail pages
2. **Filtering**: Implement UI controls to filter by shop type in:
   - Product search interfaces
   - Shop search interfaces
   - Saved search filter creation/editing
3. **Shop Management**: Add shop type selection in shop creation and editing forms

**Example TypeScript Updates**:
```typescript
// Before
interface GetShopData {
  shopId: string;
  name: string;
  domains: string[];
  image?: string;
  created: string;
  updated: string;
}

// After
interface GetShopData {
  shopId: string;
  name: string;
  shopType: 'AUCTION_HOUSE' | 'COMMERCIAL_DEALER' | 'MARKETPLACE';
  domains: string[];
  image?: string;
  created: string;
  updated: string;
}

// Product search filter
interface ProductSearchData {
  language: string;
  currency: string;
  productQuery: string;
  shopNameQuery?: string;
  shopType?: ('AUCTION_HOUSE' | 'COMMERCIAL_DEALER' | 'MARKETPLACE')[];
  // ... other fields
}
```

### Removed

No endpoints, fields, or functionality have been removed in this update. All changes are additive, though the `shopType` field is mandatory in certain contexts.

## 2026-01-11 - Product Image Prohibited Content Classification

This update enhances product images with prohibited content classification to identify and flag images containing sensitive or prohibited content such as Nazi Germany symbols and insignia.

### Added

#### New Data Types

**ProhibitedContentData** (enum):
- Classification of prohibited or sensitive content that may be present in product images
- Possible values:
  - `UNKNOWN`: Content classification has not been determined
  - `NONE`: No prohibited content detected in the image
  - `NAZI_GERMANY`: Image contains Nazi Germany symbols, insignia, or related content
- Default value: `UNKNOWN`

**ProductImageData** (object):
- Product image with prohibited content classification
- Properties:
  - `url` (string, required): URL to the product image
  - `prohibitedContent` (ProhibitedContentData, required): Prohibited content classification for this image
- Example:
  ```json
  {
    "url": "https://my-shop.com/images/product-1.jpg",
    "prohibitedContent": "NONE"
  }
  ```

### Changed

#### GET /api/v1/products/{shopId}/{shopsProductId}

**Response Structure Change**: The `images` field in the product response has been enhanced from a simple array of URL strings to an array of `ProductImageData` objects that include prohibited content classification.

**Endpoint**: `GET /api/v1/products/{shopId}/{shopsProductId}`

**Old Response Structure**:
```json
{
  "item": {
    "productId": "550e8400-e29b-41d4-a716-446655440000",
    "images": [
      "https://my-shop.com/images/product-1.jpg",
      "https://my-shop.com/images/product-2.jpg"
    ]
  }
}
```

**New Response Structure**:
```json
{
  "item": {
    "productId": "550e8400-e29b-41d4-a716-446655440000",
    "images": [
      {
        "url": "https://my-shop.com/images/product-1.jpg",
        "prohibitedContent": "NONE"
      },
      {
        "url": "https://my-shop.com/images/product-2.jpg",
        "prohibitedContent": "NAZI_GERMANY"
      }
    ]
  }
}
```

**Impact on Other Endpoints**:
This change affects all endpoints that return product data with images:
- `GET /api/v1/products/search` - Product search results
- `GET /api/v1/products/similar/{shopId}/{shopsProductId}` - Similar products
- `GET /api/v1/me/watchlist` - Watchlist products

**Frontend Impact**:
- Frontend applications must update their code to access image URLs via `image.url` instead of using the URL string directly
- Display appropriate warnings or content filters based on the `prohibitedContent` classification
- Handle the `NAZI_GERMANY` classification according to regional legal requirements (e.g., Germany's restrictions on Nazi symbols)
- The `prohibitedContent` field may be `UNKNOWN` during a transition period while existing images are being classified

**Note**: This is a breaking change to the API response structure. Frontend applications must be updated to handle the new image object structure.

## 2026-01-06 - Watchlist Quota Restriction

This update implements a quota limit on the number of products a user can add to their watchlist, preventing unlimited watchlist growth and ensuring fair resource usage.

### Added

#### New Error Code

**WATCHLIST_QUOTA_EXCEEDED**:
- Error code returned when a user attempts to add a product to their watchlist after reaching the maximum quota
- Used in the watchlist product creation endpoint

#### New Error Response for POST /api/v1/me/watchlist

**422 Unprocessable Entity** - Watchlist quota exceeded:

When a user attempts to add a product to their watchlist but already has the maximum number of products (5), the API now returns:

- **Status Code**: 422 Unprocessable Entity
- **Response Body**:
  ```json
  {
    "status": 422,
    "title": "Unprocessable Content",
    "error": "WATCHLIST_QUOTA_EXCEEDED",
    "detail": "Exceeded the maximum amount of watchlist entries. There are already 5/5 watchlist entries occupied."
  }
  ```

### Changed

#### POST /api/v1/me/watchlist

**Behavior Change**: The endpoint now enforces a quota limit of 5 watchlist products per user.

- **Endpoint**: `POST /api/v1/me/watchlist`
- **Maximum Quota**: 5 watchlist products per user
- **Validation**: Before creating a new watchlist entry, the system checks if the user already has 5 or more products in their watchlist
- **New Response**: Returns HTTP 422 with error code `WATCHLIST_QUOTA_EXCEEDED` when quota is exceeded

**Updated Description**: The endpoint description now includes information about the 5-product quota limit:
> "Each user is limited to a maximum of 5 watchlist products. If the user already has 5 products in their watchlist, adding another will result in a 422 Unprocessable Entity error."

**Request**: No changes to request format
```json
{
  "shopId": "550e8400-e29b-41d4-a716-446655440000",
  "shopsProductId": "6ba7b810"
}
```

**Updated Responses**:
- **201 Created**: Product successfully added to watchlist (when quota not exceeded)
- **400 Bad Request**: Invalid request body
- **401 Unauthorized**: Invalid or missing JWT token
- **422 Unprocessable Entity**: Watchlist quota exceeded (NEW)
- **500 Internal Server Error**: Internal server error

**Frontend Impact**:
- Frontend applications should handle the new 422 error response
- Display appropriate user feedback when the watchlist quota is reached
- Consider showing the user's current watchlist count (e.g., "5/5 products in watchlist")
- Provide options to remove existing products before adding new ones

## 2026-01-04 - Product Search Filter Enhancement

This update extends the product search filtering capabilities by adding new filter options for origin year and product attributes (authenticity, condition, provenance, restoration).

### Added

#### New Search Filter Fields

The `ProductSearchData` and `PatchProductSearchData` objects now support filtering by additional product attributes:

**New Fields in `ProductSearchData`**:

1. **originYear** (RangeQueryInt32, optional): Filter products by origin year range
   - Type: Object with optional `min` and `max` integer fields
   - Description: Filter products whose origin year falls within the specified range
   - Both `min` and `max` are optional and inclusive
   - Nullable: Yes
   
   **Example**:
   ```json
   {
     "originYear": {
       "min": 1742,
       "max": 1953
     }
   }
   ```

2. **authenticity** (Array of AuthenticityData, optional): Filter by authenticity classifications
   - Type: Array of enum strings
   - Values: `ORIGINAL`, `LATER_COPY`, `REPRODUCTION`, `QUESTIONABLE`, `UNKNOWN`
   - Default: Empty array `[]`
   - Nullable: Yes
   - Unique items: Yes
   
   **Example**:
   ```json
   {
     "authenticity": ["ORIGINAL", "LATER_COPY"]
   }
   ```

3. **condition** (Array of ConditionData, optional): Filter by product condition assessments
   - Type: Array of enum strings
   - Values: `EXCELLENT`, `GREAT`, `GOOD`, `FAIR`, `POOR`, `UNKNOWN`
   - Default: Empty array `[]`
   - Nullable: Yes
   - Unique items: Yes
   
   **Example**:
   ```json
   {
     "condition": ["EXCELLENT", "GREAT"]
   }
   ```

4. **provenance** (Array of ProvenanceData, optional): Filter by provenance documentation levels
   - Type: Array of enum strings
   - Values: `COMPLETE`, `PARTIAL`, `CLAIMED`, `NONE`, `UNKNOWN`
   - Default: Empty array `[]`
   - Nullable: Yes
   - Unique items: Yes
   
   **Example**:
   ```json
   {
     "provenance": ["PARTIAL", "COMPLETE"]
   }
   ```

5. **restoration** (Array of RestorationData, optional): Filter by restoration work levels
   - Type: Array of enum strings
   - Values: `NONE`, `MINOR`, `MAJOR`, `UNKNOWN`
   - Default: Empty array `[]`
   - Nullable: Yes
   - Unique items: Yes
   
   **Example**:
   ```json
   {
     "restoration": ["NONE", "MINOR"]
   }
   ```

#### New Sort Field

**SortProductFieldData** enum now includes:

- **originYear**: Sort products by their origin year (ascending or descending)
  - Can be used with the `sort` query parameter on search endpoints
  - Supports both `asc` and `desc` ordering via the `order` parameter

**Updated Enum Values**: `score`, `price`, `originYear`, `updated`, `created`

#### New Schema Type

**RangeQueryInt32**:
- Type: Object for integer range queries
- Properties:
  - `min` (integer, optional): Minimum value (inclusive)
  - `max` (integer, optional): Maximum value (inclusive)
- Used for filtering by origin year ranges

### Changed

#### Affected Endpoints

All endpoints that accept `ProductSearchData` now support the new filter fields:

1. **POST /api/v1/products/search**
   - Request body `ProductSearchData` now accepts `originYear`, `authenticity`, `condition`, `provenance`, and `restoration` fields
   - Can now sort results by `originYear` using the `sort` query parameter
   - All new filter fields are optional

2. **POST /api/v1/me/search-filters**
   - Request body `PostUserSearchFilterData.productSearch` now accepts the new filter fields
   - Users can save search filters with these additional criteria

3. **PATCH /api/v1/me/search-filters/{userSearchFilterId}**
   - Request body `PatchUserSearchFilterData.productSearch` now accepts the new filter fields
   - Users can update existing search filters to include these criteria

#### Search Behavior

- All new filter fields work as conjunctive (AND) filters when specified
- When array filter fields (authenticity, condition, provenance, restoration) contain multiple values, products matching ANY of the specified values will be returned (disjunctive OR within each field)
- Empty arrays or null values for these fields will not apply any filtering for that criterion
- The `originYear` range filter includes products whose origin year falls within the specified min/max bounds (inclusive)

**Example Request**:
```json
POST /api/v1/products/search
{
  "language": "en",
  "currency": "USD",
  "productQuery": "antique vase",
  "originYear": {
    "min": 1800,
    "max": 1900
  },
  "authenticity": ["ORIGINAL"],
  "condition": ["EXCELLENT", "GREAT"],
  "provenance": ["COMPLETE", "PARTIAL"]
}
```

This request will search for:
- Products matching "antique vase"
- With origin year between 1800-1900
- That are verified ORIGINAL
- In EXCELLENT or GREAT condition
- With COMPLETE or PARTIAL provenance

## 2026-01-01 - Product Attribute Enrichment

This update introduces comprehensive attribute enrichment for antique products, adding fields for origin year, authenticity, condition, provenance, and restoration status.

### Added

#### New Product Attribute Fields

All endpoints that return `GetProductData` now include the following optional fields with detailed antique product attributes:

**New Fields in `GetProductData`**:

1. **Origin Year Fields** - Temporal information about when the antique was created:
   - `originYearMin` (integer, optional): Lower bound of estimated origin year range
   - `originYear` (integer, optional): Exact year of origin (when known precisely)
   - `originYearMax` (integer, optional): Upper bound of estimated origin year range
   
   **Rules**:
   - When `originYear` is present, both `originYearMin` and `originYearMax` will be `null`
   - When the year is expressed as a range, `originYear` will be `null` and min/max will contain the range bounds
   - Either min or max can be present alone to indicate "after this year" or "before this year"
   - All year fields are nullable and optional

   **Examples**:
   ```json
   // Exact year known
   {
     "originYear": 1837,
     "originYearMin": null,
     "originYearMax": null
   }
   
   // Year range estimated
   {
     "originYear": null,
     "originYearMin": 1900,
     "originYearMax": 1950
   }
   
   // Only upper bound known (before this year)
   {
     "originYear": null,
     "originYearMin": null,
     "originYearMax": 1800
   }
   
   // Only lower bound known (after this year)
   {
     "originYear": null,
     "originYearMin": 1700,
     "originYearMax": null
   }
   
   // No origin year information
   {
     "originYear": null,
     "originYearMin": null,
     "originYearMax": null
   }
   ```

2. **authenticity** (AuthenticityData, optional): Authenticity classification
   - Type: Enum string
   - Values: `ORIGINAL`, `LATER_COPY`, `REPRODUCTION`, `QUESTIONABLE`, `UNKNOWN`
   - Default: `UNKNOWN`
   - Nullable: Yes

3. **condition** (ConditionData, optional): Physical condition assessment
   - Type: Enum string
   - Values: `EXCELLENT`, `GREAT`, `GOOD`, `FAIR`, `POOR`, `UNKNOWN`
   - Default: `UNKNOWN`
   - Nullable: Yes

4. **provenance** (ProvenanceData, optional): Documentation trail and ownership history
   - Type: Enum string
   - Values: `COMPLETE`, `PARTIAL`, `CLAIMED`, `NONE`, `UNKNOWN`
   - Default: `UNKNOWN`
   - Nullable: Yes

5. **restoration** (RestorationData, optional): Level of restoration work performed
   - Type: Enum string
   - Values: `NONE`, `MINOR`, `MAJOR`, `UNKNOWN`
   - Default: `UNKNOWN`
   - Nullable: Yes

#### New Data Type Enums

**AuthenticityData**
- Authenticity classification of antique products
- Values and meanings:
  - `ORIGINAL`: Verified original antique from the stated period
  - `LATER_COPY`: Antique copy made at a later time but still historical
  - `REPRODUCTION`: Modern reproduction or replica
  - `QUESTIONABLE`: Authenticity is disputed or uncertain
  - `UNKNOWN`: Authenticity has not been determined (default)

**ConditionData**
- Physical condition assessment of antique products
- Values and meanings:
  - `EXCELLENT`: Near-perfect condition with minimal wear
  - `GREAT`: Very good condition with minor signs of age
  - `GOOD`: Good condition with moderate wear consistent with age
  - `FAIR`: Fair condition with significant wear but structurally sound
  - `POOR`: Poor condition with major damage or deterioration
  - `UNKNOWN`: Condition has not been assessed (default)

**ProvenanceData**
- Documentation trail and ownership history
- Values and meanings:
  - `COMPLETE`: Full documented history from origin to present
  - `PARTIAL`: Some documentation exists but history has gaps
  - `CLAIMED`: Provenance is claimed by seller but lacks documentation
  - `NONE`: No provenance documentation available
  - `UNKNOWN`: Provenance status has not been determined (default)

**RestorationData**
- Level of restoration work performed
- Values and meanings:
  - `NONE`: No restoration, original condition preserved
  - `MINOR`: Minor restoration or conservation work (cleaning, small repairs)
  - `MAJOR`: Significant restoration or reconstruction work
  - `UNKNOWN`: Restoration history has not been determined (default)

### Changed

#### Affected Endpoints

All endpoints returning `GetProductData` (either directly or wrapped in `PersonalizedGetProductData`) now include these new optional fields:

1. **GET /api/v1/products/{shopId}/{shopsProductId}**
   - Response body now includes origin year and attribute fields
   - All new fields are optional and may be `null`

2. **GET /api/v1/products/{shopId}/{shopsProductId}/similar**
   - Each similar product in the response array includes the new attribute fields

3. **POST /api/v1/products/search**
   - Search results now include attribute information for each product

4. **GET /api/v1/me/watchlist**
   - Watchlist products now display enriched attribute information

### Example Product Responses

**Complete Example with All New Fields**:
```json
{
  "item": {
    "productId": "550e8400-e29b-41d4-a716-446655440000",
    "eventId": "550e8400-e29b-41d4-a716-446655440001",
    "shopId": "550e8400-e29b-41d4-a716-446655440000",
    "shopsProductId": "chopin-etudes-1833",
    "shopName": "Historic Manuscripts Ltd",
    "title": {
      "text": "Frédéric Chopin - Études Op. 10 - First Edition 1833",
      "language": "en"
    },
    "description": {
      "text": "Rare first edition of Chopin's groundbreaking Études Op. 10...",
      "language": "en"
    },
    "price": {
      "currency": "EUR",
      "amount": 450000
    },
    "state": "AVAILABLE",
    "url": "https://historic-manuscripts.com/chopin-etudes-op10-1833",
    "images": [
      "https://historic-manuscripts.com/images/chopin-1.jpg"
    ],
    "originYear": 1833,
    "originYearMin": null,
    "originYearMax": null,
    "authenticity": "ORIGINAL",
    "condition": "EXCELLENT",
    "provenance": "COMPLETE",
    "restoration": "MINOR",
    "created": "2024-01-01T10:00:00Z",
    "updated": "2024-01-01T12:00:00Z"
  },
  "userState": {
    "watchlist": {
      "watching": true,
      "notifications": true
    }
  }
}
```

**Example with Year Range**:
```json
{
  "productId": "660e8400-e29b-41d4-a716-446655440001",
  "shopName": "Antique Furniture Gallery",
  "title": {
    "text": "Victorian Mahogany Writing Desk",
    "language": "en"
  },
  "originYear": null,
  "originYearMin": 1837,
  "originYearMax": 1901,
  "authenticity": "QUESTIONABLE",
  "condition": "GOOD",
  "provenance": "PARTIAL",
  "restoration": "MAJOR"
}
```

**Example with Only Upper Bound**:
```json
{
  "productId": "880e8400-e29b-41d4-a716-446655440003",
  "shopName": "Ancient Artifacts Museum",
  "title": {
    "text": "Roman Bronze Lamp",
    "language": "en"
  },
  "originYear": null,
  "originYearMin": null,
  "originYearMax": 300,
  "authenticity": "ORIGINAL",
  "condition": "FAIR",
  "provenance": "PARTIAL"
}
```

**Example with Minimal Attributes**:
```json
{
  "productId": "770e8400-e29b-41d4-a716-446655440002",
  "shopName": "Modern Reproductions Inc",
  "title": {
    "text": "Replica Louis XVI Chair",
    "language": "en"
  },
  "originYear": 2020,
  "authenticity": "REPRODUCTION",
  "condition": "EXCELLENT",
  "provenance": "NONE",
  "restoration": "NONE"
}
```

### Removed

No endpoints, fields, or functionality have been removed in this update. All changes are additive and backward compatible.

## 2025-12-16 - Shop API Identifier Flexibility

This update enhances the shop API endpoints to accept both shop IDs (UUIDs) and shop domains as identifiers, providing more flexible ways to retrieve and update shop information. This change allows clients to reference shops using either their unique UUID or any of their registered domains.

### Changed

#### GET /api/v1/shops/{shopIdentifier}

The endpoint path parameter has been changed from `{shopId}` to `{shopIdentifier}` to support multiple identifier formats.

**Path Changes**:
- **Before**: `/api/v1/shops/{shopId}`
- **After**: `/api/v1/shops/{shopIdentifier}`

**Parameter Changes**:

Before (UUID only):
```
Parameter: shopId (required)
Type: string (UUID format)
Example: "550e8400-e29b-41d4-a716-446655440000"
```

After (UUID or domain):
```
Parameter: shopIdentifier (required)
Type: string
Accepts:
  - Shop ID (UUID format): "550e8400-e29b-41d4-a716-446655440000"
  - Shop domain: "tech-store-premium.com", "shop.example.com"
```

**Description Updated**:
The endpoint now explicitly documents that it accepts both:
1. **Shop ID (UUID)**: The unique identifier assigned to the shop when created
2. **Shop Domain**: Any domain registered to the shop (e.g., "tech-store.com", "example.shop.com")

**Examples**:

Using shop ID:
```http
GET /api/v1/shops/550e8400-e29b-41d4-a716-446655440000
```

Using shop domain:
```http
GET /api/v1/shops/tech-store-premium.com
```

**Error Response Changes**:

**400 Bad Request** error examples updated:

Before:
- `BAD_PATH_PARAMETER_VALUE`: "Missing field 'shopId'"
- `INVALID_UUID`: "Invalid UUID format" (for field "shopId")

After:
- `BAD_PATH_PARAMETER_VALUE`: "Missing field 'shopIdentifier'"
- `INVALID_SHOP_IDENTIFIER`: "Invalid shop identifier format" (for field "shopIdentifier")

**Migration Impact**:
- Frontend must update API calls from `/api/v1/shops/{shopId}` to `/api/v1/shops/{shopIdentifier}`
- Both UUID and domain string formats are now accepted
- Domain format provides a more intuitive way to reference shops
- Existing UUID-based calls continue to work with the new parameter name

#### PATCH /api/v1/shops/{shopIdentifier}

The endpoint path parameter has been changed from `{shopId}` to `{shopIdentifier}` to support multiple identifier formats.

**Path Changes**:
- **Before**: `/api/v1/shops/{shopId}`
- **After**: `/api/v1/shops/{shopIdentifier}`

**Parameter Changes**:

Before (UUID only):
```
Parameter: shopId (required)
Type: string (UUID format)
Example: "550e8400-e29b-41d4-a716-446655440000"
```

After (UUID or domain):
```
Parameter: shopIdentifier (required)
Type: string
Accepts:
  - Shop ID (UUID format): "550e8400-e29b-41d4-a716-446655440000"
  - Shop domain: "tech-store-premium.com", "shop.example.com"
```

**Description Updated**:
The endpoint now explicitly documents that it accepts both shop ID (UUID) and shop domain as the identifier.

**Examples**:

Using shop ID:
```http
PATCH /api/v1/shops/550e8400-e29b-41d4-a716-446655440000
Content-Type: application/json

{
  "name": "Updated Shop Name"
}
```

Using shop domain:
```http
PATCH /api/v1/shops/tech-store-premium.com
Content-Type: application/json

{
  "name": "Updated Shop Name"
}
```

**Error Response Changes**:

**400 Bad Request** error examples updated:

Before:
- `BAD_PATH_PARAMETER_VALUE`: "Missing field 'shopId'"
- `INVALID_UUID`: "Invalid UUID format" (for field "shopId")

After:
- `BAD_PATH_PARAMETER_VALUE`: "Missing field 'shopIdentifier'"
- `INVALID_SHOP_IDENTIFIER`: "Invalid shop identifier format" (for field "shopIdentifier")

**Migration Impact**:
- Frontend must update API calls from `/api/v1/shops/{shopId}` to `/api/v1/shops/{shopIdentifier}`
- Both UUID and domain string formats are now accepted
- Domain format allows updating a shop using any of its registered domains
- Request and response body structures remain unchanged

### Added

#### New Error Code: INVALID_SHOP_IDENTIFIER

**Error Code**: `INVALID_SHOP_IDENTIFIER`
- **HTTP Status**: 400 Bad Request
- **Endpoints**: `GET /api/v1/shops/{shopIdentifier}`, `PATCH /api/v1/shops/{shopIdentifier}`
- **When**: Provided shop identifier is neither a valid UUID nor a valid domain format
- **Description**: The shop identifier could not be parsed as either a UUID or a domain

**Error Response Example**:
```json
{
  "status": 400,
  "title": "Bad Request",
  "error": "INVALID_SHOP_IDENTIFIER",
  "source": {
    "field": "shopIdentifier",
    "sourceType": "path"
  },
  "detail": "Invalid shop identifier format"
}
```

**Examples of Invalid Shop Identifiers**:
- Empty string: `""`
- Invalid UUID: `"not-a-uuid"`
- Invalid domain: `"localhost"` (no TLD)
- Special characters: `"shop@example"`
- Malformed: `"..shop.com"`

**Frontend Impact**:
- Handle `INVALID_SHOP_IDENTIFIER` as a new 400 error type for shop endpoints
- Display appropriate error message when shop identifier is malformed
- Validate shop identifier format client-side before making requests

### Implementation Notes

**Shop Identifier Format**:
The `shopIdentifier` parameter accepts two formats:

1. **UUID Format** (Shop ID):
   - Standard UUID v4 format: `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`
   - Example: `"550e8400-e29b-41d4-a716-446655440000"`
   - Case-insensitive
   
2. **Domain Format**:
   - Valid internet domain name
   - Must contain at least one dot (TLD required)
   - Examples: 
     - `"tech-store.com"`
     - `"shop.example.com"`
     - `"subdomain.shop.co.uk"`
   - Domain matching is case-insensitive
   - Must be a domain registered to the shop

**Backend Type**: New `ShopIdentifierData` enum in Rust
- Serializes/deserializes as a plain string (untagged enum)
- Automatically determines whether input is UUID or domain
- Converted to internal `ShopIdentifier` type for service layer

**Lookup Behavior**:
- **For UUID**: Direct lookup by shop ID (primary key)
- **For domain**: Lookup via domain index to find shop ID, then retrieve shop
- Both methods return the same complete shop data
- Domain lookup may return `SHOP_NOT_FOUND` if domain is not registered

**API Gateway Route Changes**:
- Route updated in CloudFormation: `GET /api/v1/shops/{shopIdentifier}`
- Route updated in CloudFormation: `PATCH /api/v1/shops/{shopIdentifier}`
- Path parameter name changed from `shopId` to `shopIdentifier` in API Gateway configuration

### Migration Guide

For frontend developers integrating these changes:

1. **Update API Endpoint URLs**:
   ```typescript
   // Before
   const getShopUrl = (shopId: string) => 
     `https://api.aura-historia.com/api/v1/shops/${shopId}`;
   
   // After
   const getShopUrl = (shopIdentifier: string) => 
     `https://api.aura-historia.com/api/v1/shops/${shopIdentifier}`;
   ```

2. **Flexible Shop Identification**:
   ```typescript
   // Can now use either UUID or domain
   await getShop("550e8400-e29b-41d4-a716-446655440000"); // UUID
   await getShop("tech-store-premium.com");              // Domain
   ```

3. **Update Error Handling**:
   ```typescript
   try {
     const shop = await getShop(identifier);
   } catch (error) {
     if (error.code === 'INVALID_SHOP_IDENTIFIER') {
       // Handle invalid shop identifier format
       showError('Invalid shop identifier. Please provide a valid UUID or domain.');
     } else if (error.code === 'SHOP_NOT_FOUND') {
       // Shop doesn't exist or domain not registered
       showError('Shop not found.');
     }
   }
   ```

4. **Parameter Validation**:
   ```typescript
   const isValidShopIdentifier = (identifier: string): boolean => {
     // Check if it's a UUID
     const uuidRegex = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
     if (uuidRegex.test(identifier)) return true;
     
     // Check if it's a valid domain (contains at least one dot)
     const domainRegex = /^[a-z0-9]+([\-\.]{1}[a-z0-9]+)*\.[a-z]{2,}$/i;
     return domainRegex.test(identifier);
   };
   ```

5. **Update API Client Types**:
   ```typescript
   // Before
   interface GetShopParams {
     shopId: string; // UUID
   }
   
   // After
   interface GetShopParams {
     shopIdentifier: string; // UUID or domain
   }
   ```

### Benefits

**For Frontend Developers**:
- More intuitive shop references using human-readable domains
- Flexibility to use whichever identifier is available
- Simplifies deep linking and URL construction
- No need to store/retrieve shop UUID when domain is known

**For API Consumers**:
- Single endpoint supports multiple identification methods
- Backward compatible (UUIDs still work)
- Reduces need for separate domain-to-ID lookup calls
- More RESTful and user-friendly API design

**Use Cases**:
- **UUID**: Internal system references, database relations, unique identification
- **Domain**: User-facing features, shop discovery, URL construction, external integrations

### Removed

No endpoints, parameters, or functionality have been removed. This is a backward-compatible enhancement that adds domain-based identification while preserving UUID-based identification.

The error code `INVALID_UUID` is no longer returned for shop identifier validation failures. It has been replaced by the more general `INVALID_SHOP_IDENTIFIER` error code.

## 2025-12-10 - User Account Management

This update introduces comprehensive user account management capabilities, allowing authenticated users to view and update their account information including personal details, language preferences, and currency preferences.

### Added

#### GET /api/v1/me/account

Retrieves the authenticated user's account information.

**Endpoint Details**:
- **Method**: GET
- **Path**: `/api/v1/me/account`
- **Authentication**: Required (Cognito JWT)

**Response: 200 OK**

Returns complete user account data.

**Response Headers**:
- `Last-Modified` (string, HTTP date): When the user account was last updated
  - Example: `Wed, 01 Jan 2024 12:00:00 GMT`
- `Access-Control-Allow-Origin` (string): CORS header
  - Example: `*`

**Response Body**: `GetUserAccountData`

**Example Response**:
```json
{
  "userId": "550e8400-e29b-41d4-a716-446655440000",
  "email": "user@example.com",
  "firstName": "John",
  "lastName": "Doe",
  "language": "en",
  "currency": "EUR",
  "created": "2024-01-01T10:00:00Z",
  "updated": "2024-01-01T12:00:00Z"
}
```

**Error Responses**:

**401 Unauthorized**:
- `UNAUTHORIZED`: Missing or invalid JWT token

**404 Not Found**:
- `USER_NOT_FOUND`: User account does not exist

**500 Internal Server Error**:
- `INTERNAL_SERVER_ERROR`: Unexpected server error

#### PATCH /api/v1/me/account

Updates the authenticated user's account information.

**Endpoint Details**:
- **Method**: PATCH
- **Path**: `/api/v1/me/account`
- **Authentication**: Required (Cognito JWT)
- **Partial Updates**: All fields in request body are optional - only provided fields will be updated

**Request Body**: `PatchUserAccountData` (required, but all fields optional)
- `email` (string, email, optional): New email address
- `firstName` (string, optional, max 64 characters): New first name
- `lastName` (string, optional, max 64 characters): New last name
- `language` (LanguageData, optional): New preferred language
- `currency` (CurrencyData, optional): New preferred currency

**Example Requests**:

Update all fields:
```json
{
  "email": "newemail@example.com",
  "firstName": "Jane",
  "lastName": "Smith",
  "language": "de",
  "currency": "USD"
}
```

Update name only:
```json
{
  "firstName": "Jane",
  "lastName": "Smith"
}
```

Update preferences only:
```json
{
  "language": "fr",
  "currency": "GBP"
}
```

**Response: 200 OK**

Returns the updated user account data.

**Response Headers**:
- `Last-Modified` (string, HTTP date): When the user account was last updated
  - Example: `Wed, 01 Jan 2024 12:30:00 GMT`
- `Access-Control-Allow-Origin` (string): CORS header
  - Example: `*`

**Response Body**: `GetUserAccountData`

**Example Response**:
```json
{
  "userId": "550e8400-e29b-41d4-a716-446655440000",
  "email": "newemail@example.com",
  "firstName": "Jane",
  "lastName": "Smith",
  "language": "de",
  "currency": "USD",
  "created": "2024-01-01T10:00:00Z",
  "updated": "2024-01-01T12:30:00Z"
}
```

**Error Responses**:

**400 Bad Request**:
- `BAD_BODY_VALUE`: Missing or invalid request body
  - Examples: Empty body, malformed JSON

**401 Unauthorized**:
- `UNAUTHORIZED`: Missing or invalid JWT token

**404 Not Found**:
- `USER_NOT_FOUND`: User account does not exist

**500 Internal Server Error**:
- `INTERNAL_SERVER_ERROR`: Unexpected server error

#### New Data Types

**GetUserAccountData**
- Complete user account information
- Properties:
  - `userId` (string, UUID, required): Unique identifier for the user
  - `email` (string, email, required): User's email address
  - `firstName` (string, optional, max 64 characters): User's first name
  - `lastName` (string, optional, max 64 characters): User's last name
  - `language` (LanguageData, optional): User's preferred language (de, en, fr, es)
  - `currency` (CurrencyData, optional): User's preferred currency (EUR, GBP, USD, AUD, CAD, NZD)
  - `created` (string, date-time, required): When the user account was created (RFC3339)
  - `updated` (string, date-time, required): When the user account was last updated (RFC3339)

**PatchUserAccountData**
- Request body schema for updating user account
- All fields are optional - partial updates supported
- Properties:
  - `email` (string, email, optional): New email address
  - `firstName` (string, optional, max 64 characters): New first name
  - `lastName` (string, optional, max 64 characters): New last name
  - `language` (LanguageData, optional): New preferred language
  - `currency` (CurrencyData, optional): New preferred currency

#### New Error Codes

**USER_EXISTS_ALREADY** (409 Conflict)
- Returned when attempting to create a user that already exists
- Indicates the user ID is already in use in the system

**USER_NOT_FOUND** (404 Not Found)
- Returned when attempting to retrieve or update a user account that does not exist
- Applies to both GET and PATCH operations on `/api/v1/me/account`

### Implementation Notes

**User Account Structure**:
- User accounts are stored in DynamoDB with partition key format: `user#{userId}`
- The `userId` is derived from Cognito's JWT token `sub` claim
- All fields except `userId`, `email`, `created`, and `updated` are optional
- Field truncation: `firstName` and `lastName` are automatically truncated to 64 characters if longer
- Default values: When a user is created via Cognito sign-up, optional fields start as null

**Update Behavior**:
- PATCH operations support partial updates - only provided fields are modified
- Empty request body returns 400 error
- The `userId`, `created`, and `email` (when not provided in update) remain unchanged
- `updated` timestamp is set to current time on any modification
- Updates are performed using DynamoDB UpdateItem for atomicity

**Authentication**:
- Both endpoints require Cognito JWT authentication via `Authorization: Bearer <token>` header
- The `userId` is extracted from the JWT token's `sub` claim
- Invalid or missing tokens return 401 Unauthorized

**Field Constraints**:
- Email: Must be valid email format
- First Name: Optional, max 64 characters, auto-truncated if longer
- Last Name: Optional, max 64 characters, auto-truncated if longer
- Language: Must be one of: de, en, fr, es
- Currency: Must be one of: EUR, GBP, USD, AUD, CAD, NZD

### Migration Guide

For frontend developers integrating these changes:

1. **Retrieve User Account**:
   ```typescript
   const getUserAccount = async (accessToken: string): Promise<GetUserAccountData> => {
     const response = await fetch('https://api.aura-historia.com/api/v1/me/account', {
       method: 'GET',
       headers: {
         'Authorization': `Bearer ${accessToken}`,
       },
     });
     
     if (!response.ok) {
       throw new Error(`Failed to get user account: ${response.status}`);
     }
     
     return response.json();
   };
   ```

2. **Update User Account**:
   ```typescript
   const updateUserAccount = async (
     accessToken: string,
     updates: PatchUserAccountData
   ): Promise<GetUserAccountData> => {
     const response = await fetch('https://api.aura-historia.com/api/v1/me/account', {
       method: 'PATCH',
       headers: {
         'Authorization': `Bearer ${accessToken}`,
         'Content-Type': 'application/json',
       },
       body: JSON.stringify(updates),
     });
     
     if (!response.ok) {
       throw new Error(`Failed to update user account: ${response.status}`);
     }
     
     return response.json();
   };
   ```

3. **Handle Optional Fields**:
   ```typescript
   // Check if optional fields are present
   const displayName = (user: GetUserAccountData): string => {
     if (user.firstName && user.lastName) {
       return `${user.firstName} ${user.lastName}`;
     }
     if (user.firstName) {
       return user.firstName;
     }
     if (user.lastName) {
       return user.lastName;
     }
     return user.email;
   };
   ```

4. **Error Handling**:
   - Handle 401 errors by redirecting to login
   - Handle 404 errors by creating/initializing user profile
   - Handle 400 errors by validating input before sending

### Removed

No endpoints or schemas were removed in this update.

## 2025-12-08 - Shop Domain Normalization

This update changes how shop identifiers are handled in the API. Instead of storing and returning full URLs for shops, the system now uses normalized domains. This improves consistency, reduces storage overhead, and simplifies shop matching logic when enriching product data.

### Changed

#### Shop Data Structure - URLs Replaced with Domains

All shop-related data types now use `domains` instead of `urls`. Domains are normalized strings (lowercase, no scheme, no www prefix, no path/query/fragment) extracted from URLs.

**Affected Data Types**:
- `GetShopData`
- `PostShopData`
- `PatchShopData`

**Migration Guide**:

**Before (using URLs)**:
```json
{
  "shopId": "550e8400-e29b-41d4-a716-446655440000",
  "name": "Tech Store Premium",
  "urls": [
    "https://tech-store-premium.com",
    "https://tech-store-premium.de",
    "https://apple.tech-store-premium.com"
  ]
}
```

**After (using domains)**:
```json
{
  "shopId": "550e8400-e29b-41d4-a716-446655440000",
  "name": "Tech Store Premium",
  "domains": [
    "tech-store-premium.com",
    "tech-store-premium.de",
    "apple.tech-store-premium.com"
  ]
}
```

**Key Differences**:
- Field name changed from `urls` to `domains`
- Values are now plain domain strings, not full URLs
- Domains are normalized: lowercase, no scheme (`https://`), no `www.` prefix
- Paths, query parameters, and fragments are stripped
- Port numbers are stripped (e.g., `:8080`)

**Domain Normalization Examples**:
- `https://Tech-Store.COM/products` → `tech-store.com`
- `http://www.example.com/?ref=123` → `example.com`
- `https://shop.example.com/path#anchor` → `shop.example.com`

**Frontend Impact**:
- When creating/updating shops: You can send either full URLs or domain strings - both will be normalized
- When receiving shop data: Expect domain strings, not URLs
- For display: Add `https://` prefix if you need to show or link to the shop
- For comparison: Use exact string matching on normalized domains

**API Endpoints Affected**:
- `POST /api/v1/shops` - Request and response
- `PATCH /api/v1/shops/{shopId}` - Request and response
- `GET /api/v1/shops/{shopId}` - Response
- All shop search endpoints - Response

#### Error Code Changes

**Renamed Error Code**: `SHOP_TOO_MANY_URLS` → `SHOP_TOO_MANY_DOMAINS`
- **Endpoints**: `POST /api/v1/shops`, `PATCH /api/v1/shops/{shopId}`
- **HTTP Status**: 400 Bad Request
- **When**: More than 100 domains provided in request
- **Example Detail**: "Shop can only have 100 domains but was given more: '142'"

**Updated Error Messages**:
- `SHOP_EXISTS_ALREADY` detail message updated to reference domains instead of URLs
  - Old: "...an URL for any domain of shop is already registered"
  - New: "...a domain of shop is already registered"

### Added

#### New Error Code for Product Enrichment

**Error Code**: `NO_DOMAIN`
- **Endpoint**: `PUT /api/v1/products`
- **Type**: Product enrichment failure
- **When**: Product URL does not contain a valid extractable domain
- **Description**: The product URL could not be parsed to extract a domain for shop matching
- **Examples of URLs that fail**:
  - `https://localhost` (no TLD, just hostname)
  - `https://127.0.0.1` (IP address without domain)
  - `http://localhost:8080/product` (localhost with port)

**Response Example**:
```json
{
  "failed": {
    "https://localhost:8080/item": "NO_DOMAIN"
  },
  "skipped": 0
}
```

**Frontend Impact**:
- Handle `NO_DOMAIN` as a new error type in product upload responses
- These products cannot be processed and will need valid domain-based URLs
- Display appropriate error message to users

#### Enhanced Product Enrichment Documentation

The `PUT /api/v1/products` endpoint description now explicitly documents:
- Domain extraction from product URLs
- Shop matching based on extracted domains
- Two possible failures: `SHOP_NOT_FOUND` (domain not registered) and `NO_DOMAIN` (invalid URL)

### Technical Details

**Domain Extraction Logic**:
1. Remove `http://` or `https://` scheme
2. Remove `www.` prefix
3. Extract hostname (everything before `:`, `/`, `?`, or `#`)
4. Convert to lowercase
5. Validate: must contain at least one `.` (TLD required)

**Backend Type**: New `Domain` type in Rust
- Serializes/deserializes as a plain string (not URL)
- Implements validation and normalization on creation
- Used in shop records and shop-product matching logic

**Database Changes**:
- DynamoDB partition keys updated from `shop#url#{url}` to `shop#domain#{domain}`
- Shop records now indexed by normalized domain instead of full URL
- Product enrichment queries shops by domain extracted from product URL

## 2025-12-06 - Shop Management Endpoints

This update introduces comprehensive shop management capabilities, allowing creation and modification of shop entities in the system. Previously, shops could only be retrieved or searched - now they can be fully managed through the API.

### Added

#### POST /api/v1/shops

Creates a new shop in the system with the provided details.

**Endpoint Details**:
- **Method**: POST
- **Path**: `/api/v1/shops`
- **Authentication**: None (public endpoint)

**Request Body**: `PostShopData` (required)
- `name` (string, required, max 255 characters): Display name of the shop
- `domains` (array of strings, required, min 1, max 100): All domains associated with the shop
  - Can be provided as full URLs (will be normalized) or as domain strings
  - Domains are normalized to lowercase without scheme, www prefix, or path/query/fragment
  - At least one domain is required
  - Maximum 100 domains allowed per shop
- `image` (string, URI, optional): URL to the shop's logo or image

**Example Request**:
```json
{
  "name": "Tech Store Premium",
  "domains": [
    "tech-store-premium.com",
    "tech-store-premium.de",
    "apple.tech-store-premium.com"
  ],
  "image": "https://tech-store-premium.com/logo.svg"
}
```

**Response: 201 Created**

Returns the created shop with generated ID and timestamps.

**Response Headers**:
- `Location` (string, URI): URL of the created shop resource
  - Example: `https://api.aura-historia.com/api/v1/shops/550e8400-e29b-41d4-a716-446655440000`
- `Last-Modified` (string, HTTP date): When the shop was created
  - Example: `Wed, 01 Jan 2024 12:00:00 GMT`

**Response Body**: `GetShopData`

**Example Response**:
```json
{
  "shopId": "550e8400-e29b-41d4-a716-446655440000",
  "name": "Tech Store Premium",
  "domains": [
    "tech-store-premium.com",
    "tech-store-premium.de",
    "apple.tech-store-premium.com"
  ],
  "image": "https://tech-store-premium.com/logo.svg",
  "created": "2024-01-01T12:00:00Z",
  "updated": "2024-01-01T12:00:00Z"
}
```

**Error Responses**:

**400 Bad Request**:
- `BAD_BODY_VALUE`: Missing or invalid request body
  - Examples: Empty body, malformed JSON, missing required fields
- `SHOP_TOO_MANY_DOMAINS`: More than 100 domains provided
  - Example detail: "Shop can only have 100 domains but was given more: '142'"

**409 Conflict**:
- `SHOP_EXISTS_ALREADY`: A shop with one of the provided domains already exists
  - Example detail: "Shop with name 'Tech Store Premium' exists already - a domain of shop is already registered"

**500 Internal Server Error**:
- `INTERNAL_SERVER_ERROR`: Unexpected server error

**503 Service Unavailable**:
- `UNPROCESSED_ITEMS`: Temporary database issue prevented completion
  - Example detail: "Did not succeed checking existence of shop due to DynamoDB Batch-Response containing unprocessed items"

#### PATCH /api/v1/shops/{shopId}

Updates an existing shop's information by its unique identifier.

**Endpoint Details**:
- **Method**: PATCH
- **Path**: `/api/v1/shops/{shopId}`
- **Authentication**: None (public endpoint)
- **Partial Updates**: All fields in request body are optional - only provided fields will be updated
- **No-op Behavior**: If request body is empty or only contains null values, shop is returned unchanged

**Path Parameters**:
- `shopId` (string, UUID, required): Unique identifier of the shop to update

**Request Body**: `PatchShopData` (required, but all fields optional)
- `name` (string, optional, max 255 characters): New display name for the shop
- `domains` (array of strings, optional, min 1, max 100): Complete new set of domains
  - Can be provided as full URLs (will be normalized) or as domain strings
  - **Important**: When updating domains, the complete new set must be provided (not a diff)
  - Old domains not in the new set will be removed
  - New domains in the set will be added
- `image` (string, URI, optional): New URL to the shop's logo or image

**Example Requests**:

Update only the name:
```json
{
  "name": "Tech Store Premium Plus"
}
```

Update domains (complete replacement):
```json
{
  "domains": [
    "tech-store-premium.com",
    "tech-store-premium.eu"
  ]
}
```

Update multiple fields:
```json
{
  "name": "Tech Store Premium Plus",
  "domains": [
    "tech-store-premium.com",
    "tech-store-premium.eu"
  ],
  "image": "https://tech-store-premium.com/new-logo.svg"
}
```

**Response: 200 OK**

Returns the updated shop with modified fields and new `updated` timestamp.

**Response Headers**:
- `Last-Modified` (string, HTTP date): When the shop was last updated
  - Example: `Wed, 01 Jan 2024 12:30:00 GMT`

**Response Body**: `GetShopData`

**Example Response**:
```json
{
  "shopId": "550e8400-e29b-41d4-a716-446655440000",
  "name": "Tech Store Premium Plus",
  "domains": [
    "tech-store-premium.com",
    "tech-store-premium.eu"
  ],
  "image": "https://tech-store-premium.com/new-logo.svg",
  "created": "2024-01-01T10:00:00Z",
  "updated": "2024-01-01T12:30:00Z"
}
```

**Error Responses**:

**400 Bad Request**:
- `BAD_PATH_PARAMETER_VALUE`: Missing shop ID in path
- `INVALID_UUID`: Invalid UUID format for shop ID
- `BAD_BODY_VALUE`: Missing or invalid request body
- `SHOP_TOO_MANY_DOMAINS`: More than 100 domains provided

**404 Not Found**:
- `SHOP_NOT_FOUND`: Shop with the given ID does not exist

**500 Internal Server Error**:
- `INTERNAL_SERVER_ERROR`: Unexpected server error

**503 Service Unavailable**:
- `UNPROCESSED_ITEMS`: Temporary database issue prevented completion

#### New Data Types

**PostShopData**
- Request body schema for creating a new shop
- All fields that will be stored for the shop except generated fields (shopId, created, updated)
- Properties:
  - `name` (string, required, max 255 characters): Shop display name
  - `domains` (array of strings, required, min 1, max 100): All shop domains (normalized)
  - `image` (string, URI, optional): Shop logo or image URL

**PatchShopData**
- Request body schema for updating an existing shop
- All fields are optional - partial updates supported
- Properties:
  - `name` (string, optional, max 255 characters): New shop name
  - `domains` (array of strings, optional, min 1, max 100): Complete new set of domains (normalized)
  - `image` (string, URI, optional): New shop logo or image URL

#### New Error Codes

**SHOP_EXISTS_ALREADY** (409 Conflict)
- Returned when attempting to create a shop with a domain that is already registered to another shop
- Ensures domain uniqueness across all shops in the system
- Error message includes the shop name that was attempted to be created
- Example: "Shop with name 'Tech Store Premium' exists already - a domain of shop is already registered"

**SHOP_TOO_MANY_DOMAINS** (400 Bad Request)
- Returned when request contains more than 100 domains
- Applies to both POST (create) and PATCH (update) operations
- Error message includes the actual number of domains provided
- Example: "Shop can only have 100 domains but was given more: '142'"

**UNPROCESSED_ITEMS** (503 Service Unavailable)
- Returned when database batch operations cannot complete due to unprocessed items
- Temporary error that may resolve on retry
- Indicates the system is temporarily unable to process the request due to database limitations
- Example detail: "Did not succeed checking existence of shop due to DynamoDB Batch-Response containing unprocessed items"

### Changed

#### GetShopData Schema

The `urls` field has been enhanced with validation constraints:

**Before**:
```yaml
urls:
  type: array
  items:
    type: string
    format: uri
  description: All known URLs to the shop's website
  default: []
```

**After**:
```yaml
urls:
  type: array
  items:
    type: string
    format: uri
  description: All known URLs to the shop's website
  minItems: 1
  maxItems: 100
```

**Changes**:
- **Removed** `default: []` - shops must always have at least one URL
- **Added** `minItems: 1` - enforces that shops have at least one URL
- **Added** `maxItems: 100` - enforces maximum of 100 URLs per shop

**Migration Impact**:
- No impact on existing API consumers - this is a documentation clarification of existing backend constraints
- Frontend validation should ensure at least 1 URL and maximum 100 URLs when creating or updating shops

The `name` field also received a constraint:
- **Added** `maxLength: 255` - shop names are limited to 255 characters

**Migration Impact**:
- No impact on existing API consumers - this is a documentation clarification
- Frontend validation should limit shop name input to 255 characters

### Implementation Notes

**Shop URL Management**:
- Each shop URL creates a separate lookup record in DynamoDB
- URLs are used for automatic shop identification during product ingestion
- When updating URLs via PATCH, old URL records are deleted and new ones are created
- All URL records for a shop maintain consistency with the shop's core data

**Uniqueness Constraints**:
- Shop URLs must be globally unique across all shops
- A URL can only be associated with one shop at a time
- Creating or updating a shop with a URL that exists for another shop returns `SHOP_EXISTS_ALREADY` error

**Update Behavior**:
- PATCH operations support partial updates - only provided fields are modified
- Empty request body or all-null fields result in no changes (200 OK with unchanged shop)
- URL updates replace the entire URL set (not a merge)
- `created` timestamp never changes
- `updated` timestamp is set to current time on any modification

**Performance Characteristics**:
- Shop creation performs existence check for all provided URLs before creating
- Up to 101 DynamoDB operations (1 shop record + up to 100 URL records)
- All operations are transactional - either all succeed or all fail
- Update operations may delete old URL records and create new ones

### Removed

No endpoints or schemas were removed in this update.

## 2025-11-23 - Item-Product-Renaming & ApiError problem+json

### Changed

- Renamed all item-related types to product-version
- Adjusted all error-responses with response-payload `ApiError` to return `application/problem+json` (RFC 9457) instead of `application/json`
  - Renamed field `message` to `detail` (human-readable for devs - not user-facing)
  - Added field `title` (human-readable for devs - not user-facing)

## 2025-11-10 - Similar Items Endpoint with Semantic Search

This update introduces a new endpoint for finding semantically similar items using k-nearest neighbors (k-NN) search on text embeddings. The endpoint leverages machine learning embeddings to provide relevant item recommendations based on content similarity.

### Added

#### GET /api/v1/items/{shopId}/{shopsItemId}/similar

Retrieves items similar to the specified item using semantic search based on text embeddings.

**Endpoint Details**:
- **Method**: GET
- **Path**: `/api/v1/items/{shopId}/{shopsItemId}/similar`
- **Authentication**: Optional (Cognito JWT Bearer token)
- **Supported Languages**: de, en, fr, es (via Accept-Language header)
- **Supported Currencies**: EUR, USD, GBP, AUD, CAD, NZD (via currency query parameter)

**How It Works**:
- Uses k-nearest neighbors (k-NN) search on text embeddings to find semantically similar items
- Text embeddings are 1024-dimensional vectors computed from item titles and descriptions using the baai/bge-m3 model
- Embeddings are generated nightly via batch processing for all items
- Returns up to 20 similar items, ranked by similarity score
- The original item is automatically excluded from results

**Path Parameters**:
- `shopId` (string, uuid, required): Unique identifier of the shop
- `shopsItemId` (string, required): Shop's unique identifier for the item

**Query Parameters**:
- `currency` (CurrencyData, optional): Currency for price display (default: EUR)
  - Accepted values: `EUR`, `USD`, `GBP`, `AUD`, `CAD`, `NZD`

**Request Headers**:
- `Accept-Language` (optional): Preferred language for localized content
  - Format: Language code with optional quality values
  - Examples: `de`, `en;q=0.9,de;q=0.8`, `en-US`
- `Authorization` (optional): Cognito JWT token for personalized response
  - Format: `Bearer [JWT token]`
  - When provided: Response includes user-specific state for each similar item
  - When omitted: Response contains only item data without user state

**Response: 200 OK**

Returns an array of similar items, each following the PersonalizedGetItemData schema.

**Schema**: Array of `PersonalizedGetItemData`

Each item in the array contains:
- `item` (GetItemData, required): Complete item information
  - `itemId` (string, uuid, required): Unique item identifier
  - `eventId` (string, uuid, required): Latest event identifier
  - `shopId` (string, uuid, required): Shop identifier
  - `shopsItemId` (string, required): Shop's item identifier
  - `shopName` (string, required): Shop name
  - `title` (LocalizedTextData, required): Localized title
  - `description` (LocalizedTextData, optional): Localized description
  - `price` (PriceData, optional): Price in requested currency
  - `state` (ItemStateData, required): Item state
  - `url` (string, uri, required): Item URL
  - `images` (array of strings, required): Image URLs
  - `created` (string, date-time, required): Creation timestamp
  - `updated` (string, date-time, required): Last update timestamp
- `userState` (ItemUserStateData, optional): User-specific state (only when authenticated)
  - `watchlist` (WatchlistUserStateData, required): Watchlist state
    - `watching` (boolean, required): Whether item is on user's watchlist
    - `notifications` (boolean, required): Whether notifications are enabled

**Example Response (Authenticated User)**:
```json
[
  {
    "item": {
      "itemId": "660e8400-e29b-41d4-a716-446655440001",
      "eventId": "660e8400-e29b-41d4-a716-446655440011",
      "shopId": "550e8400-e29b-41d4-a716-446655440000",
      "shopsItemId": "6ba7b811",
      "shopName": "My Shop",
      "title": {
        "text": "Similar Product",
        "language": "en"
      },
      "description": {
        "text": "This product has similar features",
        "language": "en"
      },
      "price": {
        "currency": "EUR",
        "amount": 2899
      },
      "state": "AVAILABLE",
      "url": "https://my-shop.com/products/similar-product",
      "images": [
        "https://my-shop.com/images/similar-1.jpg"
      ],
      "created": "2024-01-01T10:00:00Z",
      "updated": "2024-01-01T12:00:00Z"
    },
    "userState": {
      "watchlist": {
        "watching": true,
        "notifications": false
      }
    }
  },
  {
    "item": {
      "itemId": "770e8400-e29b-41d4-a716-446655440002",
      "eventId": "770e8400-e29b-41d4-a716-446655440012",
      "shopId": "550e8400-e29b-41d4-a716-446655440000",
      "shopsItemId": "6ba7b812",
      "shopName": "My Shop",
      "title": {
        "text": "Another Similar Item",
        "language": "en"
      },
      "price": {
        "currency": "EUR",
        "amount": 3199
      },
      "state": "AVAILABLE",
      "url": "https://my-shop.com/products/another-item",
      "images": [],
      "created": "2024-01-01T11:00:00Z",
      "updated": "2024-01-01T13:00:00Z"
    },
    "userState": {
      "watchlist": {
        "watching": false,
        "notifications": false
      }
    }
  }
]
```

**Example Response (Anonymous User)**:
```json
[
  {
    "item": {
      "itemId": "660e8400-e29b-41d4-a716-446655440001",
      "eventId": "660e8400-e29b-41d4-a716-446655440011",
      "shopId": "550e8400-e29b-41d4-a716-446655440000",
      "shopsItemId": "6ba7b811",
      "shopName": "My Shop",
      "title": {
        "text": "Similar Product",
        "language": "en"
      },
      "price": {
        "currency": "EUR",
        "amount": 2899
      },
      "state": "AVAILABLE",
      "url": "https://my-shop.com/products/similar-product",
      "images": [
        "https://my-shop.com/images/similar-1.jpg"
      ],
      "created": "2024-01-01T10:00:00Z",
      "updated": "2024-01-01T12:00:00Z"
    }
  }
]
```

**Response: 202 Accepted**

Returned when the item's text embedding has not yet been computed (typically for items less than 24 hours old).
Embeddings are generated during nightly batch processing.

**Headers**:
- `Location` (string, uri): URL to poll for similar items once embeddings are computed
  - Example: `https://api.aura-historia.com/api/v1/items/550e8400-e29b-41d4-a716-446655440000/6ba7b810/similar`

**Response Body**: None

**When This Occurs**:
- Item was created less than 24 hours ago and hasn't been processed by nightly embedding generation
- Item's text embedding is missing from the OpenSearch index
- Embedding generation is scheduled but not yet completed

**Migration Impact**:
- Frontend should implement polling logic when receiving 202 response
- Use exponential backoff to avoid excessive requests
- Check again after 24 hours or on next user visit
- Display appropriate message to users (e.g., "Similar items will be available soon")

**Response: 400 Bad Request**

Returned when request parameters are invalid.

**Error Codes**:
- `BAD_PATH_PARAMETER_VALUE`: Missing or invalid path parameter
- `INVALID_UUID`: Invalid UUID format for shopId
- `BAD_QUERY_PARAMETER_VALUE`: Invalid query parameter value
- `INVALID_CURRENCY`: Invalid currency code

**Example**:
```json
{
  "status": 400,
  "error": "BAD_PATH_PARAMETER_VALUE",
  "source": {
    "field": "shopId",
    "sourceType": "path"
  }
}
```

**Response: 404 Not Found**

Returned when the specified item does not exist.

**Error Code**: `ITEM_NOT_FOUND`

**Example**:
```json
{
  "status": 404,
  "error": "ITEM_NOT_FOUND",
  "message": "Item with ShopId '550e8400-e29b-41d4-a716-446655440000' and ShopsItemId '6ba7b810' not found."
}
```

**Response: 500 Internal Server Error**

Returned when an unexpected server error occurs.

**Error Code**: `INTERNAL_SERVER_ERROR`

**Technical Details**:

The similar items feature is powered by:
- **Embedding Model**: baai/bge-m3 (1024-dimensional vectors)
- **Search Method**: k-NN (k-nearest neighbors) using cosine similarity
- **Index**: OpenSearch with KNN plugin
- **Field Name**: `textEmbedding` in the items index
- **Batch Processing**: Nightly enrichment pipeline generates embeddings for items without them
- **Maximum Results**: 20 similar items per request
- **Exclusion**: Original item is automatically filtered from results

**Use Cases**:
- Product recommendations on item detail pages
- "You might also like" sections
- Discovery of related items across different shops
- Content-based filtering for personalization

**Performance Characteristics**:
- Fast k-NN search using approximate nearest neighbors (ANN)
- Sub-second response times for most queries
- Scalable to millions of items
- Real-time personalization when authenticated

**Limitations**:
- Requires items to have been processed by nightly embedding generation (24-hour delay for new items)
- Similarity is based purely on text content (title + description)
- Does not consider user behavior, purchase history, or collaborative filtering
- Limited to 20 results per request
- Results are not paginated (returns all similar items up to limit)

### Changed

No existing endpoints or schemas were modified in this update.

### Removed

No endpoints or schemas were removed in this update.

## 2025-11-07 - API Personalization and User Resource Path Restructuring

This update introduces personalized API responses for item endpoints and reorganizes user-specific resources under a `/api/v1/me/` prefix for better API structure and clarity.

### Added

#### Personalized Item Responses

Item retrieval and search endpoints now support optional authentication, returning personalized data that includes user-specific state when a valid JWT token is provided.

**New Schema: `PersonalizedGetItemData`**
- Wrapper for item data with optional user-specific state
- Properties:
  - `item` (GetItemData, required): Complete item information
  - `userState` (ItemUserStateData, optional): User-specific state (only present when authenticated)

**Example Response (Authenticated User)**:
```json
{
  "item": {
    "itemId": "550e8400-e29b-41d4-a716-446655440000",
    "eventId": "550e8400-e29b-41d4-a716-446655440001",
    "shopId": "550e8400-e29b-41d4-a716-446655440000",
    "shopsItemId": "6ba7b810",
    "shopName": "My Shop",
    "title": {
      "text": "Amazing Product",
      "language": "en"
    },
    "state": "AVAILABLE",
    "url": "https://my-shop.com/products/amazing-product",
    "created": "2024-01-01T10:00:00Z",
    "updated": "2024-01-01T12:00:00Z"
  },
  "userState": {
    "watchlist": {
      "watching": true,
      "notifications": false
    }
  }
}
```

**Example Response (Anonymous User)**:
```json
{
  "item": {
    "itemId": "550e8400-e29b-41d4-a716-446655440000",
    "eventId": "550e8400-e29b-41d4-a716-446655440001",
    "shopId": "550e8400-e29b-41d4-a716-446655440000",
    "shopsItemId": "6ba7b810",
    "shopName": "My Shop",
    "title": {
      "text": "Amazing Product",
      "language": "en"
    },
    "state": "AVAILABLE",
    "url": "https://my-shop.com/products/amazing-product",
    "created": "2024-01-01T10:00:00Z",
    "updated": "2024-01-01T12:00:00Z"
  }
}
```

**New Schema: `PersonalizedItemSearchResultData`**
- Paginated collection of personalized items using cursor-based pagination
- Properties:
  - `items` (array of PersonalizedGetItemData, required): Personalized items in current page
  - `size` (integer, required): Number of items in current page
  - `total` (integer, optional): Total count of matching items
  - `searchAfter` (array, optional): Cursor for next page

**New Schema: `ItemUserStateData`**
- User-specific state information for an item
- Properties:
  - `watchlist` (WatchlistUserStateData, required): Watchlist-related state

**New Schema: `WatchlistUserStateData`**
- Watchlist-specific user state for an item
- Properties:
  - `watching` (boolean, required): Whether the item is on the user's watchlist
  - `notifications` (boolean, required): Whether notifications are enabled for this watchlist item

### Changed

#### Renamed Search-Filters

- A `searchFilter` no longer exists. The object used for searching items is now called `ItemSearchData` along with its field `itemSearch`
- User-stored search-filters are now called `UserSearchFilterData`. 

#### GET /api/v1/items/{shopId}/{shopsItemId}

This endpoint now supports optional authentication and returns personalized data.

**Authentication Changed**:
- **Before**: No authentication support
- **After**: Optional `Authorization` header (Cognito JWT Bearer token)

**Response Schema Changed**:
- **Before**: Returns `GetItemData` directly
- **After**: Returns `PersonalizedGetItemData` (wrapper with `item` and optional `userState`)

**New Request Header**:
- `Authorization` (optional): Cognito JWT token
  - Format: `Bearer [JWT token]`
  - When provided: Response includes `userState` with watchlist information
  - When omitted: Response contains only `item` without `userState`

**Response Body Structure**:
```json
{
  "item": { /* GetItemData */ },
  "userState": { /* ItemUserStateData - only when authenticated */ }
}
```

**Migration Impact**:
- Frontend must update response parsing to access item data via `response.item` instead of directly
- User state is available via `response.userState` when user is authenticated
- Anonymous requests work identically but with wrapped response structure

#### POST /api/v1/items/search

This endpoint now supports optional authentication and returns personalized data for each item in search results.

**Authentication Changed**:
- **Before**: No authentication support
- **After**: Optional `Authorization` header (Cognito JWT Bearer token)

**Response Schema Changed**:
- **Before**: Returns `ItemSearchResultData` with array of `GetItemData`
- **After**: Returns `PersonalizedItemSearchResultData` with array of `PersonalizedGetItemData`

**New Request Header**:
- `Authorization` (optional): Cognito JWT token
  - Format: `Bearer [JWT token]`
  - When provided: Each item in results includes `userState` with watchlist information
  - When omitted: Items contain only item data without `userState`

**Response Body Structure**:
```json
{
  "items": [
    {
      "item": { /* GetItemData */ },
      "userState": { /* ItemUserStateData - only when authenticated */ }
    }
  ],
  "size": 21,
  "total": 84,
  "searchAfter": "[2999, \"6ba7b810-9dad-11d1-80b4-00c04fd430c8\"]"
}
```

**Migration Impact**:
- Frontend must update item access from `response.items[i]` to `response.items[i].item`
- User state per item available via `response.items[i].userState` when authenticated
- Pagination fields (`size`, `total`, `searchAfter`) remain at root level

#### User Resource Path Restructuring

All user-specific endpoints have been reorganized under the `/api/v1/me/` prefix to better reflect their nature as user-scoped resources.

**Watchlist Endpoints**:
- `GET /api/v1/watchlist` → `GET /api/v1/me/watchlist`
- `POST /api/v1/watchlist` → `POST /api/v1/me/watchlist`
- `DELETE /api/v1/watchlist/{shopId}/{shopsItemId}` → `DELETE /api/v1/me/watchlist/{shopId}/{shopsItemId}`
- `PATCH /api/v1/watchlist/{shopId}/{shopsItemId}` → `PATCH /api/v1/me/watchlist/{shopId}/{shopsItemId}`

**Search Filter Endpoints**:
- `GET /api/v1/search-filters` → `GET /api/v1/me/search-filters`
- `POST /api/v1/search-filters` → `POST /api/v1/me/search-filters`
- `GET /api/v1/search-filters/{userSearchFilterId}` → `GET /api/v1/me/search-filters/{userSearchFilterId}`
- `PATCH /api/v1/search-filters/{userSearchFilterId}` → `PATCH /api/v1/me/search-filters/{userSearchFilterId}`
- `DELETE /api/v1/search-filters/{userSearchFilterId}` → `DELETE /api/v1/me/search-filters/{userSearchFilterId}`

**Migration Impact**:
- All API clients must update endpoint URLs to use `/api/v1/me/` prefix for user resources
- Request and response structures remain unchanged - only the path has changed
- Authentication requirements remain unchanged (all still require JWT)
- Location headers in POST responses now point to new paths

**Example URL Changes**:
- Old: `https://api.aura-historia.com/api/v1/watchlist`
- New: `https://api.aura-historia.com/api/v1/me/watchlist`

- Old: `https://api.aura-historia.com/api/v1/search-filters/6ba7b810-9dad-11d1-80b4-00c04fd430c8`
- New: `https://api.aura-historia.com/api/v1/me/search-filters/6ba7b810-9dad-11d1-80b4-00c04fd430c8`

### Migration Guide

For frontend developers integrating these changes:

1. **Update Item GET Endpoint**:
   ```typescript
   // Before
   const item: GetItemData = await getItem(shopId, shopsItemId);
   
   // After - Anonymous
   const response: PersonalizedGetItemData = await getItem(shopId, shopsItemId);
   const item = response.item;
   
   // After - Authenticated
   const response: PersonalizedGetItemData = await getItem(shopId, shopsItemId, token);
   const item = response.item;
   const isWatching = response.userState?.watchlist.watching ?? false;
   const notificationsEnabled = response.userState?.watchlist.notifications ?? false;
   ```

2. **Update Complex Search Endpoint**:
   ```typescript
   // Before
   const result: ItemSearchResultData = await searchItems(filter);
   const items: GetItemData[] = result.items;
   
   // After - Anonymous
   const result: PersonalizedItemSearchResultData = await searchItems(filter);
   const items = result.items.map(p => p.item);
   
   // After - Authenticated
   const result: PersonalizedItemSearchResultData = await searchItems(filter, token);
   const itemsWithState = result.items.map(personalized => ({
     item: personalized.item,
     watching: personalized.userState?.watchlist.watching ?? false,
     notifications: personalized.userState?.watchlist.notifications ?? false
   }));
   ```

3. **Update Watchlist Endpoint URLs**:
   ```typescript
   // Before
   const baseUrl = 'https://api.aura-historia.com/api/v1/watchlist';
   
   // After
   const baseUrl = 'https://api.aura-historia.com/api/v1/me/watchlist';
   ```

4. **Update Search Filter Endpoint URLs**:
   ```typescript
   // Before
   const baseUrl = 'https://api.aura-historia.com/api/v1/search-filters';
   
   // After
   const baseUrl = 'https://api.aura-historia.com/api/v1/me/search-filters';
   ```

5. **Handle Optional Authentication**:
   - Item endpoints can now be called without authentication
   - When calling without auth, expect `userState` to be absent in responses
   - Use optional chaining when accessing user state: `response.userState?.watchlist.watching`

### Removed

#### GET /api/v1/items (Simple Text Search)

The simple text search endpoint has been removed. Use `POST /api/v1/items/search` instead for all search operations.

**Removed Endpoint**:
- `GET /api/v1/items?q={query}&language={lang}&currency={cur}&sort={field}&order={order}&searchAfter={cursor}&size={size}`

**Migration Path**:
Use the complex search endpoint (`POST /api/v1/items/search`) which provides all the functionality of the simple search and more.

**Before** (Simple Search):
```http
GET /api/v1/items?q=smartphone+case&language=en&currency=USD&sort=price&order=asc&size=21
```

**After** (Complex Search):
```http
POST /api/v1/items/search?sort=price&order=asc&size=21
Content-Type: application/json

{
  "language": "en",
  "currency": "USD",
  "itemQuery": "smartphone case"
}
```

**Migration Impact**:
- All clients using simple search must migrate to complex search
- Query parameters `q`, `language`, and `currency` move to request body
- Sort and pagination parameters remain as query parameters
- Response structure is identical (wrapped in `PersonalizedItemSearchResultData` with authentication)
- Additional filtering options are available through the request body (shopNameQuery, price range, state, date ranges)

**Example Request/Response**:

Before (Simple Search):
```http
GET /api/v1/items?q=smartphone+case&language=en&currency=USD&sort=price&order=asc&size=21
```

After (Complex Search):
```http
POST /api/v1/items/search?sort=price&order=asc&size=21
Content-Type: application/json

{
  "language": "en",
  "currency": "USD",
  "itemQuery": "smartphone case"
}
```

Response (identical structure, wrapped in PersonalizedItemSearchResultData):
```json
{
  "items": [
    {
      "item": { /* GetItemData */ },
      "userState": { /* Optional when authenticated */ }
    }
  ],
  "size": 21,
  "total": 127,
  "searchAfter": "[2999, \"550e8400-e29b-41d4-a716-446655440000\"]"
}
```

No other endpoints or schemas have been removed in this update. The old watchlist and search-filter paths at `/api/v1/watchlist*` and `/api/v1/search-filters*` are deprecated and will be removed in a future update. Clients should migrate to the new `/api/v1/me/*` paths immediately.

## 2025-10-24 - Search Filter Names and Shop Search Keyset Pagination

This update introduces user-defined names for search filters and migrates shop search from offset-based pagination to cursor-based (keyset) pagination for better performance and scalability.

### Added

#### Search Filter Names

All search filter endpoints now support a `searchFilterName` field that allows users to assign custom names to their saved search filters.

**New Field in `UserSearchFilterData`:**
```json
{
  "userId": "550e8400-e29b-41d4-a716-446655440000",
  "searchFilterId": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
  "searchFilterName": "My Tech Store Search",
  "searchFilter": {
    "language": "en",
    "currency": "USD",
    "itemQuery": "smartphone"
  },
  "created": "2024-01-01T10:00:00Z",
  "updated": "2024-01-01T12:00:00Z"
}
```

**New Schema: `PostUserSearchFilterData`**
- Used for creating search filters
- Requires both `searchFilterName` (string, max 255 characters) and `searchFilter` object

**New Schema: `PatchUserSearchFilterData`**
- Used for updating search filters
- Optional `searchFilterName` field for updating the filter name
- Optional `searchFilter` object (type: `PatchSearchFilterData`) for updating filter criteria
- Both fields are optional - either or both can be updated

**New Schema: `PatchSearchFilterData`**
- Contains the actual search filter criteria that can be partially updated
- All fields from `SearchFilterData` are optional in this schema

### Changed

#### POST /api/v1/search-filters

**Request Body Structure Changed:**

Before:
```json
{
  "language": "en",
  "currency": "USD",
  "itemQuery": "smartphone",
  "shopNameQuery": "Tech Store",
  "price": {
    "min": 1000,
    "max": 5000
  }
}
```

After:
```json
{
  "searchFilterName": "My Tech Store Search",
  "searchFilter": {
    "language": "en",
    "currency": "USD",
    "itemQuery": "smartphone",
    "shopNameQuery": "Tech Store",
    "price": {
      "min": 1000,
      "max": 5000
    }
  }
}
```

**Response:** Now includes `searchFilterName` field in the returned `UserSearchFilterData`.

#### PATCH /api/v1/search-filters/{searchFilterId}

**Request Body Structure Changed:**

The PATCH endpoint now supports updating the search filter name separately from the filter criteria. The request body structure has been reorganized to nest the filter criteria.

Before (flat structure):
```json
{
  "shopNameQuery": "Updated Store",
  "price": {
    "min": 2000,
    "max": 8000
  }
}
```

After (nested structure):
```json
{
  "searchFilterName": "Updated Filter Name",
  "searchFilter": {
    "shopNameQuery": "Updated Store",
    "price": {
      "min": 2000,
      "max": 8000
    }
  }
}
```

**Update Options:**
- Update name only: `{ "searchFilterName": "New Name" }`
- Update filter criteria only: `{ "searchFilter": { "language": "de" } }`
- Update both: `{ "searchFilterName": "New Name", "searchFilter": { "language": "de" } }`
- No update (returns existing): `{}`

**Response:** Now includes `searchFilterName` field in the returned `UserSearchFilterData`.

#### GET /api/v1/search-filters/{searchFilterId}

**Response:** Now includes `searchFilterName` field in the returned `UserSearchFilterData`.

#### GET /api/v1/search-filters

**Response:** All items in the `items` array now include `searchFilterName` field.

#### POST /api/v1/shops/search

**Pagination Changed from Offset-Based to Cursor-Based (Keyset Pagination):**

Query Parameters:
- **Removed:** `from` (offset) parameter
- **Added:** `searchAfter` parameter (JSON array cursor)

**`searchAfter` Parameter:**
- Type: JSON array
- Description: Cursor value from previous response's `searchAfter` field
- Format: `[sortValue, shopId]` (heterogeneous array)
- Example: `["Tech Store Premium", "550e8400-e29b-41d4-a716-446655440000"]`
- Usage: Pass the `searchAfter` value from the previous page to get the next page

**Response Schema Changed:**

Before (`ShopSearchResultData`):
```json
{
  "items": [...],
  "from": 0,
  "size": 21,
  "total": 127
}
```

After (`ShopSearchResultData`):
```json
{
  "items": [...],
  "size": 2,
  "searchAfter": "[\"Tech Store Basic\", \"660f9500-f39c-42e5-b827-556766550001\"]",
  "total": 42
}
```

**Response Fields:**
- `items`: Array of shop results (unchanged)
- `size`: Number of items in current page (unchanged, now required)
- `searchAfter`: Cursor for next page (nullable, present when more results available)
- `total`: Total number of matching items (unchanged, nullable)
- **Removed:** `from` field

**Pagination Workflow:**
1. First request: Omit `searchAfter` parameter
2. Check response for `searchAfter` field
3. If `searchAfter` is present, use it in next request to get the next page
4. Repeat until `searchAfter` is not present in response

**Benefits:**
- Better performance for large result sets
- Consistent pagination even when data changes
- No risk of duplicates or skipped items during pagination

### Removed

#### Schema: `SearchFilterDataPatch`

This schema has been replaced by `PatchUserSearchFilterData` and `PatchSearchFilterData` to support the new nested structure for partial updates.

## 2025-10-21 - Item Event History Enhanced with Old and New Values

This update enhances the item history events to include both old and new values for state and price changes, making it easier for the frontend to display what changed without computing differences. This provides richer context for tracking item history and enables better UI experiences when showing price drops, increases, and state transitions.

### Changed

#### GET /api/v1/items/{shopId}/{shopsItemId}?history=true

The history array in the response now includes more detailed information for each event type. Event payloads have been restructured to provide both old and new values for changes.

**Breaking Changes to Event Payload Structure:**

1. **State Change Events** (STATE_LISTED, STATE_AVAILABLE, STATE_RESERVED, STATE_SOLD, STATE_REMOVED, STATE_UNKNOWN)
   
   **Before:**
   ```json
   {
     "eventType": "STATE_AVAILABLE",
     "payload": "AVAILABLE"
   }
   ```
   
   **After:**
   ```json
   {
     "eventType": "STATE_AVAILABLE",
     "payload": {
       "oldState": "LISTED",
       "newState": "AVAILABLE"
     }
   }
   ```

2. **Price Discovered Events** (PRICE_DISCOVERED)
   
   **Before:**
   ```json
   {
     "eventType": "PRICE_DISCOVERED",
     "payload": {
       "currency": "EUR",
       "amount": 2999
     }
   }
   ```
   
   **After:**
   ```json
   {
     "eventType": "PRICE_DISCOVERED",
     "payload": {
       "newPrice": {
         "currency": "EUR",
         "amount": 2999
       }
     }
   }
   ```

3. **Price Change Events** (PRICE_DROPPED, PRICE_INCREASED)
   
   **Before:**
   ```json
   {
     "eventType": "PRICE_DROPPED",
     "payload": {
       "currency": "EUR",
       "amount": 2499
     }
   }
   ```
   
   **After:**
   ```json
   {
     "eventType": "PRICE_DROPPED",
     "payload": {
       "oldPrice": {
         "currency": "EUR",
         "amount": 2999
       },
       "newPrice": {
         "currency": "EUR",
         "amount": 2499
       }
     }
   }
   ```

4. **Price Removed Events** (PRICE_REMOVED)
   
   **Before:**
   ```json
   {
     "eventType": "PRICE_REMOVED",
     "payload": null
   }
   ```
   
   **After:**
   ```json
   {
     "eventType": "PRICE_REMOVED",
     "payload": {
       "oldPrice": {
         "currency": "EUR",
         "amount": 2999
       }
     }
   }
   ```

#### New Data Types

**ItemEventStateChangedPayloadData**
- Used for all state change events
- Properties:
  - `oldState` (ItemStateData, required): The previous state of the item
  - `newState` (ItemStateData, required): The new state of the item

**ItemEventPriceDiscoveredPayloadData**
- Used for PRICE_DISCOVERED events when a price is first detected
- Properties:
  - `newPrice` (PriceData, required): The newly discovered price

**ItemEventPriceChangedPayloadData**
- Used for PRICE_DROPPED and PRICE_INCREASED events
- Properties:
  - `oldPrice` (PriceData, required): The previous price
  - `newPrice` (PriceData, required): The new price

**ItemEventPriceRemovedPayloadData**
- Used for PRICE_REMOVED events when a price is removed from an item
- Properties:
  - `oldPrice` (PriceData, required): The price that was removed

### Migration Guide

For frontend developers integrating these changes:

1. **Update item history event handling**:
   - All state change events now return an object with `oldState` and `newState` fields
   - Access state using `event.payload.newState` instead of just `event.payload`

2. **Update price event handling**:
   - PRICE_DISCOVERED events now have the price under `event.payload.newPrice`
   - PRICE_DROPPED and PRICE_INCREASED events now provide both `oldPrice` and `newPrice`
   - PRICE_REMOVED events now include the `oldPrice` that was removed

3. **Display improvements**:
   - You can now show "Changed from X to Y" messages for state transitions
   - Display price drops/increases with the actual old and new values
   - Show what price was removed instead of just indicating removal

4. **Example complete event object**:
   ```json
   {
     "eventType": "PRICE_DROPPED",
     "itemId": "550e8400-e29b-41d4-a716-446655440000",
     "eventId": "550e8400-e29b-41d4-a716-446655440001",
     "shopId": "550e8400-e29b-41d4-a716-446655440000",
     "shopsItemId": "6ba7b810",
     "payload": {
       "oldPrice": {
         "currency": "USD",
         "amount": 3499
       },
       "newPrice": {
         "currency": "USD",
         "amount": 2999
       }
     },
     "timestamp": "2024-01-15T14:30:00Z"
   }
   ```

### Backend Implementation Details

- Event payloads are now separate types: `ItemEventStateChangedPayloadData`, `ItemEventPriceDiscoveredPayloadData`, `ItemEventPriceChangedPayloadData`, and `ItemEventPriceRemovedPayloadData`
- Price discovery (first time a price is detected) is now distinguished from price changes with separate payload structure
- All state and price change events capture the old value before modification occurs
- The `ItemEventPayloadData` enum uses `oneOf` discriminator to allow different payload structures per event type

## 2025-10-14 - Watchlist Item Identification Refactoring

This update improves the watchlist item API by simplifying item identification. Watchlist items are now uniquely identified by their composite key (shopId + shopsItemId) rather than requiring the creation timestamp. This makes the API more intuitive and reduces the likelihood of errors.

### Changed

#### DELETE /api/v1/watchlist/{shopId}/{shopsItemId}

The DELETE endpoint has been simplified to remove the `created` query parameter requirement:

**Before**:
```
DELETE /api/v1/watchlist/{shopId}/{shopsItemId}?created={timestamp}
```

**After**:
```
DELETE /api/v1/watchlist/{shopId}/{shopsItemId}
```

**Changes**:
- **Removed** `created` query parameter - No longer required to identify the watchlist entry
- Watchlist items are now uniquely identified by the combination of `shopId` and `shopsItemId` path parameters
- Simplified error handling - No more `BAD_QUERY_PARAMETER_VALUE` or `INVALID_RFC3339_TIMESTAMP` errors related to the `created` parameter

**Migration Impact**:
- Frontend applications must update DELETE requests to remove the `created` query parameter
- The endpoint now relies solely on path parameters `{shopId}/{shopsItemId}` for item identification
- Error handling code for missing/invalid `created` timestamps can be removed

#### PATCH /api/v1/watchlist/{shopId}/{shopsItemId}

The PATCH endpoint has been simplified to remove the `created` query parameter requirement:

**Before**:
```
PATCH /api/v1/watchlist/{shopId}/{shopsItemId}?created={timestamp}
```

**After**:
```
PATCH /api/v1/watchlist/{shopId}/{shopsItemId}
```

**Changes**:
- **Removed** `created` query parameter - No longer required to identify the watchlist entry
- Watchlist items are now uniquely identified by the combination of `shopId` and `shopsItemId` path parameters
- Simplified error handling - No more `BAD_QUERY_PARAMETER_VALUE` or `INVALID_RFC3339_TIMESTAMP` errors related to the `created` parameter
- Request body and response structure remain unchanged

**Migration Impact**:
- Frontend applications must update PATCH requests to remove the `created` query parameter
- The endpoint now relies solely on path parameters `{shopId}/{shopsItemId}` for item identification
- Error handling code for missing/invalid `created` timestamps can be removed

#### POST /api/v1/watchlist

The POST endpoint now returns the created watchlist item data in the response body:

**Changes**:
- **Added** response body containing `WatchlistItemPatchResponse` data
- Response now includes all watchlist item metadata: `shopId`, `shopsItemId`, `itemId`, `notifications`, `created`, and `updated`
- The `Location` header is still included pointing to the created resource
- Default `notifications` value is `false` for newly created items
- Both `created` and `updated` timestamps are set to the same value when an item is first added

**Response Body Schema**: `WatchlistItemPatchResponse`
- `shopId` (string, uuid, required): Shop identifier
- `shopsItemId` (string, required): Shop's item identifier  
- `itemId` (string, uuid, required): Internal item identifier
- `notifications` (boolean, required): Notification setting (default: false)
- `created` (string, date-time, required): When item was added to watchlist
- `updated` (string, date-time, required): When watchlist item was last updated

**Example Response**:
```json
{
  "shopId": "550e8400-e29b-41d4-a716-446655440000",
  "shopsItemId": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
  "itemId": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "notifications": false,
  "created": "2024-01-15T08:00:00Z",
  "updated": "2024-01-15T08:00:00Z"
}
```

**Migration Impact**:
- Frontend applications can now immediately use the returned watchlist item data without making an additional GET request
- The response includes the internal `itemId` which may be useful for tracking
- Applications should update their POST request handlers to process the response body

### Implementation Details

**Backend Changes**:
- Database schema updated to use composite key (`shopId` + `shopsItemId`) as the sort key instead of `created` timestamp
- The `created` timestamp moved to a Local Secondary Index (LSI) named `lsi1` for efficient sorting by creation date
- Service layer methods now accept `shopId` and `shopsItemId` parameters instead of `created` timestamp
- New `WatchlistItemData` type moved to a shared module for reuse across API handlers
- Added `UpdateWatchlistItemCommand` for encapsulating update operations

**Error Code Changes**:
- Error code `WATCHLIST_ENTRY_NOT_FOUND` message updated to reflect new identification method:
  - Old: "There exists no Watchlist-Item that was started being watched on '{timestamp}' for user '{userId}'"
  - New: "There exists no Watchlist-Item for user '{userId}' with Shop-Id '{shopId}' and Shops-Item-Id '{shopsItemId}'"

### Migration Guide

For frontend developers integrating these changes:

1. **DELETE endpoint**:
   - Remove the `created` query parameter from DELETE requests
   - Update URL construction: `DELETE /api/v1/watchlist/${shopId}/${shopsItemId}`
   - Remove error handling for `BAD_QUERY_PARAMETER_VALUE` and `INVALID_RFC3339_TIMESTAMP` related to `created`

2. **PATCH endpoint**:
   - Remove the `created` query parameter from PATCH requests
   - Update URL construction: `PATCH /api/v1/watchlist/${shopId}/${shopsItemId}`
   - Remove error handling for `BAD_QUERY_PARAMETER_VALUE` and `INVALID_RFC3339_TIMESTAMP` related to `created`
   - Request body and response handling remain unchanged

3. **POST endpoint**:
   - Update POST request handlers to process the response body
   - The response now contains the complete watchlist item data including `itemId`, `notifications`, `created`, and `updated`
   - Use the returned data to update your local state without needing an additional GET request

4. **General**:
   - The `created` timestamp is still available in GET responses and can be used for display purposes
   - The uniqueness constraint is now on `(userId, shopId, shopsItemId)` instead of `(userId, created)`
   - A user can only have one watchlist entry per unique item (shopId + shopsItemId combination)

### Removed

No endpoints or data types have been removed. The changes are modifications to existing endpoint signatures and response structures.

## 2025-10-13 - Watchlist Notifications

This update adds notification management capabilities for watchlist items and enriches watchlist item data with additional metadata.

### Added

#### New Endpoint

**PATCH /api/v1/watchlist/{shopId}/{shopsItemId}**
- Updates settings for a specific watchlist item (currently supports toggling notifications)
- Requires the `created` timestamp as a query parameter to identify the exact entry
- Returns 200 OK with updated watchlist item data including core identifiers
- **Authentication**: Required (Cognito JWT)
- **Path Parameters**:
  - `shopId` (required): UUID of the shop
  - `shopsItemId` (required): Shop's item identifier
- **Query Parameters**:
  - `created` (required): RFC3339 timestamp of when the watchlist entry was created
- **Request Body**: `WatchlistItemPatch`
  - `notifications` (optional, boolean): Whether to enable or disable notifications for this item
- **Response Body**: `WatchlistItemPatchResponse`
  - `shopId` (string, uuid, required): Shop identifier
  - `shopsItemId` (string, required): Shop's item identifier
  - `itemId` (string, uuid, required): Internal item identifier
  - `notifications` (boolean, required): Current notification setting
  - `created` (string, date-time, required): When item was added to watchlist
  - `updated` (string, date-time, required): When watchlist item was last updated
- **Headers**:
  - `Authorization` (required): Cognito JWT Bearer token
  - `Last-Modified` (response): RFC3339 timestamp of when the item was last updated

**Example Request**:
```json
{
  "notifications": true
}
```

**Example Response**:
```json
{
  "shopId": "550e8400-e29b-41d4-a716-446655440000",
  "shopsItemId": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
  "itemId": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
  "notifications": true,
  "created": "2024-01-15T08:00:00Z",
  "updated": "2024-01-15T08:30:00Z"
}
```

**Error Responses**:
- `400 BAD_PATH_PARAMETER_VALUE`: Missing or invalid path parameters (shopId, shopsItemId)
- `400 BAD_QUERY_PARAMETER_VALUE`: Missing created timestamp
- `400 INVALID_RFC3339_TIMESTAMP`: Invalid created timestamp format
- `400 INVALID_UUID`: Invalid UUID format for shopId
- `400 BAD_BODY_VALUE`: Missing or invalid request body
- `401 UNAUTHORIZED`: Missing or invalid JWT token
- `404 WATCHLIST_ENTRY_NOT_FOUND`: Watchlist entry not found for the given parameters
- `500 INTERNAL_SERVER_ERROR`: Server error

#### New Data Types

1. **WatchlistItemPatch**
   - Request body schema for PATCH /api/v1/watchlist/{shopId}/{shopsItemId}
   - Properties:
     - `notifications` (boolean, optional): Enable or disable notifications for the watchlist item

2. **WatchlistItemPatchResponse**
   - Response schema for PATCH /api/v1/watchlist/{shopId}/{shopsItemId}
   - Properties:
     - `shopId` (string, uuid, required): Shop identifier
     - `shopsItemId` (string, required): Shop's item identifier
     - `itemId` (string, uuid, required): Internal item identifier
     - `notifications` (boolean, required): Current notification setting
     - `created` (string, date-time, required): When item was added to watchlist
     - `updated` (string, date-time, required): When watchlist item was last updated

### Changed

#### WatchlistItemData (GET /api/v1/watchlist Response)

The `WatchlistItemData` type returned by the GET /api/v1/watchlist endpoint has been enhanced with notification settings and update tracking:

**New Fields Added**:
- `notifications` (boolean, required): Whether notifications are enabled for this watchlist item
  - Default value: `false` for newly created watchlist items
- `updated` (string, date-time, required): RFC3339 timestamp of when the watchlist item was last updated
  - Initially set to the same value as `created` when an item is added to the watchlist
  - Updated whenever watchlist item settings are modified (e.g., via PATCH endpoint)

**Updated Schema**:
```json
{
  "item": {
    // ... GetItemData fields
  },
  "notifications": false,
  "created": "2024-01-15T08:00:00Z",
  "updated": "2024-01-15T08:00:00Z"
}
```

**Impact on Existing Integrations**:
- Frontend applications should update their models to include these new required fields
- The `notifications` field allows users to control whether they receive updates about price changes or availability for watchlist items
- The `updated` field helps track when settings were last modified, useful for sync and conflict resolution

### Migration Guide

For frontend developers integrating these changes:

1. **GET /api/v1/watchlist**:
   - Update your `WatchlistItemData` model to include:
     - `notifications: boolean` (required)
     - `updated: string` (required, RFC3339 format)
   - Handle these fields in your watchlist display components
   - Consider showing notification status in the UI

2. **New PATCH endpoint**:
   - Implement notification toggle functionality using `PATCH /api/v1/watchlist/{shopId}/{shopsItemId}?created={timestamp}`
   - Include the exact `created` timestamp from the watchlist item when making PATCH requests
   - The response provides updated item metadata including the new `updated` timestamp
   - Use the `Last-Modified` response header for optimistic locking if needed

3. **Error Handling**:
   - Add handlers for the new error responses from the PATCH endpoint
   - The `created` timestamp must match exactly - handle `WATCHLIST_ENTRY_NOT_FOUND` errors appropriately

### Removed

No endpoints or fields have been removed in this update.

## 2025-10-08 - Item Enrichment with Shop Information

### Changed

#### GetShopData
- Type `GetShopData` replaced field `url` that contained a single URL with an array `urls` which contains all known URLs for a shop.

#### PUT /api/v1/items - Automatic Shop Enrichment

The PUT items endpoint now automatically enriches items with shop information based on their URLs. This removes the need for clients to provide shop identifiers.

**Request Body Changes (breaking changes)**:
- **Removed**: `shopId` field - No longer required in request body
- **Removed**: `shopName` field - No longer required in request body
- **Shop Enrichment**: Shop information is now automatically determined from the item's `url` field
  - The item's URL must belong to a shop that is already registered in the system
  - If the shop cannot be identified, the item will fail with a `SHOP_NOT_FOUND` error

**Request Body Example**:
```json
{
  "items": [
    {
      "shopsItemId": "item-123",
      "title": {
        "text": "Smartphone Case",
        "language": "en"
      },
      "price": {
        "currency": "EUR",
        "amount": 2999
      },
      "state": "AVAILABLE",
      "url": "https://tech-store.com/items/case-123",
      "images": ["https://tech-store.com/images/case.jpg"]
    }
  ]
}
```

**Response Body Changes (breaking changes)**:
- **Changed**: `unprocessed` field now contains an array of item URLs (strings) instead of ItemKeyData objects
  - Old format: `[{"shopId": "uuid", "shopsItemId": "string"}]`
  - New format: `["https://shop.com/item1", "https://shop.com/item2"]`
  - These are items that could not be processed due to temporary issues and may succeed if retried

- **Added**: `failed` field - A map of item URLs to error codes for items that permanently failed processing
  - Type: Object with URL keys and error code values
  - Example: `{"https://unknown-shop.com/item": "SHOP_NOT_FOUND"}`
  - These items failed permanently and will not succeed without changes

**Response Example**:
```json
{
  "unprocessed": [
    "https://tech-store.com/temporary-issue-item"
  ],
  "failed": {
    "https://unknown-shop.com/item": "SHOP_NOT_FOUND",
    "https://tech-store.com/expensive-item": "MONETARY_AMOUNT_OVERFLOW"
  },
  "skipped": 5
}
```

**New Error Codes**:
- `SHOP_NOT_FOUND`: The shop associated with the item's URL is not registered in the system
- `MONETARY_AMOUNT_OVERFLOW`: The price amount exceeds the maximum supported value during currency conversion
- `ITEM_ENRICHMENT_FAILED`: Failed to enrich the item with additional shop and price information

**Migration Guide**:
1. Remove `shopId` and `shopName` fields from your PUT items requests
2. Ensure the `url` field points to a shop that is registered in the system
3. Handle new error responses in the `failed` field
4. Update error handling for `SHOP_NOT_FOUND`, `MONETARY_AMOUNT_OVERFLOW`, and `ITEM_ENRICHMENT_FAILED` error codes
5. Update processing of `unprocessed` field to handle URL strings instead of ItemKeyData objects

### Removed

No endpoints have been removed in this update.

## 2025-10-02 - Pagination Improvements

### Changed

#### Item Search Endpoints - Search-After Cursor Pagination

The item search endpoints now use cursor-based pagination (search-after pattern) instead of offset/limit pagination:

1. **GET /api/v1/items**
   - Changed from offset/limit pagination to search-after cursor pagination
   - **Query Parameters** (breaking changes):
     - **Removed**: `from` (offset) parameter
     - **Added**: `searchAfter` (optional, string): JSON cursor value from previous response
   - **Response Structure** (breaking changes):
     - Pagination fields are now at the root level (no nested `pagination` object)
     - Fields: `items`, `size`, `total`, `searchAfter`
     - `searchAfter` is returned when more results are available
     - Example response:
       ```json
       {
         "items": [...],
         "size": 21,
         "total": 127,
         "searchAfter": "[2999, \"6ba7b810-9dad-11d1-80b4-00c04fd430c8\"]"
       }
       ```
   - **Previous response structure** (deprecated):
     ```json
     {
       "items": [...],
       "pagination": {
         "from": 0,
         "size": 21,
         "total": 127
       }
     }
     ```

2. **POST /api/v1/items/search**
   - Changed from offset/limit pagination to search-after cursor pagination
   - **Query Parameters** (breaking changes):
     - **Removed**: `from` (offset) parameter
     - **Added**: `searchAfter` (optional, string): JSON cursor value from previous response
   - **Response Structure** (breaking changes):
     - Same as GET /api/v1/items (flattened pagination with `searchAfter`)

#### New Sort Field for Item Searches

- **Added**: `score` sort field for item search endpoints
- Available for both `GET /api/v1/items` and `POST /api/v1/items/search`
- When not specified, `score` is used as the default sort field (sorting by relevance)
- The `sort` query parameter now accepts: `score`, `price`, `updated`, `created`
- **Previous valid values**: `price`, `updated`, `created`

#### New Sort Field for Shop Search

- **Added**: `score` sort field for shop search endpoint
- Available for `POST /api/v1/shops/search`
- When not specified, `score` is used as the default sort field (sorting by relevance)
- The `sort` query parameter now accepts: `score`, `name`, `updated`, `created`
- **Previous valid values**: `name`, `updated`, `created`

#### Pagination Response Structure Changes

The following endpoints now return pagination metadata at the root level instead of in a nested `pagination` object:

1. **GET /api/v1/watchlist**
   - Already used cursor-based pagination, now with flattened structure
   - **Query Parameters** (breaking change):
     - **Renamed**: `from` → `searchAfter` (RFC3339 timestamp)
   - **Response Structure** (breaking change):
     - Pagination fields moved to root level: `items`, `size`, `searchAfter`, `total`
     - `searchAfter` field renamed from `next` and moved to root level
     - Example response:
       ```json
       {
         "items": [...],
         "size": 21,
         "searchAfter": "2024-01-15T08:00:00Z",
         "total": 127
       }
       ```
   - **Previous response structure** (deprecated):
     ```json
     {
       "items": [...],
       "pagination": {
         "from": "2024-01-01T00:00:00Z",
         "size": 21,
         "next": "2024-01-15T08:00:00Z",
         "total": 127
       }
     }
     ```

2. **GET /api/v1/search-filters**
   - Still uses offset/limit pagination
   - **Response Structure** (breaking change):
     - Pagination fields moved to root level: `items`, `from`, `size`, `total`
     - Example response:
       ```json
       {
         "items": [...],
         "from": 0,
         "size": 21,
         "total": 127
       }
       ```
   - **Previous response structure** (deprecated):
     ```json
     {
       "items": [...],
       "pagination": {
         "from": 0,
         "size": 21,
         "total": 127
       }
     }
     ```

3. **POST /api/v1/shops/search**
   - Still uses offset/limit pagination
   - **Response Structure** (breaking change):
     - Same as search filters (flattened pagination structure)

#### New Data Types

1. **ItemSearchResultData**
   - Response type for item search endpoints (GET /api/v1/items, POST /api/v1/items/search)
   - Properties:
     - `items` (array of GetItemData, required): Items in current page
     - `size` (integer, required): Number of items in current page
     - `total` (integer, optional, nullable): Total count of matching items
     - `searchAfter` (string, optional, nullable): Cursor for next page (JSON value)

2. **SearchFilterCollectionData**
   - Response type for GET /api/v1/search-filters
   - Properties (flattened pagination):
     - `items` (array of UserSearchFilterData, required): Search filters
     - `from` (integer, required): Offset
     - `size` (integer, required): Number of items in current page
     - `total` (integer, optional, nullable): Total count

3. **WatchlistCollectionData**
   - Response type for GET /api/v1/watchlist
   - Properties (flattened pagination):
     - `items` (array of WatchlistItemData, required): Watchlist items
     - `size` (integer, required): Number of items in current page
     - `searchAfter` (string, date-time, optional, nullable): Cursor for next page (RFC3339 timestamp)
     - `total` (integer, optional, nullable): Total count

4. **ShopSearchResultData**
   - Response type for POST /api/v1/shops/search
   - Properties (flattened pagination):
     - `items` (array of GetShopData, required): Shops
     - `from` (integer, required): Offset
     - `size` (integer, required): Number of items in current page
     - `total` (integer, optional, nullable): Total count

#### Updated Sort Field Enums

- **SortItemFieldData** enum now includes `score` as a value (and default)
- Values: `score`, `price`, `updated`, `created`

- **SortShopFieldData** enum now includes `score` as a value (and default)
- Values: `score`, `name`, `updated`, `created`

#### New Error Code

- `INVALID_JSON`: Returned when the `searchAfter` parameter contains invalid JSON

### Migration Guide

For frontend developers integrating these changes:

1. **Item Search Endpoints** (GET /api/v1/items, POST /api/v1/items/search):
   - Replace `from` query parameter with `searchAfter`
   - Read pagination fields from root level instead of `response.pagination.*`
   - Use `response.searchAfter` (if present) for the next page request
   - The `total` field may be `null` in some cases

2. **Watchlist Endpoint** (GET /api/v1/watchlist):
   - **Rename query parameter**: `from` → `searchAfter`
   - Read pagination fields from root level instead of `response.pagination.*`
   - **Field renamed**: `pagination.next` → `searchAfter` (at root level)
   - Use `response.searchAfter` (if present) for the next page request

3. **Search Filters Endpoint** (GET /api/v1/search-filters):
   - Read pagination fields from root level instead of `response.pagination.*`
   - Continue using `from` and `size` query parameters (no change)

4. **Shop Search Endpoint** (POST /api/v1/shops/search):
   - Read pagination fields from root level instead of `response.pagination.*`
   - Continue using `from` and `size` query parameters (no change)
   - The `score` sort field is now available and is the default

5. **Sort Fields**:
   - The `score` sort field is now available for both item search and shop search endpoints
   - When not specifying a sort field, results are sorted by relevance score (default)

### Removed

No endpoints or features have been removed in this update. The changes are modifications to existing endpoints.

## 2025-09-30 - Watchlist Feature

### Added

Three new endpoints have been added to support the user item watchlist feature:

#### New Endpoints

1. **GET /api/v1/watchlist**
   - Retrieves all items in the authenticated user's watchlist
   - Uses cursor-based pagination with RFC3339 timestamps (search-after pattern)
   - Supports sorting by creation date (when item was added to watchlist)
   - Supports language selection via `Accept-Language` header
   - Supports currency selection via `currency` query parameter
   - Returns items with full item details and the timestamp when added to watchlist
   - **Authentication**: Required (Cognito JWT)
   - **Query Parameters**:
     - `currency` (optional): Currency for price display (default: EUR)
     - `sort` (optional): Field to sort by (only `created` supported)
     - `order` (optional): Sort order `asc` or `desc`
     - `from` (optional): RFC3339 timestamp cursor for pagination
     - `size` (optional): Number of items per page (min: 1, max: 100, default: 21)
   - **Headers**:
     - `Accept-Language` (optional): Language for localized content (de, en, fr, es)
     - `Authorization` (required): Cognito JWT Bearer token

2. **POST /api/v1/watchlist**
   - Adds an item to the authenticated user's watchlist
   - Request body must contain `shopId` and `shopsItemId`
   - Returns 201 Created with `Location` header pointing to the resource
   - **Authentication**: Required (Cognito JWT)
   - **Request Body**: `ItemKeyData` (shopId, shopsItemId)
   - **Headers**:
     - `Authorization` (required): Cognito JWT Bearer token

3. **DELETE /api/v1/watchlist/{shopId}/{shopsItemId}**
   - Removes a specific item from the authenticated user's watchlist
   - Requires the `created` timestamp as a query parameter to identify the exact entry
   - Returns 204 No Content on success
   - **Authentication**: Required (Cognito JWT)
   - **Path Parameters**:
     - `shopId` (required): UUID of the shop
     - `shopsItemId` (required): Shop's item identifier
   - **Query Parameters**:
     - `created` (required): RFC3339 timestamp of when the watchlist entry was created
   - **Headers**:
     - `Authorization` (required): Cognito JWT Bearer token

#### New Data Types

1. **ItemKeyData**
   - Object identifying an item by shop ID and shop's item ID
   - Properties:
     - `shopId` (string, uuid, required): Shop identifier
     - `shopsItemId` (string, required): Shop's item identifier

2. **WatchlistItemData**
   - Object representing a watchlist entry
   - Properties:
     - `item` (GetItemData, required): Full item details
     - `created` (string, date-time, required): RFC3339 timestamp when added to watchlist

3. **WatchlistCollectionData**
   - Paginated collection of watchlist items using cursor-based pagination
   - Properties:
     - `items` (array of WatchlistItemData, required): Watchlist items in current page
     - `pagination` (PaginationDataDateTime, required): Pagination metadata

4. **PaginationDataDateTime**
   - Pagination metadata for cursor-based pagination using timestamps
   - Properties:
     - `from` (string, date-time, required): Starting cursor timestamp
     - `size` (integer, required): Number of items in current page
     - `total` (integer, optional, nullable): Total count (may not be available)
     - `next` (string, date-time, optional, nullable): Next page cursor

5. **SortWatchlistItemFieldData**
   - Enum for watchlist sorting fields
   - Values: `created`

#### New Error Codes

- `INVALID_RFC3339_TIMESTAMP`: Returned when an RFC3339 timestamp parameter is malformed
- `WATCHLIST_ENTRY_NOT_FOUND`: Returned when attempting to delete a non-existent watchlist entry

### Changed

#### Generalized type `UnprocessedPutItem`
- The type `UnprocessedPutItem` has been generalized to `ItemKeyData`. It's a composite key for identifying an item.
- `UnprocessedPutItem` therefore no longer exists. `ItemKeyData` is used now. Their payload does **not** differ.

#### Pagination Pattern
- Watchlist endpoints use cursor-based pagination with RFC3339 timestamps instead of offset-based pagination
- The `from` query parameter accepts an RFC3339 timestamp
- The response includes a `next` field in pagination metadata when more results are available
- The `total` field in pagination metadata is optional for cursor-based pagination
- The other endpoints using pagination are unaffected and still rely on Offset/Limit-Pagination.

### Removed

No endpoints or features have been removed in this update.
