# Hybrid deployment decisions

Status: v1 pure contracts reviewed; **not deployment-ready**. No provider/runtime mutation adapter or new workflow enabled.

## ADR-001 — Scope and trust boundary

Keep Rust domain/use-case boundaries and existing AWS resource ownership. Native API, ten scoped workers, cron and crawler use OCI/Compose; PostgreSQL/OpenSearch/Sequin and stable Caddy edge have independent lifecycle. Ansible owns platform/helper upgrades. No live bootstrap is authorized.

Operational TypeScript controller/helper under `deploy/control` owns private S3 conditional control records and host-local OS locks, not domain audit events. A manifest proves artifact identity, not approval. A protected environment deployer must separately issue an exact intent binding plan, expected revision, stage/account/region, inventory/configuration and trusted controller/helper identity. No expiration-based lock takeover. Unknown remote work retains ownership until observed terminal/fenced.

Default placement: one separate host per dev/prod; roles remain independent of host names. Shared hardware requires explicit inventory acknowledgment plus stable-order stage/host conflict locks. No ordinary app release restarts databases, recreates volumes, prunes globally, downgrades SQL or removes mappings.

Compatibility is explicit: compatible; compatible with named backfill gates; maintenance required; blocked. Unknown history, drift, queue/schema compatibility, provenance or live bootstrap evidence blocks mutation. No SQL keyword inference or generic replay subsystem.

## ADR-002 — Integration and ownership

Existing `task/#1412-deployment` avoids legacy develop/prod triggers. Use local reviewed iteration commits here. Do not push/merge incomplete contracts to develop. Final enablement requires iterations 11–13, verified protection/bootstrap inputs and separate authorization.

Actual orchestrator: GPT-6-Astra. Delegation exposes no model selector; successful runtime/AWS investigators report GPT-6-Astra. Requested Astra Max/Terra High execution labels cannot be selected. One storage investigator exhausted context without usable handoff; integrator recovered required inventory directly. Never attribute unavailable model work.

| Iteration | Depends on | Exclusive implementation ownership |
|---|---|---|
| 00 | — | `docs/deployment` inventory/status; docs index integrator |
| 01 | 00 | Controller schemas/types/pure tests; catalog and package/lockfiles integrator |
| 02 | 01 | Shared PG TLS config and all DB entrypoints, tests; shared Cargo/CDK files integrator |
| 03 | 01,02 | API/worker lifecycle/preflight and private operational probes |
| 04 | 02,03 | Crawler/cron startup/cancellation/custody |
| 05 | 01,02 | `src/aura-historia-migrate`, immutable migration metadata/history validation |
| 06 | 01,05 | Search planner, adapter aliases, registered projection backfill gates |
| 07 | 03–06 | Images, catalog-driven immutable bundle/build publication |
| 08 | 01; final 07 | Host inventory/Compose/Ansible/SSM packaging/Sequin/backup fixtures |
| 09 | 01,02 | CDK network, separated identities, prerequisites and handoff |
| 10 | 07,09 | Qualified Lambda preparation/activation and Caddy/CloudFront transition |
| 11 | 06–10 | Controller durable adapters/state machine; integrator owns orchestration |
| 12 | 07,11 | Workflows/protection verification; disabled until 11–13 accepted |
| 13 | 11,12 | Recovery/first-cutover/move tooling and evidence |
| 14 | All | Isolated end-to-end/failure rehearsal and independent final review |

At most three substantial implementers at once, disjoint files only. Integrator alone owns root Cargo manifests/lock, TS locks, `infra/src/application-stack.ts`, workflow orchestration and shared config types. No stateful fixture races on fixed ports; each fixture owns names and cleanup. Review against actual integration head, not stale agent baseline.

## ADR-003 — Frozen byte and command boundary (iteration 01)

JSON control records use schema version 1 with strict unknown-field rejection. Persisted operational enum values follow repository SCREAMING_SNAKE_CASE; stage names remain `dev`/`prod` and explicit test stages. CLI phase names are allowlisted identifiers, never shell.

Artifact digest: SHA-256 of exact immutable bytes, lowercase `sha256:<64 hex>`. Manifest digest is external, never a self-digest field. Canonical control JSON sorts object keys recursively, preserves array order, emits compact UTF-8 with no trailing newline; no undefined/nonfinite/unsafe-number values. Manifest publication must use those exact canonical bytes; raw digest verification precedes parsing. Canonical hashes are not S3 ETags. SQLx migration checksums use SQLx's own SHA-384 history convention, separately typed.

Runner planning, artifact verification and host inspection are read-only capabilities, separated from mutation adapters. Unsupported commands return explicit nonzero machine-readable results; no echo/mock adapter is a deploy implementation. Configuration/manifests contain references, not resolved credentials or arbitrary shell, paths, registries or Compose documents. Live examples must be rejected until replaced by approved target inputs. The initial pure protocol blocks component-key/architecture changes and multiple target architectures before ownership; future target selection/retirement needs explicit evidence and review, never deletion of mixed state.

## ADR-004 — SQLx connection construction and trust

SQLx 0.9.0 exposes no environment-free `PgConnectOptions` constructor or public client-certificate resetters. Replacing SQLx or maintaining a security-sensitive fork is not justified here. Permit its transient ambient reads only behind `platform-postgres`: `new_without_pgpass`, fixed temporary authority, reject inherited client certificate/key/CA/options, overwrite explicit connection/mode/CA/application fields, cache options. Unsupported ambient values fail even when empty. Composition roots still own reading deployment inputs; core/service never read environment. Pin SQLx exactly and regression-test its public URL projection before upgrades.

This is an explicit exception to environment-independent adapter construction, **not** a claim that the SDK is environment-free. Real stages require `verify-full` and a valid supplied CA; current rustls feature unifies that trust with WebPKI roots, not CA-only pinning. No client certificate/key support is advertised. Protected file inputs and all exposed error-source formatting remain redacted.

Dedicated migration/advisory sessions use a direct 5s-bounded connection, not a transaction pool. They count separately from pool caps. CA/credential rotation requires current configuration and process/pool recycling; existing connections do not refresh themselves. Real Lambda CA materialization/network/identity activation remain later rollout gates; a path reference alone does not install a CA file.
