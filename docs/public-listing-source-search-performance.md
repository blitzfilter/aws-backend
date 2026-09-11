# Public ListingSource search performance harness

## Status

**Executed locally on September 11, 2026.** The expanded ignored harness passed all 1,000, 10,000, and 100,000 ListingSource scenarios, including real SQLx handler timing. Cargo reported 10.97 s test time. These are one warm local-run result, not a deployment latency claim.

## Run

Requires Docker and the test-api PostgreSQL image. From repository root:

```sh
cargo test -p listing-source-postgres --lib public_listing_source_search_reader::tests::should_capture_public_listing_source_search_postgres_performance --all-features -- --ignored --nocapture
```

The ignored test uses the existing disposable `Postgres::new("migrations")` test-api convention. It creates the pinned process-local database, applies `migrations/`, and truncates application data after the test. Do not point it at a shared or remote database.

`AURA_TEST_POSTGRES_IMAGE` is optional and only for explicit local image testing. When set, record its exact value; otherwise record the pinned image reference used by test-api.

## Workload

Primary target table: `listing_sources`. The workload also reads and joins `parties`.

The default command runs all three isolated scenarios. Before each scenario it truncates prior fixture rows, seeds and validates source/operator rows, and runs `ANALYZE parties` plus `ANALYZE listing_sources`.

| Scenario | ListingSources | Operators | Distribution |
| --- | ---: | ---: | --- |
| Small | 1,000 | 50 | 20 sources/operator |
| Medium | 10,000 | 500 | 20 sources/operator |
| Large | 100,000 | 5,000 | 20 sources/operator |

Every scenario includes source and operator names with normalized `muller` prefix and `auction` contains populations, plus ordinary browse rows. It then prepares and executes the exact public-reader SQL shapes for:

- browse first page and a later cursor page;
- prefix first page and a later cursor page;
- contains first page and a later cursor page;
- exact ListingSource slug lookup.

Every prepared statement is run in every scenario with `plan_cache_mode` forced to `force_custom_plan` and then `force_generic_plan`. Output labels include `scenario_listing_sources` and `scenario_operators`. Prepared statement names include the source-count scenario, so cases can share a pooled PostgreSQL session. The harness never issues `DEALLOCATE ALL`: that command invalidates SQLx's own cached statements. Its generated `PREPARE` and `EXPLAIN EXECUTE` commands use a nonpersistent SQLx executor instead. For each execution it prints `EXPLAIN (ANALYZE, BUFFERS, SETTINGS, FORMAT TEXT)`. `EXPLAIN ANALYZE` executes the prepared statement; the output includes planning/execution time and buffer information from that run.

## Actual SQLx timing

After plan capture, each scenario invokes the production `SearchPublicListingSourcesHandler` with the SQLx public search reader for browse, prefix (`mu`), and contains (`auction`). It invokes `GetPublicListingSourceBySlugHandler` with the SQLx public details reader for the exact seeded slug. This is actual SQLx prepared/cached query execution, including the normal short transaction and application mapping; it is not an `EXPLAIN` run.

Each operation has 3 unreported warm-up calls followed by 25 sequential measured calls. The harness prints nearest-rank p50 (13th sorted sample) and p95 (24th sorted sample) in milliseconds. A p95 above 150 ms fails the local harness. The 150 ms guard matches the reader transaction-local statement timeout; it is a local regression guard, **not** a production SLO or capacity target.

Samples are warm, local, sequential, and use the disposable test-api PostgreSQL container. They include no concurrent callers, network hop, cancellation injection, cold-cache study, cross-process contention, or mixed workload. Do not generalize them to deployed latency.

The harness is test-only. It does not change production query behavior, indexes, API routes, or infrastructure.

## Required run record

Capture these fields with the command output:

| Field | Value |
| --- | --- |
| Status | Passed locally for all three scenarios |
| UTC run time | September 11, 2026 (wall-clock time not retained) |
| Git revision | Uncommitted working tree based on `5c9c5dd014b5058a594afd0c01bd9dca797e8b03` |
| Exact command | Command above |
| `AURA_TEST_POSTGRES_IMAGE` | Unset; test-api pinned image used |
| Host OS, kernel, CPU model/count, RAM, and storage class | Linux 6.8.0-117-generic x86_64; Intel Core i7-8700 @ 3.20GHz, 12 logical CPUs; 62 GiB RAM; storage class unrecorded |
| Docker version/resources | Docker 29.7.2 (build `a7dcaa6`); resource limits unrecorded |
| PostgreSQL server version/number | PostgreSQL 16.15 (Debian 16.15-1.pgdg12+2), `160015` |
| PostgreSQL image reference/digest | test-api pinned `ghcr.io/aura-historia/test-postgres:pg16-pgttl-3.0.0-r1`; digest not recaptured in this run |
| Database encoding, collation, and ctype | UTF8; `en_US.utf8`; `en_US.utf8` |
| `shared_buffers`, `effective_cache_size`, `work_mem`, `random_page_cost`, `jit` | 128MB; 4GB; 4MB; 4; on |
| Seed cardinality | 1,000/50; 10,000/500; 100,000/5,000 (`listing_sources`/`parties`) |
| `ANALYZE` completion by scenario | Passed for 1k, 10k, and 100k scenarios |
| Custom-plan output by scenario | Captured in test stdout for all scenarios |
| Generic-plan output by scenario | Captured in test stdout for all scenarios |
| Cancellations or timeouts | Unmeasured by the timing workload |
| Handler/reader failures | None in this run |
| Mixed-load/concurrency result | Unmeasured |

## Captured results

All three scenarios completed prepared browse, prefix, contains, later-page, and slug plan cases under both plan modes. The actual-handler timing workload also passed its 150 ms local p95 guard for every operation.

| ListingSources | Browse p50 / p95 | Prefix p50 / p95 | Contains p50 / p95 | Slug p50 / p95 |
| ---: | --- | --- | --- | --- |
| 1,000 | 1.378 / 1.527 ms | 2.018 / 2.281 ms | 2.069 / 2.592 ms | 0.620 / 0.741 ms |
| 10,000 | 1.472 / 1.645 ms | 3.980 / 4.110 ms | 4.611 / 4.820 ms | 0.496 / 0.675 ms |
| 100,000 | 1.306 / 1.616 ms | 31.257 / 32.202 ms | 45.330 / 46.646 ms | 0.671 / 0.765 ms |

These are nearest-rank values from 25 measured samples after 3 warm-up calls per operation/scenario. Cargo reported 10.97 s test time; shell wall time was not retained. Cancellations, timeouts, mixed-load behavior, concurrency, cold-cache behavior, and remote/deployed latency remain unmeasured.
