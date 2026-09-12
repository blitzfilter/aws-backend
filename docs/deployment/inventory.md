# Hybrid deployment inventory

Evidence baseline: `901b42f20e3cf6769763f1a9c052d307e813cb9a`, 2026-09-12.
Clean checkout on `task/#1412-deployment`. Local and cached remote `develop` match baseline; cached `origin/prod` is `6fdbab5eb102dafbebf4cf93e521d944c8986156`. No local tags. No fetch/reset. Default remote branch is `develop`.

**Code evidence, not a live audit.** Running versions, queue contents, schemas, permissions, certificates and infrastructure remain unknown. Public issues #1549 and #1412 were read; immutable UTC CalVer promotion is the requested target, not current workflow behavior.

## Accepted implementation deltas

The sections below preserve iteration 00 **baseline observations**, not current runtime behavior. Current checks/SHAs: `implementation-status.md`.

- 02: shared explicit PostgreSQL stage/TLS/password-file contract; all native/DB Lambda paths covered. Crawler now requires both database URLs and never bootstraps/migrates in `server`; `bootstrap-local` is a separate excluded tool. CA materialization remains a rollout gate.
- 03: API/worker SIGINT/SIGTERM, bounded owned cleanup and non-consuming preflight; exact business SQLx history/extension/table startup gate. API private loopback probes replace public probes; worker `/admission` stays independent of consumer `/ready`. Default API drain/stop 45/60s; worker 270/300s, configured worker ceiling 3600s. Explicit per-slot operational ports join host budgets. Unknown future history still blocks until compatible-superset evidence is implemented.
- 04 foundations: cron loopback-only operations (daemon and run-once), strict non-scheduling preflight, retained execution/drain outcomes, default300/330s budgets. Crawler capture counts distinguish accepted/durable/completed; removed local completion shortcuts. Crawler pass now owns producer/collector drain; review listener owns connection tasks; discovery owns fallible producer/forwarder completion. Server signals/preflight, service checkpoint/cancellation and full handover remain pending. Real PG tests prove advisory-session loss is not a transaction fence; handover requires confirmed prior-process termination.
- Test PostgreSQL now uses a fixed local Docker socket and checked owned-ID cleanup. Raw-replay fixtures still do not create SQLx history; process fixtures apply genuine migrations. Real Sequin/SQS and deployment rehearsal remain unpassed.

## Processes

All native target architectures remain operator inputs; local Rust target is x86_64 GNU. SQLx is 0.9.0 with Tokio/rustls/Postgres/migrate. Rust pin is 1.98.0. DB role grants below are requirements, not verified grants.

| Package / binary / command | Inputs and dependencies | Listener; durable state; shutdown |
|---|---|---|
| `aura-historia-api` | Business `POSTGRES_*`, OpenSearch, Cognito issuer/JWKS/client IDs/pool, Stripe, Zoho, Vertex/Google ADC; AWS Cognito administration | Default HTTP `0.0.0.0:8080`, `/health`, `/ready`; PG business writes; Ctrl-C only, no total drain ceiling |
| `aura-historia-worker` | `AURA_HISTORIA_WORKER_SCOPE`, scoped source queue URL, `AWS_REGION`, `STAGE`, `POSTGRES_*`; providers below | Default HTTP `0.0.0.0:8081`, `/cdc/sequin`, `/health`, `/ready`; SQS custody; SIGINT/SIGTERM, default 270s consumer drain, concurrent 20s HTTP drain |
| `aura-historia-cron` | Enabled-job list, business PG, OpenSearch, Vertex/ADC; `search-filter-periodic-match` default 15:00 UTC | Default HTTP `0.0.0.0:8082`; persisted matching progress, session advisory lock; stop admission then 300s default drain; job execution default 7,200s |
| Cron `--run-once search-filter-periodic-match` | Same business dependencies; performs real work, not preflight | No listener; same guarded job, but no custom signal path in this branch |
| Package `crawler`, binary **`server`** | `BUSINESS_DATABASE_URL`, Vertex/ADC, public HTTP/DNS; local Docker bootstrap, crawler DB; optional CloudWatch logging | Review HTTP `127.0.0.1:7878`; non-loopback/mutations require review authentication. Crawler PG state + business raw captures. No custom signal/drain; startup creates DB/applies migrations |
| Crawler `demo`, `demo-spider`, `demo-scraper`, `fetch-fixture` | Development/bootstrap/fixture tools, some fetch websites/use Vertex | **Not deployable components**; JSON/HTML fixture outputs, development DBs |
| `ci-determinator` | Git/Cargo metadata | Build utility, **not deployable** |

Evidence: native `src/main.rs`; API `src/lib.rs::{postgres_pool_from_env,run}`; worker `src/{main,lib,http}.rs`, `queue/{config,consumer}.rs`; cron `src/{main,scheduler,scheduled_job}.rs`, `wiring/search_filter_periodic_match.rs`; crawler `src/bin/server.rs`, `local_db.rs`, `service/cron/config.rs`. Cargo metadata reports 14 bin targets: four deployable native, five Lambdas, five development/build tools.

Crawler server currently ignores the documented intended `LOCAL_DB_URL` production contract: local URLs are constructed with fixed development credentials. Crawler pool caps are 16 local / 8 business. Production startup must lose Docker, CREATE DATABASE and migrations; development bootstrap remains separate. Runtime startup must eventually need only restricted DML/read roles.

### Ten worker scopes

All use one binary, one stable Standard source/DLQ pair per scope, no tier dimension. All need business PG and scoped SQS publish/consume identity.

| Scope | CDC source operations | Other dependencies | Attempt / visibility seconds |
|---|---|---|---:|
| `product-listing-opensearch` | `product_listing_events` INSERT | OpenSearch | 45 / 60 |
| `search-filter-projection` | `search_filters` INSERT/UPDATE/DELETE | OpenSearch percolator documents | 45 / 60 |
| `search-filter-percolator` | `product_listing_events` INSERT | OpenSearch, Vertex/ADC; PG matches | 240 / 300 |
| `search-filter-match-notification` | `search_filter_matches` INSERT | PG notifications/delivery intents | 45 / 60 |
| `watchlist-notification` | `product_listing_events` INSERT, routed dimensions only | PG notifications/delivery intents | 45 / 60 |
| `product-content-assessment` | `product_listing_events` INSERT, discovered only | PG assessments; no configured external model | 45 / 60 |
| `product-embedding` | `product_listing_events` INSERT, discovered/image changes | Vertex/ADC | 240 / 300 |
| `product-translation` | `product_listing_events` INSERT, discovered only | Vertex/ADC | 240 / 300 |
| `product-listing-normalization` | `product_listing_raw_revisions` INSERT | PG raw/canonical progress; reconstructible local reconciliation | 240 / 300 |
| `notification-delivery` | `notification_deliveries` INSERT | S3 templates, SES; PG lease/finalization | 240 / 360 |

Sources: `infra/src/worker-queue-config.ts`, worker `main.rs`, `queue/config.rs`, `docs/events/flow.md`. `product_listings`, `users`, watchlist and partnership tables have no default direct subscription. Credential tables must never enter CDC.

Custody: PG commit → Sequin → scoped HTTP → confirmed SQS sends → handler → confirmed completion → receipt delete. Source retention 7d; DLQ 14d preserves original enqueue age; max receives 5. Full batch validation precedes publication. Limits: 1 MiB, 100 changes, 500 jobs, 8s publish deadline within 10s HTTP deadline; operational Sequin delivery timeout must exceed 10s. Consumer-only dependency outage need not close durable ingress: `/ready` is **not** the sole ingress route signal. Schema-2 TypeID jobs reject schema 1. No queue purge/reset is authorized by this inventory.

### AWS binaries and invocation

Authority: `infra/src/constructs/{lambdas,eventing,cognito}.ts`. All packaged Rust functions: x86_64, `provided.al2023`, package = bin. No version/alias layer exists.

| Binary | DB | Memory / timeout | Invocation |
|---|---:|---:|---|
| `cloudwatch-log-retention-lambda` | No | 128 MiB / 10s | CloudTrail/CreateLogGroup via EventBridge |
| `cognito-post-confirmation` | Business | 256 MiB / 5s | Cognito post-confirmation |
| `shopify-lambda` | Business | 256 MiB / 30s | Shopify partner bus → SQS, batch 10/1s, partial failures |
| `stripe-lambda` | Business | 256 MiB / 30s | Stripe partner bus subscription events |
| `fxrate-lambda` | Business | 128 MiB / 10s | 06:00/18:00 UTC EventBridge; first-create initializer; absent ephemeral |

Two inline CDK provider functions (Node 20) own WAF and initial FX setup. They are infrastructure assets, not Cargo ZIP targets. Initial FX Create invokes a DB-using function; **business schema/network/secrets must precede compute creation**, currently an external prerequisite. Updates/deletes do not repeat capture. Failure remains CloudFormation failure. Both dev and prod currently resolve `/fxratesapi/prod/api-token`; this shared reference is not proof of credential availability or acceptable future isolation. The initializer uses stable source ID `deployment:fxrate:initial:{stage}:v1`; preserve its idempotency on compute recreation (`eventing.ts::createInitialFxRateSnapshot`).

### Current CDK ownership and bootstrap inputs

`infra/src/application-stack.ts` composes **data → compute → API**; prod observability depends on all three. Data owns queues, unbound native policies and external storage settings; compute owns Lambdas/Cognito/eventing/FX initializer; API owns Gateway/CloudFront/WAF provider; observability owns alarms/topic. Preserve these ownership/logical-identity boundaries during transition.

Current compute input is required `CommitSHA: String`, not format-constrained (`infra/src/parameters.ts`). Existing imported buckets are `aura-historia-binary-artifacts-eu-central-1`, `aura-historia-mail-templates-eu-central-1` and `aura-historia-cfn-artifcats-eu-central-1` (intentional existing spelling). `CliCredentialsStackSynthesizer` stages under `<stage>/`; bucket/permission provisioning is external. Workflow bucket variables must match these imports. These are baseline code identifiers, not verified deployed resources or substitutes for future private control/ECR prerequisites.

## Connections, identities and edge

- `platform-postgres::PostgresPoolConfig` builds options without explicit SSL mode/trust/hostname policy. Native API/worker/cron and four DB Lambdas call it. Dedicated cron advisory-lock connection in `search-filter-postgres/src/periodic_search_filter_matching_run_lock.rs` is outside the pool cap. Crawler URL constructors bypass it. TLS is not repository-enforced.
- Runtime pool defaults are two shared-PG connections. Budget must include all ten workers, cron's extra session, both API slots, crawler pools, Lambda concurrency, Sequin, migrations and reserve. No global capacity contract yet.
- No VPC/NAT/EIP path for DB Lambdas exists. CDK creates no RDS resource. Internet reachability and verified TLS are unknown.
- Native AWS identity uses credential-chain integration; Google uses ADC. Queue policies are **unbound** managed policies, not functioning host identities. SSM registration is not an application credential mechanism.
- `ssmValue` uses ordinary `ssm` dynamic references. A `/secrets/` path does not imply secure storage. Never render resolved templates/env into logs.
- Native mail keys are `<STAGE>/<COMMIT_SHA>/mjml/<template>/<language>.html`. All 25 workflow inputs exist: five template groups × de/en/es/fr/it. Runtime needs separate S3 read + SES send.
- API Gateway route catalog is empty. CloudFront still points at regional API Gateway. Existing `/api/*` cache policy plus auth guard does not prove personalized-cache isolation: guard sets one shared marker for JWT-looking Bearer tokens, ignores opaque tokens. Native cutover must disable shared personalized caching and preserve native JWT/Aura-token validation.
- Native worker HTTP has no ingress authentication middleware; isolation is external. Crawler controls must stay private. Caddy/SSM/Compose deployment tooling does not yet exist.

## Database and search histories

Business: one immutable file, `migrations/20260725090000_initial_business_schema.sql`. Initial baseline creates `pg_trgm`/`unaccent`, maintained `name_search` triggers, GIN trigrams and C-collation public ordering/prefix indexes. It requires provisioned/preloaded **pg_ttl_index** for asynchronous credential/provider-receipt cleanup. Applied checksums must not be repaired by editing/stamping/resetting history. SQLx history/checksums remain authoritative.

Crawler: separate files under `src/crawler/migrations/`:

1. `20260101000000_initial_schema.sql`
2. `20260514000000_crawler_reviews.sql`
3. `20260711000000_removed_page_schema.sql`
4. `20260817000000_add_generation_review_page_role.sql`
5. `20260831000000_crawler_review_domain_ownership.sql`
6. `20260831000001_url_pattern_state.sql`

No common migration history or automatic adoption is safe. Actual deployed baseline remains unknown. `docs/object-ids.md` describes a breaking development reset/TypeID transition; a stage named dev alone does **not** authorize reset.

OpenSearch desired assets: `opensearch/mappings/{product_listings,user_search_filters}.json`; five `analysis/*_synonyms.txt` files referenced by relative `analysis/` paths. Both families contain analysis/KNN settings; 768-dimensional vectors; filter family includes percolator. Real-stage engine/plugin version is an unresolved platform input, not implied by a client crate version.

Concrete `product-listings` defaults: `product-listing-opensearch/src/{product_listing_search_projection,product_listing_search_reader,product_listing_similar_products_reader}.rs`. Search-filter index configuration lives in `search-filter-opensearch/src/index.rs`; test bootstrap also constructs physical families. Audit these readers/writers and fixture/rebuild paths before alias activation. New aliases must not collide with existing physical names.

Both projection adapters use `VersionType::External`; equal/older writes are stale. Deletion is a content-free `projectionDeleted` tombstone, not physical delete. Readers exclude true before pagination/KNN/percolation. Withdrawn source rows support projection repair; hard-deleted filter history cannot be invented. General online rebuild and stale-alias rollback are not supported deployment operations.

## Test infrastructure and drift

`src/test-api` owns PID-scoped process-lived PostgreSQL/LocalStack/Sequin fixtures and cleanup hooks; reuse with one stateful owner. PostgreSQL image includes pg_ttl_index 3.0.0; override is explicitly local-test-only. LocalStack tag 2026.07.6 uses test credentials/endpoints; OpenSearch test policies are intentionally insecure and **not** production templates.

Sequin fixture: `sequin/sequin:v0.14.6`, separate Sequin metadata PG database plus Redis (`src/test-api/src/sequin.rs`). YAML-configured HTTP sinks use five tables above; scoped fixtures use insert-only except filter insert/update/delete. Stable production sink identities, pause/resume behavior, offset recovery and initial-backfill policy still need version-specific implementation/testing. Fixture bootstrap credentials, root keys and endpoint substitutions must never transfer into platform defaults.

Worker `tests/process_durability.rs` covers real process death/restart, SIGTERM, duplicate/lost-response custody; process helper currently selects content assessment only. API acceptance runs an in-process router; crawler collector tests prove channel-close flush, not OS-signal custody. No complete TLS/network/process deployment rehearsal exists.

Legacy `.github/workflows/deploy.yml`: develop/**prod branch**, filtered paths, artifact publication with deploy credentials before environment gate; manual SHA can differ from checked-out infra source. Retired embed/translate product-pipeline paths still appear. Only CDK mutation job declares `aws-dev`/`aws-prod`; native workflow absent. No immutable versioned bundle/provenance/intent/control store.

Missing tracked docs referenced by indexes/architecture include `durable-worker-runbook.md`, `product-listing-raw-normalization-runbook.md`, and several admin/public-search/rewrite inventories. This inventory reconstructs only facts checked against code; it does not claim to recover missing documents.

## Required bootstrap inputs

No values below have been supplied or verified. Real-stage readiness remains **false**.

| Gate | Required typed inputs/evidence | Owner |
|---|---|---|
| GitHub | Environment names; reviewer IDs; self-review/bypass flags; branch/tag rulesets; required check identities; actual OIDC subject template | Repository owner |
| AWS | Account ID; region; bootstrap/publisher/planner/deployer/recovery ARNs; private artifact/control buckets; ECR repositories; approved SSM documents/node tags | AWS owner |
| Hosts | Host ID; OS/version; architecture; role placement; management address; provider firewall ownership; explicit shared-hardware acknowledgment | Host owner |
| Network | Public origin/PG DNS; private endpoints/routing; exact NAT EIPs/admin CIDRs; IPv4/IPv6 policy; certificate DNS/SAN identities | Infrastructure owner |
| Certificates/identity | CA references; issuer; renewal/trust overlap; Roles Anywhere profile/trust anchor/certificate references; Vertex federation or mounted ADC rotation | Security owner |
| Secrets | Separate runtime/migration/replication/backup references; origin header; provider references; rotation/cache policy | Secret owner |
| Capacity | CPU/memory/disk profile; per-process/Lambda connection caps; blue/green headroom; WAL/slot budget | Operator |
| Recovery | Off-host backup/snapshot destinations; retention; recovered keys/roles/configuration; restore evidence; RPO/RTO targets | Data owner |
| First cutover | Actual current schemas/processes/wire versions/queues/CDC positions; in-memory work evidence; compatible rollback or repair-forward plan | Release/data owner |
