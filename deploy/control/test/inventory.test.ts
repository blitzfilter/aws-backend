import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { isDeepStrictEqual } from 'node:util';
import { inventoryFixture, insecureLocalFixture, runtimeFixture } from '../fixtures/inventory-fixtures.js';
import {
  Inventory, RuntimeConfiguration, parseInventory, parseRuntimeConfiguration,
} from '../src/contracts/inventory.js';
import { inventoryJsonSchemas } from '../src/contracts/export-inventory-schemas.js';
import { WorkerScope } from '../src/contracts/primitives.js';

type Path = (string | number)[];
function changed(input: unknown, path: Path, value: unknown, remove = false): unknown {
  // Detach reference aliases too, so mutating a client's CA does not mutate the server's CA.
  const copy: unknown = JSON.parse(JSON.stringify(input));
  let parent = copy as Record<string | number, unknown>;
  for (const key of path.slice(0, -1)) parent = parent[key] as Record<string | number, unknown>;
  const key = path.at(-1)!;
  if (remove) delete parent[key];
  else parent[key] = value;
  return copy;
}
function invalidInventory(input: unknown): void {
  assert.equal(Inventory.safeParse(input).success, false);
  assert.throws(() => parseInventory(input), { name: 'Error', message: 'Invalid inventory' });
}
function invalidRuntime(input: unknown): void {
  assert.equal(RuntimeConfiguration.safeParse(input).success, false);
  assert.throws(() => parseRuntimeConfiguration(input), { name: 'Error', message: 'Invalid runtime configuration' });
}
const rolePath = (role: string, ...rest: Path): Path => ['stages', 0, 'roles', role, ...rest];
const nativePg = rolePath('postgres_business', 'endpoint');
const publicPg = rolePath('postgres_business', 'lambda_endpoint');
const database = rolePath('postgres_business', 'database');

function sharedHostFixture(): Inventory {
  const inventory = inventoryFixture(['dev', 'prod']);
  const host = inventory.hosts[0]!;
  const removed = inventory.hosts[1]!;
  function relocate(value: unknown): void {
    if (typeof value !== 'object' || value === null) return;
    for (const [key, child] of Object.entries(value)) {
      const record = value as Record<string, unknown>;
      if ((key === 'host_id' || key === 'target_host_id') && child === removed.id) record[key] = host.id;
      else if (key === 'port' && typeof child === 'number') record[key] = child + 20000;
      else if ((key === 'host' || key === 'server_name') && child === removed.dns.private) record[key] = host.dns.private;
      else if ((key === 'host' || key === 'server_name') && child === removed.dns.public) record[key] = host.dns.public;
      else relocate(child);
    }
  }
  // JSON copy avoids visiting shared fixture reference objects twice.
  inventory.stages[1] = JSON.parse(JSON.stringify(inventory.stages[1])) as Inventory['stages'][number];
  relocate(inventory.stages[1]);
  host.owned_stages = ['dev', 'prod'];
  host.shared_hardware_acknowledged = true;
  inventory.hosts = [host];
  return inventory;
}

for (const layout of ['ALL_IN_ONE', 'SPLIT_HOST'] as const) {
  test(`accepts separate dev/prod placement: ${layout}`, () => {
    const inventory = inventoryFixture(['dev', 'prod'], layout);
    assert.doesNotThrow(() => parseInventory(inventory));
    for (const stage of ['dev', 'prod'] as const) {
      assert.doesNotThrow(() => parseRuntimeConfiguration(runtimeFixture(stage, inventory)));
    }
    assert.equal(inventory.hosts.length, layout === 'ALL_IN_ONE' ? 2 : 6);
    assert.deepEqual(inventory.stages[0]!.roles.workers.map(worker => worker.scope), WorkerScope.options);
  });
}
for (const stage of ['local', 'ephemeral', 'test'] as const) {
  test(`explicit nonreal stage permits intentionally insecure fixtures: ${stage}`, () => {
    assert.doesNotThrow(() => parseRuntimeConfiguration(insecureLocalFixture(stage)));
  });
}
test('shared hardware requires explicit ownership and aggregates both stages', () => {
  const inventory = sharedHostFixture();
  assert.doesNotThrow(() => parseInventory(inventory));
  invalidInventory(changed(inventory, ['hosts', 0, 'shared_hardware_acknowledged'], false));
  invalidInventory(changed(inventory, ['hosts', 0, 'owned_stages'], ['dev']));
  // Fits either stage alone, but not both stages together.
  invalidInventory(changed(inventory, ['hosts', 0, 'capacity', 'cpu_millicores'], 6000));
  invalidInventory(changed(inventory, ['hosts', 0, 'capacity', 'memory_mib'], 12000));
  invalidInventory(changed(inventory, ['hosts', 0, 'capacity', 'disk_mib'], 36000));
});

test('schemas and safe parsers preserve data without injecting defaults or mutating input', () => {
  const inventory = inventoryFixture();
  const runtime = runtimeFixture('test', inventory);
  const before = JSON.stringify(runtime);
  assert.equal(isDeepStrictEqual(parseRuntimeConfiguration(runtime), runtime), true);
  assert.equal(JSON.stringify(runtime) === before, true);
  assert.equal(Inventory.safeParse(inventory).success, true);
  assert.equal(RuntimeConfiguration.safeParse(runtime).success, true);
});

const missingInventory: Path[] = [
  ['schema_version'], ['example'], ['hosts'], ['stages'], ['hosts', 0, 'id'], ['hosts', 0, 'architecture'],
  ['hosts', 0, 'operating_system'], ['hosts', 0, 'owned_stages'], ['hosts', 0, 'shared_hardware_acknowledged'],
  ['hosts', 0, 'capacity'], ['hosts', 0, 'reserve'], ['hosts', 0, 'dns', 'management'],
  ['hosts', 0, 'dns', 'public'], ['hosts', 0, 'dns', 'private'], ['hosts', 0, 'ssm_node_identity'],
  ['stages', 0, 'aws', 'account_id'], ['stages', 0, 'aws', 'region'],
  rolePath('api'), rolePath('workers'), rolePath('cron'), rolePath('crawler'),
  rolePath('postgres_business'), rolePath('postgres_crawler'), rolePath('opensearch'), rolePath('sequin'), rolePath('edge'),
  rolePath('api', 'slots', 'green'), rolePath('api', 'resources_per_slot'),
  rolePath('api', 'credentials', 'business', 'pool_max'), rolePath('api', 'credentials', 'aws', 'profile_ref'),
  rolePath('crawler', 'credentials', 'crawler'), [...database, 'migrator'], [...database, 'backup'],
  [...database, 'sequin'], [...database, 'reserve_sessions'], [...database, 'history_id'],
  publicPg, [...nativePg, 'routing'], [...nativePg, 'tls', 'ca_ref'], [...publicPg, 'tls', 'ca_ref'],
  ['stages', 0, 'lambda_postgres_clients', 'shopify', 'reserved_concurrency'],
  ['stages', 0, 'lambda_postgres_clients', 'stripe', 'pool_max'],
];
for (const path of missingInventory) {
  test(`inventory requires ${path.join('.')}`, () => invalidInventory(changed(inventoryFixture(['dev']), path, undefined, true)));
}

const invalidInventoryCases: { name: string; path: Path; value: unknown }[] = [
  { name: 'future version', path: ['schema_version'], value: 2 },
  { name: 'unknown top-level bag', path: ['environment'], value: {} },
  { name: 'unknown nested field', path: ['hosts', 0, 'capacity', 'gpus'], value: 1 },
  { name: 'Compose input', path: rolePath('api', 'compose'), value: 'services: {}' },
  { name: 'mount input', path: rolePath('crawler', 'mounts'), value: ['/etc'] },
  { name: 'shell input', path: rolePath('cron', 'command'), value: 'echo unsafe' },
  { name: 'path input', path: rolePath('api', 'credentials', 'vertex', 'configuration_ref', 'path'), value: '/etc/adc.json' },
  { name: 'resolved secret', path: rolePath('api', 'credentials', 'stripe_ref', 'value'), value: 'not-a-real-secret' },
  { name: 'reference is not a URL', path: rolePath('api', 'credentials', 'stripe_ref', 'id'), value: 'https://invalid.test/secret' },
  { name: 'reference is not a path', path: rolePath('api', 'credentials', 'stripe_ref', 'id'), value: '/secrets/stripe' },
  { name: 'unreviewed stage', path: ['stages', 0, 'stage'], value: 'preview' },
  { name: 'wrong stage case', path: ['stages', 0, 'stage'], value: 'DEV' },
  { name: 'unknown host stage', path: ['hosts', 0, 'owned_stages'], value: ['development'] },
  { name: 'duplicate owned stage', path: ['hosts', 0, 'owned_stages'], value: ['dev', 'dev'] },
  { name: 'unlisted host ownership', path: ['hosts', 0, 'owned_stages'], value: ['prod'] },
  { name: 'unsupported OS', path: ['hosts', 0, 'operating_system'], value: 'UBUNTU_22_04' },
  { name: 'unsupported architecture', path: ['hosts', 0, 'architecture'], value: 'aarch64' },
  { name: 'unknown host', path: rolePath('api', 'host_id'), value: 'unlisted-host' },
  { name: 'unknown scope', path: rolePath('workers', 0, 'scope'), value: 'product-listing-fast' },
  { name: 'duplicate scope', path: rolePath('workers', 1, 'scope'), value: WorkerScope.options[0] },
  { name: 'mismatched scope credentials', path: rolePath('workers', 0, 'credentials', 'queue_scope'), value: WorkerScope.options[1] },
  { name: 'unscoped AWS identity', path: rolePath('workers', 0, 'credentials', 'aws', 'stage'), value: 'prod' },
  { name: 'SSM is not application identity', path: rolePath('api', 'credentials', 'aws', 'method'), value: 'SSM_NODE' },
  { name: 'mounted ADC must be a protected file reference', path: rolePath('api', 'credentials', 'vertex', 'configuration_ref', 'provider'), value: 'SSM_SECURE' },
  { name: 'raw ADC JSON', path: rolePath('api', 'credentials', 'vertex', 'credentials'), value: { private_key: 'not-a-real-key' } },
  { name: 'crawler start forbidden', path: rolePath('crawler', 'ownership_plan'), value: 'START' },
  { name: 'cron start forbidden', path: rolePath('cron', 'ownership_plan'), value: 'START' },
  { name: 'wrong database target', path: [...database, 'target'], value: 'crawler' },
  { name: 'host reserve overflow', path: ['hosts', 0, 'reserve', 'cpu_millicores'], value: 64001 },
  { name: 'fractional allocation', path: rolePath('api', 'resources_per_slot', 'cpu_millicores'), value: 0.1 },
  { name: 'unsafe integer', path: rolePath('api', 'resources_per_slot', 'disk_mib'), value: Number.MAX_SAFE_INTEGER + 1 },
  { name: 'nonfinite allocation', path: rolePath('api', 'resources_per_slot', 'memory_mib'), value: Infinity },
  { name: 'zero pool cap', path: rolePath('cron', 'credentials', 'business', 'pool_max'), value: 0 },
  { name: 'invalid port', path: rolePath('sequin', 'listener', 'port'), value: 65536 },
  { name: 'unknown bind', path: rolePath('sequin', 'listener', 'bind'), value: '0.0.0.0' },
  { name: 'endpoint target mismatch', path: [...nativePg, 'routing', 'target_host_id'], value: 'other-host' },
  { name: 'endpoint port mismatch', path: [...nativePg, 'port'], value: 5434 },
  { name: 'Lambda endpoint port mismatch', path: [...publicPg, 'port'], value: 5434 },
  { name: 'Lambda cannot use private routing', path: [...publicPg, 'routing', 'method'], value: 'PRIVATE_NETWORK' },
  { name: 'native cannot use public routing', path: [...nativePg, 'routing', 'method'], value: 'PUBLIC_INTERNET' },
  { name: 'public Lambda endpoint cannot bind private only', path: rolePath('postgres_business', 'listener', 'bind'), value: 'PRIVATE' },
  { name: 'private service cannot bind loopback', path: rolePath('opensearch', 'listener', 'bind'), value: 'LOOPBACK' },
  { name: 'no real localhost route', path: [...nativePg, 'routing', 'method'], value: 'LOCAL_TEST' },
  { name: 'no native container DNS', path: [...nativePg, 'host'], value: 'postgres' },
  { name: 'no real localhost', path: [...nativePg, 'host'], value: 'localhost' },
  { name: 'no raw IP identity', path: [...publicPg, 'tls', 'server_name'], value: '127.0.0.1' },
  { name: 'certificate identity mismatch', path: [...publicPg, 'tls', 'server_name'], value: 'other.validation-only.unit' },
  { name: 'native insecure TLS', path: [...nativePg, 'tls'], value: { mode: 'DISABLED' } },
  { name: 'Lambda insecure TLS', path: [...publicPg, 'tls'], value: { mode: 'DISABLED' } },
  { name: 'TLS require is insufficient', path: [...publicPg, 'tls', 'mode'], value: 'REQUIRE' },
  { name: 'TLS prefer is insufficient', path: [...nativePg, 'tls', 'mode'], value: 'PREFER' },
  { name: 'verify CA without hostname is insufficient', path: [...publicPg, 'tls', 'mode'], value: 'VERIFY_CA' },
  { name: 'example flag blocks real inventory', path: ['example'], value: true },
];
for (const { name, path, value } of invalidInventoryCases) {
  test(`inventory rejects ${name}`, () => invalidInventory(changed(inventoryFixture(['dev']), path, value)));
}

test('role and host completeness, duplicate stages and hosts', () => {
  const inventory = inventoryFixture(['dev']);
  const roles = inventory.stages[0]!.roles;
  invalidInventory(changed(inventory, rolePath('workers'), roles.workers.slice(1)));
  invalidInventory(changed(inventory, rolePath('workers'), [...roles.workers, roles.workers[0]]));
  invalidInventory(changed(inventory, ['hosts'], [...inventory.hosts, inventory.hosts[0]]));
  invalidInventory(changed(inventory, ['stages'], [...inventory.stages, inventory.stages[0]]));
  const split = inventoryFixture(['dev'], 'SPLIT_HOST');
  invalidInventory(changed(split, ['hosts', 1, 'dns', 'private'], split.hosts[0]!.dns.private));
  invalidInventory(changed(split, ['hosts', 1, 'ssm_node_identity', 'managed_node_id'], split.hosts[0]!.ssm_node_identity.managed_node_id));
  invalidInventory(changed(split, ['hosts', 2, 'owned_stages'], ['prod']));
});

for (const bind of ['LOOPBACK', 'PRIVATE', 'PUBLIC', 'ALL_IPV4', 'ALL_IPV6', 'ALL_INTERFACES'] as const) {
  test(`port reservation catches collisions including ${bind}`, () => {
    const inventory = inventoryFixture(['dev']);
    const roles = inventory.stages[0]!.roles;
    roles.api.slots.green.listener.port = roles.api.slots.blue.listener.port;
    roles.api.slots.green.endpoint.port = roles.api.slots.blue.listener.port;
    roles.api.slots.green.listener.bind = bind;
    invalidInventory(inventory);
  });
}
test('shared-host ports collide across stages even with acknowledged hardware', () => {
  const inventory = sharedHostFixture();
  const api = inventory.stages[1]!.roles.api;
  api.slots.blue.listener.port = 8080;
  api.slots.blue.endpoint.port = 8080;
  invalidInventory(inventory);
});
test('a worker role moves independently of host names and other roles', () => {
  const inventory = inventoryFixture(['dev'], 'SPLIT_HOST');
  const worker = inventory.stages[0]!.roles.workers[0]!;
  const host = inventory.hosts[2]!;
  host.architecture = 'arm64';
  worker.host_id = host.id;
  worker.endpoint.host = host.dns.private;
  worker.endpoint.routing.target_host_id = host.id;
  worker.endpoint.routing.method = 'WIREGUARD';
  if (worker.endpoint.tls.mode === 'VERIFY_FULL') worker.endpoint.tls.server_name = host.dns.private;
  assert.doesNotThrow(() => parseInventory(inventory));
});
test('same port on different hosts is allowed', () => {
  const inventory = inventoryFixture(['dev'], 'SPLIT_HOST');
  const roles = inventory.stages[0]!.roles;
  roles.crawler.listener.port = roles.cron.listener.port;
  roles.crawler.endpoint.port = roles.cron.listener.port;
  assert.doesNotThrow(() => parseInventory(inventory));
});
for (const [dimension, exact] of [['cpu_millicores', 5750], ['memory_mib', 11776], ['disk_mib', 35840]] as const) {
  test(`global ${dimension} budget includes nineteen allocations (both API slots) plus reserve`, () => {
    const inventory = inventoryFixture(['dev']);
    inventory.hosts[0]!.capacity[dimension] = exact;
    assert.doesNotThrow(() => parseInventory(inventory));
    inventory.hosts[0]!.capacity[dimension] -= 1;
    invalidInventory(inventory);
  });
}

test('PostgreSQL exact budgets include API overlap, cron lock, Lambdas and maintenance', () => {
  const inventory = inventoryFixture(['dev']);
  assert.doesNotThrow(() => parseInventory(inventory)); // 68 business, 24 crawler.
  for (const path of [
    rolePath('api', 'credentials', 'business', 'pool_max'),
    rolePath('workers', 0, 'credentials', 'business', 'pool_max'),
    rolePath('cron', 'credentials', 'business', 'pool_max'),
    rolePath('crawler', 'credentials', 'business', 'pool_max'),
    rolePath('crawler', 'credentials', 'crawler', 'pool_max'),
    ['stages', 0, 'lambda_postgres_clients', 'shopify', 'reserved_concurrency'],
    ['stages', 0, 'lambda_postgres_clients', 'stripe', 'pool_max'],
    [...database, 'migrator', 'sessions'], [...database, 'backup', 'sessions'],
    [...database, 'sequin', 'sessions'], [...database, 'reserve_sessions'],
  ] satisfies Path[]) invalidInventory(changed(inventory, path, 100));
  invalidInventory(changed(inventory, [...database, 'max_connections'], 67));
  invalidInventory(changed(inventory, rolePath('postgres_crawler', 'database', 'max_connections'), 23));
  // Raising API's per-slot cap by one requires two connections, not one.
  const apiGrowth = inventoryFixture(['dev']);
  apiGrowth.stages[0]!.roles.api.credentials.business.pool_max = 3;
  apiGrowth.stages[0]!.roles.postgres_business.database.max_connections = 69;
  invalidInventory(apiGrowth);
  apiGrowth.stages[0]!.roles.postgres_business.database.max_connections = 70;
  assert.doesNotThrow(() => parseInventory(apiGrowth));
});
test('connection multiplication cannot round or overflow into an accepted budget', () => {
  const inventory = inventoryFixture(['dev']);
  const stage = inventory.stages[0]!;
  stage.lambda_postgres_clients.cognito_post_confirmation.reserved_concurrency = Number.MAX_SAFE_INTEGER;
  stage.lambda_postgres_clients.cognito_post_confirmation.pool_max = Number.MAX_SAFE_INTEGER;
  stage.roles.postgres_business.database.max_connections = Number.MAX_SAFE_INTEGER;
  invalidInventory(inventory);
});

test('runtime, migrator, backup, replication and database histories are separate', () => {
  const inventory = inventoryFixture(['dev']);
  const roles = inventory.stages[0]!.roles;
  invalidInventory(changed(inventory, [...database, 'migrator', 'identity'], roles.api.credentials.business.identity));
  invalidInventory(changed(inventory, [...database, 'backup', 'identity'], roles.postgres_business.database.migrator.identity));
  invalidInventory(changed(inventory, [...database, 'sequin', 'identity'], roles.postgres_business.database.backup.identity));
  invalidInventory(changed(inventory, [...database, 'migrator', 'identity', 'principal'], roles.api.credentials.business.identity.principal));
  invalidInventory(changed(inventory, [...database, 'migrator', 'identity', 'secret_ref'], {
    ...roles.api.credentials.business.identity.secret_ref, revision: 'another-revision',
  }));
  invalidInventory(changed(inventory, rolePath('postgres_crawler', 'database', 'history_id'), roles.postgres_business.database.history_id));
  invalidInventory(changed(inventory, rolePath('crawler', 'credentials', 'crawler', 'identity', 'secret_ref'), roles.crawler.credentials.business.identity.secret_ref));
  invalidInventory(changed(inventory, rolePath('api', 'credentials', 'aws', 'application_identity_ref'), inventory.hosts[0]!.ssm_node_identity.registration_ref));
  invalidInventory(changed(inventory, rolePath('cron', 'credentials', 'aws', 'application_identity_ref'), roles.api.credentials.aws.application_identity_ref));
  invalidInventory(changed(inventory, rolePath('api', 'credentials', 'opensearch_ref'), roles.opensearch.admin_identity_ref));
});
test('scope-specific optional dependencies are required only for their reviewed consumers', () => {
  const inventory = inventoryFixture(['dev']);
  const workers = inventory.stages[0]!.roles.workers;
  invalidInventory(changed(inventory, rolePath('workers', 0, 'credentials', 'opensearch_ref'), undefined, true));
  invalidInventory(changed(inventory, rolePath('workers', 2, 'credentials', 'vertex'), undefined, true));
  invalidInventory(changed(inventory, rolePath('workers', 9, 'credentials', 'email'), undefined, true));
  invalidInventory(changed(inventory, rolePath('workers', 5, 'credentials', 'vertex'), workers[2]!.credentials.vertex));
  invalidInventory(changed(inventory, rolePath('workers', 5, 'credentials', 'email'), workers[9]!.credentials.email));
});
test('Vertex federation uses protected references, never a mounted path or token value', () => {
  const inventory = inventoryFixture(['dev']);
  const vertex = inventory.stages[0]!.roles.api.credentials.vertex;
  const federation = {
    method: 'WORKLOAD_IDENTITY_FEDERATION', configuration_ref: vertex.configuration_ref,
    subject_token_ref: { ...vertex.configuration_ref, id: 'fixture-subject-token' },
  };
  assert.doesNotThrow(() => parseInventory(changed(inventory, rolePath('api', 'credentials', 'vertex'), federation)));
  invalidInventory(changed(inventory, rolePath('api', 'credentials', 'vertex'), { ...federation, subject_token: 'not-a-real-token' }));
});

test('client CA must match the provisioned trust reference, including revision', () => {
  const inventory = inventoryFixture(['dev']);
  const ca = inventory.stages[0]!.roles.postgres_business.ca_ref;
  for (const endpoint of [nativePg, publicPg]) {
    invalidInventory(changed(inventory, [...endpoint, 'tls', 'ca_ref'], { ...ca, id: 'unrelated-ca' }));
    invalidInventory(changed(inventory, [...endpoint, 'tls', 'ca_ref'], { ...ca, revision: 'stale-ca' }));
  }
});
test('public Lambda PostgreSQL is distinct from native business and crawler endpoints', () => {
  const inventory = inventoryFixture(['dev']);
  const roles = inventory.stages[0]!.roles;
  invalidInventory(changed(inventory, publicPg, roles.postgres_business.endpoint));
  invalidInventory(changed(inventory, publicPg, roles.postgres_crawler.endpoint));
  const host = inventory.hosts[0]!;
  const originalPublicHostname = host.dns.public;
  function setPublicHostname(hostname: string): void {
    host.dns.public = hostname;
    for (const endpoint of [roles.postgres_business.lambda_endpoint, roles.edge.endpoint]) {
      endpoint.host = hostname;
      assert(endpoint.tls.mode === 'VERIFY_FULL');
      endpoint.tls.server_name = hostname;
    }
  }
  setPublicHostname(host.dns.private);
  invalidInventory(inventory);
  assert.equal(Inventory.safeParse(inventory).error?.issues.length, 1);
  setPublicHostname(originalPublicHostname);
  assert.doesNotThrow(() => parseInventory(inventory));
});
test('split hosts never accept implicit localhost or container service DNS, even in test', () => {
  const inventory = inventoryFixture(['test'], 'SPLIT_HOST');
  for (const host of ['localhost', 'postgres', 'opensearch', 'sequin']) {
    invalidInventory(changed(inventory, [...nativePg, 'host'], host));
  }
  const endpoint = inventory.stages[0]!.roles.postgres_business.endpoint;
  endpoint.host = 'localhost';
  endpoint.routing.method = 'LOCAL_TEST';
  endpoint.tls = { mode: 'DISABLED' };
  invalidInventory(inventory);
});
for (const hostname of ['localhost', '127.0.0.1', 'example.com', 'db.example.net', 'db.example.org', 'db.invalid', 'db.test', 'db.localhost', 'db.example']) {
  test(`real host rejects reserved DNS: ${hostname}`, () => {
    invalidInventory(changed(inventoryFixture(['prod']), ['hosts', 0, 'dns', 'management'], hostname));
  });
}
for (const account of ['000000000000', '111111111111', '123456789012', '999999999999']) {
  test(`real account rejects reserved example ${account}`, () => invalidInventory(changed(inventoryFixture(['prod']), ['stages', 0, 'aws', 'account_id'], account)));
}

const missingRuntime: Path[] = [
  ['schema_version'], ['example'], ['inventory'], ['stage'], ['release', 'source_sha'], ['release', 'manifest_digest'],
  ['release', 'images', 'worker'], ['api', 'probe'], ['api', 'drain_seconds'], ['api', 'stop_seconds'],
  ['workers', 0, 'execution_seconds'], ['workers', 0, 'visibility_seconds'], ['workers', 0, 'queues', 'source', 'url'],
  ['workers', 0, 'queues', 'source', 'arn'], ['workers', 0, 'queues', 'dlq'], ['cron', 'ownership_plan'],
  ['crawler', 'ownership_plan'], ['email_assets', 'bucket_ref'], ['email_assets', 'prefix'], ['preflight'],
];
for (const path of missingRuntime) {
  test(`runtime requires ${path.join('.')}`, () => invalidRuntime(changed(runtimeFixture('dev'), path, undefined, true)));
}
const invalidRuntimeCases: { name: string; path: Path; value: unknown }[] = [
  { name: 'unknown stage', path: ['stage'], value: 'preview' },
  { name: 'stage absent from inventory', path: ['stage'], value: 'prod' },
  { name: 'example marked real', path: ['example'], value: true },
  { name: 'endpoint overlay', path: ['endpoint_overrides'], value: {} },
  { name: 'environment overlay', path: ['environment'], value: {} },
  { name: 'local queue override for real stage', path: ['test_queue_endpoint'], value: { host: 'localhost', port: 4566, transport: 'HTTP' } },
  { name: 'API drain below 45', path: ['api', 'drain_seconds'], value: 44 },
  { name: 'API stop below 60', path: ['api', 'stop_seconds'], value: 59 },
  { name: 'API stop lacks termination headroom', path: ['api', 'drain_seconds'], value: 60 },
  { name: 'worker drain below 270', path: ['workers', 0, 'drain_seconds'], value: 269 },
  { name: 'worker stop below 300', path: ['workers', 0, 'stop_seconds'], value: 299 },
  { name: 'worker stop lacks termination headroom', path: ['workers', 0, 'drain_seconds'], value: 300 },
  { name: 'worker execution below scope minimum', path: ['workers', 2, 'execution_seconds'], value: 45 },
  { name: 'worker actual execution exceeds drain', path: ['workers', 2, 'execution_seconds'], value: 280 },
  { name: 'worker visibility below actual execution', path: ['workers', 2, 'visibility_seconds'], value: 240 },
  { name: 'mail visibility below queue minimum', path: ['workers', 9, 'visibility_seconds'], value: 300 },
  { name: 'cron drain below 300', path: ['cron', 'drain_seconds'], value: 299 },
  { name: 'cron stop lacks termination headroom', path: ['cron', 'stop_seconds'], value: 300 },
  { name: 'crawler stop lacks termination headroom', path: ['crawler', 'stop_seconds'], value: 60 },
  { name: 'cron runtime start', path: ['cron', 'ownership_plan'], value: 'START' },
  { name: 'crawler runtime start', path: ['crawler', 'ownership_plan'], value: 'START' },
  { name: 'probe deadline too short', path: ['api', 'probe', 'deadline_seconds'], value: 11 },
  { name: 'probe zero interval', path: ['api', 'probe', 'interval_seconds'], value: 0 },
  { name: 'probe unbounded timeout', path: ['api', 'probe', 'timeout_seconds'], value: 31 },
  { name: 'probe arbitrary URL', path: ['api', 'probe', 'url'], value: 'https://invalid.test' },
  { name: 'unknown worker scope', path: ['workers', 0, 'scope'], value: 'all' },
  { name: 'duplicate worker scope', path: ['workers', 1, 'scope'], value: WorkerScope.options[0] },
  { name: 'queue receive in preflight', path: ['preflight', 'queue_read'], value: 'RECEIVE_MESSAGE' },
  { name: 'queue receive count in preflight', path: ['preflight', 'max_messages'], value: 1 },
  { name: 'mutating preflight', path: ['preflight', 'mode'], value: 'MUTATE' },
  { name: 'email prefix wrong stage', path: ['email_assets', 'prefix'], value: `prod/${runtimeFixture('dev').release.source_sha}/mjml/` },
  { name: 'email prefix wrong SHA', path: ['email_assets', 'prefix'], value: `dev/${'1'.repeat(40)}/mjml/` },
  { name: 'email arbitrary path', path: ['email_assets', 'prefix'], value: '/tmp/mjml/' },
  { name: 'floating image tag', path: ['release', 'images', 'api'], value: 'latest' },
  { name: 'all-zero example image', path: ['release', 'images', 'api'], value: `sha256:${'0'.repeat(64)}` },
  { name: 'repeated example manifest', path: ['release', 'manifest_digest'], value: `sha256:${'a'.repeat(64)}` },
];
for (const { name, path, value } of invalidRuntimeCases) {
  test(`runtime rejects ${name}`, () => invalidRuntime(changed(runtimeFixture('dev'), path, value)));
}
test('worker drain covers actual configured scope execution, not just the 270s floor', () => {
  const runtime = runtimeFixture('dev');
  const worker = runtime.workers[2]!;
  worker.execution_seconds = 400;
  worker.visibility_seconds = 460;
  worker.drain_seconds = 400;
  worker.stop_seconds = 430;
  assert.doesNotThrow(() => parseRuntimeConfiguration(runtime));
  worker.drain_seconds = 399;
  invalidRuntime(runtime);
});
test('missing runtime scope and embedded inventory overflow cannot bypass validation', () => {
  const runtime = runtimeFixture('dev');
  invalidRuntime(changed(runtime, ['workers'], runtime.workers.slice(1)));
  invalidRuntime(changed(runtime, ['inventory', ...database, 'max_connections'], 67));
});
test('real source SHA rejects examples even with a matching email prefix', () => {
  const runtime = runtimeFixture('prod');
  runtime.release.source_sha = '0'.repeat(40);
  runtime.email_assets.prefix = `prod/${runtime.release.source_sha}/mjml/`;
  invalidRuntime(runtime);
});

test('queue source and DLQ are exact account/region/stage/scope pairs', () => {
  const runtime = runtimeFixture('dev');
  const worker = runtime.workers[0]!;
  invalidRuntime(changed(runtime, ['workers', 0, 'queues'], { source: worker.queues.dlq, dlq: worker.queues.source }));
  invalidRuntime(changed(runtime, ['workers', 0, 'queues', 'source', 'url'], worker.queues.dlq.url));
  invalidRuntime(changed(runtime, ['workers', 0, 'queues', 'source', 'arn'], worker.queues.dlq.arn));
  invalidRuntime(changed(runtime, ['workers', 0, 'queues'], runtime.workers[1]!.queues));
  for (const kind of ['source', 'dlq'] as const) {
    const queue = worker.queues[kind];
    for (const [from, to] of [['-dev', '-prod'], ['eu-central-1', 'us-east-1'], ['100200300400', '400300200100']]) {
      invalidRuntime(changed(runtime, ['workers', 0, 'queues', kind], {
        url: queue.url.replace(from!, to!), arn: queue.arn.replace(from!, to!),
      }));
    }
    for (const url of [
      `${queue.url}/`, `${queue.url}?override=1`, `${queue.url}#fragment`, queue.url.replace('https:', 'http:'),
      queue.url.replace('https://', 'https://user:password@'), queue.url.replace('aura-worker', '%61ura-worker'),
    ]) invalidRuntime(changed(runtime, ['workers', 0, 'queues', kind, 'url'], url));
    invalidRuntime(changed(runtime, ['workers', 0, 'queues', kind, 'arn'], queue.arn.replace('arn:aws:', 'arn:aws-cn:')));
  }
});
for (const [region, partition, suffix] of [
  ['us-gov-west-1', 'aws-us-gov', 'amazonaws.com'], ['cn-north-1', 'aws-cn', 'amazonaws.com.cn'],
] as const) {
  test(`queue identity derives partition and DNS suffix for ${region}`, () => {
    const runtime = runtimeFixture('dev');
    runtime.inventory.stages[0]!.aws.region = region;
    for (const worker of runtime.workers) for (const queue of Object.values(worker.queues)) {
      queue.url = queue.url.replace('eu-central-1.amazonaws.com', `${region}.${suffix}`);
      queue.arn = queue.arn.replace('arn:aws:sqs:eu-central-1:', `arn:${partition}:sqs:${region}:`);
    }
    assert.doesNotThrow(() => parseRuntimeConfiguration(runtime));
  });
}
test('either example flag independently blocks an otherwise valid real-stage runtime', () => {
  for (const stage of ['dev', 'prod'] as const) {
    const runtime = runtimeFixture(stage);
    assert.doesNotThrow(() => parseRuntimeConfiguration(runtime));
    invalidRuntime(changed(runtime, ['example'], true));
    invalidRuntime(changed(runtime, ['inventory', 'example'], true));
  }
});

function rebindExampleStage(runtime: RuntimeConfiguration, stage: RuntimeConfiguration['stage']): void {
  runtime.stage = stage;
  const selected = runtime.inventory.stages[0]!;
  selected.stage = stage;
  for (const host of runtime.inventory.hosts) host.owned_stages = [stage];
  const roles = selected.roles;
  for (const application of [roles.api, ...roles.workers, roles.cron, roles.crawler]) {
    application.credentials.aws.stage = stage;
  }
  const { region, account_id } = selected.aws;
  for (const worker of runtime.workers) for (const kind of ['source', 'dlq'] as const) {
    const name = `aura-worker-${worker.scope}${kind === 'dlq' ? '-dlq' : ''}-${stage}`;
    worker.queues[kind] = {
      url: `https://sqs.${region}.amazonaws.com/${account_id}/${name}`,
      arn: `arn:aws:sqs:${region}:${account_id}:${name}`,
    };
  }
  runtime.email_assets.bucket_ref = `${stage}-mail-bucket`;
  runtime.email_assets.prefix = `${stage}/${runtime.release.source_sha}/mjml/`;
}

for (const stage of ['dev', 'prod'] as const) {
  const target = runtimeFixture(stage);
  const placeholders: { name: string; path: Path; value: string; replacement: string }[] = [
    { name: 'DNS', path: ['inventory', 'hosts', 0, 'dns', 'management'], value: 'management-test-one.example.test', replacement: target.inventory.hosts[0]!.dns.management },
    { name: 'account', path: ['inventory', 'stages', 0, 'aws', 'account_id'], value: '000000000000', replacement: target.inventory.stages[0]!.aws.account_id },
    { name: 'source SHA', path: ['release', 'source_sha'], value: '0'.repeat(40), replacement: target.release.source_sha },
    { name: 'manifest digest', path: ['release', 'manifest_digest'], value: `sha256:${'0'.repeat(64)}`, replacement: target.release.manifest_digest },
    { name: 'image digest', path: ['release', 'images', 'api'], value: `sha256:${'0'.repeat(64)}`, replacement: target.release.images.api },
  ];
  for (const { name, path, value, replacement } of placeholders) {
    test(`promotion to ${stage} rejects retained placeholder ${name} with both example flags cleared`, () => {
      // Keep TLS and topology valid; only the named placeholder survives promotion.
      const runtime = changed(target, path, value) as RuntimeConfiguration;
      runtime.example = true;
      runtime.inventory.example = true;
      rebindExampleStage(runtime, 'test');
      assert.doesNotThrow(() => parseRuntimeConfiguration(runtime));
      runtime.example = false;
      runtime.inventory.example = false;
      rebindExampleStage(runtime, stage);
      invalidRuntime(runtime);
      assert.equal(RuntimeConfiguration.safeParse(runtime).error?.issues.length, 1);
      const repaired = changed(runtime, path, replacement) as RuntimeConfiguration;
      rebindExampleStage(repaired, stage);
      assert.doesNotThrow(() => parseRuntimeConfiguration(repaired));
    });
  }
}

test('safe parse errors expose no input, Zod details, causes, URLs or thrown getter contents', () => {
  const marker = 'sensitive-input-marker';
  const getterInput = Object.defineProperty({}, 'schema_version', { get() { throw new Error(marker); } });
  for (const parse of [parseInventory, parseRuntimeConfiguration]) {
    for (const input of [null, false, [], marker, { [marker]: `https://user:${marker}@invalid.test` }, getterInput]) {
      try { parse(input); assert.fail('Expected generic contract error'); }
      catch (error) {
        assert(error instanceof Error);
        assert.equal(error.constructor, Error);
        assert.match(error.message, /^Invalid (?:inventory|runtime configuration)$/);
        assert.equal(error.message.includes(marker), false);
        assert.equal(Object.hasOwn(error, 'cause'), false);
        assert.equal(Object.hasOwn(error, 'issues'), false);
      }
    }
  }
});

test('committed JSON schemas are derived, strict and current; semantic checks remain in Zod', () => {
  function checkObjects(value: unknown): number {
    if (!value || typeof value !== 'object') return 0;
    const object = value as Record<string, unknown>;
    let count = 0;
    if (object.type === 'object') {
      assert.equal(object.additionalProperties, false);
      assert(object.properties && typeof object.properties === 'object');
      count += 1;
    }
    return count + Object.values(object).reduce<number>((sum, child) => sum + checkObjects(child), 0);
  }
  for (const [name, expected] of Object.entries(inventoryJsonSchemas())) {
    const actual: unknown = JSON.parse(readFileSync(new URL(`../../../schemas/${name}`, import.meta.url), 'utf8'));
    assert.deepEqual(actual, expected);
    assert.equal(expected.$schema, 'https://json-schema.org/draft/2020-12/schema');
    assert.equal(expected.type, 'object');
    assert(checkObjects(expected) > 20);
  }
});
