# Iteration 08 — crawler Auction extraction

**Status:** PASS

## Objective

Capture reliable source Auction evidence from crawler pages without giving crawler canonical-write authority.

## Delivered

- One Lot-tissimo-only extractor accepts only the fixture-backed HTTPS lot URL shape:
  ```text
  /{locale}/auction-catalogues/{auctioneer}/catalogue-id-{sourceAuctionId}/lot-{sourceLotId}
  ```
- It rejects unknown hosts, malformed IDs/path shapes, and query/fragment wrappers. It does not hash URLs or match names.
- The checked-in Lot-tissimo fixture now has reviewed selector evidence for `rawAuctionName` and `rawAuctionLotNumber`.
- The crawler maps this source key, catalogue URL, optional name, and lot label into the one current raw `auction` patch. Shared facts use `auctionMetadata`; no old raw spelling or schema discriminator was introduced.
- Crawler still only captures immutable raw observations. Normalization and transactional Auction resolution remain outside crawler.

## Non-goals

No generic URL rule, name-only resolution, source-wide timing inference, direct ProductListing/Auction writes, Auction browse/search, bids, outcomes, Party attribution, or session model.

## Verification

```text
cargo fmt --all -- --check                                                        PASS
cargo depgraph-check check                                                         PASS
cargo check --workspace                                                            PASS
cargo check -p crawler                                                             PASS
cargo test -p crawler --lib scraper::auction::tests --all-features                PASS
cargo test -p crawler --lib scraper::raw_input::tests --all-features              PASS
cargo test -p crawler --test scraper_parsing_pipeline --all-features              PASS
cargo test -p crawler --all-features                                               PASS
cargo test -p product-listing-normalization --all-features                         PASS
cargo test -p product-service --all-features                                       PASS
cargo test -p product-listing-postgres --test product_listing_raw_normalization \
  --all-features should_resolve_crawler_auction_and_fill_only_absent_embedded_metadata \
  -- --exact                                                                       PASS
cargo test -p aura-historia-worker --lib --all-features                            PASS
cargo test -p aura-historia-worker --test process_durability --all-features \
  should_persist_accepted_work_after_process_dies_before_handler_commit_t07 -- --exact  PASS
cargo test -p aura-historia-worker --all-features                                  PASS
```

The PostgreSQL integration test captures two crawler-shaped raw revisions, then drives the real normalizer and resolver. It proves one same-source Auction is linked to the listing, later conflicting name/URL/format candidates preserve initially accepted values, and an initially absent reported lot count fills.

The full worker suite completed under the authorized 20-minute bound, including its real-infrastructure process tests.

No shared database, queue, remote data, or deployment was reset. A matching development checkout needs an explicitly authorized disposable-environment reset before use.
