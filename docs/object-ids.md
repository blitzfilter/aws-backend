# Object IDs

Aura object IDs are typed durable object identities backed by RFC 9562 UUIDv7 values. Humans and application protocols see TypeID v0.3 strings:

```text
<prefix>_<26-character lowercase TypeID suffix>
```

The prefix is part of the type contract. Parsing is strict: malformed, noncanonical, wrong-prefix, non-v7, and bare UUID input fail. No legacy UUID fallback exists.

Reference: <https://github.com/jetify-com/typeid/tree/main/spec>

## Ownership and implementation

`domain-primitives` owns the reusable `object_id_newtype!` facility, strict errors, serde, and TypeID codec. Entity crates own their concrete ID types.

The codec uses `strong_id` 0.4 with default features disabled and only `typeid` enabled. This dependency stays private to `domain-primitives`; generated domain APIs expose only Aura types and `uuid::Uuid`. Aura wrappers separately enforce UUIDv7 and canonical input because the dependency parser accepts other UUID versions.

Official TypeID v0.3 vectors prove the codec. New IDs use `Uuid::now_v7()`. Raw UUID construction is fallible.

## Prefix registry

Prefixes are durable and collision-free.

| Rust type | Prefix | Owner |
|---|---|---|
| `ProductListingId` | `pl` | `product-listing-core` |
| `PartyId` | `pty` | `party-core` |
| `ListingSourceId` | `ls` | `listing-source-core` |
| `UserId` | `usr` | `user-core` |
| `PartnershipId` | `psh` | `partnership-core` |
| `PartnershipApplicationId` | `pa` | `partnership-core` |
| `UserSearchFilterId` | `sf` | `search-filter-core` |
| `NotificationId` | `ntf` | `notification-core` |
| `NotificationDeliveryId` | `nd` | `notification-core` |
| `FxRateId` | `fx` | `fxrate-core` |
| `OAuthClientId` | `oc` | `credential-core` |
| `AccessTokenId` | `at` | `user-core` |
| `ProductListingRawStreamId` | `prs` | `product-listing-service` |
| `ProductListingRawRevisionId` | `prr` | `product-listing-service` |
| `EventId` | `evt` | `domain-primitives` |
| `CrawlerDomainId` | `cd` | `crawler` |
| `CrawlerReviewId` | `cr` | `crawler` |
| `CrawlerReviewPageId` | `crp` | `crawler` |
| `CrawlerReviewUrlId` | `cru` | `crawler` |

The crawler review types replace bare `Uuid` fields that identify persisted rows. They are durable crawler entities exposed by its review API. Crawler schema evaluation also has synthetic references today; those are not object IDs and must become an explicit input-page index/reference rather than nil or fabricated UUID values.

## Boundary policy

| Boundary | Representation |
|---|---|
| Domain and service | concrete typed ID backed by `Uuid` |
| `Display`, `FromStr`, serde | canonical prefixed TypeID |
| REST path, query, request, response | typed TypeID |
| Public cursor field that directly represents an object ID | TypeID |
| Semantic worker/SQS field and key | TypeID |
| Structured log object identity | TypeID |
| OpenSearch document and object-derived `_id` | TypeID |
| PostgreSQL PK/FK, UUID array, bind, and row | native `uuid` |
| CDC value read from PostgreSQL | UUID text, immediately mapped `Uuid -> typed ID` |

PostgreSQL adapters call `as_uuid()` or `into_uuid()` when writing. Reads use fallible `TryFrom<Uuid>` and report invalid persisted versions as corruption. Code must never stringify a typed ID and reparse it as UUID.

Database keyset order and advisory-lock bytes continue to use the backing UUID where that is the storage contract.

## Storage-only JSON

Internal persisted JSON deliberately keeps canonical lowercase hyphenated UUID text where PostgreSQL or storage codecs depend on UUID semantics:

- partnership application proposal `listing_source_id`;
- ProductListing event payload `listingSourceId`;
- ProductListing sale observation `fxRateId`;
- ProductListing enrichment `sourceEventId`;
- notification product snapshot `listing_source_id`;
- search-filter persisted ProductListing and ListingSource ID sets.

Crawler `validation_summary.schema_matrix` is also returned by the crawler review API, has no SQL UUID cast dependency, and therefore uses TypeIDs for persisted `CrawlerReviewId` and `CrawlerReviewPageId` references. Evaluations created before a review row exists use an explicit absent review ID and input-page index/reference. They never use nil or fabricated UUID placeholders.

Adapter-local UUID codecs must encode with the backing UUID and decode `UUID -> typed ID` explicitly. They must not use object-ID `Display` or public serde. Public history, notification, and crawler review DTOs expose TypeIDs.

Raw source payload, normalization context, and provenance remain opaque source evidence; Aura identities belong in dedicated native UUID columns unless a field is explicitly documented above.

## External identity and exclusions

Cognito `sub` is an opaque provider identity, not `UserId`. Persist the verified `(issuer, subject)` separately and resolve it to an independently generated `usr_` UUIDv7. Session revocation reads the stored Cognito identity; it never derives a subject from `UserId`.

These are not Aura object IDs:

- `PartySlugId`, `ListingSourceSlugId`, `ProductListingSlugId`;
- `SourceListingId` and other provider-controlled IDs;
- OAuth authorization codes, exchange codes, client secrets, raw access tokens, and PKCE values;
- webhook/provider delivery IDs and Stripe customer IDs;
- notification lease tokens and crawler session cookies;
- request IDs, correlation IDs, idempotency keys, SQS receipt/message IDs, Sequin delivery IDs/LSNs, and OpenSearch PIT IDs;
- URLs and secret/webhook credentials.

`OAuthClientId` and `AccessTokenId` identify durable records and are object IDs. Their associated secret or bearer values are not.

The legacy `partner_shop_application_id` notification column has no current object type. It is stale schema, not a new object-ID contract.

## Wire and reset policy

This change is intentionally breaking.

- REST accepts no bare UUID object IDs.
- Worker semantic wire format becomes schema version 2; version 1 is rejected, not compatibility-decoded.
- CDC remains raw UUID because it mirrors PostgreSQL.
- OpenSearch indexes are rebuilt with TypeID documents; old UUID documents are not read.
- Development PostgreSQL and crawler databases reset so all object rows become UUIDv7. Crawler UUIDv4 database defaults are removed; application code generates typed UUIDv7 values before inserts.
- Development queues drain/reset before schema-v2 deployment.
- Cognito users must be recreated or explicitly registered into the new issuer/subject mapping after a database reset.
- Stripe sandbox metadata carrying old User IDs resets or is rewritten.

## Adding an object ID

1. Confirm the value is a durable Aura object identity, not a slug, credential, external ID, or operational token.
2. Add one short unique lowercase prefix to this registry.
3. Define the type in its semantic owner:

   ```rust
   domain_primitives::object_id_newtype!(ExampleId, "ex");
   ```

4. Persist only `id.as_uuid()` in native PostgreSQL `uuid` columns.
5. Expose the typed ID through public/application boundaries.
6. Test prefix assignment, wrong-prefix rejection, bare UUID rejection, serde, and storage roundtrip.
7. Never use `Display` as a persistence codec.
