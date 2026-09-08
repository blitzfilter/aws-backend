# DOX

## Purpose

- Own CDK app, constructs, and infra tests.
- Own cloud contract for backend crates.

## Core Design

- CDK app composes focused stacks: data, compute, API, and prod observability.
- `src/application-stack.ts` wire stack set and public outputs. Keep it orchestration-only.
- `src/config.ts` own stage drift. Same stack shape for `prod`, `dev`, `ephemeral`. Difference must be on purpose.
- `src/worker-queue-config.ts` own ten native worker scopes and queue settings. `src/constructs/worker-queues.ts` own separate Standard source/DLQ pairs, exact unbound IAM, and handoff outputs. Keep Shopify catalog/wiring separate.
- Prefer typed definition maps for repeated resources like Lambdas and queues. No copy-paste forests.
- CloudFormation input surface stay tiny. Compute deploy version come from `CommitSHA`. Secrets and external IDs come from SSM dynamic refs. Fixed shared buckets stay fixed.
- Postgres is self-hosted. Infra passes explicit `POSTGRES_*` env vars from SSM/test settings; no RDS Proxy.
- Infra own runtime glue: env vars, triggers, schedules, IAM, queue wiring, outputs, retention, alarms. Rust crates own business rules.

## Ownership

- This doc rule `infra/**`.
- Keep app entry, constructs, tests, synth flow, and deploy contract in sync.

## Local Contracts

- Read root, then here, before edit.
- If code adds or changes env var, trigger, queue, event bus, schedule, API route, Cognito need, workflow step, search dependency, or IAM action, update infra in same change.
- If infra change shifts behavior, update nearest code doc, tests, and public docs when contract go public.
- New Lambda means: add definition, wire event source or route, grant IAM, set env, wire deploy flow, and update test wiring when needed.
- Keep outputs intentional. Export stable values people or tests really use. No noisy output spam.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep stage drift low. Dev, prod, ephemeral should differ on purpose only.
- Prefer high-level CDK constructs. Drop lower only when CDK no fit or exact CloudFormation control matter.
- Keep names stable, predictable, and stage-suffixed.
- Least-privilege IAM first. Grant exact action and resource. Wildcard only when AWS force hand.
- Keep secrets out of code and templates when possible. Prefer SSM dynamic refs or deploy-time imports.
- Prod safety first: retain durable prod data, keep rollback simple, avoid surprise replacement of stateful resources.
- No hotswap mindset. Prefer normal CloudFormation change-set rollout and rollback semantics.
- API Lambdas be short request-response handlers. Keep timeout and memory conservative.
- Lambda queue workers do external I/O and side effects. Tune batch, retry, and visibility conservatively. Visibility should clearly exceed Lambda timeout; heavy Lambda workers use around `6x` timeout.
- Polling native Rust workers are not Lambda event sources; no `6x` rule. Match runtime initial visibility: 60s short, 300s percolator/embedding/translation/normalization, 360s delivery. Keep bounded execution and heartbeat contract in sync.
- Native queues: source7d/DLQ14d, `maxReceiveCount=5`, poll20s, SQS-managed encryption, TLS-only, intended-source DLQ redrive allow, prod deletion/replacement Retain. No tier dimension.
- Native IAM: exact scoped publish/consume actions; paired DLQ attribute-read only for startup checks. No new long-lived identity or runtime operator powers. External bare-metal identity owner binds exported policy ARNs and queue outputs; CDK does not deploy that process.
- Queue suffix and runtime `STAGE` must match `prod`/`dev`/`ephemeral`; custom stack prefixes never rename queues. Document LocalStack runtime endpoint restrictions honestly in README.
- Scheduled sync jobs should fail fast, not camp for long timeouts.
- Ephemeral stage should mock or localize third-party integration when possible.
- Ephemeral API may add LocalStack-only broad invoke grants for path-param routes; real stages stay tighter.
- Keep prod-only alarms and noisy observability out of lower stages unless signal justify cost.

## Verification

- Use workflow-pinned Node 26 and `npm --prefix infra ci`; keep lockfile and policy gates intact.
- `npm --prefix infra run build`
- `npm --prefix infra test`
- `npm --prefix infra run synth -- --context stage=dev`
- `npm --prefix infra run synth -- --context stage=prod`
- `npm --prefix infra run synth -- --context stage=ephemeral`
- `npm --prefix infra run synth:all`

## Child DOX Index

- No child `AGENTS.md`.
- `README.md` — stack, worker queue, identity handoff, and rollout contracts.
- `examples/worker.env.example` — native worker queue environment, no credentials.
