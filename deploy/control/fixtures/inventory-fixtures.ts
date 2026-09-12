import { createHash } from 'node:crypto';
import type { Inventory, RuntimeConfiguration } from '../src/contracts/inventory.js';
import { WorkerScope, isRealStage, type Stage } from '../src/contracts/primitives.js';

// Offline syntax fixtures only. These names, node IDs and protected IDs are synthetic;
// no reference resolves and none is an operator inventory or evidence of readiness.
const reference = (id: string) => ({ provider: 'PROTECTED_FILE' as const, id, revision: 'fixture-v1' });
const identity = (id: string) => ({ principal: id, secret_ref: reference(`${id}-secret`) });
const resources = () => ({ cpu_millicores: 250, memory_mib: 512, disk_mib: 1024 });
const probe = () => ({
  kind: 'HEALTH_AND_READINESS' as const, timeout_seconds: 5, interval_seconds: 2,
  successes_required: 2, deadline_seconds: 30,
});
const digest = (label: string) => `sha256:${createHash('sha256').update(label).digest('hex')}`;

export function inventoryFixture(
  stages: Stage[] = ['test'], layout: 'ALL_IN_ONE' | 'SPLIT_HOST' = 'ALL_IN_ONE',
): Inventory {
  const hosts: Inventory['hosts'] = [];
  const stageInventories: Inventory['stages'] = [];
  for (const stage of stages) {
    const hostKinds = layout === 'ALL_IN_ONE' ? ['one'] : ['app', 'data', 'crawl'];
    for (const kind of hostKinds) {
      const id = `${stage}-${kind}`;
      const domain = isRealStage(stage) ? 'validation-only.unit' : 'example.test';
      hosts.push({
        id, architecture: 'x86_64', operating_system: 'UBUNTU_24_04',
        owned_stages: [stage], shared_hardware_acknowledged: false,
        capacity: { cpu_millicores: 64000, memory_mib: 131072, disk_mib: 1048576 },
        reserve: { cpu_millicores: 1000, memory_mib: 2048, disk_mib: 16384 },
        dns: { management: `management-${id}.${domain}`, public: `public-${id}.${domain}`, private: `private-${id}.${domain}` },
        ssm_node_identity: {
          managed_node_id: `mi-${(hosts.length + 1).toString(16).padStart(17, '0')}`,
          registration_ref: reference(`${id}-ssm-registration`),
        },
      });
    }
    function hostFor(kind: 'app' | 'data' | 'crawl') {
      const id = `${stage}-${layout === 'ALL_IN_ONE' ? 'one' : kind}`;
      const host = hosts.find(candidate => candidate.id === id);
      if (!host) throw new Error('Missing fixture host');
      return host;
    }
    function service(key: string, port: number, kind: 'app' | 'data' | 'crawl' = 'app') {
      const host = hostFor(kind);
      const ca = reference(`${stage}-${key}-ca`);
      return {
        host_id: host.id, resources: resources(),
        listener: { port, bind: 'ALL_INTERFACES' as const },
        endpoint: {
          host: host.dns.private, port,
          tls: { mode: 'VERIFY_FULL' as const, server_name: host.dns.private, ca_ref: ca },
          routing: { method: 'PRIVATE_NETWORK' as const, target_host_id: host.id },
        },
        ca_ref: ca,
      };
    }
    function aws(key: string) {
      const id = `${stage}-${key}-aws`;
      return {
        method: 'ROLES_ANYWHERE' as const, stage,
        application_identity_ref: reference(`${id}-role`),
        trust_anchor_ref: reference(`${stage}-trust-anchor`),
        profile_ref: reference(`${id}-profile`),
        certificate_ref: reference(`${id}-certificate`),
        private_key_ref: reference(`${id}-key`),
      };
    }
    const vertex = (key: string) => ({ method: 'MOUNTED_ADC' as const, configuration_ref: reference(`${stage}-${key}-adc`) });
    const pool = (key: string, pool_max = 2) => ({ identity: identity(`${stage}-${key}`), pool_max });
    const apiService = service('api', 8080);
    const green = service('api', 8084);
    const workers = WorkerScope.options.map((scope, index) => {
      const search = ['product-listing-opensearch', 'search-filter-projection', 'search-filter-percolator'].includes(scope);
      const model = ['search-filter-percolator', 'product-embedding', 'product-translation'].includes(scope);
      const key = `worker-${scope}`;
      return {
        ...service(key, 8100 + index), scope,
        credentials: {
          business: pool(`${key}-business`), aws: aws(key), queue_scope: scope,
          ...(search ? { opensearch_ref: reference(`${stage}-${key}-search`) } : {}),
          ...(model ? { vertex: vertex(key) } : {}),
          ...(scope === 'notification-delivery' ? {
            email: { s3_read_ref: reference(`${stage}-mail-read`), ses_send_ref: reference(`${stage}-mail-send`) },
          } : {}),
        },
      };
    });
    const businessService = service('postgres-business', 5432, 'data');
    const dataHost = hostFor('data');
    const edgeHost = hostFor('app');
    const edgeCa = reference(`${stage}-edge-ca`);
    const lambdaPool = (key: string) => ({ ...pool(`lambda-${key}`), reserved_concurrency: 2 });
    stageInventories.push({
      stage, aws: { account_id: '100200300400', region: 'eu-central-1' },
      roles: {
        api: {
          host_id: apiService.host_id, resources_per_slot: resources(), ca_ref: apiService.ca_ref,
          slots: {
            blue: { listener: apiService.listener, endpoint: apiService.endpoint, operations_listener: { port: 9080, bind: 'LOOPBACK' } },
            green: { listener: green.listener, endpoint: green.endpoint, operations_listener: { port: 9084, bind: 'LOOPBACK' } },
          },
          credentials: {
            business: pool('api-business'), aws: aws('api'), vertex: vertex('api'),
            opensearch_ref: reference(`${stage}-api-search`), cognito_ref: reference(`${stage}-cognito`),
            stripe_ref: reference(`${stage}-stripe`), zoho_ref: reference(`${stage}-zoho`),
          },
        },
        workers,
        cron: {
          host_id: hostFor('app').id, resources: resources(),
          operations_listener: { port: 8082, bind: 'LOOPBACK' }, ownership_plan: 'READ_ONLY_TARGET',
          credentials: { business: pool('cron-business'), aws: aws('cron'), vertex: vertex('cron'), opensearch_ref: reference(`${stage}-cron-search`) },
        },
        crawler: {
          ...service('crawler', 7878, 'crawl'), ownership_plan: 'READ_ONLY_TARGET',
          credentials: {
            business: pool('crawler-business', 8), crawler: pool('crawler-runtime', 16),
            aws: aws('crawler'), vertex: vertex('crawler'), review_auth_ref: reference(`${stage}-crawler-review`),
          },
        },
        postgres_business: {
          ...businessService,
          lambda_endpoint: {
            host: dataHost.dns.public, port: 5432,
            routing: { method: 'PUBLIC_INTERNET', target_host_id: dataHost.id },
            tls: { mode: 'VERIFY_FULL', server_name: dataHost.dns.public, ca_ref: businessService.ca_ref },
          },
          database: {
            target: 'business', name: `${stage}-business`, history_id: `${stage}-business-history`, max_connections: 68,
            migrator: { identity: identity(`${stage}-business-migrator`), sessions: 2 },
            backup: { identity: identity(`${stage}-business-backup`), sessions: 1 },
            sequin: { identity: identity(`${stage}-business-replication`), sessions: 4 }, reserve_sessions: 10,
          },
        },
        postgres_crawler: {
          ...service('postgres-crawler', 5433, 'data'),
          database: {
            target: 'crawler', name: `${stage}-crawler`, history_id: `${stage}-crawler-history`, max_connections: 24,
            migrator: { identity: identity(`${stage}-crawler-migrator`), sessions: 2 },
            backup: { identity: identity(`${stage}-crawler-backup`), sessions: 1 }, reserve_sessions: 5,
          },
        },
        opensearch: { ...service('opensearch', 9200, 'data'), admin_identity_ref: reference(`${stage}-search-admin`) },
        sequin: { ...service('sequin', 7376, 'data'), admin_identity_ref: reference(`${stage}-sequin-admin`) },
        edge: {
          host_id: edgeHost.id, resources: resources(),
          http: { port: 80, bind: 'PUBLIC' }, https: { port: 443, bind: 'PUBLIC' },
          endpoint: {
            host: edgeHost.dns.public, port: 443, routing: { method: 'PUBLIC_INTERNET', target_host_id: edgeHost.id },
            tls: { mode: 'VERIFY_FULL', server_name: edgeHost.dns.public, ca_ref: edgeCa },
          },
          ca_ref: edgeCa, certificate_ref: reference(`${stage}-edge-certificate`),
          private_key_ref: reference(`${stage}-edge-key`), origin_auth_ref: reference(`${stage}-origin-auth`),
        },
      },
      lambda_postgres_clients: {
        cognito_post_confirmation: lambdaPool('cognito'), shopify: lambdaPool('shopify'),
        stripe: lambdaPool('stripe'), fxrate: lambdaPool('fxrate'),
      },
    });
  }
  return { schema_version: 1, example: !stages.some(isRealStage), hosts, stages: stageInventories };
}

export function runtimeFixture(stage: Stage = 'test', inventory = inventoryFixture([stage])): RuntimeConfiguration {
  const source_sha = createHash('sha1').update('offline inventory contract release').digest('hex');
  const selected = inventory.stages.find(candidate => candidate.stage === stage);
  if (!selected) throw new Error('Missing fixture stage');
  const { region, account_id } = selected.aws;
  function queue(scope: WorkerScope, dlq: boolean) {
    const name = `aura-worker-${scope}${dlq ? '-dlq' : ''}-${stage}`;
    return {
      url: `https://sqs.${region}.amazonaws.com/${account_id}/${name}`,
      arn: `arn:aws:sqs:${region}:${account_id}:${name}`,
    };
  }
  return {
    schema_version: 1, example: !isRealStage(stage), inventory, stage,
    release: {
      source_sha, manifest_digest: digest('offline manifest'),
      images: { api: digest('offline api'), worker: digest('offline worker'), cron: digest('offline cron'), crawler: digest('offline crawler') },
    },
    api: { probe: probe(), drain_seconds: 45, stop_seconds: 60 },
    workers: WorkerScope.options.map(scope => {
      const slow = ['search-filter-percolator', 'product-embedding', 'product-translation', 'product-listing-normalization', 'notification-delivery'].includes(scope);
      return {
        scope, probe: probe(), drain_seconds: 270, stop_seconds: 300,
        execution_seconds: slow ? 240 : 45, visibility_seconds: scope === 'notification-delivery' ? 360 : slow ? 300 : 60,
        queues: { source: queue(scope, false), dlq: queue(scope, true) },
      };
    }),
    cron: { probe: probe(), drain_seconds: 300, stop_seconds: 330, execution_seconds: 7200, ownership_plan: 'READ_ONLY_TARGET' },
    crawler: { probe: probe(), drain_seconds: 60, stop_seconds: 90, ownership_plan: 'READ_ONLY_TARGET' },
    email_assets: { bucket_ref: `${stage}-mail-bucket`, prefix: `${stage}/${source_sha}/mjml/` },
    preflight: { mode: 'READ_ONLY', queue_read: 'GET_QUEUE_ATTRIBUTES' },
  };
}

export function insecureLocalFixture(stage: Stage = 'local'): RuntimeConfiguration {
  if (isRealStage(stage)) throw new Error('Fixture requires a nonreal stage');
  const runtime = runtimeFixture(stage);
  const roles = runtime.inventory.stages[0]!.roles;
  const privateEndpoints = [roles.api.slots.blue.endpoint, roles.api.slots.green.endpoint,
    ...roles.workers.map(worker => worker.endpoint), roles.crawler.endpoint,
    roles.postgres_business.endpoint, roles.postgres_crawler.endpoint, roles.opensearch.endpoint, roles.sequin.endpoint];
  for (const endpoint of privateEndpoints) {
    endpoint.host = 'localhost';
    endpoint.routing.method = 'LOCAL_TEST';
    endpoint.tls = { mode: 'DISABLED' };
  }
  roles.postgres_business.lambda_endpoint.tls = { mode: 'DISABLED' };
  roles.edge.endpoint.tls = { mode: 'DISABLED' };
  runtime.test_queue_endpoint = { host: 'localhost', port: 4566, transport: 'HTTP' };
  for (const worker of runtime.workers) for (const queue of Object.values(worker.queues)) {
    queue.url = queue.url.replace('https://sqs.eu-central-1.amazonaws.com', 'http://localhost:4566');
  }
  return runtime;
}
