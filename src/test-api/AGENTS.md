# DOX

## Purpose

- Own `test-api` crate.

## Core Design

- LocalStack and AWS integration test harness.
- Root modules: `api_gateway`, `aura_historia_api`, `cloudformation`, `cognito`, `eventbridge`, `localstack`, `opensearch`, `postgres`, `s3`, `sequin`, `ses`, `signal`, `sqs`.
- Child crates: `test-api-macros`.
- Main neighbors: `application`, `test-api-macros`.
- Test crate. Favor stable helpers and black-box assertions.
- `#[aura_integration_test]` tests run serially inside one compatible suite process against process-local LocalStack and optional service containers like Postgres. It always tears down services in reverse setup order, including after a test panic.
- Postgres pulls the immutable reference in `postgres/image-ref.txt`, preloads `pg_ttl_index`, creates the extension, starts its worker, applies schema-only migrations once per suite process, then truncates application data between tests. Extension-owned metadata survives teardown. Ordinary tests never build pg-ttl; use `AURA_TEST_POSTGRES_IMAGE` only for explicit local image testing. Use `Postgres::new_per_test` only for migrations that seed test data; optional setup scripts always run before each test.
- LocalStack and Postgres use process-id-scoped container names and host ports so separate test binaries/processes can run in parallel. LocalStack records its first normalized service/environment topology and rejects incompatible later requests.
- OpenSearch bootstraps its domain, pipelines, mappings, and indexes once per suite process; teardown clears only canonical documents, including `user_search_filters`.
- `Sqs { name }` keeps Shopify compatibility. `SqsQueuePair` configures source/DLQ names and attributes; `unique(test_name)` adds PID/random identity. URLs use the actual LocalStack endpoint and `/000000000000/name`, never hardcoded host port 4566. Setup checks returned and canonical queue identities; cleanup purges only that pair, including invisible messages.
- `WorkerSqs::new(scope, visibility_seconds)` uses exact `aura-worker-<scope>-test` / `aura-worker-<scope>-dlq-test` names, process-isolated LocalStack, 7d/14d retention, max receive 5, poll 20, SSE, deny-insecure-transport metadata, source redrive deny-all, and DLQ redrive allow for only its source. Visibility: 300 normalization/percolator/embedding/translation, 360 delivery, 60 others. Teardown deletes only the worker pair; next setup recreates it, isolating cancelled server-side long polls that survive purge. LocalStack permits immediate recreation; this is not an AWS cleanup recipe. HTTP LocalStack tests prove policy metadata, not TLS enforcement.

## Ownership

- This doc rule `src/test-api/**`.
- Parent doc: `src/AGENTS.md`.
- Child docs below rule deeper child crates.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract, route/event shape, env vars, or child index change.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Tests prove behavior, not implementation trivia.
- Share helpers before copy-paste fixtures.
- Prefer `Postgres`/`OperationalBackendPostgres` and `postgres` feature over legacy `Rds`/`rds` in new tests.
- Use process-lived `AuraHistoriaApi` helper for local black-box tests against `aura-historia-api`.
- Use `Postgres::new("migrations")` for the shared schema-only business migrations.
- Use `Sequin::worker_webhook()` after Postgres and target fixtures in `#[aura_integration_test]` when a test must verify real worker delivery. It starts pinned process-lived Redis/Sequin sinks for `product_listing_events`, `search_filters`, `search_filter_matches`, plus insert-only `notification_deliveries` and `product_listing_raw_revisions`; it retries startup three times for transient container or replication-slot failures, waits for HTTP health and an active logical replication slot, then has no per-test reset. PID-named containers are removed on normal exit and SIGINT/SIGTERM. Start the worker at `get_sequin_worker_webhook_bind_addr()` before writing watched source rows, except a narrow startup-reconciliation test that intentionally commits raw work before worker startup.

- Scoped SQS worker tests use `Sequin::worker_webhook_for_tables` (fully qualified table names). `worker_webhooks(primary, secondary)` routes two table groups to independent runtimes; secondary binds `get_sequin_secondary_worker_webhook_bind_addr()`. One immutable Sequin topology per process. Only search-filter sources include update/delete. Order Postgres, targets/SQS, then Sequin; start HTTP before source commits.

## Verification

- `cargo check -p test-api`
- `cargo test -p test-api --all-features`

## Child DOX Index

- `src/test-api/src/test-api-macros/AGENTS.md` — `test-api-macros` crate.
