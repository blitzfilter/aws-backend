# Implementation status

**Deployment readiness: false. Live changes: none.**

Baseline: `901b42f20e3cf6769763f1a9c052d307e813cb9a` (clean), branch `task/#1412-deployment`. No pushed refs, PRs, tags or environment approvals. Only local iteration commits authorized by playbook.

Models: orchestrator GPT-6-Astra; delegation has no model selector. Runtime investigator `2ac2923c-d3e0-4d8d-b62e-24dba77970c3` and AWS investigator `5d2424e8-0e8b-4dd9-b0cc-d3507ead76ac` reported GPT-6-Astra. Storage session `c58e4961-7eb6-4168-b5e5-e512323417fb` exhausted context; no result credited, integrator recovered evidence.

| Iteration | State | Accepted commit / evidence |
|---|---|---|
| 00 | Accepted | `303f50b8de5c603f2c11498979cb2867634c620e`; reviewer `5d2424e8-0e8b-4dd9-b0cc-d3507ead76ac` accepted after two P2 inventory omissions fixed; no runtime change |
| 01 | Accepted default-off contracts | `f96206473b1fc0395089008b799306c67251f6da`; independent reviewers `c6ac1bf8-98c5-4451-a690-0e1df1d045b5` (release/topology/catalog) and `b37687e6-7f20-44dd-8cae-96c3fb29216d` (state/hash/CLI/bootstrap) accepted after fixes |
| 02 | Accepted local TLS/configuration slice | Local commit follows; reviewers `08d567dc-f8ac-48b9-9740-801995410d99` (shared/TLS) and `96c8525b-bafa-4807-ad71-b3c0bfce2263` (callers/crawler/CDK) accepted; no runtime activation |
| 03–14 | Not started | Dependency gates in `architecture-decisions.md`; no functionality claimed |

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

## Iteration 02 evidence

Shared policy owner `aa94ad01-8a18-40b3-8a38-3dbed9b65c06`; native/Lambda wiring `465d79c7-d0d6-4264-9663-76d7fcdb7ecc`; crawler `560d0d58-5b33-4ce6-855d-a30979503a92`. Prior runtime session exhausted context with zero edits; replacement owns accepted work. Available model GPT-6-Astra.

Every native and DB Lambda entrypoint now uses explicit stage/verified TLS policy; crawler URL bypass removed. Protected password files and safe complete error chains supported; direct sessions bounded to 5s outside pool cap. Crawler server no longer starts Docker/creates databases/migrates; separate `bootstrap-local` rejects real/missing stages. Crawler read-only history checks do not prove full schema/role readiness. This startup portion overlaps 04; cancellation/process custody still pending. CDK passes stage/mode/CA-path reference but does not deliver the CA file. Exact SQLx pin and constrained ambient-read exception: ADR-004; no CA-only pinning claimed.

Checks: shared `cargo test --locked --offline -p platform-postgres --all-features --lib` **119 pass** (six ignored: four real TLS tests run separately plus two subprocess helpers covered by parent tests); `cargo test --locked --offline -p platform-postgres --lib tls_tests::should_ -- --ignored --test-threads=1` **4 pass**, independently rerun by reviewer. DEP-30 **local integration** proves verified TLS, bad host/CA/expired cert/password/plaintext refusal, structured/URL/direct-session paths; host firewall/NAT remains untested. Fixed local Unix Docker, only acquired IDs cleaned; no remaining fixture resources/keys.

Caller config checks: seven-package all-target/all-feature check pass; native/Lambda **60 targeted tests** pass, then full API config **10 pass** after ordering fix; crawler `local_db::` **79 pass**, pool-sizing **3 pass**. Shared Clippy `-D warnings` pass. Integrated `cargo check --workspace --all-targets --all-features --locked --offline`, depgraph, full fmt check pass. Infra build/test/dev-prod-ephemeral synth pass (existing warnings remain). Controller fresh Node26.8.2 build/**571 tests**/catalog pass after excluding new bootstrap tool.

New required runtime inputs: `STAGE`, `POSTGRES_SSL_MODE`, real-stage readable `POSTGRES_SSL_ROOT_CERT`; one password source; crawler both URLs and pre-provisioned histories. Crawler acquire timeout changed 30s→5s. Credential/CA rotation requires pool/process recycle; Lambda current-config delivery remains 09/10 gate. Editing `infra/examples/worker.env.example` was blocked by a tool security rule (not bypassed); updated contract is in `infra/README.md` and crate docs instead.

Review fixes cover every error-source formatter, invalid-Unicode rejection, API validation before AWS discovery, owned-ID Docker cleanup and explicit SQLx ambient exception. No live account/host/firewall/secret changes. Reverting 02 would remove TLS enforcement and restore unsafe old startup; do not deploy such a rollback. Full workspace library test still has baseline timeout; no longer rerun authorized. No data migrations changed.

## Required external rollout gates

Owner must supply/authorize GitHub environments/reviewers/OIDC subjects; account/region and bootstrap identities/storage; machines/OS/placement; DNS/NAT/firewalls; CA/certificate and runtime/migrator/backup/provider secret references; connection/resource budgets; off-host recovery/restore evidence; current legacy state and first-cutover plan. Exact input table: `inventory.md`.

No external input blocks offline implementation. Missing live proof blocks only the relevant rollout, never authorizes guessed bootstrap. Global DEP-01–DEP-38 acceptance cases are not yet satisfied; record test level and exact command as coverage lands.
