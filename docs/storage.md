# Storage Contracts

## Notifications

PostgreSQL is the sole production owner of notifications and external-delivery intent. Notification storage has no TTL.

- `notifications` stores one immutable typed content snapshot per semantic reason. `origin_event_id` is provenance only and is absent for partnership-application notifications.
- Watchlist idempotency is `(user_id, origin_event_id, kind)`. Search-filter idempotency is `(user_id, user_search_filter_id, product_listing_id, origin_event_id)`. Partnership-application idempotency is `(user_id, partnership_application_id)`.
- A single Product event may create a watchlist notification and one distinct notification for every matching search filter.
- `notification_deliveries` owns durable external-delivery state, separate from a Notification. One Notification may have several delivery rows. Each row is unique per `(notification_id, channel, target_key)` and contains no copied notification payload or target value. The application planner selects channels and inserts requested rows in the same PostgreSQL transaction as newly inserted notifications; each channel adapter resolves its own target. EMAIL/PRIMARY is the sole production plan.
- The generic delivery worker consumes committed `notification_deliveries` INSERT rows through one Sequin subscription, claims a lease, dispatches by channel, and finalizes that same lease. EMAIL is the only valid stored channel now; a future channel needs a core enum and schema migration plus its sender. EMAIL resolves its current target and performs S3/SES I/O after the claim transaction commits. A send/finalize crash can duplicate an external delivery, so delivery is at-least-once, not exactly-once. The bounded in-memory worker queue remains non-durable by design; its post-ack loss window is tracked by #1558.
- Read models and REST mutations use `notification_id`; they never expose `origin_event_id`.
- Corrupt persisted payload/version/source-shape state is an operation error. It is never silently treated as a missing notification.

## Watchlist concurrency and intervals

- `product_listing_watchlist.version` is internal optimistic-concurrency metadata. Ordinary aggregate updates compare the loaded version and increment it once; a mismatch is a conflict, never not-found or a silent retry. REST never exposes it.
- Tier reconciliation locks the user row first, then affected watchlist rows. It increments `version` for every changed row, so concurrent stale user writes fail.
- `product_listing_watchlist.active_since` is non-null only for `ACTIVE` rows and marks the beginning of the current active interval.
- `product_listing_watchlist.notifications_enabled_since` is non-null exactly when `notifications = true` and marks the beginning of the current email-enabled interval.
- Watchlist notification readers compare both interval starts with immutable `product_listing_events.event_time`; deactivation/reactivation and email disable/re-enable start new intervals. These fields are repository-owned persistence metadata, not REST payload fields.

## Credentials

PostgreSQL is authoritative for User access tokens and canonical OAuth credentials:

- `access_tokens` stores only token short/hash material, never a raw User access token. It has an internal optimistic-concurrency version; `user_id` cascades on User deletion. OAuth origin keeps its client ID as historical data without a foreign key, so deleting a client does not silently revoke issued tokens.
- `oauth_clients` stores only client-secret short/hash material, never a raw client secret. Client metadata updates use optimistic concurrency.
- `oauth_authorization_codes` and `oauth_third_party_exchange_codes` are one-time rows consumed atomically with `DELETE ... RETURNING`.
- A successful authorization-code exchange consumes the authorization code, creates the User access token, and creates the third-party exchange code in one PostgreSQL transaction. Semantic misuse of a found code commits its deletion; a later persistence failure rolls the valid exchange back.
- Expiry remains service correctness. `pg_ttl_index` performs asynchronous physical cleanup from absolute `expires_at` values with offset `0` for access tokens, authorization codes, and third-party exchange codes. Non-expiring access tokens have `expires_at IS NULL`.
- Credential tables are operational only. They must not enter Sequin/CDC publications, projections, analytics, or credential-bearing logs. The raw token in a third-party exchange-code row is short-lived escrow needed by that exchange only.

The initial business schema requires a provisioned and preloaded `pg_ttl_index` extension before it runs.

## ProductListing events and revisions

`product_listings` remains the authoritative ProductListing write model. Its revision fields have separate purposes:

- `version` is numeric aggregate optimistic-concurrency metadata. It starts at 1, advances once for each changed domain write, and never advances for enrichment or assessment writes.
- `current_event_id` identifies the latest projection-visible ProductListing event.
- `projection_version` is the positive monotonic external source version for complete OpenSearch projection writes.
- `content_source_event_id` identifies the title/description source used by content assessment and translation.
- `embedding_source_event_id` identifies the title/description/first-image source used by embedding. Discovery initializes it; an image change advances it and clears the stored embedding atomically.

These are separate concepts. Enrichment advances `current_event_id` and `projection_version` when it changes projection-visible state, but not aggregate `version`.

`product_listing_events` is the immutable ProductListing event journal and direct Sequin CDC source, not an outbox. Every row has immutable event ID/time, a positive persisted schema version, and an object JSON payload. Allowed groups are `DOMAIN` and `ENRICHMENT`, with the initial schema constraining domain events to `PRODUCT_LISTING_DISCOVERED`/`PRODUCT_LISTING_CHANGED` and enrichment events to `ENRICHMENT_EMBEDDED`/`ENRICHMENT_TRANSLATED_TITLES`. Application and router code fail closed on the concrete v1 type/group/version/payload contracts. Deferred same-listing foreign keys tie current and source marker IDs to journal rows.

`product_listing_raw_streams` is a mutable, change-only capture head for `WEB_CRAWL`, `SHOPIFY`, and `WOOCOMMERCE`; `product_listing_raw_revisions` is immutable source evidence. `product_listing_raw_normalization_heads` serializes stream progress and binding, while `product_listing_raw_normalizations` retains immutable terminal outcomes and stable rejection codes. Raw JSON has no broad GIN index. Raw revisions CDC only to the `product-listing-normalization` worker scope; they never enter `product_listing_events` or downstream ProductListing consumers. Safe backlog, rejection, growth, and crawler-dormancy queries are in `docs/product-listing-raw-normalization-runbook.md`.

Public history reads only `DOMAIN` `PRODUCT_LISTING_DISCOVERED` and `PRODUCT_LISTING_CHANGED` rows. It strictly decodes v1 payloads through direct DTO mapping without aggregate reconstruction, orders by `event_time ASC, event_id ASC`, and reports invalid persisted event data as an operation error instead of silently omitting it.

## Indexed read paths

- User watchlist lists use `created DESC, product_listing_id ASC`; reverse product watcher reads use `product_listing_id, user_id ASC`.

- Saved-filter matches support both `created ASC, product_listing_id ASC` and `created DESC, product_listing_id ASC` for a fixed filter.
- Product domain-event history orders by `event_time ASC, event_id ASC` for one product.

## FX snapshots

PostgreSQL is authoritative for canonical FX data.

- `fx_rates` stores immutable snapshot identity, database-assigned monotonic `generation`, capture time, provider source, and idempotent provider event ID.
- `fx_rate_quotes` stores one positive scaled `units_per_eur` quote for every supported currency, including `EUR = 1_000_000`.
- Quote rows are not arbitrary pairwise rates. They are inserted with their parent in one PostgreSQL transaction.
- OpenSearch does not store authoritative FX data. Its Product and saved-filter documents are rebuildable projections only.
- Product reads, writes, search, and projections must use persisted snapshots. They must not call the FX provider.
- ProductListing source pricing never stores an FX ID. An explicit `ListingSaleObservation` stores paired immutable `sale_observation_fx_rate_id` and `sale_observed_at`; both are null or both present. Only the dedicated observation write selects the latest `captured_at <= observed_at`. Generic create, update, and upsert never read FX or infer an observation from availability.
- ProductListing detail, watchlist, and saved-match readers return source pricing plus an optional observation. They select latest `captured_at <= valuation_at` for current pricing. They select the stored observation snapshot only for `SoldOut` listings or intentional withdrawn history; active relisted listings always use current FX. They use scaled-integer HalfUp conversion; missing, invalid, or mismatched snapshots fail explicitly.
- ProductListing OpenSearch documents store only optional native `sourcePrice { amount, currency }`. A `SoldOut` observation may project `saleObservationFxRateId`, `saleObservedAt`, and complete target-currency `salePrices`; active relisted listings omit this sale-observation pricing. Partial observation metadata or partial `salePrices` is invalid. Estimates never enter ProductListing OpenSearch.
- Product search first pages capture `valuation_at` and pin latest persisted `captured_at <= valuation_at`; each continuation loads the cursor's exact `fx_rate_id`, never a newer snapshot. A requested display range compiles to exact native `sourcePrice` intervals for active product_listings and to the original target-currency range on immutable sold `salePrices`; a sold Product without `salePrices` never matches a price range. Price sorting is not supported.
- `product_listings.projection_version` is the positive monotonic source version for rebuildable Product OpenSearch writes. It advances for Product aggregate, embedding, and translation changes; the projection uses OpenSearch external versioning.
- Saved-filter documents retain the requested price range and compile it directly against private temporary `priceByCurrency.<currency>` fields. They store no FX ID or generation. For an accepted current Product event, percolation selects the immutable sale snapshot when present; otherwise it selects latest `captured_at <= product_listing_events.event_time` ordered by `captured_at DESC, generation DESC`. It converts the native main price into every supported currency with checked HalfUp arithmetic. FX capture alone does not rewrite saved filters, reproject Products, create matches, or send notifications.
- `search_filter_matches.price_valuation_basis` and `price_fx_rate_id` are null together for non-price matches. Price matches persist `CURRENT` with the periodic run snapshot, `EVENT` with the event-effective snapshot, or `SALE_OBSERVATION` with the immutable observation snapshot. The FX ID references immutable `fx_rates`.
- `search_filter_periodic_match_state` holds durable per-filter periodic matching progress. Missing rows start at the authoritative Search Filter creation timestamp; this operational state is not part of ordinary Search Filter read views.

`fx_rates.source_event_id` is the scheduled or deployment-bootstrap capture idempotency key. Canonical captures serialize and require `captured_at` to be strictly newer than the prior capture, except duplicate source IDs; historical import needs a separate workflow. `generation` orders persisted snapshots; `captured_at` supports historical selection.
