# DOX

## Purpose

- Own hybrid release catalog, contracts, controller, host tooling and offline tests.
- Application business code stays in Rust. Deployment records are operational only.

## Contracts

- Read root and matching `docs/arch.md` rules before edits; status lives in `docs/deployment/implementation-status.md`.
- Current integration is default-off: no workflow calls this package for live mutation.
- Strict versioned schemas; references only, no credentials or payloads in manifests/journals/logs.
- Manifest integrity never grants approval. Exact intent/plan/current revision required independently.
- S3 ETags are conditional tokens, not artifact hashes. Unknown remote outcomes retain operation ownership; no timed lock stealing.
- Typed allowlisted operations and explicit argv only. No arbitrary shell, Compose, path, registry or mount inputs.
- Production examples cannot satisfy real-stage setup gates. No cloud/host mutation in unit tests.
- Integrator owns package/lockfiles, shared types, catalog and orchestration. At most three disjoint substantial implementers.

## Verification

- `npm --prefix deploy/control ci`
- `npm --prefix deploy/control test`
- `npm --prefix deploy/control run validate-catalog`
- Generated JSON schemas must match typed sources; unsupported commands fail nonzero.

## Index

- `catalog.json` — deployable bins, assets, scopes and migration streams.
- `control/` — TypeScript contracts/CLI/planning/state tests; Node26 production target.
- `schemas/` — generated strict JSON Schema documents; semantic checks also mandatory.
- `bootstrap/` — owner-provided protection/setup specifications, never applied implicitly.
- `compose/` — offline application-template contract. Cron/crawler render only; API/worker blocked pending trusted private bind input. No executor or protected-file materializer. Retired singleton uses restart=no; active recovery needs durable owner-aware reconciliation.
