# Iteration 10 — Public Auction browsing

**Status:** PASS

## Objective

Ship bounded public PostgreSQL reads for Auction directory, detail, and visible catalogue.

## Delivered

- `GET /api/v1/auctions` uses fixed `created DESC, auction_id DESC` keyset paging. It filters exact source, format, reported status, and explicit exact-instant schedule role ranges `[from,to)`; date-only points do not match.
- `GET /api/v1/auctions/{auctionId}` returns safe source data, explicit schedule, source-reported lot count, and separate active visible listing count. It does not expose source keys, evidence, policy, audit, or versions.
- `GET /api/v1/auctions/{auctionId}/product-listings` validates Auction existence separately from page size. It reads active assigned listings from PostgreSQL in `catalogue_position ASC NULLS LAST, product_listing_id ASC` order with an Auction-scoped keyset cursor, then uses the normal ProductListing detail presentation for localized text, source/referral URL, exact FX pricing, user state, content-policy image redaction, lot timing, and safe Auction summary.
- All routes accept optional authenticated personalization context, use strict TypeIDs, map missing Auctions to `AUCTION_NOT_FOUND`, and return `Cache-Control: no-store`.

## Non-goals

No Auction OpenSearch index, Auction-ID full-text search filter, saved-search codec, metadata fan-out, bidding, result, or reminder work.

## Contract/reset

This iteration adds readers and public routes only. No schema, raw, event, index, queue, or discriminator contract changes. No reset was run or needed.

## Verification

```text
cargo fmt --all -- --check                                              PASS
cargo depgraph-check check                                               PASS
cargo check --workspace                                                   PASS
cargo clippy --locked -p auction-service -p auction-postgres \
  -p product-listing-service -p product-listing-postgres -p aura-historia-api \
  --all-targets --all-features -- -D warnings -D clippy::result-large-err PASS
cargo test -p auction-service -p auction-postgres -p product-listing-service \
  -p product-listing-postgres -p aura-historia-api --lib --all-features  PASS
cargo test -p product-listing-postgres --tests --all-features            PASS
cargo test -p auction-postgres --tests --all-features                    PASS
cargo test -p aura-historia-api --test api \
  should_browse_public_auction_directory_detail_and_empty_catalogue_anonymously \
  --all-features                                                         PASS
cargo test --workspace --lib --all-features                              PASS
```

Focused router tests and full workspace library tests passed. Real PostgreSQL coverage proves active-only catalogue visibility, position/UUID tie ordering, null-position continuation, empty existing catalogues, full context/referral mapping, exact directory role/range `[from,to)` behavior with date-only exclusion, detail missing behavior, and independent reported/visible counts. The test harness has no established query-count probe; no custom counter was added. The reader implementation remains one bounded catalogue query plus the existing bounded presentation reads.

## Audit

No compatibility reader, new protocol version, dual write, transition migration, Auction index, controller SQL, or Auction execution capability was added. Public directory decoding now permits omitted optional exact-time bounds; invalid supplied optional credentials remain `401`, never anonymous.
