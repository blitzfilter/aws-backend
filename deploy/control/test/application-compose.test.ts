import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { test } from 'node:test';
import { inventoryFixture, insecureLocalFixture, runtimeFixture } from '../fixtures/inventory-fixtures.js';
import { readCatalog, workspaceRoot } from '../src/catalog.js';
import { canonicalHash } from '../src/contracts/hash.js';
import { parseRuntimeConfiguration, type RuntimeConfiguration } from '../src/contracts/inventory.js';
import { WorkerScope } from '../src/contracts/primitives.js';
import {
  ApplicationComposeError, parseApplicationComposeIdentity, renderApplicationCompose,
  type ApplicationComposeIdentity, type ApplicationComposeSelection,
} from '../src/host/application-compose.js';

function identityInput(runtime: RuntimeConfiguration, selection: ApplicationComposeSelection = { role: 'cron' }) {
  const stage = runtime.inventory.stages.find(stage => stage.stage === runtime.stage)!;
  const placement = selection.role === 'worker'
    ? stage.roles.workers.find(worker => worker.scope === selection.scope)!
    : stage.roles[selection.role];
  const host = runtime.inventory.hosts.find(host => host.id === placement.host_id)!;
  const components = { api: 'aura-historia-api', worker: 'aura-historia-worker', cron: 'aura-historia-cron', crawler: 'crawler' } as const;
  return {
    selection,
    target: { stage: runtime.stage, account: stage.aws.account_id, region: stage.aws.region },
    host_id: host.id,
    configuration_sha256: canonicalHash(runtime),
    manifest_sha256: runtime.release.manifest_digest,
    native_image: {
      component: components[selection.role], architecture: host.architecture,
      rust_target: host.architecture === 'arm64' ? 'aarch64-unknown-linux-gnu' : 'x86_64-unknown-linux-gnu',
      source_sha: runtime.release.source_sha,
      image: {
        registry: `${stage.aws.account_id}.dkr.ecr.${stage.aws.region}.${stage.aws.region.startsWith('cn-') ? 'amazonaws.com.cn' : 'amazonaws.com'}`,
        repository: `aura/${components[selection.role]}`,
        digest: runtime.release.images[selection.role],
      },
    },
  };
}
function render(runtime: RuntimeConfiguration, selection: ApplicationComposeSelection = { role: 'cron' }): string {
  return renderApplicationCompose(runtime, parseApplicationComposeIdentity(identityInput(runtime, selection)));
}
function changed(input: unknown, path: (string | number)[], value: unknown): unknown {
  const copy = JSON.parse(JSON.stringify(input)) as Record<string | number, unknown>;
  let parent = copy;
  for (const key of path.slice(0, -1)) parent = parent[key] as typeof copy;
  parent[path.at(-1)!] = value;
  return copy;
}
function blocked(action: () => unknown, code: string): void {
  assert.throws(action, error => error instanceof ApplicationComposeError && error.code === code && error.message === code);
}

for (const layout of ['ALL_IN_ONE', 'SPLIT_HOST'] as const) {
  for (const stage of ['dev', 'prod', 'test'] as const) {
    for (const role of ['cron', 'crawler'] as const) {
      test(`renders isolated ${stage}/${role}, ${layout}, without mutating input`, () => {
        const runtime = parseRuntimeConfiguration(runtimeFixture(stage, inventoryFixture([stage], layout)));
        const before = structuredClone(runtime);
        const identity = parseApplicationComposeIdentity(identityInput(runtime, { role }));
        const identityBefore = structuredClone(identity);
        const text = renderApplicationCompose(runtime, identity);
        assert.equal(text, renderApplicationCompose(runtime, identity));
        assert.deepEqual(runtime, before);
        assert.deepEqual(identity, identityBefore);
        const document = JSON.parse(text);
        assert.deepEqual(Object.keys(document).sort(), ['name', 'networks', 'services']);
        assert.equal(document.name, `aura-${stage}-${role}`);
        assert.deepEqual(Object.keys(document.services), [role]);
        const service = document.services[role];
        const entry = readCatalog().native.find(entry => entry.id === identity.native_image.component)!;
        assert.equal(entry.status, 'IMPLEMENTED');
        assert.deepEqual(service.entrypoint, [`/usr/local/bin/${entry.binary}`]);
        assert.deepEqual(service.command, []);
        assert.equal(service.image, `${identity.native_image.image.registry}/${identity.native_image.image.repository}@${runtime.release.images[role]}`);
        assert.equal(service.labels['io.aura.host-id'], identity.host_id);
        assert.equal(service.labels['io.aura.configuration-sha256'], canonicalHash(runtime));
        assert.equal(service.labels['io.aura.manifest-sha256'], runtime.release.manifest_digest);
        assert.equal(service.environment.STAGE, stage);
        assert.equal(service.stop_grace_period, `${runtime[role].stop_seconds}s`);
        assert.equal(service.environment[role === 'cron' ? 'AURA_HISTORIA_CRON_STOP_TIMEOUT_SECONDS' : 'CRAWLER_STOP_TIMEOUT_SECONDS'], String(runtime[role].stop_seconds));
        assert.equal(service.environment[role === 'cron' ? 'AURA_HISTORIA_CRON_SHUTDOWN_GRACE_SECONDS' : 'CRAWLER_SHUTDOWN_GRACE_SECONDS'], String(runtime[role].drain_seconds));
        assert.equal(service.environment[role === 'cron' ? 'AURA_HISTORIA_CRON_HEALTH_BIND_ADDR' : 'CRAWLER_OPERATIONS_BIND_ADDR'], `127.0.0.1:${runtime.inventory.stages[0]!.roles[role].operations_listener.port}`);
        assert.deepEqual(service.healthcheck, { disable: true });
        assert.equal(service.restart, 'no');
        assert.equal(service.pull_policy, 'never');
        assert.equal(service.user, '10001:10001');
        assert.equal(service.init, true);
        assert.equal(service.read_only, true);
        assert.deepEqual(service.cap_drop, ['ALL']);
        assert.deepEqual(service.security_opt, ['no-new-privileges:true']);
        assert.equal(service.pids_limit, 256);
        assert.equal(service.cpus, '0.250');
        assert.equal(service.mem_limit, '512m');
        assert.equal(service.memswap_limit, service.mem_limit);
        assert.equal(service.shm_size, '1m');
        assert.deepEqual(service.tmpfs, ['/tmp:rw,noexec,nosuid,nodev,size=64m,mode=1777']);
        assert.deepEqual(service.ulimits.core, { soft: 0, hard: 0 });
        assert.deepEqual(service.logging, { driver: 'local', options: { 'max-size': '4m', 'max-file': '3', compress: 'false' } });
        const root = `/etc/aura-historia/${stage}/${role}`;
        assert.deepEqual(service.env_file, [{ path: `${root}/runtime.env`, required: true, format: 'raw' }]);
        assert.equal(service.environment.GOOGLE_APPLICATION_CREDENTIALS, `${root}/google-adc.json`);
        assert.equal(service.environment.POSTGRES_SSL_ROOT_CERT, `${root}/postgres-ca.pem`);
        assert.deepEqual(service.volumes, ['google-adc.json', 'postgres-ca.pem'].map(file => ({
          type: 'bind', source: `${root}/${file}`, target: `${root}/${file}`,
          read_only: true, bind: { create_host_path: false },
        })));
        assert.deepEqual(document.networks, { application: { driver: 'bridge' } });
        assert.deepEqual(service.networks, ['application']);
        for (const forbidden of ['ports', 'expose', 'platform', 'build', 'privileged', 'cap_add', 'devices', 'network_mode', 'pid', 'ipc', 'container_name', 'depends_on', 'extends', 'deploy', 'profiles', 'post_start', 'pre_stop']) {
          assert.equal(forbidden in service, false, forbidden);
        }
        assert.equal('volumes' in document, false);
        for (const forbidden of ['docker.sock', 'CMD-SHELL', 'host.docker.internal', 'localhost', 'bootstrap-local', 'migrate', 'prune', '--remove-orphans', 'unless-stopped', 'on-failure', '${']) {
          assert.equal(text.includes(forbidden), false, forbidden);
        }
        assert.equal(text.includes('-secret'), false);
        if (role === 'cron') {
          assert.equal(service.environment.POSTGRES_HOST, runtime.inventory.stages[0]!.roles.postgres_business.endpoint.host);
          assert.equal(service.environment.POSTGRES_MAX_CONNECTIONS, '2');
          assert.equal(service.environment.COMMIT_SHA, runtime.release.source_sha);
          assert.equal(service.environment.PERIODIC_MATCH_MAX_RUN_SECONDS, '7200');
          assert.equal(service.environment.AURA_HISTORIA_CRON_ENABLED_JOBS, 'search-filter-periodic-match');
        } else {
          assert.equal(service.environment.CRAWLER_REVIEW_BIND_ADDR, '127.0.0.1:7878');
          assert.equal(service.environment.CRAWLER_STARTUP_TIMEOUT_SECONDS, '60');
          assert.equal(service.environment.SPIDER_MAX_SIZE_BYTES, '8388608');
          for (const key of ['COMMIT_SHA', 'POSTGRES_MAX_CONNECTIONS', 'BUSINESS_DATABASE_URL', 'LOCAL_DB_URL']) assert.equal(key in service.environment, false);
        }
      });
    }
  }
}

for (const selection of [
  { role: 'api', slot: 'blue' }, { role: 'api', slot: 'green' },
  ...WorkerScope.options.map(scope => ({ role: 'worker' as const, scope })),
] as const) {
  for (const layout of ['ALL_IN_ONE', 'SPLIT_HOST'] as const) {
    test(`blocks missing private bind address: ${JSON.stringify(selection)} ${layout}`, () => {
      const runtime = parseRuntimeConfiguration(runtimeFixture('test', inventoryFixture(['test'], layout)));
      blocked(() => render(runtime, selection), 'PRIVATE_BIND_IP_UNAVAILABLE');
    });
  }
}

test('PRIVATE intent and a numeric DNS field still do not constitute a typed bind IP', () => {
  const runtime = runtimeFixture();
  const stage = runtime.inventory.stages[0]!;
  const host = runtime.inventory.hosts[0]!;
  host.dns.private = '10.20.30.40';
  for (const service of [stage.roles.postgres_business, stage.roles.postgres_crawler, stage.roles.opensearch, stage.roles.sequin, ...stage.roles.workers, ...Object.values(stage.roles.api.slots)]) {
    service.listener.bind = 'PRIVATE';
    service.endpoint.host = host.dns.private;
    if (service.endpoint.tls.mode === 'VERIFY_FULL') service.endpoint.tls.server_name = host.dns.private;
  }
  // Business listener is also used by its public Lambda endpoint.
  stage.roles.postgres_business.listener.bind = 'ALL_INTERFACES';
  const parsed = parseRuntimeConfiguration(runtime);
  blocked(() => render(parsed, { role: 'api', slot: 'blue' }), 'PRIVATE_BIND_IP_UNAVAILABLE');
  blocked(() => render(parsed, { role: 'worker', scope: 'product-embedding' }), 'PRIVATE_BIND_IP_UNAVAILABLE');
});

for (const [path, value] of [
  [['target', 'stage'], 'prod'], [['target', 'account'], '900800700600'], [['target', 'region'], 'us-east-1'],
  [['host_id'], 'test-data'], [['configuration_sha256'], `sha256:${'b'.repeat(64)}`],
  [['manifest_sha256'], `sha256:${'b'.repeat(64)}`], [['native_image', 'source_sha'], 'b'.repeat(40)],
  [['native_image', 'component'], 'aura-historia-migrate'], [['native_image', 'architecture'], 'arm64'],
  [['native_image', 'rust_target'], 'aarch64-unknown-linux-gnu'],
  [['native_image', 'image', 'digest'], `sha256:${'b'.repeat(64)}`],
  [['native_image', 'image', 'registry'], 'public.ecr.aws'],
  [['native_image', 'image', 'registry'], '100200300400.dkr.ecr.us-east-1.amazonaws.com'],
  [['native_image', 'image', 'registry'], '900800700600.dkr.ecr.eu-central-1.amazonaws.com'],
  [['native_image', 'image', 'registry'], '100200300400.dkr.ecr.eu-central-1.amazonaws.com.evil.test'],
] as const) {
  test(`rejects mismatched identity ${path.join('.')}: ${value}`, () => {
    const runtime = parseRuntimeConfiguration(runtimeFixture());
    const input = changed(identityInput(runtime), [...path], value);
    blocked(() => renderApplicationCompose(runtime, parseApplicationComposeIdentity(input)), 'IDENTITY_MISMATCH');
  });
}

for (const [path, value] of [
  [['selection'], { role: 'worker' }], [['selection'], { role: 'api', slot: 'red' }],
  [['selection'], { role: 'worker', scope: 'product-embedding;id' }],
  ...['postgres_business', 'postgres_crawler', 'opensearch', 'sequin', 'edge', 'aura-historia-migrate', 'jobs'].map(role => [['selection'], { role }] as const),
  [['target', 'stage'], '../prod'], [['host_id'], 'test-one\nCANARY'],
  [['native_image', 'image', 'registry'], 'evil.test/;CANARY'],
  [['native_image', 'image', 'repository'], '../CANARY'],
  [['native_image', 'image', 'repository'], 'repo:latest'],
  [['native_image', 'image', 'repository'], 'repo/${CANARY}'],
  [['native_image', 'image', 'repository'], 'repo\n'],
  [['native_image', 'image', 'repository'], 'repo\u2028'],
  [['native_image', 'image', 'repository'], 'repo\u2029'],
  [['native_image', 'source_sha'], `${'b'.repeat(40)}\n`],
  [['native_image', 'image', 'digest'], 'latest'],
  [['native_image', 'image', 'digest'], `sha256:${'b'.repeat(64)}\nCANARY`],
  [['env_file'], '/tmp/CANARY'], [['environment'], { STAGE: 'prod' }],
  [['volumes'], ['/var/run/docker.sock:/var/run/docker.sock']], [['command'], ['sh', '-c', 'CANARY']],
  [['private_bind_ip'], '10.0.0.5'], [['selection', 'private_bind_ip'], '10.0.0.5'],
] as const) {
  test(`rejects unsupported/injected input ${path.join('.')}: ${JSON.stringify(value)}`, () => {
    const runtime = parseRuntimeConfiguration(runtimeFixture());
    const input = changed(identityInput(runtime), [...path], value);
    blocked(() => parseApplicationComposeIdentity(input), 'INVALID_INPUT');
    blocked(() => renderApplicationCompose(runtime, input as ApplicationComposeIdentity), 'INVALID_INPUT');
  });
}

test('revalidates runtime shape, budgets and config identity after parsing', () => {
  const runtime = parseRuntimeConfiguration(runtimeFixture());
  const identity = parseApplicationComposeIdentity(identityInput(runtime));
  runtime.cron.stop_seconds = 360;
  blocked(() => renderApplicationCompose(runtime, identity), 'IDENTITY_MISMATCH');
  runtime.cron.stop_seconds = 329;
  blocked(() => render(runtime), 'INVALID_INPUT');
  blocked(() => renderApplicationCompose(changed(runtimeFixture(), ['environment'], { CANARY: 'value' }) as RuntimeConfiguration, identity), 'INVALID_INPUT');
  blocked(() => renderApplicationCompose(changed(runtimeFixture(), ['inventory', 'hosts', 0, 'dns', 'private'], 'evil\nCANARY') as RuntimeConfiguration, identity), 'INVALID_INPUT');
});

test('rejects trailing runtime controls and invalid local cron source SHA', () => {
  const runtime = runtimeFixture();
  runtime.inventory.stages[0]!.roles.cron.credentials.vertex.configuration_ref.id += '\n';
  blocked(() => render(runtime), 'INVALID_INPUT');
  for (const sha of ['a'.repeat(40), '0123456789abcdef0123456789abcdef01234567']) {
    const local = runtimeFixture();
    local.release.source_sha = sha;
    local.email_assets.prefix = `${local.stage}/${sha}/mjml/`;
    blocked(() => render(local), 'INVALID_INPUT');
  }
});

test('redacts malformed-input getter errors without retaining payload or cause', () => {
  const input = { get selection(): never { throw new Error('CANARY'); } };
  blocked(() => parseApplicationComposeIdentity(input), 'INVALID_INPUT');
});

test('keeps nondefault lifecycle/resource values consistent and bounded', () => {
  const runtime = runtimeFixture();
  runtime.cron = { ...runtime.cron, drain_seconds: 3570, stop_seconds: 3600, execution_seconds: 1234 };
  runtime.crawler = { ...runtime.crawler, drain_seconds: 310, stop_seconds: 345, startup_seconds: 91 };
  runtime.inventory.stages[0]!.roles.cron.resources = { cpu_millicores: 1234, memory_mib: 128, disk_mib: 16 };
  const cron = JSON.parse(render(runtime)).services.cron;
  assert.equal(cron.cpus, '1.234');
  assert.equal(cron.mem_limit, '128m');
  assert.equal(cron.memswap_limit, '128m');
  assert.equal(cron.tmpfs[0], '/tmp:rw,noexec,nosuid,nodev,size=16m,mode=1777');
  assert.equal(cron.stop_grace_period, '3600s');
  assert.equal(cron.environment.AURA_HISTORIA_CRON_SHUTDOWN_GRACE_SECONDS, '3570');
  assert.equal(cron.environment.AURA_HISTORIA_CRON_STOP_TIMEOUT_SECONDS, '3600');
  assert.equal(cron.environment.PERIODIC_MATCH_MAX_RUN_SECONDS, '1234');
  const crawler = JSON.parse(render(runtime, { role: 'crawler' })).services.crawler;
  assert.equal(crawler.stop_grace_period, '345s');
  assert.equal(crawler.environment.CRAWLER_STOP_TIMEOUT_SECONDS, '345');
  assert.equal(crawler.environment.CRAWLER_SHUTDOWN_GRACE_SECONDS, '310');
  assert.equal(crawler.environment.CRAWLER_STARTUP_TIMEOUT_SECONDS, '91');
});

test('blocks insufficient runtime resources and unconfigurable crawler pool caps', () => {
  for (const [key, value] of [['memory_mib', 127], ['disk_mib', 15]] as const) {
    const runtime = runtimeFixture();
    runtime.inventory.stages[0]!.roles.cron.resources[key] = value;
    blocked(() => render(runtime), 'RUNTIME_BUDGET_UNSUPPORTED');
  }
  for (const pool of ['business', 'crawler'] as const) {
    const runtime = runtimeFixture();
    runtime.inventory.stages[0]!.roles.crawler.credentials[pool].pool_max = 1;
    blocked(() => render(runtime, { role: 'crawler' }), 'RUNTIME_BUDGET_UNSUPPORTED');
  }
});

test('rejects cross-container localhost and mismatched crawler TLS without guessing', () => {
  const local = parseRuntimeConfiguration(insecureLocalFixture());
  for (const role of ['cron', 'crawler'] as const) blocked(() => render(local, { role }), 'CONTAINER_ENDPOINT_UNSUPPORTED');
  const runtime = runtimeFixture();
  runtime.inventory.stages[0]!.roles.postgres_crawler.endpoint.tls = { mode: 'DISABLED' };
  blocked(() => render(runtime, { role: 'crawler' }), 'CRAWLER_TLS_POLICY_MISMATCH');
});

test('supports explicit nonreal TLS disable without creating a fake CA requirement', () => {
  const runtime = runtimeFixture();
  runtime.inventory.stages[0]!.roles.postgres_business.endpoint.tls = { mode: 'DISABLED' };
  runtime.inventory.stages[0]!.roles.postgres_crawler.endpoint.tls = { mode: 'DISABLED' };
  for (const role of ['cron', 'crawler'] as const) {
    const service = JSON.parse(render(runtime, { role })).services[role];
    assert.equal(service.environment.POSTGRES_SSL_MODE, 'disable');
    assert.equal('POSTGRES_SSL_ROOT_CERT' in service.environment, false);
    assert.equal(service.volumes.length, 1);
  }
});

test('does not turn protected reference IDs into filesystem paths', () => {
  const runtime = runtimeFixture();
  const reference = runtime.inventory.stages[0]!.roles.cron.credentials.vertex.configuration_ref;
  reference.id = 'different-protected-adc-id';
  const service = JSON.parse(render(runtime)).services.cron;
  assert.equal(service.volumes[0].source, '/etc/aura-historia/test/cron/google-adc.json');
  assert.equal(JSON.stringify(service).includes(reference.id), false);
});

test('blocks WIF rather than inventing subject-token or executable credential-source wiring', () => {
  const runtime = runtimeFixture();
  const credentials = runtime.inventory.stages[0]!.roles.cron.credentials;
  credentials.vertex = {
    method: 'WORKLOAD_IDENTITY_FEDERATION', configuration_ref: credentials.vertex.configuration_ref,
    subject_token_ref: { provider: 'PROTECTED_FILE', id: 'subject-token', revision: 'v1' },
  };
  blocked(() => render(runtime), 'ADC_MATERIALIZATION_UNSUPPORTED');
});

test('supports exact ARM target without a Compose platform/emulation override', () => {
  const runtime = runtimeFixture();
  runtime.inventory.hosts[0]!.architecture = 'arm64';
  const service = JSON.parse(render(runtime)).services.cron;
  assert.equal('platform' in service, false);
});

for (const region of ['cn-north-1', 'us-gov-west-1']) {
  test(`requires exact private ECR partition suffix: ${region}`, () => {
    const inventory = inventoryFixture();
    inventory.stages[0]!.aws.region = region;
    const runtime = runtimeFixture('test', inventory);
    for (const worker of runtime.workers) for (const queue of Object.values(worker.queues)) {
      queue.arn = queue.arn.replace('arn:aws:', region.startsWith('cn-') ? 'arn:aws-cn:' : 'arn:aws-us-gov:');
      if (region.startsWith('cn-')) queue.url = queue.url.replace('.amazonaws.com/', '.amazonaws.com.cn/');
    }
    const parsed = parseRuntimeConfiguration(runtime);
    assert.match(JSON.parse(render(parsed)).services.cron.image, /\.dkr\.ecr\./);
    const identity = identityInput(parsed);
    identity.native_image.image.registry += '.evil.test';
    blocked(() => renderApplicationCompose(parsed, parseApplicationComposeIdentity(identity)), 'IDENTITY_MISMATCH');
  });
}

test('rendered lifecycle variable names exist in the actual runtime parsers', () => {
  const runtime = runtimeFixture();
  const cronSource = readFileSync(resolve(workspaceRoot, 'src/aura-historia-cron/src/config.rs'), 'utf8')
    + readFileSync(resolve(workspaceRoot, 'src/aura-historia-cron/src/wiring/search_filter_periodic_match.rs'), 'utf8');
  const crawlerSource = readFileSync(resolve(workspaceRoot, 'src/crawler/src/bin/server_runtime/config.rs'), 'utf8');
  for (const role of ['cron', 'crawler'] as const) {
    const service = JSON.parse(render(runtime, { role })).services[role];
    for (const key of Object.keys(service.environment).filter(key => /^(AURA_HISTORIA_CRON_|PERIODIC_MATCH_|SEARCH_FILTER_|CRAWLER_|SPIDER_)/.test(key))) {
      assert.ok((role === 'cron' ? cronSource : crawlerSource).includes(`"${key}"`), key);
    }
  }
});
