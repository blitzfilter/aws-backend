# Offline native application Compose

Task 08a. Pure renderer; **not a deployment command or live-readiness gate**. No Docker, shell, filesystem, DNS, AWS, start/stop or credential resolution in rendering. No new dependency. Frozen shared contracts stay unchanged.

## Files and call boundary

- `../control/src/host/application-compose.ts`: `parseApplicationComposeIdentity(input)` and `renderApplicationCompose(parsedRuntime, validatedIdentity) -> string` (Docker Compose JSON, final newline).
- `../control/test/application-compose.test.ts`: offline unit/contract tests with synthetic inventory; no working operator credentials or images.

Identity is a strict, locally branded record:

| Field | Source / validation |
| --- | --- |
| `selection` | `{role: 'api', slot: 'blue'|'green'}`, `{role: 'worker', scope: WorkerScope}`, `{role: 'cron'}` or `{role: 'crawler'}` only |
| `target` | `stage`, `account`, `region`; exact selected runtime inventory stage/AWS target |
| `host_id` | Exact selected role placement, not an arbitrary host override |
| `configuration_sha256` | Existing `canonicalHash(parsedRuntime)`; closed inventory snapshot included |
| `manifest_sha256` | Exact `runtime.release.manifest_digest` |
| `native_image` | Existing release native-image record: component, architecture, Rust target, source SHA, registry/repository/digest |

The trusted integrator must extract this identity from an independently approved target and verified release/configuration, **not builder claims**. Renderer rechecks runtime semantic parsing, identity shape, target, digest, source, role and architecture on every call. Registry must equal target account's private regional ECR hostname, including China/GovCloud partition handling. Tags, registry aliases, overlays, arbitrary env/maps, bind IPs, paths and commands are not inputs. No `platform`/emulation override.

Parsing/branding/hash equality do **not** prove approval, provenance, image existence, actual image architecture, or protected-file contents. Upstream still requires `parseManifest` then `verifyManifestCatalog` against installed trusted catalog. Baseline catalog's pending migrator currently blocks complete release acceptance; this renderer neither bypasses that gate nor runs a migrator.

## Supported output and blockers

| Selection | Result |
| --- | --- |
| Cron | One service in `aura-<stage>-cron`; real periodic-match daemon argv/env |
| Crawler | One service in `aura-<stage>-crawler`; catalog `server` binary, no bootstrap/demo |
| API blue/green | `PRIVATE_BIND_IP_UNAVAILABLE`; no partial Compose document |
| Each of ten worker scopes | Same blocker, in both all-in-one and split placement |
| Data, edge, migrator, arbitrary aggregate role | Rejected |

**Integrator blocker:** frozen inventory has `Listener.bind = PRIVATE` intent and `Host.dns.private` hostname, not an authenticated literal private host bind IP. Even numeric text in the DNS field is not that contract. Do not resolve DNS, substitute wildcard/loopback, use host networking, invent an overlay, or omit required ingress. Integrator must evolve/revalidate the shared contract before API/worker templates can render. Keep future API projects slot-isolated (`api-blue`, `api-green`) and workers scope-isolated; never combine with jobs or platform services. No positive API/worker runnable-output claim in this slice.

Other fail-closed limits:

- No cross-container `localhost`, numeric IP aliases, or `LOCAL_TEST` endpoints, even on one host. Use inventory's routable DNS/TLS names. Renderer never rewrites dependencies to Compose service names.
- Crawler server has fixed business/crawler pool caps **8/16**; mismatched inventory rejects. No fictional pool env keys. Both DBs use the same TLS mode; verify-full uses one bundle containing their approved CAs. Mixed modes reject.
- Only mounted ADC profile renders. WIF subject-token/executable-source materialization is not implemented; rejects rather than guessing paths or helper commands.
- Memory must be at least 128 MiB, disk at least 16 MiB for this fixed template. Host budget/port/pool/lifecycle invariants still come from semantic runtime parsing. Cron pool count must fit runtime `u32`.
- All errors expose fixed `ApplicationComposeError.code` only, never supplied payloads or causes.

## Fixed template safety

- Compose **v2.30+** required (`env_file.format: raw`). JSON uses standard Compose fields; no YAML/Compose dependency added.
- Exec-form `/usr/local/bin/<installed catalog binary>`; cleared image command, nonroot `10001:10001`, init forwarding, SIGTERM.
- `restart: "no"` for every service. Retired singleton must not resurrect after daemon reboot. This also means active services do **not** recover automatically. Eventual owner-aware reconciler must read durable ownership/actual state before any restart; ordinary systemd/Compose autostart is not a substitute.
- One project/service/private bridge per rendered job. No host networking, port publication, expose list, shared namespace, external network, platform services, named/data volumes, Docker socket, privileged devices, hooks or destructive commands.
- Read-only root, `cap_drop: [ALL]`, `no-new-privileges`, 256 PID cap, 1024 file limit, core dumps disabled. Exact inventory CPU/memory cap; memory+swap limit equals memory (no extra swap). 1 MiB shared memory; `/tmp` only writable scratch, noexec/nosuid/nodev, at most 64 MiB and at most 1/8 memory. tmpfs consumes the container memory budget, not extra host allocation.
- Local Docker logging: 4 MiB × 3 files, compression disabled. Rotation is approximate; 16 MiB minimum is not a hard total host-disk quota. Images, Docker metadata, log overhead and both API slots still need host headroom gates.
- `stop_grace_period` exactly matches selected runtime `stop_seconds`; real drain/stop env names also match. Cron execution remains independently configured (default 7200s vs 300s drain); shutdown can cancel work, not promise a completed job.
- Image healthcheck disabled, not replaced with `/bin/true`, shell/curl, or `--check-config`. No installed native probe tool assumed. Cron/crawler operations bind `127.0.0.1:<inventory port>` **inside their own container namespace**. Crawler review also stays there. A future allowlisted namespace-local probe adapter must inspect exact `/health`, `/ready`, `/ops/version` paths using parsed probe budgets. Nothing is published/proxied. Readiness is not proof of process termination, provider authorization, or old-owner fencing.

## Protected files: future materializer required

Renderer emits references only. **Materializer does not exist here; output cannot currently be launched from repository fixtures.** Paths derive only from fixed stage/component IDs:

```text
/etc/aura-historia/<stage>/<cron|crawler>/runtime.env
/etc/aura-historia/<stage>/<cron|crawler>/google-adc.json
/etc/aura-historia/<stage>/<cron|crawler>/postgres-ca.pem
```

Root-owned ancestors/directories: no group/other write; component directory root:10001 mode0750. `runtime.env`: root:root mode0600, read by trusted Compose helper, never mounted into container. ADC/CA: root:10001 mode0440; nonroot process can read, cannot write. Only individual fixed read-only bind mounts, `create_host_path: false`; no whole credential directory or host root mount. No symlink in any ancestor or leaf, no nonregular file. Pure renderer cannot stat/enforce ownership/mode/contents; trusted host gate must verify under custody immediately before use. Host root remains trusted.

Materializer must bind file contents and protected reference revisions to the approved configuration/image identity; fixed paths do not prove freshness. Permit only role-specific keys below, never an arbitrary env map, image env override, PG options, endpoint/proxy override, loader variable, extra path or executable credential source. `format: raw` preserves literal dollar/quote characters; write no surrounding shell quotes and reject NUL/newlines in env values. Compose explicit environment wins over env-file keys, but allowlisting is still required. Never log/print resolved Compose environment or secret files.

| Role | Required protected `runtime.env` keys |
| --- | --- |
| Both | `VERTEX_AI_PROJECT_ID`, `VERTEX_AI_LOCATION`, `VERTEX_AI_MODEL`, resolved from approved Vertex configuration, not invented from stage |
| Cron | `POSTGRES_PASSWORD` from cron business principal's reference; `OPENSEARCH_USERNAME`, `OPENSEARCH_PASSWORD` from its search reference (required in dev/prod; runtime ignores search auth in explicit local stages) |
| Crawler | `BUSINESS_DATABASE_URL`, `LOCAL_DB_URL`, `CRAWLER_REVIEW_AUTH_TOKEN` from its distinct business/crawler/review references |

Crawler URLs are actual required runtime inputs, **not** `*_FILE` variables. Materializer must percent-encode credentials, bind host/port/database/principal exactly to the corresponding parsed inventory endpoint/identity, reject URL query policy overrides, and never use localhost. Both protected URLs use the renderer's shared PostgreSQL TLS policy/CA bundle. Cron's host/port/database/principal/pool/TLS are rendered explicitly; do not also supply `POSTGRES_PASSWORD_FILE`.

ADC JSON must come from the selected `MOUNTED_ADC.configuration_ref`, be readable by UID10001, and use an approved non-executable credential source with no arbitrary referenced files/URLs. Google auth/refresh is not verified here. CA PEM must resolve the approved Postgres CA references; no test CA. Crawler combines both DB CAs in the fixed bundle. Cron OpenSearch uses default transport trust, **not a fictional OpenSearch CA env variable**: prove the pinned image's trust store accepts the configured search certificate (including any private CA), or block launch. Do not disable certificate verification.

Cron currently needs no AWS SDK credential discovery; crawler CloudWatch is off when its optional group/stream inputs are absent. This template intentionally supplies neither, uses bounded Docker logs, and mounts no Roles Anywhere/SSM private keys. API/worker AWS credential wiring remains blocked/unimplemented. Do not inherit AWS credentials, credential-process config or metadata fallback from image/host. ADC file mounts pin an inode; renewal/rotation must not silently rely on atomic host replacement updating an existing mount. Rotation needs a separately designed, owner-aware materialization/recreation protocol.

## Unrun image/host/operation gates

Before any authorized launch, integrator/operator must independently establish:

1. Verified complete release/provenance and exact approved plan/intent/config revision; CAS ownership and authenticated host/architecture observation. Read-only target config itself grants no launch or scheduler ownership.
2. Exact private ECR digest already present (`pull_policy: never`), native Linux architecture and matching executable ABI/libraries. No multiarch-index ambiguity, emulation or mutable tag. Image builder must install catalog binaries at the fixed paths and bake crawler's matching **build-time** `COMMIT_SHA`; runtime env cannot set crawler identity.
3. Image metadata has no `VOLUME`, unsafe inherited env/provider/proxy/loader configuration or unexpected exposed listeners; fixed exec/user/filesystem policies actually work. No image build recipe or image execution was delivered/tested here.
4. Protected-file materialization/permissions/reference binding above; real DNS/private routing from container namespace to DB/search, TLS trust, schemas and credentials; host firewall/egress policy. A Compose bridge is not an authorization or egress firewall.
5. Compose v2.30+ validation against real protected files without leaking resolved env; Docker cgroup/swap/log/tmpfs/security enforcement and aggregate disk headroom. Independent Compose5.4.0 `config --no-env-resolution --quiet` accepted all12 generated job/topology/stage configurations with an empty process environment. This proves structure only, not file resolution, minimum-version compatibility or daemon enforcement.
6. Genuine binary preflight, namespace-local operational probes, signal/drain/stop behavior, confirmed predecessor termination, singleton handover, reboot and retired-owner non-resurrection. No fake readiness assertion in unit tests. Renderer neither schedules these checks nor performs them.

## Offline verification and safe removal

From repository root:

```sh
npm --prefix deploy/control run build
node --test deploy/control/dist/test/application-compose.test.js
npm --prefix deploy/control test
npm --prefix deploy/control run validate-catalog
```

Tests check JSON/template shape, installed catalog names, real runtime env-name parity, identity/injection negatives, all blocked worker scopes/API slots, split placement, resources/deadlines, and forbidden controls. Integrated Node26.8.2 build/full controller **686 tests**/catalog pass, including93 renderer tests. Independent reviewer also ran686 tests on Node24 and12 Compose5.4.0 structural checks. No image, Docker-daemon, TLS or readiness acceptance; minimum Compose2.30 compatibility remains untested.

No live resources or protected files are created, so this task has nothing operational to tear down. Safe code removal: remove only these three new task files (and empty new directories / task-specific ignored build outputs if desired); leave shared contracts/catalog/package/locks and other work untouched. Future live retirement must use independently approved ownership and exact observed container IDs, confirm termination before successors, and retain journal evidence. Never remove by project/name prefix alone, use `down -v`, `--remove-orphans`, prune, or delete data volumes. This renderer supplies no removal/start/restart commands.
