# Implementation status

**Deployment readiness: false. Live changes: none.**

Baseline: `901b42f20e3cf6769763f1a9c052d307e813cb9a` (clean), branch `task/#1412-deployment`. No pushed refs, PRs, tags or environment approvals. Only local iteration commits authorized by playbook.

Models: orchestrator GPT-6-Astra; delegation has no model selector. Runtime investigator `2ac2923c-d3e0-4d8d-b62e-24dba77970c3` and AWS investigator `5d2424e8-0e8b-4dd9-b0cc-d3507ead76ac` reported GPT-6-Astra. Storage session `c58e4961-7eb6-4168-b5e5-e512323417fb` exhausted context; no result credited, integrator recovered evidence.

| Iteration | State | Accepted commit / evidence |
|---|---|---|
| 00 | Accepted | `303f50b8de5c603f2c11498979cb2867634c620e`; reviewer `5d2424e8-0e8b-4dd9-b0cc-d3507ead76ac` accepted after two P2 inventory omissions fixed; no runtime change |
| 01 | Accepted default-off contracts | Local commit follows; independent reviewers `c6ac1bf8-98c5-4451-a690-0e1df1d045b5` (release/topology/catalog) and `b37687e6-7f20-44dd-8cae-96c3fb29216d` (state/hash/CLI/bootstrap) accepted after fixes |
| 02–14 | Not started | Dependency gates in `architecture-decisions.md`; no functionality claimed |

## Baseline checks

| Command / environment | Result |
|---|---|
| Git status/SHA/default-ref; Cargo locked/offline no-deps metadata | Pass; 71 workspace packages, 14 binaries |
| `cargo check --workspace --locked --offline` / Rust 1.98.0 | Pass |
| `cargo test --workspace --lib --all-features --locked --offline` / local Docker | **Timed out at 240s** during suite; not a pass. Longer rerun needs explicit decision; targeted tests may proceed |
| `cargo depgraph-check check` | Pass |
| Initial investigator offline `npm --prefix infra ci` | Failed missing cached tarball; build/test/synth consequently unavailable then |
| `npm --prefix infra ci --ignore-scripts --no-audit --no-fund --fetch-retries=0 --fetch-timeout=20000` | Pass; 315 packages; no lock/source changes |
| `npm --prefix infra run build` | Pass |
| `npm --prefix infra test` | Pass; existing ts-jest/CDK deprecation warnings |
| `npm --prefix infra run synth:all` / no credential-file/metadata access | Pass, dev/prod/ephemeral. Existing Node20 provider and unversioned Lambda artifact warnings; templates not printed |
| Local tooling | Node24.19/npm11.17 (workflow target Node26), Docker29.7; Ansible/actionlint not on PATH |

Synth/tests are not AWS deployment evidence. Infra Node26 parity, runtime process/TLS tests, full suite, VM/backup/firewall, actual GitHub protections and SSM/Lambda connectivity remain unpassed until recorded otherwise. No unrelated warning/security gate weakened.

## Iteration 01 evidence

Implemented strict versioned Zod/JSON schemas, machine-readable catalog validation against Cargo/CDK/assets, typed CLI failures, canonical hashing, UTC CalVer, safe output, runtime/topology budgets and pure mixed-state ownership transitions. GitHub policy draft remains non-actionable until owner inputs/live verification. No adapters/workflows enabled. Details and public module boundary: `deploy/control/README.md`.

Implementers (available GPT-6-Astra): release `04f452d5-a394-4515-83bf-abc4ee0be68b`; topology `6ad1114e-4c6f-4400-9b52-0e5318a6fda0`; state `fece453a-d3e1-4895-b32a-158320fff491`; bootstrap `2044ba28-3f10-4c70-b35a-20133de4d18d`; integrator owns package/lock/catalog/hash/CLI. All based on 00 commit and revalidated with integrated files.

Checks: `npm --prefix deploy/control run schemas`, `npm --prefix deploy/control test` (**571 pass**), `npm --prefix deploy/control run validate-catalog` (4 native, 5 Lambda, 10 scopes, 25 templates; migrator explicitly unavailable), `git diff --check` pass. Initial Node24.19 fallback, then registry-confirmed pinned Node26.8.2 via `npm exec --yes --package=node@26.8.2`: version, fresh `node node_modules/typescript/bin/tsc`, `node --test dist/test/*.test.js` (**571 pass**) and catalog CLI all passed. Initial Node types build issue fixed; no remaining type errors. No existing Rust/infra behavior changed.

Reviewer regression fixes: exact trusted catalog asset completeness; unmasked endpoint/example negatives; completing-attempt evidence required (failed-attempt facts cannot commit release); unsupported architecture retirement rejected before ownership; noncanonical/accessor/symbol arrays rejected without invoking getters.

DEP coverage is **pure/unit only**, not complete acceptance: DEP-03 (CalVer/ref policy), 04/06/07/27 (ownership/intent protocol), 08 (typed inputs), 09 (catalog omissions), 29 (classification), 32 (budgets), 37 (placement), 38 (combined host resource/port accounting only, no distributed host lock). Other aspects/cases remain unpassed. No artifact/provider/backup/process/approval behavior simulated as working tooling.

Rollback/removal: 01 has no active call sites; remove its tooling commit without data/runtime effects. Operational schema v1 is draft-installed only; no automatic upgrade of old state. New inputs: verified CA/identities, per-role pools/resources/ports, exact registries/artifacts/compatibility evidence, approved revisions/nonces and GitHub protection owners. Architecture/key-set retirement remains deliberately blocked pending explicit protocol.

## Required external rollout gates

Owner must supply/authorize GitHub environments/reviewers/OIDC subjects; account/region and bootstrap identities/storage; machines/OS/placement; DNS/NAT/firewalls; CA/certificate and runtime/migrator/backup/provider secret references; connection/resource budgets; off-host recovery/restore evidence; current legacy state and first-cutover plan. Exact input table: `inventory.md`.

No external input blocks offline implementation. Missing live proof blocks only the relevant rollout, never authorizes guessed bootstrap. Global DEP-01–DEP-38 acceptance cases are not yet satisfied; record test level and exact command as coverage lands.
