import { isIP } from 'node:net';
import { z } from 'zod';
import { canonicalHash } from '../contracts/hash.js';
import { parseRuntimeConfiguration, type RuntimeConfiguration } from '../contracts/inventory.js';
import { Account, Digest, Identifier, Region, Stage, WorkerScope } from '../contracts/primitives.js';
import { ReleaseManifest } from '../contracts/release.js';

// Installed catalog binary constants, not caller templates. Tests detect catalog drift.
// The image packaging gate must install these executables in /usr/local/bin.
const Templates = {
  api: { component: 'aura-historia-api', binary: 'aura-historia-api' },
  worker: { component: 'aura-historia-worker', binary: 'aura-historia-worker' },
  cron: { component: 'aura-historia-cron', binary: 'aura-historia-cron' },
  crawler: { component: 'crawler', binary: 'server' },
} as const;

const Selection = z.discriminatedUnion('role', [
  z.strictObject({ role: z.literal('api'), slot: z.enum(['blue', 'green']) }),
  z.strictObject({ role: z.literal('worker'), scope: WorkerScope }),
  z.strictObject({ role: z.literal('cron') }),
  z.strictObject({ role: z.literal('crawler') }),
]);
export type ApplicationComposeSelection = z.infer<typeof Selection>;
const Identity = z.strictObject({
  selection: Selection,
  target: z.strictObject({ stage: Stage, account: Account, region: Region }),
  host_id: Identifier,
  configuration_sha256: Digest,
  manifest_sha256: Digest,
  native_image: ReleaseManifest.shape.native_images.element,
}).brand<'ApplicationComposeIdentity'>();
export type ApplicationComposeIdentity = z.infer<typeof Identity>;

export type ApplicationComposeBlocker =
  | 'INVALID_INPUT'
  | 'IDENTITY_MISMATCH'
  | 'PRIVATE_BIND_IP_UNAVAILABLE'
  | 'CONTAINER_ENDPOINT_UNSUPPORTED'
  | 'RUNTIME_BUDGET_UNSUPPORTED'
  | 'CRAWLER_TLS_POLICY_MISMATCH'
  | 'ADC_MATERIALIZATION_UNSUPPORTED';

export class ApplicationComposeError extends Error {
  constructor(readonly code: ApplicationComposeBlocker) {
    super(code);
    this.name = 'ApplicationComposeError';
  }
}

/** Shape validation only. Trusted approval/provenance adapters must supply this identity. */
export function parseApplicationComposeIdentity(input: unknown): ApplicationComposeIdentity {
  try {
    const identity = Identity.parse(input);
    cleanStrings(identity);
    return identity;
  } catch {
    throw new ApplicationComposeError('INVALID_INPUT');
  }
}

function requireMatch(condition: boolean, code: ApplicationComposeBlocker): asserts condition {
  if (!condition) throw new ApplicationComposeError(code);
}

function cleanStrings(value: unknown): void {
  // Reject control text at the rendering boundary, independent of leaf URL normalization.
  // Compose also interpolates dollar expressions in JSON strings, not just YAML.
  if (typeof value === 'string') {
    requireMatch(!/[\u0000-\u001f\u007f-\u009f\u2028\u2029$]/.test(value), 'INVALID_INPUT');
  } else if (value !== null && typeof value === 'object') {
    for (const child of Object.values(value)) cleanStrings(child);
  }
}

function containerEndpoint(endpoint: RuntimeConfiguration['inventory']['stages'][number]['roles']['postgres_business']['endpoint']): void {
  // LOCAL_TEST/localhost mean the host in existing fixtures, not another container.
  // Do not resolve DNS or replace it with localhost even for all-in-one placement.
  requireMatch(endpoint.routing.method !== 'LOCAL_TEST'
    && endpoint.host.includes('.') && !endpoint.host.endsWith('.localhost')
    && isIP(new URL(`http://${endpoint.host}`).hostname) === 0, 'CONTAINER_ENDPOINT_UNSUPPORTED');
}

/** Pure, one-project/one-service Compose JSON. No file/provider/Docker access or approval. */
export function renderApplicationCompose(
  runtime: RuntimeConfiguration,
  identity: ApplicationComposeIdentity,
): string {
  try {
    // Reparse to reject cast inputs, unknown overlays and post-validation mutation.
    return render(parseRuntimeConfiguration(runtime), parseApplicationComposeIdentity(identity));
  } catch (error) {
    if (error instanceof ApplicationComposeError) throw error;
    throw new ApplicationComposeError('INVALID_INPUT');
  }
}

function render(runtime: RuntimeConfiguration, identity: ApplicationComposeIdentity): string {
  cleanStrings(runtime);
  const { selection, native_image: native, target } = identity;
  const template = Templates[selection.role];
  const stage = runtime.inventory.stages.find(candidate => candidate.stage === runtime.stage);
  requireMatch(stage !== undefined, 'IDENTITY_MISMATCH');
  const { roles } = stage;
  const placement = selection.role === 'worker'
    ? roles.workers.find(worker => worker.scope === selection.scope)
    : roles[selection.role];
  requireMatch(placement !== undefined, 'IDENTITY_MISMATCH');
  const host = runtime.inventory.hosts.find(candidate => candidate.id === placement.host_id);
  requireMatch(host !== undefined, 'IDENTITY_MISMATCH');
  const suffix = stage.aws.region.startsWith('cn-') ? 'amazonaws.com.cn' : 'amazonaws.com';
  const registry = `${stage.aws.account_id}.dkr.ecr.${stage.aws.region}.${suffix}`;
  requireMatch(target.stage === runtime.stage && target.account === stage.aws.account_id
    && target.region === stage.aws.region && identity.host_id === host.id
    && identity.configuration_sha256 === canonicalHash(runtime)
    && identity.manifest_sha256 === runtime.release.manifest_digest
    && native.component === template.component && native.source_sha === runtime.release.source_sha
    && native.image.digest === runtime.release.images[selection.role]
    && native.image.registry === registry && native.architecture === host.architecture
    && native.rust_target.startsWith(host.architecture === 'arm64' ? 'aarch64-' : 'x86_64-'),
  'IDENTITY_MISMATCH');

  // Frozen Host.dns.private is a hostname, not an authenticated bind address.
  // PRIVATE is intent only; no DNS resolution, wildcard, host network or caller-IP escape.
  if (selection.role === 'api' || selection.role === 'worker') {
    throw new ApplicationComposeError('PRIVATE_BIND_IP_UNAVAILABLE');
  }

  if (selection.role === 'cron') {
    requireMatch(!/^([a-f0-9])\1+$/.test(runtime.release.source_sha)
      && !runtime.release.source_sha.startsWith('0123456789abcdef'), 'INVALID_INPUT');
  }
  const role = roles[selection.role];
  const lifecycle = runtime[selection.role];
  const resources = role.resources;
  requireMatch(resources.memory_mib >= 128 && resources.disk_mib >= 16
    && role.credentials.business.pool_max <= 4294967295, 'RUNTIME_BUDGET_UNSUPPORTED');
  requireMatch(role.credentials.vertex.method === 'MOUNTED_ADC', 'ADC_MATERIALIZATION_UNSUPPORTED');
  containerEndpoint(roles.postgres_business.endpoint);
  if (selection.role === 'crawler') {
    containerEndpoint(roles.postgres_crawler.endpoint);
    // Server pool caps are code-level, not environment inputs. Never pretend to set them.
    requireMatch(roles.crawler.credentials.business.pool_max === 8
      && roles.crawler.credentials.crawler.pool_max === 16, 'RUNTIME_BUDGET_UNSUPPORTED');
    requireMatch(roles.postgres_business.endpoint.tls.mode === roles.postgres_crawler.endpoint.tls.mode,
      'CRAWLER_TLS_POLICY_MISMATCH');
  } else {
    containerEndpoint(roles.opensearch.endpoint);
  }

  const service = selection.role;
  const root = `/etc/aura-historia/${runtime.stage}/${service}`;
  const protectedFile = (file: 'postgres-ca.pem' | 'google-adc.json') => ({
    type: 'bind' as const, source: `${root}/${file}`, target: `${root}/${file}`,
    read_only: true, bind: { create_host_path: false },
  });
  const tls = roles.postgres_business.endpoint.tls.mode;
  const environment: Record<string, string> = {
    STAGE: runtime.stage,
    AWS_REGION: stage.aws.region,
    AWS_EC2_METADATA_DISABLED: 'true',
    GOOGLE_APPLICATION_CREDENTIALS: `${root}/google-adc.json`,
    POSTGRES_SSL_MODE: tls === 'VERIFY_FULL' ? 'verify-full' : 'disable',
    ...(tls === 'VERIFY_FULL' ? { POSTGRES_SSL_ROOT_CERT: `${root}/postgres-ca.pem` } : {}),
    LOG_LEVEL: 'info',
  };
  if (selection.role === 'cron') {
    const database = roles.postgres_business;
    const search = roles.opensearch.endpoint;
    Object.assign(environment, {
      COMMIT_SHA: runtime.release.source_sha,
      AURA_HISTORIA_CRON_HEALTH_BIND_ADDR: `127.0.0.1:${roles.cron.operations_listener.port}`,
      AURA_HISTORIA_CRON_ENABLED_JOBS: 'search-filter-periodic-match',
      AURA_HISTORIA_CRON_SHUTDOWN_GRACE_SECONDS: String(lifecycle.drain_seconds),
      AURA_HISTORIA_CRON_STOP_TIMEOUT_SECONDS: String(lifecycle.stop_seconds),
      PERIODIC_MATCH_MAX_RUN_SECONDS: String(runtime.cron.execution_seconds),
      SEARCH_FILTER_PERIODIC_MATCH_CRON: '0 0 15 * * * *',
      POSTGRES_HOST: database.endpoint.host,
      POSTGRES_PORT: String(database.endpoint.port),
      POSTGRES_DATABASE: database.database.name,
      POSTGRES_USERNAME: roles.cron.credentials.business.identity.principal,
      POSTGRES_MAX_CONNECTIONS: String(roles.cron.credentials.business.pool_max),
      OPENSEARCH_ENDPOINT_URL: `${search.tls.mode === 'VERIFY_FULL' ? 'https' : 'http'}://${search.host}:${search.port}`,
    });
  } else {
    Object.assign(environment, {
      // Crawler COMMIT_SHA is compiled into the image, not read from environment.
      CRAWLER_OPERATIONS_BIND_ADDR: `127.0.0.1:${roles.crawler.operations_listener.port}`,
      CRAWLER_REVIEW_BIND_ADDR: `127.0.0.1:${roles.crawler.review_listener.port}`,
      CRAWLER_SHUTDOWN_GRACE_SECONDS: String(lifecycle.drain_seconds),
      CRAWLER_STOP_TIMEOUT_SECONDS: String(lifecycle.stop_seconds),
      CRAWLER_STARTUP_TIMEOUT_SECONDS: String(runtime.crawler.startup_seconds),
      SPIDER_MAX_SIZE_BYTES: '8388608',
      CRAWLER_REVIEW_REQUIRED: 'true',
      CRAWLER_REVIEW_URL_PATTERN_REQUIRED: 'true',
    });
  }

  return `${JSON.stringify({
    name: `aura-${runtime.stage}-${service}`,
    services: {
      [service]: {
        image: `${native.image.registry}/${native.image.repository}@${native.image.digest}`,
        pull_policy: 'never',
        entrypoint: [`/usr/local/bin/${template.binary}`],
        command: [],
        user: '10001:10001',
        init: true,
        restart: 'no',
        stop_signal: 'SIGTERM',
        stop_grace_period: `${lifecycle.stop_seconds}s`,
        read_only: true,
        cap_drop: ['ALL'],
        security_opt: ['no-new-privileges:true'],
        pids_limit: 256,
        cpus: `${Math.floor(resources.cpu_millicores / 1000)}.${String(resources.cpu_millicores % 1000).padStart(3, '0')}`,
        mem_limit: `${resources.memory_mib}m`,
        memswap_limit: `${resources.memory_mib}m`,
        shm_size: '1m',
        tmpfs: [`/tmp:rw,noexec,nosuid,nodev,size=${Math.min(64, Math.floor(resources.memory_mib / 8))}m,mode=1777`],
        ulimits: { core: { soft: 0, hard: 0 }, nofile: { soft: 1024, hard: 1024 } },
        logging: { driver: 'local', options: { 'max-size': '4m', 'max-file': '3', compress: 'false' } },
        // Disable any inherited image probe; no installed native probe executable exists.
        healthcheck: { disable: true },
        env_file: [{ path: `${root}/runtime.env`, required: true, format: 'raw' }],
        environment,
        volumes: [protectedFile('google-adc.json'), ...(tls === 'VERIFY_FULL' ? [protectedFile('postgres-ca.pem')] : [])],
        networks: ['application'],
        labels: {
          'io.aura.configuration-sha256': identity.configuration_sha256,
          'io.aura.manifest-sha256': identity.manifest_sha256,
          'io.aura.host-id': host.id,
        },
      },
    },
    networks: { application: { driver: 'bridge' } },
  }, null, 2)}\n`;
}
