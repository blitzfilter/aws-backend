# Auctions

**Status:** iteration 01 implements the pure `auction-core` model only. `Auction`, its strict `auc_` identity, source key, optional metadata, discovered/changed payloads, and precision-bearing schedule values exist. No Auction table, route, raw field, listing membership, or API behavior exists yet. Current ProductListing auction timestamps remain the shipped baseline until their owning iteration replaces them.

See [implementation plan](auction-implementation.md) and the [iteration records](auction-iterations/00-inventory.md).

## Scope

An Auction is Aura's source-scoped record of one sale occasion. It supports discovery, cataloguing, metadata correction, and public reads. It is **not** an execution platform: no bids, winners, payments, results, auctioneer Party attribution, venue, sessions, global merge, physical-object deduplication, or reminder delivery are in scope.

## Target identity

`auction-core` owns `AuctionId` (`auc_` strict UUIDv7 TypeID). `auc` is registered in the [object-ID registry](object-ids.md). Future PostgreSQL ownership will store its backing UUID.

An Auction key is:

```text
(ListingSourceId, SourceAuctionId)
```

`SourceAuctionId` is a trimmed, opaque source value: 1–512 UTF-8 bytes after outer Unicode-whitespace trimming, no NUL, and exact preservation of case, punctuation, and internal whitespace. It is unique only within its ListingSource. Names, schedules, URLs, lot labels, Parties, and source operators are not key parts. A URL-derived ID is allowed only for a source-specific, fixture-backed extractor; there is no generic URL or name matching.

## Target ProductListing context

The final listing-owned context has three distinct states:

| Stored value | Meaning |
| --- | --- |
| `None` | No reliable auction-participation assertion. |
| Context with no membership | Auction participation is asserted, but membership is unresolved. |
| Context with membership | The listing belongs to exactly one same-source Auction. |

An asserted empty context is valid and must not collapse to `None`. The context will hold optional opaque `LotNumber`, optional one-based `CataloguePosition`, and qualified lot timing. A lot remains one ProductListing even if it describes multiple physical objects.

Ordinary source and partner writes may attach an unresolved listing or a listing with no context to a reliable same-source key. They preserve a current membership when evidence is sparse, and reject/diagnose A-to-B membership changes. They never infer a key from a name. Administrative correction is the only reassignment or retraction path; it replaces the entire context with a required restricted reason. A listing-owned override barrier then blocks ordinary source/partner auction changes until an explicitly safe release establishes raw-stream revision floors.

## Time semantics

Auction times in `auction-core` retain either an exact instant or a source calendar date, with a validated IANA source timezone when supplied. Date-only values never become midnight instants. Exact comparisons and future exact-time filters use only instants; date-only values remain visible but do not match them.

Auction schedule roles are `BIDDING_OPENS`, `LIVE_STARTS`, `LOTS_BEGIN_CLOSING`, and `SCHEDULED_END`. Lot roles are `bidding_opens`, `scheduled_closes`, and exact `reported_closed_at`. Auction-level milestones are never copied into lot deadlines. Passing a scheduled time never changes status, availability, sale observation, or result.

The eventual public contracts use half-open `[from, to)` exact-instant filters with role-bearing names. They distinguish auction milestones from lot milestones.

## Target metadata policy

A reliable source key is sufficient for grouping, not for arbitrary metadata replacement. Embedded metadata from a listing/crawler/ordinary partner write may fill only absent, unprotected shared Auction fields. Equal fields are no-ops; conflicts and protected fields are preserved with bounded diagnostics. Administrators can set or clear named fields with expected-version checking; every explicitly touched field remains protected, including a cleared value. Policy-only changes write restricted audit data and do not invent domain events.

Auction discovery and semantic metadata changes use `AUCTION_DISCOVERED` and `AUCTION_CHANGED`. Listing membership/context changes remain ProductListing domain changes. Raw evidence links and normalization diagnostics are ingestion-owned and commit with the source revision's canonical work.

## Target reads and boundaries

The eventual public resources are a bounded PostgreSQL Auction directory, Auction detail, and a PostgreSQL catalogue of visible assigned ProductListings. Listing full-text search remains OpenSearch and will expose an exact `auctionId` filter over listing-owned membership only. Auction metadata is batch-hydrated from PostgreSQL; no Auction OpenSearch index or metadata fan-out projection is planned.

The directory, catalogue, and existing listing reads preserve current visibility, image assessment, localization, FX, and referral-url behavior. Public data never exposes source auction keys, raw evidence, correction reasons, actor details, authority state, or persistence versions.

## Breaking development rewrite

This is a direct development-only rewrite. It will replace raw/API/event/index contracts in their owning iterations; it will not introduce a successor contract version, compatibility reader, dual write, aliases, backfill, or migration bridge. Existing correctness counters—aggregate versions, raw stream revisions, event IDs, policy CAS, and projection fences—remain.

Current immutable raw observations will not be rewritten during normal operation. Incompatible disposable development data must be reset and recaptured only under explicit authorization. The exact affected records and approved tooling are recorded per iteration.
