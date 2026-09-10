# DOX

## Purpose

- Own axum REST API runtime and transport auth service for #1341.

## Core Design

- `main.rs` bootstraps logging, config, and graceful shutdown.
- `lib.rs` owns runtime config, axum router, health/readiness endpoints, server loop, and composition root wiring.
- `state.rs` owns axum application state shared by route modules.
- `error.rs` owns API problem JSON errors.
- `wire.rs` owns REST codecs for canonical semantic leaf types and generic domain-`FromStr` object-ID parsing for path, query, and body fields. Invalid object IDs use `INVALID_OBJECT_ID` without exposing codec details. DTO structs keep JSON shape and use codecs; API does not duplicate canonical enums only for Serde.
- `auth/` owns bearer auth extraction, Cognito JWT verification via cached JWKS, verified Cognito identity resolution, Aura access-token auth, active-user status verification, and mapping to `OperationContext`.
- Auth accepts Cognito access JWTs and Aura access tokens through one interface, then verifies the resolved User remains present and unsuspended. Missing or suspended Users make either supplied credential invalid; temporary status checks map to temporary auth failure and internal checks map to internal auth failure. Cognito needs `AURA_HISTORIA_COGNITO_ISSUER`, `AURA_HISTORIA_COGNITO_JWKS_URL`, and comma-separated `AURA_HISTORIA_COGNITO_APP_CLIENT_IDS`; it fetches JWKS with bounded cache/refresh, treats `sub` as opaque provider identity, and resolves the verified `(issuer, subject)` through the User service and PostgreSQL identity mapping before creating open-world first-party `Principal::User`. Aura access tokens map explicit scopes to closed-world delegated capabilities.
- Auth extractors only authenticate. Required capability and business policy checks belong in service/use-case code, not controllers.
- Global axum transport middleware creates UUID request IDs; it accepts only bounded safe `X-Correlation-Id` values (max 128 ASCII alphanumeric, `.`, `_`, `-`) and returns both IDs on every response. It also owns safe request tracing, sensitive-header redaction, CORS including WooCommerce topic, signature, and delivery-ID headers, a 1 MiB body cap, and a 30-second timeout.
- `/health` reports process liveness. `/ready` returns `204` only after PostgreSQL pool ingestion and OpenSearch ping succeed; it returns `503` otherwise. pg-ttl worker health is monitored as PostgreSQL platform health, not a request-path readiness dependency.
- No API Gateway adapter.

- `admin_overview/` owns the admin-only `GET /api/v1/admin/overview` controller. It maps the bounded, authoritative PostgreSQL overview result to a versioned REST response and always sends `Cache-Control: no-store`; it exposes no PII or secrets and persists no audit event.
- `parties/` owns admin Party collection and detail create/read/update/delete routes. Party DELETE hard-deletes only an unused Party after commit; ListingSources and retained active or dissolved Partnerships block, no dependent history cascades, and every response uses `Cache-Control: no-store`.
- `listing_sources/` owns authenticated canonical admin create at `POST /api/v1/admin/listing-sources`, admin ID detail/update/delete at `GET`/`PATCH`/`DELETE /api/v1/admin/listing-sources/{listingSourceId}`, slug lookup, `GET /api/v1/me/listing-sources`, and bounded admin search at `GET /api/v1/admin/listing-sources`. Eligible hard deletion returns bodyless `204` with `no-store` after PostgreSQL commit; it has no synchronous crawler shutdown, crawler-DB, or OpenSearch transaction. New crawler passes refresh authoritative source scope before admission, and in-flight raw capture is fenced by the business missing-source outcome. Detail, mutations, and admin search use ListingSource service use cases; the `me` list uses the Partnership administered-listing-source use case. Admin reads never expose provider secrets or crawler-local configuration.
- `users/` owns account, admin user (`GET /api/v1/admin/users` collection search plus `GET`, `PATCH`, and `DELETE /api/v1/admin/users/{userId}` item operations), admin suspension/reactivation at `PUT`/`DELETE /api/v1/admin/users/{userId}/suspension` (suspension reasons are capped at 1,000 bytes and must contain no secrets because they are operationally logged; reactivation accepts no body), Cognito global session revocation at `POST /api/v1/admin/users/{userId}/sessions/revoke`, owner access-token controllers, bounded admin token metadata listing at `GET /api/v1/admin/users/{userId}/access-tokens`, and admin token revocation at `DELETE /api/v1/admin/users/{userId}/access-tokens` (bulk) and `DELETE /api/v1/admin/users/{userId}/access-tokens/{accessTokenId}` (single). No legacy `/api/v1/users/{userId}` admin item route remains.
- `newsletter/` owns public newsletter subscription REST controller. It uses optional canonical auth and a User service use case; production wiring reads Postgres user-profile fallback data and writes Zoho Campaigns subscriptions.
- `notifications/` owns authenticated canonical notification list, seen-state update, and deletion REST controllers. It uses PostgreSQL-backed notification service use cases; list items expose a localized immutable reason-specific rendering snapshot but never origin-event or delivery/provider state. Watchlist main-price changes preserve tagged `MONETARY` source currency or explicit `ON_REQUEST` and ignore user currency preferences. Notification image URLs are already presented from the snapshot assessment and stored `showUnassessedOrSensitiveContent` preference. Notification item paths use canonical `{notificationId}` values and list pagination uses an opaque JSON `[created RFC3339 timestamp, Notification ID]` cursor.
- `watchlist/` owns watchlist REST controllers. ProductListing watchlist paths use `{productListingId}`. `GET /api/v1/me/watchlist` uses Postgres-backed `application` `Cursor`/`CursoredResult` pagination and API-local JSON cursor collection data; its tie-safe `searchAfter` is `[created RFC3339 timestamp, ProductListing ID]`. It returns personalized ProductListing details with `no-store`. Watch creation and inactive-to-active PATCH enforce active-entry quotas: Free 20, Pro 100, Ultimate unlimited. Missing ProductListings map to `404 PRODUCT_LISTING_NOT_FOUND`; withdrawn ProductListings map to `409 PRODUCT_LISTING_UNAVAILABLE` only for watch creation or reactivation, while notification-only updates and deactivation remain valid.
- `search_filters/` owns Postgres-backed saved-search filter CRUD and match-feedback REST controllers. Raw enhanced descriptions are validated at transport mapping before embedding or any write transaction.
- `billing/` owns authenticated Stripe checkout, portal, and management REST controllers backed by canonical User state and Stripe billing service use cases. Runtime requires `STRIPE_API_KEY`, checkout success/cancel URLs, portal return URL, and configured Pro/Ultimate monthly/yearly price IDs.
- `product_listings/` owns ProductListing detail, search, immutable history, and similar-listing controllers. History maps service-owned entries to REST: one committed domain event per item; changed entries expose ordered semantic changes and discovery exposes image count only. Canonical ProductListing response identity uses raw `sourceListingId`, issued `productListingTitleSlugId`, and a `source` summary (`listingSourceId`, `name`, `slugId`); title-slug detail lookup uses `/api/v1/product-listings/by-slug/{productListingTitleSlugId}`. Public detail by ID or title slug returns `404 PRODUCT_LISTING_NOT_FOUND` for withdrawn ProductListings; it never returns `410`. Search and saved-search filters use `listingSourceId` / `excludeListingSourceId`. ProductListing read paths use canonical `{productListingId}` values. Listing images are already presented by service: URLs are nullable and listing-level `contentPolicy` is explicitly nullable. API never computes image visibility. The stored `showUnassessedOrSensitiveContent` preference controls URL redaction; source APIs accept URLs only. Detail and watchlist use joined Postgres reads; search and KNN use OpenSearch facts plus service-owned one-query PostgreSQL ListingSource-summary hydration. ProductListing assessment never enters OpenSearch or emits an event.

- `partnership_applications/` owns canonical own/admin partnership application REST controllers at `/api/v1/me/partnership-applications` and `/api/v1/admin/partnership-applications`; admin detail, mark-in-review, and decision are at `/api/v1/admin/partnership-applications/{partnershipApplicationId}` and `/api/v1/admin/partnership-applications/{partnershipApplicationId}/decision`.
- `partnerships/` owns the admin-only bounded active Partnership collection/detail routes at `GET /api/v1/admin/partnerships` and `GET /api/v1/admin/partnerships/{partnershipId}`, semantic dissolution at `DELETE /api/v1/admin/partnerships/{partnershipId}`, idempotent membership grants/revocations at `PUT`/`DELETE /api/v1/admin/partnerships/{partnershipId}/members/{userId}`, and ListingSource grant/revocation at `PUT`/`DELETE /api/v1/admin/partnerships/{partnershipId}/listing-source-grants/{listingSourceId}`. Dissolution retains a historical `DISSOLVED` Partnership row but atomically removes all current membership/grant access; all mutations return `204 No Content` for changed and documented no-op outcomes. ListingSource grant creation requires the Partnership and ListingSource to share a Party; revocation validates both records and removes only the target grant. All use `Cache-Control: no-store` and safe Party data without contact data, provider credentials, or other secrets.
- `partner_product_listings/` owns synchronous partner ProductListing batch create, update, upsert, and DELETE-route withdrawal controllers at `/api/v1/listing-sources/{listingSourceId}/product-listings`. Batches use `sourceListingId`, a tagged main-price `MONETARY` or `ON_REQUEST` payload (monetary-only estimates), return partial failures with `listingSourceId` and `sourceListingId`, allow at most 100 entries, and call one ProductListing service use case per entry. Aura access tokens require `product-listings:write`.
- `webhooks/` owns direct Shopify and WooCommerce product webhook transport. The WooCommerce path uses `/api/v1/webhooks/woocommerce/{listingSourceId}`. Controllers pass raw provider bytes plus topic/signature headers to the WooCommerce-owned intake mapper; it checks `ProductListingsWrite` before provider work, verifies the signature before parsing, retains semantic product JSON, maps source fields to raw-values V2 with `MACHINE_DECIMAL`, and keeps provider price strings unchanged in source payload. An optional topic-scoped delivery receipt retains only a canonical semantic-source-payload evidence digest for 90 days; receipt expiry never removes captured raw-revision provenance. It maps valid `date_modified_gmt` source order only after topic/status maps to raw capture. `product.created` and `product.updated` events with missing or unsupported status use raw-capture authorization before acknowledgement. A `204` means a captured changed, unchanged, duplicate, or stale outcome, or an authorized ignored event—not completed canonical ProductListing mutation.
- `oauth/` owns OAuth REST controllers for admin-only client registration at `POST /api/v1/admin/oauth-clients`, the bounded client collection at `GET /api/v1/admin/oauth-clients`, client detail at `GET`, metadata update at `PATCH`, and deletion at `DELETE /api/v1/admin/oauth-clients/{clientId}`, authorization code, token, revoke, and introspection flows. Client administration requires the persisted `ADMIN` role for user and delegated-user principals; delegated creation/update/deletion requires `access-tokens:write`, and delegated collection/detail reads require `access-tokens:read`. Service use cases enforce delegated `access-tokens:read`/`write` capabilities, and authorization requests cannot exceed delegated caller scopes. Admin creation returns the raw client secret only once, points `Location` to the admin detail route, and uses `Cache-Control: no-store`; OAuth protocol routes remain under `/api/v1/oauth`.

- Cognito session revocation requires `AURA_HISTORIA_COGNITO_USER_POOL_ID` and AWS credentials permitted to call `cognito-idp:ListUsers` and `cognito-idp:AdminUserGlobalSignOut` for that pool. The User service reads the stored Cognito identity through `SqlxUserCognitoIdentityRegistryFactory`; the Cognito adapter finds the provider username from its opaque `sub` before sign-out. Neither assumes Aura `UserId` is a Cognito identifier.
- Newsletter runtime requires `ZOHO_LIST_KEY`, `ZOHO_CLIENT_ID`, `ZOHO_CLIENT_SECRET`, `ZOHO_REFRESH_TOKEN`, `ZOHO_ACCOUNTS_URL`, and `ZOHO_CAMPAIGNS_URL`; credentials and contact payloads are never logged.
- ProductListing text search first pages use a short repository read transaction for the latest persisted FX snapshot; continuation pages load the ProductListing cursor's pinned snapshot ID. It then uses native OpenSearch hybrid BM25 plus KNN when relevance-sorted query embedding succeeds; embedding failure and explicit non-score sorts use BM25. `PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED` is a strict boolean, default `false`; it selects concurrent source/user-state/assessment reads for this public-search handler only. ProductListing search runtime needs `OPENSEARCH_ENDPOINT_URL`; outside `STAGE=ephemeral`, it also needs `OPENSEARCH_USERNAME` and `OPENSEARCH_PASSWORD`. Black-box ProductListing search, partner-write, and Shopify webhook tests seed a complete persisted FX snapshot after each PostgreSQL fixture reset so current-FX reads exercise their real invariants.
- Search-filter create/update embeddings use Vertex AI Gemini through typed API config. `VERTEX_AI_PROJECT_ID` and `VERTEX_AI_LOCATION` may override the legacy project and `eu` defaults; Google ADC supplies credentials (normally `GOOGLE_APPLICATION_CREDENTIALS`).

## Ownership

- This doc rule `src/aura-historia-api/**`.
- Parent doc: `src/AGENTS.md`.

## Local Contracts

- Read repo root, `src/AGENTS.md`, then here before edit.
- Update this doc when env vars, route shape, dependencies, or runtime behavior changes.
- Public API route behavior must update `docs/swagger.yaml` and `docs/CHANGELOG.md` when routes become real.

## Work Guidance

- Keep runtime glue thin.
- API implements no service port or use case. It only maps HTTP/auth transport and composes adapters from adapter crates. Transport-local auth and readiness traits are allowed.
- Put business behavior in domain crates and services.
- Use runtime-neutral request/auth context; no API Gateway context.

## Test Lifecycle

- Keep API acceptance source modules by route/unit under `tests/api_cases/`, but run compatible modules through the single `tests/api.rs` suite binary. Shared Postgres, LocalStack/OpenSearch, and normal API server fixtures are process-lived; mutable DB/OpenSearch data resets after each test.

## Verification

- `cargo check -p aura-historia-api`
- `cargo test -p aura-historia-api --all-features`

## Child DOX Index


- `admin_overview/` — administrator overview REST controller.
- `listing_sources/` — ListingSource REST controllers.
- `parties/` — admin Party collection and detail REST controllers.
- `users/` — user account, admin, and access-token REST controllers.
- `newsletter/` — public newsletter subscription REST controller.
- `notifications/` — canonical notification REST controllers.
- `watchlist/` — watchlist REST controllers.

- `partnership_applications/` — canonical own/admin partnership application REST controllers.
- `partnerships/` — admin Partnership collection and detail REST controllers.
- `partner_product_listings/` — synchronous partner ProductListing batch write controllers.
- `oauth/` — OAuth REST controllers.
- `product_listings/` — canonical ProductListing detail and search REST controllers.
- `search_filters/` — saved-search filter and match REST controllers.
- `billing/` — Stripe billing session REST controllers.
- `webhooks/` — Shopify and WooCommerce product webhook REST controllers.
