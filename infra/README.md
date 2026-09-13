# Aura Historia infrastructure

This directory contains the AWS CDK app for the Aura Historia backend.

The application is split into small CDK/CloudFormation stacks with typed
configuration objects instead of hand-written templates. The same stack code is
synthesized for:

- `prod` — real AWS production resources, production alarms enabled
- `dev` — real AWS development resources, production alarms disabled
- `ephemeral` — LocalStack resources, including a local OpenSearch domain

## Structure

```text
bin/app.ts                 # CDK entrypoint and stage selection
src/application-stack.ts   # data, compute, API, observability stack composition
src/config.ts              # stage configuration, fixed buckets, SSM dynamic refs
src/worker-queue-config.ts # typed native worker scopes, timing, retention, alarms
src/parameters.ts          # deployment artifact version input
src/resources/             # synth-time resources, e.g. Cognito email HTML and inline JS
src/constructs/            # focused infrastructure modules
  api.ts                   # HTTP API Gateway routes, domain, CloudFront, WAF, CORS, JWT authorizer
  cognito.ts               # Cognito user pool, public client, IdPs, hosted UI domain
  eventing.ts              # EventBridge buses/rules, SQS mappings, Pipes
  lambdas.ts               # Lambda definitions, env vars, IAM grants
  lambda-egress.ts         # standalone hybrid IPv4 NAT/EIP foundation, NOT instantiated
  observability.ts         # prod-only alarms and alarm topic
  opensearch.ts            # external dev/prod endpoint or LocalStack domain
  queues.ts                # existing Shopify Lambda queue and DLQ
  worker-queues.ts          # separate native worker queues, scoped IAM, handoff outputs
  storage.ts               # Postgres connection settings

```

## PostgreSQL TLS integration gate

Hybrid work remains on a non-deploying integration branch. **Do not activate this configuration before CA delivery, network and identity prerequisites are implemented and approved.** Deployment status: `docs/deployment/implementation-status.md`.

Every DB-using Lambda now receives `STAGE` and `POSTGRES_SSL_MODE`. Dev/prod require `verify-full` plus `POSTGRES_SSL_ROOT_CERT`, whose file path comes from `/postgres/{stage}/ssl-root-cert-path`. The CA must already exist at that path in the approved Lambda runtime/configuration; CDK does not materialize it. Missing/invalid CA blocks startup. Ephemeral explicitly uses `disable`. This is a configuration contract, not proof of Internet reachability or completed Lambda secret handoff.

All native constructors share that policy, including crawler URLs and cron's dedicated session. Native credentials may use `POSTGRES_PASSWORD_FILE` (0400/0600) instead of `POSTGRES_PASSWORD`; both together fail. Root CA files are nonsecret and at most 1 MiB. Do not set ambient `PGSSLCERT`, `PGSSLKEY`, `PGSSLROOTCERT` or `PGOPTIONS`. Supplied CA plus WebPKI trust is documented in deployment ADR-004; client certificates are not supported yet.

Keep total connections within the reviewed inventory budget: ten worker pools + both API pools + cron pool **and one dedicated session** + crawler business/local pools + Lambda reserved concurrency × per-function pool + Sequin + migration/backup sessions + reserve. Defaults of two connections are per process, not an environment budget. Lambda concurrency/network enforcement lands in the approved prerequisite iteration.

Certificate rotation: install old+new trust bundle first, restart/recycle each client through lifecycle controls, rotate server certificate, verify fresh connections, then remove old trust. Existing pools and published Lambda environment snapshots do not refresh themselves. Never roll secrets back with old code. No live rotation performed here.

## Hybrid Lambda-egress foundation

`LambdaEgress` synthesizes an explicit IPv4 VPC, public NAT subnets, private Lambda subnets, owned EIPs and dedicated no-ingress SG. `SINGLE` versus `PER_AZ` is an explicit cost/availability choice; DB egress requires exact public `/32` and port plus a separate explicit HTTPS policy. Account/region/AZs/CIDRs are required literal inputs, never discovery or guessed live values.

**Not instantiated by this application.** Existing stack/resource identities, invocation targets and network attachment remain unchanged. No Lambda attachment, runtime identity/CA delivery, firewall mutation or verified reachability. Construct contract, operator inputs, validation and limits: [`lambda-egress-09a.md`](lambda-egress-09a.md). Future bootstrap consumes these typed handles only after review/approval; EIP tokens are not allocated addresses.

## Common commands

Use Node **26**, matching the workflow pin. `npm ci` uses `package-lock.json`;
no dependency or policy-gate overrides are needed. Run from `infra/`:

```bash
npm ci
npm run build
npm test
npm run synth -- --context stage=dev
npm run synth -- --context stage=prod
npm run synth -- --context stage=ephemeral
npm run synth:all
```

These commands build, test, and synthesize only; they do not deploy.

Synth creates these stacks per stage:

- `application-{stage}-data` — Postgres settings, Shopify/worker SQS, unbound worker IAM policies, and LocalStack OpenSearch
- `application-{stage}-compute` — Lambdas, Cognito, eventing, schedules
- `application-{stage}-api` — HTTP API Gateway routes, domain, CloudFront, integrations, authorizer
- `application-prod-observability` — prod-only alarms and alarm topic

Dev CloudFront owns the wildcard alias `*.dev.aura-historia.com`; the API URL stays
`api.dev.aura-historia.com`. This avoids stale exact DNS targets blocking distribution
creation. Prod uses the exact alias `api.aura-historia.com`.

Deployments should use `cdk deploy --all` without hotswap. CI uses
CloudFormation change sets (`--method change-set`) so stack updates keep
CloudFormation's normal rollback semantics.

Deployments do not require a full CDK bootstrap stack in the target account/region.
Each stack uses `CliCredentialsStackSynthesizer` with the existing staging bucket
`aura-historia-cfn-artifcats-eu-central-1`. CDK uploads large CloudFormation
templates and any future file assets under the stage prefix (`${stage}/`). Lambda
ZIPs and scheduled Fargate images are still referenced as prebuilt S3/ECR
artifacts keyed by `CommitSHA`, not as CDK-managed assets. The retired periodic
matcher ECS image is no longer built or referenced by CDK.

Rollback is performed by redeploying a previous `CommitSHA` parameter value to the
compute stack. Lambda ZIP keys and mail-template prefixes include that SHA, so CDK
points compute resources back to the previously uploaded artifacts.

## Native processes

Production native processes are:

- `aura-historia-api`
- `aura-historia-worker`
- `aura-historia-cron`

## Native worker queue contract (#1558)

`src/worker-queue-config.ts` owns the typed catalog and shared settings. All ten
scopes are enabled in `prod`, `dev`, and `ephemeral`. This catalog is separate from
the Shopify Lambda catalog: no tier dimension, new Lambda, event-source mapping,
or change to existing Shopify resources/wiring.

Each enabled scope owns one **Standard source queue** and one **Standard DLQ**:

- Source: `aura-worker-<scope>-<stage>`.
- DLQ: `aura-worker-<scope>-dlq-<stage>`.
- Stage is exactly CDK's `prod`, `dev`, or `ephemeral`, not a stack-name prefix or
  the frontend's `stage` label. Names are validated against SQS's 80-character
  limit; they are never truncated. Runtime `STAGE` must match the queue suffix.
- Source retention: **7 days** (604800s). DLQ retention: **14 days** (1209600s).
- Source `maxReceiveCount`: **5**. Long polling: **20s** on both queues.
- SQS-managed encryption on both; no customer KMS key or extra KMS grants.
- Both resource policies deny all SQS access over non-TLS transport. No public
  Allow or cross-account access is granted.
- DLQ `redrivePermission=byQueue` allows only its named source ARN. Source queues
  use `denyAll` so they cannot become another queue's DLQ. This is not permission
  for a runtime to perform operator replay/redrive.
- Prod queues retain on **deletion and replacement**. Dev/ephemeral queues delete.
  Retained old queues need explicit operator inventory/recovery; renaming a queue
  does not migrate its messages or consumers.

| Runtime scope | Output stem after `Worker` | Initial source visibility |
| --- | --- | ---: |
| `product-listing-opensearch` | `ProductListingOpensearch` | 60s |
| `search-filter-projection` | `SearchFilterProjection` | 60s |
| `search-filter-percolator` | `SearchFilterPercolator` | 300s |
| `search-filter-match-notification` | `SearchFilterMatchNotification` | 60s |
| `watchlist-notification` | `WatchlistNotification` | 60s |
| `product-content-assessment` | `ProductContentAssessment` | 60s |
| `product-embedding` | `ProductEmbedding` | 300s |
| `product-translation` | `ProductTranslation` | 300s |
| `product-listing-normalization` | `ProductListingNormalization` | 300s |
| `notification-delivery` | `NotificationDelivery` | 360s |

These are **polling Rust processes**, not Lambda SQS event sources. The infra
six-times-Lambda-timeout guidance does **not** apply. Values match the worker's
45s short / 240s slow execution budgets; notification's 360s visibility leaves
headroom around its five-minute service-owned lease. Workers own bounded
execution, visibility heartbeats, retry, and deletion after successful handling.
Standard SQS may duplicate/reorder messages; handlers must remain idempotent.

### Identity and outputs

The bare-metal runtime's AWS role/trust and process deployment are **not defined
in this CDK app**. No IAM user, access key, new runtime role, or invented deploy
binding is created. Reuse the existing AWS credential/assumed-role arrangement.
The external identity owner attaches only the needed per-scope managed policies;
do not reuse the CI deploy role or an unrelated Lambda role as the worker role.

| Unbound policy | Exact source actions | Paired DLQ actions |
| --- | --- | --- |
| Publisher | `sqs:SendMessage`, `sqs:GetQueueAttributes` | `sqs:GetQueueAttributes` only |
| Consumer | `sqs:ReceiveMessage`, `sqs:DeleteMessage`, `sqs:ChangeMessageVisibility`, `sqs:GetQueueAttributes` | `sqs:GetQueueAttributes` only |

DLQ attribute reads are required by the runtime's startup validation. No runtime
DLQ message access, `GetQueueUrl`, batch pseudo-actions, wildcard resource grants,
purge, queue deletion, or operator redrive actions are included. A process that
both accepts CDC and polls its scoped queue needs **both** policies for that
scope. Existing S3 template-read and SES-send permissions remain separate and
unchanged; attaching queue policies is additive, not a replacement.

The data stack (or single ephemeral stack) outputs:

- `WorkerQueueAwsRegion` — effective CloudFormation region; set `AWS_REGION`
  explicitly to this value. `AWS_DEFAULT_REGION` alone is not this runtime's
  configuration contract. Queue URL, ARN, and SDK region must agree.
- `WorkerQueueStage` — set `STAGE` to this exact value.
- Per table stem: `Worker<Stem>QueueUrl`, `Worker<Stem>QueueArn`,
  `Worker<Stem>DeadLetterQueueUrl`, `Worker<Stem>DeadLetterQueueArn`,
  `Worker<Stem>PublisherPolicyArn`, `Worker<Stem>ConsumerPolicyArn`.
- Managed policy names: `aura-worker-<scope>-publisher-<stage>` and
  `aura-worker-<scope>-consumer-<stage>`.

Set `AURA_HISTORIA_WORKER_SCOPE` to the exact runtime scope and
`AURA_HISTORIA_WORKER_QUEUE_URL` to its **source** `QueueUrl`, never its DLQ.
See [`examples/worker.env.example`](examples/worker.env.example). Preserve existing
`POSTGRES_*` and scope-specific OpenSearch/Vertex settings. EMAIL delivery still
requires `S3_BUCKET_NAME_TEMPLATES`, `NOTIFICATION_EMAIL_FROM`,
`NOTIFICATION_EMAIL_REPLY_TO`, `COMMIT_SHA`, and `STAGE`, with existing S3/SES grants.
No secrets or credentials belong in the example or stack outputs.

For LocalStack, `singleStack=true` still produces `...-ephemeral` names. Use
`STAGE=ephemeral`; substituting `local` or `test` implies different queue names.
`AWS_ENDPOINT_URL_SQS` is allowed only in `ephemeral`, `local`, or `test`, with
exactly the same origin as the queue URL. Real AWS stages must not set endpoint
overrides; the runtime rejects global `AWS_ENDPOINT_URL`.

### Operations and rollout boundary

Prod adds two alarms per scope on the existing `cloudwatch-alarms-prod` SNS topic:

- Source `ApproximateAgeOfOldestMessage` **>= 900s**.
- DLQ `ApproximateNumberOfMessagesVisible` **>= 1**.

Both use **Maximum**, one **5-minute** evaluation period, and missing data as
**not breaching**. Lower stages have no alarms. Existing topic subscriptions and
Lambda/API alarms remain unchanged. These are backlog signals, not proof of
consumer health; idle queues can have missing metrics.

Investigate DLQ failures, correct the cause, then replay under separately owned
operator authorization. Never give runtime roles purge/redrive powers. Standard
queue retention keeps the original enqueue timestamp when a message moves to the
DLQ, so operators should not assume a fresh 14-day recovery window on arrival.

This provisions the infra side only. It neither deploys a worker nor changes
Sequin subscriptions, external IAM trust, credentials, or S3/SES configuration.
Before runtime cutover, the external owner must attach the exported policies,
apply the matching environment, verify startup attribute checks/readiness, and
verify real publish/consume/retry/DLQ behavior. Synthesis alone does not establish
an end-to-end durable-delivery guarantee or change the documented MVP guarantee.

## Deployment inputs

Only the compute stack exposes a CloudFormation parameter:

- `CommitSHA` — artifact version to deploy or roll back to

The Lambda artifact and mail-template buckets are fixed in `src/config.ts`:

- `aura-historia-binary-artifacts-eu-central-1`
- `aura-historia-mail-templates-eu-central-1`

LocalStack acceptance tests synthesize one ephemeral stack with CDK context
`singleStack=true` and pass the host-mapped edge port as `localStackMappedPort`.
These values are synth-time context, not CloudFormation parameters.

## Stage-specific SSM parameters

Real AWS stages resolve external integration settings via CloudFormation dynamic
references to SSM Parameter Store. Required paths are stage-specific for `prod`
and `dev`:

```text
/opensearch/{stage}/endpoint-url
/opensearch/{stage}/username
/opensearch/{stage}/password
/eventbridge/{stage}/stripe-event-bus-name
/eventbridge/{stage}/shopify-event-bus-name
/stripe/{stage}/pro-product-id
/stripe/{stage}/ultimate-product-id
/stripe/{stage}/pro-monthly-price-id
/stripe/{stage}/pro-yearly-price-id
/stripe/{stage}/ultimate-monthly-price-id
/stripe/{stage}/ultimate-yearly-price-id
/certificates/{stage}/api-regional-certificate-arn
/certificates/{stage}/api-cloudfront-certificate-arn
/secrets/{stage}/gemini-api-key
/secrets/{stage}/google-application-credentials
/secrets/{stage}/google-geocoding-api-key

/secrets/{stage}/zoho-accounts-url
/secrets/{stage}/zoho-campaigns-url
/secrets/{stage}/zoho-client-id
/secrets/{stage}/zoho-client-secret
/secrets/{stage}/zoho-list-key
/secrets/{stage}/zoho-refresh-token
```

`fxrate-lambda` currently reads `/fxratesapi/prod/api-token` for the scheduled
sync. On first real-stage compute-stack creation, a custom resource synchronously
invokes this same Lambda with stable deployment source ID `deployment:fxrate:initial:{stage}:v1`.
Deployment fails when this initial capture fails; it must run after PostgreSQL
business migrations. Updates, deletes, and `ephemeral` do not invoke it. The
`ephemeral` stage uses local/mock values for third-party integrations where possible.
