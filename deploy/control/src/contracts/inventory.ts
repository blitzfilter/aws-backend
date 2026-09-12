import { isIP } from 'node:net';
import { z } from 'zod';
import {
  Account, Architecture, DatabaseTarget, Digest, Identifier, Positive, Region,
  Revision, SchemaVersion, SecretReference, Sha, Stage, WorkerScope, isRealStage,
} from './primitives.js';

const Hostname = z.string().max(253).regex(/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)*$/);
const PortNumber = Positive.max(65535);
const Seconds = Positive.max(86400);
const Resources = z.strictObject({
  cpu_millicores: Positive,
  memory_mib: Positive,
  disk_mib: Positive,
});
const Listener = z.strictObject({
  port: PortNumber,
  bind: z.enum(['LOOPBACK', 'PRIVATE', 'PUBLIC', 'ALL_IPV4', 'ALL_IPV6', 'ALL_INTERFACES']),
}).describe('TCP reservation. One host port belongs to one listener, even across bind addresses.');
const Tls = z.discriminatedUnion('mode', [
  z.strictObject({ mode: z.literal('VERIFY_FULL'), server_name: Hostname, ca_ref: SecretReference }),
  z.strictObject({ mode: z.literal('DISABLED') }),
]);
const PrivateEndpoint = z.strictObject({
  host: Hostname,
  port: PortNumber,
  tls: Tls,
  routing: z.strictObject({
    method: z.enum(['PRIVATE_NETWORK', 'WIREGUARD', 'LOCAL_TEST']),
    target_host_id: Identifier,
  }),
});
const PublicEndpoint = PrivateEndpoint.extend({
  routing: z.strictObject({ method: z.literal('PUBLIC_INTERNET'), target_host_id: Identifier }),
});
const DatabaseIdentity = z.strictObject({ principal: Identifier.max(63), secret_ref: SecretReference });
const Pool = z.strictObject({ identity: DatabaseIdentity, pool_max: Positive });
const RolesAnywhere = z.strictObject({
  method: z.literal('ROLES_ANYWHERE'),
  stage: Stage,
  application_identity_ref: SecretReference,
  trust_anchor_ref: SecretReference,
  profile_ref: SecretReference,
  certificate_ref: SecretReference,
  private_key_ref: SecretReference,
});
const Vertex = z.discriminatedUnion('method', [
  z.strictObject({
    method: z.literal('MOUNTED_ADC'),
    configuration_ref: SecretReference.extend({ provider: z.literal('PROTECTED_FILE') }),
  }),
  z.strictObject({
    method: z.literal('WORKLOAD_IDENTITY_FEDERATION'),
    configuration_ref: SecretReference,
    subject_token_ref: SecretReference,
  }),
]);
const Host = z.strictObject({
  id: Identifier,
  architecture: Architecture,
  operating_system: z.literal('UBUNTU_24_04'),
  owned_stages: z.array(Stage).min(1).max(Stage.options.length),
  shared_hardware_acknowledged: z.boolean(),
  capacity: Resources,
  reserve: Resources,
  dns: z.strictObject({ management: Hostname, public: Hostname, private: Hostname }),
  ssm_node_identity: z.strictObject({
    managed_node_id: z.string().regex(/^(?:mi-[0-9a-f]{17}|i-(?:[0-9a-f]{8}|[0-9a-f]{17}))$/),
    registration_ref: SecretReference,
  }),
});
const Service = z.strictObject({
  host_id: Identifier,
  resources: Resources,
  listener: Listener,
  endpoint: PrivateEndpoint,
  ca_ref: SecretReference,
});
const ApiSlot = z.strictObject({
  listener: Listener,
  endpoint: PrivateEndpoint,
  operations_listener: z.strictObject({ port: PortNumber, bind: z.literal('LOOPBACK') }),
}).describe('Business ingress and separate HTTP operational listener. Probe through an allowlisted slot-local execution, never publish/proxy operations. Ports conservatively reserve host-wide capacity, including shared network namespaces.');
const Api = z.strictObject({
  host_id: Identifier,
  resources_per_slot: Resources,
  ca_ref: SecretReference,
  slots: z.strictObject({
    blue: ApiSlot,
    green: ApiSlot,
  }),
  credentials: z.strictObject({
    business: Pool,
    aws: RolesAnywhere,
    opensearch_ref: SecretReference,
    vertex: Vertex,
    cognito_ref: SecretReference,
    stripe_ref: SecretReference,
    zoho_ref: SecretReference,
  }),
});
const Worker = Service.extend({
  scope: WorkerScope,
  credentials: z.strictObject({
    business: Pool,
    aws: RolesAnywhere,
    queue_scope: WorkerScope,
    opensearch_ref: SecretReference.optional(),
    vertex: Vertex.optional(),
    email: z.strictObject({ s3_read_ref: SecretReference, ses_send_ref: SecretReference }).optional(),
  }),
});
const Cron = Service.extend({
  ownership_plan: z.literal('READ_ONLY_TARGET'),
  credentials: z.strictObject({
    business: Pool, aws: RolesAnywhere, opensearch_ref: SecretReference, vertex: Vertex,
  }),
});
const Crawler = Service.extend({
  ownership_plan: z.literal('READ_ONLY_TARGET'),
  credentials: z.strictObject({
    business: Pool, crawler: Pool, aws: RolesAnywhere, vertex: Vertex, review_auth_ref: SecretReference,
  }),
});
const MaintenanceIdentity = z.strictObject({ identity: DatabaseIdentity, sessions: Positive });
const Database = z.strictObject({
  target: DatabaseTarget,
  name: Identifier.max(63),
  history_id: Identifier,
  max_connections: Positive,
  migrator: MaintenanceIdentity,
  backup: MaintenanceIdentity,
  reserve_sessions: Positive,
});
const BusinessPostgres = Service.extend({
  lambda_endpoint: PublicEndpoint,
  database: Database.extend({
    target: DatabaseTarget.extract(['business']),
    sequin: MaintenanceIdentity,
  }),
});
const CrawlerPostgres = Service.extend({
  database: Database.extend({ target: DatabaseTarget.extract(['crawler']) }),
});
const LambdaPool = z.strictObject({
  identity: DatabaseIdentity, reserved_concurrency: Revision, pool_max: Positive,
});
const StageInventory = z.strictObject({
  stage: Stage,
  aws: z.strictObject({ account_id: Account, region: Region }),
  roles: z.strictObject({
    api: Api,
    workers: z.array(Worker).length(WorkerScope.options.length),
    cron: Cron,
    crawler: Crawler,
    postgres_business: BusinessPostgres,
    postgres_crawler: CrawlerPostgres,
    opensearch: Service.extend({ admin_identity_ref: SecretReference }),
    sequin: Service.extend({ admin_identity_ref: SecretReference }).describe('Resources cover Sequin and its dedicated metadata PostgreSQL/Redis, not the business or crawler databases.'),
    edge: z.strictObject({
      host_id: Identifier,
      resources: Resources,
      http: Listener,
      https: Listener,
      endpoint: PublicEndpoint,
      ca_ref: SecretReference,
      certificate_ref: SecretReference,
      private_key_ref: SecretReference,
      origin_auth_ref: SecretReference,
    }),
  }),
  lambda_postgres_clients: z.strictObject({
    cognito_post_confirmation: LambdaPool,
    shopify: LambdaPool,
    stripe: LambdaPool,
    fxrate: LambdaPool,
  }),
});

type Host = z.infer<typeof Host>;
type StageInventory = z.infer<typeof StageInventory>;
type Service = z.infer<typeof Service>;
type Endpoint = z.infer<typeof PrivateEndpoint> | z.infer<typeof PublicEndpoint>;
type Reference = z.infer<typeof SecretReference>;
type DatabaseIdentity = z.infer<typeof DatabaseIdentity>;
type Resources = z.infer<typeof Resources>;
type Listener = z.infer<typeof Listener>;
type IssueContext = z.RefinementCtx;

function reject(ctx: IssueContext): void {
  ctx.addIssue({ code: 'custom', message: 'Invalid deployment contract' });
}

function unique(values: readonly string[]): boolean {
  return new Set(values).size === values.length;
}

function reservedHostname(host: string): boolean {
  return isIP(host) !== 0 || !host.includes('.') || /^[0-9.]+$/.test(host)
    || /(?:^|\.)(?:localhost|local|test|invalid|example)$/.test(host)
    || /(?:^|\.)example\.(?:com|net|org)$/.test(host);
}

function reservedAccount(account: string): boolean {
  return /^(\d)\1{11}$/.test(account) || account === '123456789012' || account === '012345678901';
}

function sameReference(left: Reference, right: Reference): boolean {
  return left.provider === right.provider && left.id === right.id && left.revision === right.revision;
}

// Minimums mirror docs/deployment/inventory.md and infra/src/worker-queue-config.ts.
// They are safety floors, not runtime defaults; every configured budget is required.
const WorkerBudgets = {
  'product-listing-opensearch': { execution: 45, visibility: 60, search: true, vertex: false },
  'search-filter-projection': { execution: 45, visibility: 60, search: true, vertex: false },
  'search-filter-percolator': { execution: 240, visibility: 300, search: true, vertex: true },
  'search-filter-match-notification': { execution: 45, visibility: 60, search: false, vertex: false },
  'watchlist-notification': { execution: 45, visibility: 60, search: false, vertex: false },
  'product-content-assessment': { execution: 45, visibility: 60, search: false, vertex: false },
  'product-embedding': { execution: 240, visibility: 300, search: false, vertex: true },
  'product-translation': { execution: 240, visibility: 300, search: false, vertex: true },
  'product-listing-normalization': { execution: 240, visibility: 300, search: false, vertex: false },
  'notification-delivery': { execution: 240, visibility: 360, search: false, vertex: false },
} satisfies Record<WorkerScope, { execution: number; visibility: number; search: boolean; vertex: boolean }>;

function validateEndpoint(
  endpoint: Endpoint, host: Host, hostId: string, listener: Listener, ca: Reference,
  real: boolean, singleHost: boolean, ctx: IssueContext,
): void {
  const isPublic = endpoint.routing.method === 'PUBLIC_INTERNET';
  const local = endpoint.routing.method === 'LOCAL_TEST';
  const expectedHost = isPublic ? host.dns.public : host.dns.private;
  if (endpoint.routing.target_host_id !== hostId || endpoint.port !== listener.port
    || (endpoint.host !== expectedHost && !(local && singleHost && endpoint.host === 'localhost'))
    || (local && (real || !singleHost))
    || (!singleHost && (!endpoint.host.includes('.') || endpoint.host === 'localhost'))
    || (real && reservedHostname(endpoint.host))) reject(ctx);
  if ((listener.bind === 'LOOPBACK' && !local)
    || (isPublic && listener.bind === 'PRIVATE')
    || (!isPublic && !local && listener.bind === 'PUBLIC')) reject(ctx);
  if (endpoint.tls.mode === 'VERIFY_FULL') {
    if (endpoint.tls.server_name !== endpoint.host || !sameReference(endpoint.tls.ca_ref, ca)
      || (real && reservedHostname(endpoint.tls.server_name))) reject(ctx);
  } else if (real) reject(ctx);
}

function validateDatabaseIdentities(identities: DatabaseIdentity[], ctx: IssueContext): void {
  // Revisions cannot disguise reuse of a principal or a protected logical identity.
  if (!unique(identities.map(identity => identity.principal))
    || !unique(identities.map(identity => identity.secret_ref.id))) reject(ctx);
}

export const Inventory = z.strictObject({
  schema_version: SchemaVersion,
  example: z.boolean(),
  hosts: z.array(Host).min(1).max(100),
  stages: z.array(StageInventory).min(1).max(Stage.options.length),
}).superRefine((inventory, ctx) => {
  const hosts = new Map(inventory.hosts.map(host => [host.id, host]));
  if (hosts.size !== inventory.hosts.length
    || !unique(inventory.stages.map(stage => stage.stage))
    || !unique(inventory.hosts.map(host => host.ssm_node_identity.managed_node_id))) reject(ctx);
  const definedStages = new Set(inventory.stages.map(stage => stage.stage));
  const dnsOwners = new Map<string, string>();
  const usedHosts = new Map<string, Set<Stage>>();
  const totals = new Map<string, { cpu_millicores: bigint; memory_mib: bigint; disk_mib: bigint }>();
  const ports = new Set<string>();
  const historyIds: string[] = [];
  const protectedIdentities: string[] = [];
  const nativeIdentityIds: string[] = [];
  const ssmIdentityIds = new Set(inventory.hosts.map(host => host.ssm_node_identity.registration_ref.id));
  for (const host of inventory.hosts) {
    if (!unique(host.owned_stages) || host.owned_stages.some(stage => !definedStages.has(stage))
      || (host.owned_stages.length > 1 && !host.shared_hardware_acknowledged)) reject(ctx);
    const real = host.owned_stages.some(isRealStage);
    for (const dns of Object.values(host.dns)) {
      const owner = dnsOwners.get(dns);
      if ((owner !== undefined && owner !== host.id) || (real && reservedHostname(dns))) reject(ctx);
      dnsOwners.set(dns, host.id);
    }
    totals.set(host.id, {
      cpu_millicores: BigInt(host.reserve.cpu_millicores),
      memory_mib: BigInt(host.reserve.memory_mib),
      disk_mib: BigInt(host.reserve.disk_mib),
    });
  }
  function allocate(hostId: string, stage: Stage, resources: Resources, listeners: Listener[], copies = 1): Host | undefined {
    const host = hosts.get(hostId);
    const total = totals.get(hostId);
    if (!host || !total) { reject(ctx); return undefined; }
    if (!host.owned_stages.includes(stage)) reject(ctx);
    const uses = usedHosts.get(hostId) ?? new Set<Stage>();
    uses.add(stage);
    usedHosts.set(hostId, uses);
    for (const key of ['cpu_millicores', 'memory_mib', 'disk_mib'] as const) {
      total[key] += BigInt(resources[key]) * BigInt(copies);
    }
    // Conservative reservation also catches IPv6 dual-stack and wildcard aliases.
    for (const listener of listeners) {
      const key = `${hostId}:${listener.port}`;
      if (ports.has(key)) reject(ctx);
      ports.add(key);
    }
    return host;
  }
  for (const stage of inventory.stages) {
    const real = isRealStage(stage.stage);
    if (real && (inventory.example || reservedAccount(stage.aws.account_id))) reject(ctx);
    const roles = stage.roles;
    const allPlacements = [roles.api, ...roles.workers, roles.cron, roles.crawler,
      roles.postgres_business, roles.postgres_crawler, roles.opensearch, roles.sequin, roles.edge];
    const singleHost = new Set(allPlacements.map(role => role.host_id)).size === 1;
    const services: Service[] = [...roles.workers, roles.cron, roles.crawler,
      roles.postgres_business, roles.postgres_crawler, roles.opensearch, roles.sequin];
    for (const service of services) {
      const host = allocate(service.host_id, stage.stage, service.resources, [service.listener]);
      if (host) validateEndpoint(service.endpoint, host, service.host_id, service.listener, service.ca_ref, real, singleHost, ctx);
    }
    const apiHost = allocate(roles.api.host_id, stage.stage, roles.api.resources_per_slot,
      Object.values(roles.api.slots).flatMap(slot => [slot.listener, slot.operations_listener]), 2);
    if (apiHost) {
      for (const slot of Object.values(roles.api.slots)) {
        validateEndpoint(slot.endpoint, apiHost, roles.api.host_id, slot.listener, roles.api.ca_ref, real, singleHost, ctx);
      }
    }
    const edgeHost = allocate(roles.edge.host_id, stage.stage, roles.edge.resources, [roles.edge.http, roles.edge.https]);
    if (edgeHost) validateEndpoint(roles.edge.endpoint, edgeHost, roles.edge.host_id, roles.edge.https, roles.edge.ca_ref, real, singleHost, ctx);
    const businessHost = hosts.get(roles.postgres_business.host_id);
    if (businessHost) validateEndpoint(roles.postgres_business.lambda_endpoint, businessHost,
      roles.postgres_business.host_id, roles.postgres_business.listener, roles.postgres_business.ca_ref, real, singleHost, ctx);
    if (roles.postgres_business.endpoint.host === roles.postgres_business.lambda_endpoint.host) reject(ctx);
    if (!unique(roles.workers.map(worker => worker.scope))) reject(ctx);
    for (const worker of roles.workers) {
      const needs = WorkerBudgets[worker.scope];
      if (worker.credentials.queue_scope !== worker.scope
        || Boolean(worker.credentials.opensearch_ref) !== needs.search
        || Boolean(worker.credentials.vertex) !== needs.vertex
        || Boolean(worker.credentials.email) !== (worker.scope === 'notification-delivery')) reject(ctx);
    }
    const applications = [roles.api, ...roles.workers, roles.cron, roles.crawler];
    for (const application of applications) {
      const aws = application.credentials.aws;
      if (aws.stage !== stage.stage) reject(ctx);
      for (const ref of [aws.application_identity_ref, aws.trust_anchor_ref, aws.profile_ref, aws.certificate_ref, aws.private_key_ref]) {
        if (ssmIdentityIds.has(ref.id)) reject(ctx);
      }
      nativeIdentityIds.push(aws.application_identity_ref.id, aws.certificate_ref.id, aws.private_key_ref.id);
    }
    const business = roles.postgres_business.database;
    const crawler = roles.postgres_crawler.database;
    historyIds.push(business.history_id, crawler.history_id);
    const nativePools = applications.map(application => application.credentials.business);
    const lambdas = Object.values(stage.lambda_postgres_clients);
    const businessIdentities = [...nativePools.map(pool => pool.identity), ...lambdas.map(pool => pool.identity),
      business.migrator.identity, business.backup.identity, business.sequin.identity];
    const crawlerIdentities = [roles.crawler.credentials.crawler.identity, crawler.migrator.identity, crawler.backup.identity];
    validateDatabaseIdentities(businessIdentities, ctx);
    validateDatabaseIdentities(crawlerIdentities, ctx);
    protectedIdentities.push(...[...businessIdentities, ...crawlerIdentities].map(identity => identity.secret_ref.id));
    // The API pool appears twice, and cron's advisory lock is outside its pool.
    const nativeConnections = nativePools.reduce((sum, pool) => sum + BigInt(pool.pool_max), 0n)
      + BigInt(roles.api.credentials.business.pool_max) + 1n;
    const lambdaConnections = lambdas.reduce((sum, client) =>
      sum + BigInt(client.reserved_concurrency) * BigInt(client.pool_max), 0n);
    const businessConnections = nativeConnections + lambdaConnections + BigInt(business.migrator.sessions)
      + BigInt(business.backup.sessions) + BigInt(business.sequin.sessions) + BigInt(business.reserve_sessions);
    const crawlerConnections = BigInt(roles.crawler.credentials.crawler.pool_max)
      + BigInt(crawler.migrator.sessions) + BigInt(crawler.backup.sessions) + BigInt(crawler.reserve_sessions);
    if (businessConnections > BigInt(business.max_connections) || crawlerConnections > BigInt(crawler.max_connections)) reject(ctx);
    const searchCredentials = [roles.api.credentials.opensearch_ref, roles.cron.credentials.opensearch_ref,
      ...roles.workers.flatMap(worker => worker.credentials.opensearch_ref ? [worker.credentials.opensearch_ref] : [])];
    if (!unique([roles.opensearch.admin_identity_ref.id, ...searchCredentials.map(ref => ref.id)])) reject(ctx);
  }
  if (!unique(historyIds) || !unique(protectedIdentities) || !unique(nativeIdentityIds)) reject(ctx);
  for (const host of inventory.hosts) {
    const uses = usedHosts.get(host.id);
    const total = totals.get(host.id);
    if (!uses || uses.size !== host.owned_stages.length) reject(ctx);
    if (total) for (const key of ['cpu_millicores', 'memory_mib', 'disk_mib'] as const) {
      if (total[key] > BigInt(host.capacity[key])) reject(ctx);
    }
  }
}).describe('Version 1 placement and protected references only. Semantic topology, identity and budget checks require parseInventory; JSON Schema alone is insufficient.');
export type Inventory = z.infer<typeof Inventory>;

const Probe = z.strictObject({
  kind: z.literal('HEALTH_AND_READINESS'),
  timeout_seconds: Positive.max(30),
  interval_seconds: Positive.max(60),
  successes_required: Positive.max(10),
  deadline_seconds: Positive.max(900),
}).superRefine((probe, ctx) => {
  if (probe.timeout_seconds * probe.successes_required
    + probe.interval_seconds * (probe.successes_required - 1) > probe.deadline_seconds) reject(ctx);
});
const Lifecycle = z.strictObject({ probe: Probe, drain_seconds: Seconds, stop_seconds: Seconds });
const Queue = z.strictObject({
  url: z.string().url().max(2048),
  arn: z.string().regex(/^arn:aws(?:-us-gov|-cn)?:sqs:[a-z0-9-]+:[0-9]{12}:[a-z0-9-]{1,80}$/),
});
const RuntimeWorker = Lifecycle.extend({
  drain_seconds: Seconds.max(3600),
  stop_seconds: Seconds.max(3600),
  scope: WorkerScope,
  execution_seconds: Seconds,
  visibility_seconds: Positive.max(43200),
  queues: z.strictObject({ source: Queue, dlq: Queue }),
});

function reservedHash(value: string): boolean {
  return /^(?:sha256:)?([a-f0-9])\1+$/.test(value)
    || /^(?:sha256:)?(?:0123456789abcdef)+/.test(value);
}

export const RuntimeConfiguration = z.strictObject({
  schema_version: SchemaVersion,
  example: z.boolean(),
  // A closed snapshot avoids unvalidated inventory overlays and endpoint overrides.
  inventory: Inventory,
  stage: Stage,
  release: z.strictObject({
    source_sha: Sha,
    manifest_digest: Digest,
    images: z.strictObject({ api: Digest, worker: Digest, cron: Digest, crawler: Digest }),
  }),
  api: Lifecycle,
  workers: z.array(RuntimeWorker).length(WorkerScope.options.length),
  cron: Lifecycle.extend({ ownership_plan: z.literal('READ_ONLY_TARGET'), execution_seconds: Seconds }),
  crawler: Lifecycle.extend({ ownership_plan: z.literal('READ_ONLY_TARGET') }),
  email_assets: z.strictObject({
    bucket_ref: Identifier,
    prefix: z.string().regex(new RegExp(`^(?:${Stage.options.join('|')})/[0-9a-f]{40}/mjml/$`)),
  }),
  preflight: z.strictObject({ mode: z.literal('READ_ONLY'), queue_read: z.literal('GET_QUEUE_ATTRIBUTES') }),
  test_queue_endpoint: z.strictObject({
    host: z.literal('localhost'), port: PortNumber, transport: z.literal('HTTP'),
  }).optional(),
}).superRefine((runtime, ctx) => {
  const stage = runtime.inventory.stages.find(candidate => candidate.stage === runtime.stage);
  if (!stage) { reject(ctx); return; }
  const real = isRealStage(runtime.stage);
  if (real && (runtime.example || runtime.test_queue_endpoint
    || Object.values(runtime.release.images).some(reservedHash)
    || reservedHash(runtime.release.source_sha) || reservedHash(runtime.release.manifest_digest))) reject(ctx);
  if (runtime.api.drain_seconds < 45 || runtime.api.stop_seconds < 60
    || runtime.api.stop_seconds < runtime.api.drain_seconds + 15) reject(ctx);
  if (runtime.cron.drain_seconds < 300 || runtime.cron.stop_seconds < runtime.cron.drain_seconds + 30
    || runtime.crawler.stop_seconds < runtime.crawler.drain_seconds + 30) reject(ctx);
  if (!unique(runtime.workers.map(worker => worker.scope))) reject(ctx);
  const { account_id: account, region } = stage.aws;
  const partition = region.startsWith('cn-') ? 'aws-cn' : region.startsWith('us-gov-') ? 'aws-us-gov' : 'aws';
  const suffix = partition === 'aws-cn' ? 'amazonaws.com.cn' : 'amazonaws.com';
  const queueOrigin = runtime.test_queue_endpoint
    ? `http://localhost:${runtime.test_queue_endpoint.port}` : `https://sqs.${region}.${suffix}`;
  for (const worker of runtime.workers) {
    const floor = WorkerBudgets[worker.scope];
    if (worker.execution_seconds < floor.execution || worker.drain_seconds < 270
      || worker.drain_seconds < worker.execution_seconds || worker.stop_seconds < 300
      || worker.stop_seconds < worker.drain_seconds + 30
      || worker.visibility_seconds < floor.visibility
      || worker.visibility_seconds < worker.execution_seconds + 15) reject(ctx);
    for (const kind of ['source', 'dlq'] as const) {
      // Keep exact Standard names aligned with infra/src/worker-queue-config.ts.
      const name = `aura-worker-${worker.scope}${kind === 'dlq' ? '-dlq' : ''}-${runtime.stage}`;
      const queue = worker.queues[kind];
      if (queue.url !== `${queueOrigin}/${account}/${name}`
        || queue.arn !== `arn:${partition}:sqs:${region}:${account}:${name}`) reject(ctx);
    }
  }
  if (runtime.email_assets.prefix !== `${runtime.stage}/${runtime.release.source_sha}/mjml/`) reject(ctx);
}).describe('Version 1 read-only runtime target snapshot, not deployment approval. Semantic queue, TLS, identity and deadline checks require parseRuntimeConfiguration; JSON Schema alone is insufficient.');
export type RuntimeConfiguration = z.infer<typeof RuntimeConfiguration>;

function parse<T>(schema: z.ZodType<T>, input: unknown, message: string): T {
  try {
    const result = schema.safeParse(input);
    if (result.success) return result.data;
  } catch {
    // No input, getter exception, Zod issue, URL or resolved secret crosses this boundary.
  }
  throw new Error(message);
}

export function parseInventory(input: unknown): Inventory {
  return parse(Inventory, input, 'Invalid inventory');
}

export function parseRuntimeConfiguration(input: unknown): RuntimeConfiguration {
  return parse(RuntimeConfiguration, input, 'Invalid runtime configuration');
}
