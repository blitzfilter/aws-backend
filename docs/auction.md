# Auctions

**Status:** iterations 01–11 pass. See the [implementation plan](auction-implementation.md) and [iteration 11 record](auction-iterations/11-auction-membership-search.md).

## Scope

An Auction is Aura's source-scoped record of one sale occasion. It supports discovery, cataloguing, metadata correction, and public reads. It is **not** an execution platform: no bids, winners, payments, results, auctioneer Party attribution, venue, sessions, global merge, physical-object deduplication, or reminder delivery are in scope.

## Target identity

`auction-core` owns `AuctionId` (`auc_` strict UUIDv7 TypeID). `auc` is registered in the [object-ID registry](object-ids.md). PostgreSQL stores its backing UUID.

An Auction key is:

```text
(ListingSourceId, SourceAuctionId)
```

`SourceAuctionId` is a trimmed, opaque source value: 1–512 UTF-8 bytes after outer Unicode-whitespace trimming, no NUL, and exact preservation of case, punctuation, and internal whitespace. It is unique only within its ListingSource. Names, schedules, URLs, lot labels, Parties, and source operators are not key parts. A URL-derived ID is allowed only for a source-specific, fixture-backed extractor; there is no generic URL or name matching.

## Current persistence and administration

PostgreSQL is authoritative for standalone source-scoped Auctions. The initial business schema owns:

- `auctions`, with immutable `(listing_source_id, source_auction_id)` uniqueness, a root optimistic-lock version, restrictive ListingSource foreign key, and optional localized metadata;
- bounded `auction_schedule_points`, one row per asserted schedule role;
- immutable `auction_events` for `AUCTION_DISCOVERED` and `AUCTION_CHANGED` payloads using the established journal convention and schema value `1`; it has no CDC or worker consumer in this iteration;
- restricted `auction_metadata_policy_audits` and closed-world `auction_metadata_field_protections` for fields touched by an administrator.

`auction-service` has authenticated-administrator create, update, and detail use cases. They use one caller-owned PostgreSQL transaction for root state, journal write, and policy audit. Admin creation protects supplied fields; an explicit update protects every touched field, including a clear or equal write. A policy-only touch advances the Auction storage version and audit state, but appends no false domain event. Audit actor labels are validated before persistence (nonempty, no NUL, at most 512 UTF-8 bytes).

Iteration 03 exposes only these administrator routes: `POST /api/v1/admin/auctions`, `GET /api/v1/admin/auctions/{auctionId}`, and `PATCH /api/v1/admin/auctions/{auctionId}`. They require the persisted administrator role and always return `Cache-Control: no-store`. Create requires immutable `listingSourceId` and `sourceAuctionId`, returns `201` plus the detail `Location`, and rejects duplicate source keys with `409 CONFLICT`. GET/PATCH require strict `auc_` TypeIDs; bare UUIDs and wrong prefixes return `400 INVALID_OBJECT_ID`. PATCH requires a positive `expectedVersion`; omitted fields remain unchanged and explicit `null` clears only documented metadata/schedule fields. Its response exposes the resulting `expectedVersion` and closed-world `protectedFields` for administration only. Exact schedule instants and date-only values retain their precision; no public Auction endpoint exists yet.

Embedded metadata acceptance is implemented as a service policy for later listing integration: it can fill only an absent, unprotected shared field. Equal, protected, and conflicting candidates do not replace shared facts. It is not reachable from raw or partner listing writes until iteration 06.

A retained Auction blocks ListingSource deletion, including ID-only, ended, and cancelled Auctions. There is no Auction deletion endpoint or lifecycle in this iteration.

The initial business schema changed directly. Local disposable PostgreSQL state must be recreated through the established test harness or an explicitly authorized local reset before running a checkout with this schema; no shared database, queue, or remote environment was reset here.

## Current ProductListing context

Iteration 06 has three distinct listing values:

| Stored value | Meaning |
| --- | --- |
| `None` | No reliable auction-participation assertion. |
| `Some` with no membership | Auction participation is asserted but source identity is unresolved. |
| `Some` with `AuctionMembership` | The listing is assigned to one same-source Auction. |

An asserted empty context never collapses to `None`. The context holds optional `AuctionMembership`, opaque `LotNumber`, one-based `CataloguePosition`, and qualified lot timing. A lot remains one ProductListing even if it describes multiple physical objects. Reliable `SourceAuctionId` resolves or creates an Auction inside the caller-owned ProductListing transaction; name, URL, and timing never infer identity.

Typed partner creation accepts omitted or `null` auction as no assertion. Partner and raw writes can supply `auction.sourceAuctionId`; the same source key resolves/creates one Auction and embedded metadata can fill absent shared fields. Existing membership plus a different reliable key fails with `MEMBERSHIP_CHANGE_REQUIRES_CORRECTION`; no second Auction is created for that rejected reassignment. Typed update/upsert omits auction to preserve it and rejects `null`; raw `auction: CLEAR` also preserves existing context. No key is inferred from name, URL, or timing.

Crawler extraction is a raw producer. Its only implemented source rule is fixture-backed Lot-tissimo lot URLs: the exact HTTPS `/{locale}/auction-catalogues/{auctioneer}/catalogue-id-{id}/lot-{id}` shape yields the opaque source Auction ID and catalogue URL. Reviewed selector evidence supplies optional catalogue name and lot number. It emits the current `auctionMetadata` raw field; the real PostgreSQL raw-to-normalization path proves that one source key links one Auction, later conflicting embedded fields preserve initial accepted values, and absent shared fields can fill. Unknown hosts, malformed paths, query/fragment wrappers, names without that URL rule, and unqualified timing leave the Auction patch unchanged; crawler never resolves or writes canonical state.

Iteration 07 adds an administrator-only complete-context correction. It checks both the ProductListing version and an independent auction-policy version; absent policy is version `0`. A correction may remove the outer assertion, leave it unresolved, or assign a same-source Auction. It requires a trimmed nonblank restricted reason (1–1,024 bytes), rejects withdrawn listings, retains unrelated listing state, and activates a listing-owned override barrier even when the final context is unchanged. The reason, actor/audit data, raw evidence, and policy state are never public Listing history or discovery data.

While active, raw auction patches are preserved with `MANUAL_AUCTION_OVERRIDE_PRESERVED`; unrelated normalized facts still apply. Typed partner create/update/upsert attempts to alter an existing listing's Auction context conflict atomically. An administrator can release the barrier only with matching listing/policy versions. Release changes policy only: it appends no ProductListing event and does not alter listing facts. It records the global immutable raw-capture generation and currently linked stream revision floors, so observations captured at or before release cannot replay Auction context after release. Later captures use ordinary membership policy.

## Time semantics

Auction times in `auction-core` retain either an exact instant or a source calendar date, with a validated IANA source timezone when supplied. Date-only values never become midnight instants. Exact comparisons and future exact-time filters use only instants; date-only values remain visible but do not match them.

Auction schedule roles are `BIDDING_OPENS`, `LIVE_STARTS`, `LOTS_BEGIN_CLOSING`, and `SCHEDULED_END`. Lot roles are `bidding_opens`, `scheduled_closes`, and exact `reported_closed_at`. Auction-level milestones are never copied into lot deadlines. Passing a scheduled time never changes status, availability, sale observation, or result.

Current listing search and saved filters expose half-open `[min, max)` exact-instant filters named `lotBiddingOpens` and `lotScheduledCloses`. They exclude absent/date-only values and never represent auction-level milestones. `lotReportedClosedAt` is projected for current listing facts but is not a filter in this iteration.

## Target metadata policy

A reliable source key is sufficient for grouping, not for arbitrary metadata replacement. Embedded metadata from a listing/crawler/ordinary partner write may fill only absent, unprotected shared Auction fields. Equal fields are no-ops; conflicts and protected fields are preserved with bounded diagnostics. Administrators can set or clear named fields with expected-version checking; every explicitly touched field remains protected, including a cleared value. Policy-only changes write restricted audit data and do not invent domain events.

Auction discovery and semantic metadata changes use `AUCTION_DISCOVERED` and `AUCTION_CHANGED`. Listing membership/context changes remain ProductListing domain changes. Raw evidence links and normalization diagnostics are ingestion-owned and commit with the source revision's canonical work.

## Target reads and boundaries

Public browsing is PostgreSQL-backed: `GET /api/v1/auctions`, `GET /api/v1/auctions/{auctionId}`, and `GET /api/v1/auctions/{auctionId}/product-listings`. Detail returns safe source data, explicit schedule values, source `reportedLotCount`, and separate current `visibleListingCount`; it never claims catalogue completeness. Directory uses fixed `created DESC, auction_id DESC` keyset order and may filter exact instant schedule points by explicit role using `[from,to)`, excluding date-only values. Catalogue returns only visible active assigned listings, uses the normal personalized ProductListing detail presentation, orders `cataloguePosition ASC NULLS LAST` then backing listing UUID, and has an Auction-scoped cursor. These reads use `Cache-Control: no-store`; an ID-only Auction and an empty visible catalogue are valid. Listing full-text search remains OpenSearch and exposes an exact `auctionId` filter over resolved listing-owned membership only. Repeated strict `auc_` IDs OR together (maximum 100 distinct IDs) and intersect other filters before pagination; unresolved/no-context listings never match. Saved-search codecs and percolation use the same filter and tier policy as `listingSourceId`. Auction metadata is batch-hydrated from PostgreSQL; no Auction OpenSearch index or metadata fan-out projection is planned.

The directory, catalogue, and existing listing reads preserve current visibility, image assessment, localization, FX, and referral-url behavior. Existing ProductListing detail, search, similar, and watchlist reads now batch current resolved Auction summaries from PostgreSQL. A summary contains only Auction ID, optional localized name, format, reported status, and qualified schedule. No context remains no context; an asserted context without membership remains unresolved; a resolved ID missing from the authoritative batch is an integrity failure, not an unresolved result. Search remains eventually consistent for listing-owned facts while its Auction summary is current; no shared metadata is indexed or fanned out. Public data never exposes source auction keys, raw evidence, correction reasons, actor details, authority state, or persistence versions.

## Breaking development rewrite

This is a direct development-only rewrite. It will replace raw/API/event/index contracts in their owning iterations; it will not introduce a successor contract version, compatibility reader, dual write, aliases, backfill, or migration bridge. Existing correctness counters—aggregate versions, raw stream revisions, event IDs, policy CAS, and projection fences—remain.

Current immutable raw observations will not be rewritten during normal operation. Incompatible disposable development data must be reset and recaptured only under explicit authorization. Iteration 07 extends the initial business schema; no reset was run. The exact affected records and approved tooling are recorded per iteration.
