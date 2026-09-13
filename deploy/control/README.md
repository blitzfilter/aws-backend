# Deployment controller contracts

**Default-off. No live mutation adapters or enabled workflow.** Node 26 production target; pinned TypeScript/Zod dependencies and lockfile. This iteration implements validation and pure protocol logic, not S3/Docker/SSM/CloudFormation execution.

From repository root:

```sh
npm --prefix deploy/control ci
npm --prefix deploy/control test
npm --prefix deploy/control run validate-catalog
node deploy/control/dist/src/cli/main.js --help
```

`release validate-catalog` checks actual Cargo bin metadata, the CDK Lambda/scope AST, all MJML inputs and OpenSearch assets. It reports pending components explicitly. A `NOT_IMPLEMENTED` catalog entry prevents bundle acceptance. All other proposed deployment/host commands validate typed arguments, then return `NOT_IMPLEMENTED` / exit 8. They do not contact providers or fake success.

## Frozen v1 boundary

Iteration 03 makes a **breaking draft-only addition**: both `slots.blue.operations_listener` and `slots.green.operations_listener` are required, including inventories embedded in runtime snapshots. Worker drain/stop values now cap at 3600s. No persisted/live v1 records exist in this integration; regenerate older fixtures/snapshots with explicit loopback ports. Schema version remains 1 only on that no-live-records premise. A deployed schema change would need an explicit version/migration protocol.

Iteration 04 continues that **draft-only break**: cron no longer has a network `endpoint`, `listener`, or `ca_ref`; it has a required loopback `operations_listener` for slot-local inspection. Recreate old snapshots. Cron drain/stop cap at 3600s, execution at 7200s; a 7200s execution can be cancelled by the default 300s drain. Crawler likewise replaces `endpoint`/`listener`/`ca_ref` with separate required loopback `review_listener` and `operations_listener`; the daemon does not serve native HTTPS. This initial deployment profile intentionally supports only local review access, not the runtime's optional remote-review/proxy mode. Probe inside the workload namespace; do not publish either listener. Crawler requires `startup_seconds` (1–3600), drain/stop cap3600, stop>=drain+30, dev/prod drain>=300; defaults60/300/330. Explicit local/ephemeral/test may shorten drain. Recreate all older embedded snapshots; no live v1 record or schema/data upgrade is implied.

- `src/contracts/release.ts`: strict manifest, plan, intent and SQLx metadata. **`parseManifest` must be followed by `verifyManifestCatalog(manifest, installedTrustedCatalog)`**. Neither proves artifact existence/provenance or approves execution.
- `src/contracts/inventory.ts`: independent host/role placement, exact scoped queues, hostname/TLS requirements, protected references, resource/pool/deadline budgets. Both API slots reserve business and distinct loopback operational ports. Operations use slot-local execution, never public routing. API drain/stop floors are 45/60s; worker floors 270/300s, both worker values at most 3600s. Real stages cannot use test endpoint overrides. Cron/crawler target configuration does not start jobs.
- API `/health`, `/ready`, `/version` are loopback-only; worker operational routes stay on its private listener. Worker `/admission` is independent of consumer `/ready` and expresses local publication admission, not a promise that the next SQS send succeeds. Future controller routing must use that distinction. Crawler `/health`, `/ready`, `/ops/version` are exact slot-local paths with no query suffix; review auth is separate. Readiness is not process-termination or full provider-auth proof. `--check-config` is supported only by these new binaries; old worker/crawler binaries ignored arguments and could start work. Capability/provenance checks must precede preflight.
- `src/state/model.ts`: pure conditional revision transitions, durable nonce, per-phase attempt/read-back journal, mixed actual component identities. Only successful completing-attempt evidence can finish its phase. Uncertainty never expires ownership. Storage adapters must CAS the returned state **before** an external effect.
- `src/contracts/hash.ts`: canonical compact UTF-8 control JSON, sorted keys, dense ordered arrays, safe integer numbers. Manifest publication must use these exact bytes; verify raw artifact digest before parsing. No self-digest. SQLx SHA-384 checksums differ from artifact SHA-256. S3 ETag is only a conditional-write token.
- `src/contracts/bootstrap.ts`: owner-supplied GitHub protection/ref/OIDC policy, separate creation/immutability rules. Status remains disabled even if untrusted input claims live setup exists.
- `src/contracts/result.ts`: stable exit codes, safe errors. Arbitrary payloads are fully redacted; separately validate any operational fields before logging.

Source schemas generate `../schemas/*.schema.json` with `npm --prefix deploy/control run schemas`. Tests compare generated schemas. **JSON Schema alone is insufficient**: semantic parsers also enforce identity, completeness, budgets and transitions. Fixture builders under `fixtures/` are synthetic, not working production configuration or evidence.

The required eventual trust chain is installed catalog/controller → immutable complete artifact/provenance verification → read-only target plan → independently authenticated approved intent → conditional ownership → authenticated observations → verified actual component state. No builder-supplied catalog, approval or observations may substitute for those trusted adapters.

## Intentional limits

No live inspection, artifact upload/download, migration application, service start/stop, scheduler fencing, backup restore, promotion or credential resolution exists here yet. No S3/OS-lock correctness is claimed from pure tests. Node/host helper bundling and provenance verification land later.

The v1 protocol blocks component-key/architecture retirement and multiple architecture selections before acquisition until an approved target-selection/retirement protocol exists. It preserves old observations rather than deleting unknown runtime state. Planned migrator identity is a one-shot tool checked in `postgres_expand`, never a singleton scheduler.

Full acceptance coverage and external rollout gates: `docs/deployment/implementation-status.md`. No compatibility rollback may downgrade SQL, remove mappings, reset indexes or purge queues.
