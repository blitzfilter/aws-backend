import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import {
  DeploymentIntent, DeploymentPlan, MigrationMetadata, ReleaseManifest,
  assertIntentBinding, parseIntent, parseManifest, parseMigrationMetadata, parsePlan,
} from '../src/contracts/release.js';
import type { TrustedIntentContext } from '../src/contracts/release.js';
import { releaseSchemaDocuments } from '../src/contracts/export-release-schemas.js';
import {
  exampleIntent, exampleManifest, exampleManifestPolicy, examplePlan, fixtureDigest, fixtureSource,
} from '../fixtures/release-fixtures.js';

function invalid(label: string, work: () => unknown): void {
  assert.throws(work, (error: unknown) => {
    assert.ok(error instanceof Error);
    assert.equal(error.constructor, Error);
    assert.equal(error.message, `invalid ${label}`);
    assert.deepEqual(Object.keys(error), []);
    assert.equal(error.cause, undefined);
    return true;
  });
}
function reordered(value: unknown, reverse: boolean): unknown {
  if (Array.isArray(value)) return value.map(item => reordered(item, reverse));
  if (value !== null && typeof value === 'object') {
    const entries = Object.entries(value).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0);
    if (reverse) entries.reverse();
    return Object.fromEntries(entries.map(([key, item]) => [key, reordered(item, reverse)]));
  }
  return value;
}
// Test-only implementation for dependency injection, not the production hash owner.
// Inputs here are finite, complete typed fixtures. The integrator tests byte rules separately.
const hashPlan = (plan: DeploymentPlan): string => `sha256:${createHash('sha256').update(JSON.stringify(reordered(plan, false)), 'utf8').digest('hex')}`;
function bindingFixture() {
  const plan = structuredClone(examplePlan);
  const intent = exampleIntent(hashPlan(plan));
  const { schema_version: _version, kind: _kind, example: _example, ...approved } = structuredClone(intent);
  const expected: TrustedIntentContext = { ...approved, hash_plan: hashPlan };
  return { plan, intent, expected };
}

const manifest = () => structuredClone(exampleManifest);
const parseExample = (input: unknown) => parseManifest(input, exampleManifestPolicy);
test('complete synthetic release represents every component, stream, family, scope and mail language', () => {
  const input = manifest();
  const parsed = parseExample(JSON.parse(JSON.stringify(input)));
  assert.deepEqual(parsed, input);
  assert.equal(parsed.example, true);
  assert.equal(parsed.native_images.length, 5);
  assert.ok(parsed.native_images.some(image => image.component === 'aura-historia-migrate'));
  assert.equal(parsed.lambdas.length, 5);
  assert.equal(parsed.mail_bundle.templates.length, 25);
  assert.deepEqual(parsed.migrations.streams.map(stream => stream.migrations.length), [1, 6]);
  assert.equal(parsed.search.families.length, 2);
  assert.equal(parsed.queues.length, 10);
  assert.equal('target' in parsed, false);
  assert.equal('manifest_sha256' in parsed, false);
  assert.deepEqual(parseMigrationMetadata(parsed.migrations), parsed.migrations);
});
test('migration OCI component is required even when another architecture fills the image count', () => {
  const input = manifest();
  input.native_images = input.native_images.filter(image => image.component !== 'aura-historia-migrate');
  input.native_images.push({ ...input.native_images[0]!, architecture: 'x86_64', rust_target: 'x86_64-unknown-linux-gnu' });
  invalid('manifest', () => parseExample(input));
});
test('fixture mail groups and analysis names match the supplied catalog asset identities', () => {
  const parsed = parseExample(manifest());
  const groups = [
    'partnership-application/approval', 'partnership-application/rejection', 'search-filter/match',
    'watchlist/product-update/availability', 'watchlist/product-update/price',
  ];
  assert.deepEqual([...new Set(parsed.mail_bundle.templates.map(template => template.template))], groups);
  for (const group of groups) {
    assert.deepEqual(parsed.mail_bundle.templates.filter(template => template.template === group).map(template => template.language), ['de', 'en', 'es', 'fr', 'it']);
  }
  for (const family of parsed.search.families) {
    assert.deepEqual(family.analysis_assets.map(asset => asset.name), [
      'english-synonyms', 'german-synonyms', 'french-synonyms', 'spanish-synonyms', 'italian-synonyms',
    ]);
    for (const asset of family.analysis_assets) assert.ok(asset.artifact.key.endsWith(`/search/analysis/${asset.name}.txt`));
  }
});
test('structural/source-policy parsing does not replace the mandatory trusted catalog completeness gate', () => {
  const input = manifest();
  input.mail_bundle.templates.pop();
  input.search.families[0]!.analysis_assets.pop();
  input.migrations.streams[1]!.migrations.pop();
  assert.deepEqual(parseExample(input), input);
});
for (const field of [
  'transaction_mode', 'lock_timeout_ms', 'statement_timeout_ms', 'affected_capabilities',
  'required_extensions', 'old_new_compatibility_evidence_sha256',
] as const) {
  test(`each migration must explicitly declare ${field}, even when marked compatible`, () => {
    for (const stream of [0, 1]) {
      const input = manifest();
      const migration: Record<string, unknown> = input.migrations.streams[stream]!.migrations[0]!;
      delete migration[field];
      invalid('manifest', () => parseExample(input));
      invalid('migration metadata', () => parseMigrationMetadata(input.migrations));
    }
  });
}
const invalidMigrationFields: ReadonlyArray<readonly [string, readonly unknown[]]> = [
  ['transaction_mode', ['AUTO', 'transactional', 'BEGIN;COMMIT', true, null]],
  ['lock_timeout_ms', [0, -1, 1.5, 2_147_483_648, Infinity, NaN, '5000', null]],
  ['statement_timeout_ms', [0, -1, 1.5, 2_147_483_648, Infinity, NaN, '60000', null]],
  ['affected_capabilities', [null, 'schema', ['schema', 'schema'], ['schema;drop'], ['schema\n'], [{ id: 'schema' }]]],
  ['required_extensions', [null, 'unaccent', ['pg_trgm', 'pg_trgm'], ['pg_trgm;select'], ['../unaccent'], ['pg_trgm\n'], [{ name: 'pg_trgm' }]]],
  ['old_new_compatibility_evidence_sha256', [null, fixtureSource, `sha384:${'a'.repeat(96)}`, `sha256:${'A'.repeat(64)}`, `${fixtureDigest('a')}\n`]],
];
for (const [field, values] of invalidMigrationFields) {
  test(`migration rejects malformed or unsafe ${field}`, () => {
    for (const value of values) {
      const input = manifest();
      Object.assign(input.migrations.streams[0]!.migrations[0]!, { [field]: value });
      invalid('manifest', () => parseExample(input));
      invalid('migration metadata', () => parseMigrationMetadata(input.migrations));
    }
  });
}
test('both explicit transaction modes and bounded timeouts are represented without SQL inference', () => {
  for (const transaction_mode of ['TRANSACTIONAL', 'NONTRANSACTIONAL'] as const) {
    const input = manifest();
    const migration = input.migrations.streams[0]!.migrations[0]!;
    Object.assign(migration, { transaction_mode, lock_timeout_ms: 1, statement_timeout_ms: 2_147_483_647 });
    assert.deepEqual(parseMigrationMetadata(input.migrations), input.migrations);
    assert.equal(parseExample(input).migrations.streams[0]!.migrations[0]!.transaction_mode, transaction_mode);
  }
});
test('explicitly empty migration dependency lists are distinct from omitted lists', () => {
  const input = manifest();
  const migration = input.migrations.streams[0]!.migrations[0]!;
  migration.affected_capabilities = [];
  migration.required_extensions = [];
  assert.deepEqual(parseMigrationMetadata(input.migrations), input.migrations);
});
for (const template of ['../secret', '/absolute', 'watchlist/../secret', 'watchlist//price', 'watchlist/price;id', 'watchlist/price\n', 'watchlist\\price', 'watchlist/%2e%2e']) {
  test(`mail template rejects unsafe nested key ${JSON.stringify(template)}`, () => {
    const input = manifest();
    input.mail_bundle.templates[0]!.template = template;
    input.mail_bundle.templates[0]!.key = `${template}/${input.mail_bundle.templates[0]!.language}.html`;
    invalid('manifest', () => parseExample(input));
  });
}

test('complete plan, intent and exact binding; parsers do not mutate caller input', () => {
  const { plan, intent, expected } = bindingFixture();
  const before = structuredClone({ plan, intent });
  assert.deepEqual(parsePlan(plan), plan);
  assert.deepEqual(parseIntent(intent), intent);
  assertIntentBinding(intent, plan, expected);
  assert.deepEqual({ plan, intent }, before);
});
test('examples require explicit opt-in; default and false fail closed', () => {
  const { allowExamples: _allow, ...realPolicy } = exampleManifestPolicy;
  invalid('manifest', () => parseManifest(manifest(), realPolicy));
  invalid('manifest', () => parseManifest(manifest(), { ...realPolicy, allowExamples: false }));
  const candidate = manifest();
  candidate.example = false;
  assert.equal(parseManifest(candidate, realPolicy).example, false);
  // The marker is not proof: real-stage approval/provenance still belongs outside parsing.
});

const records = [
  ['manifest', exampleManifest, parseExample],
  ['plan', examplePlan, parsePlan],
  ['intent', exampleIntent(fixtureDigest('a')), parseIntent],
  ['migration metadata', exampleManifest.migrations, parseMigrationMetadata],
] as const;
for (const [label, fixture, parse] of records) {
  test(`${label}: missing/unsupported version and unknown controls fail safely`, () => {
    const missing: Record<string, unknown> = { ...fixture };
    delete missing.schema_version;
    invalid(label, () => parse(missing));
    for (const schema_version of [0, 2, '1', null]) invalid(label, () => parse({ ...fixture, schema_version }));
    for (const key of ['command', 'shell', 'url', 'model_selector', 'skip_validation']) {
      invalid(label, () => parse({ ...fixture, [key]: 'sensitive-payload-do-not-echo' }));
    }
    invalid(label, () => parse(null));
  });
}

const manifestFailures: ReadonlyArray<readonly [string, (input: ReleaseManifest) => void]> = [
  ['self digest', input => { Object.assign(input, { manifest_sha256: fixtureDigest('a') }); }],
  ['alternate self digest', input => { Object.assign(input, { digest: fixtureDigest('a') }); }],
  ['environment target', input => { Object.assign(input, { target: examplePlan.target }); }],
  ['nested command', input => { Object.assign(input.native_images[0]!, { command: 'sh -c forbidden' }); }],
  ['duplicate native component/architecture', input => { input.native_images.push(input.native_images[0]!); }],
  ['duplicate Lambda component/architecture', input => { input.lambdas.push(input.lambdas[0]!); }],
  ['missing native component', input => { input.native_images.pop(); }],
  ['missing Lambda component', input => { input.lambdas.pop(); }],
  ['duplicate worker scope', input => { input.queues[1] = input.queues[0]!; }],
  ['duplicate migration stream', input => { input.migrations.streams[1] = input.migrations.streams[0]!; }],
  ['duplicate search family', input => { input.search.families[1] = input.search.families[0]!; }],
  ['duplicate target', input => { input.targets.push(input.targets[0]!); }],
  ['target architecture mismatch', input => { input.targets[0]!.architecture = 'arm64'; }],
  ['component architecture mismatch', input => { input.native_images[0]!.architecture = 'x86_64'; }],
  ['duplicate capability', input => { input.required_capabilities.controller.push(input.required_capabilities.controller[0]!); }],
  ['duplicate mail key', input => { input.mail_bundle.templates.push(input.mail_bundle.templates[0]!); }],
  ['stage in mail prefix', input => { input.mail_bundle.prefix = `prod/${fixtureSource}/mjml`; }],
  ['mail key outside declared template', input => { input.mail_bundle.templates[0]!.key = 'other/en.html'; }],
  ['duplicate queue accepted version', input => { input.queues[0]!.accepted_versions.push(2); }],
  ['unknown emitted queue version', input => { input.queues[0]!.emitted_versions = [3]; }],
  ['missing queue schema', input => { input.queues[0]!.schemas = []; }],
  ['rollback consumer incompatibility', input => { input.queues[0]!.rollback.required_accepted_versions = [1]; }],
  ['tombstone schema incompatible', input => { input.queues[0]!.tombstone.minimum_schema_version = 3; }],
  ['queue purge control', input => { Object.assign(input.queues[0]!.rollback, { purge: true }); }],
  ['null object version', input => { input.lambdas[0]!.artifact.version_id = 'null'; }],
  ['object outside release namespace', input => { input.lambdas[0]!.artifact.key = 'other/function.zip'; }],
  ['toolchain mismatch', input => { input.infrastructure.toolchain.node = '25.0.0'; }],
  ['unknown compatibility class', input => { Object.assign(input.search.families[0]!.compatibility, { class: 'PROBABLY_COMPATIBLE' }); }],
  ['unknown backfill policy', input => { input.search.families[0]!.compatibility = { class: 'COMPATIBLE', backfill: { policy: 'NONE' } }; Object.assign(input.search.families[0]!.compatibility.backfill, { policy: 'RUN_COMMAND' }); }],
  ['unknown incompatible policy', input => { Object.assign(input.search.families[0]!, { incompatible_policy: 'REBUILD_ANYWAY' }); }],
  ['maintenance rebuild without metadata', input => { input.search.families[0]!.operation = 'MAINTENANCE_REBUILD'; }],
  ['unmarked contract migration', input => { input.migrations.streams[0]!.migrations[0]!.change = 'CONTRACT'; }],
  ['unnamed data backfill', input => { input.migrations.streams[0]!.migrations[0]!.change = 'DATA_BACKFILL'; }],
  ['duplicate migration version', input => { input.migrations.streams[1]!.migrations[1]!.version = input.migrations.streams[1]!.migrations[0]!.version; }],
  ['reordered SQLx history', input => { input.migrations.streams[1]!.migrations.reverse(); }],
];
for (const [name, change] of manifestFailures) {
  test(`manifest rejects ${name}`, () => {
    const input = manifest();
    change(input);
    invalid('manifest', () => parseExample(input));
  });
}
for (const bad of ['a'.repeat(64), `sha256:${'A'.repeat(64)}`, `sha256:${'a'.repeat(63)}`, `sha384:${'a'.repeat(96)}`, `${fixtureDigest('a')}\n`]) {
  test(`SHA-256 malformed value rejected: ${JSON.stringify(bad.slice(0, 12))}/${bad.length}`, () => {
    const input = manifest();
    input.native_images[0]!.image.digest = bad;
    invalid('manifest', () => parseExample(input));
    const plan = structuredClone(examplePlan);
    plan.inventory_sha256 = bad;
    invalid('plan', () => parsePlan(plan));
  });
}
for (const checksum of [`sha384:${'d'.repeat(95)}`, `sha384:${'D'.repeat(96)}`, fixtureDigest('d'), 'd'.repeat(96), `sha384:${'d'.repeat(96)}\n`]) {
  test(`SQLx checksum rejects wrong shape ${checksum.length}/${checksum.slice(0, 9)}`, () => {
    const input = manifest();
    input.migrations.streams[0]!.migrations[0]!.sqlx_checksum = checksum;
    invalid('manifest', () => parseExample(input));
    invalid('migration metadata', () => parseMigrationMetadata(input.migrations));
  });
}
for (const key of ['../secret', '/absolute.zip', 'lambda/../secret', 'lambda/a;id.zip', 'lambda/a$(id).zip', 'lambda/a`id`.zip', 'lambda/a|id.zip', 'lambda/a\\b.zip', 'lambda/a%2fb.zip', 'lambda/a\n.zip', 'lambda/a.zip\n']) {
  test(`unsafe object key rejected ${JSON.stringify(key)}`, () => {
    const input = manifest();
    input.lambdas[0]!.artifact.key = `releases/${fixtureSource}/${key}`;
    invalid('manifest', () => parseExample(input));
  });
}
for (const field of ['source_sha', 'build', 'native', 'lambda', 'mail', 'migration', 'search', 'infrastructure'] as const) {
  test(`source SHA must agree for ${field}`, () => {
    const input = manifest();
    const wrong = 'f'.repeat(40);
    if (field === 'source_sha') input.source_sha = wrong;
    if (field === 'build') input.build.source_sha = wrong;
    if (field === 'native') input.native_images[0]!.source_sha = wrong;
    if (field === 'lambda') input.lambdas[0]!.source_sha = wrong;
    if (field === 'mail') input.mail_bundle.source_sha = wrong;
    if (field === 'migration') input.migrations.streams[0]!.source_sha = wrong;
    if (field === 'search') input.search.families[0]!.source_sha = wrong;
    if (field === 'infrastructure') input.infrastructure.source_sha = wrong;
    invalid('manifest', () => parseExample(input));
  });
}
test('external repository/workflow/source/registry policy is exact, not suffix or manifest-selected trust', () => {
  for (const policy of [
    { ...exampleManifestPolicy, trusted_repository: 'other/repository' },
    { ...exampleManifestPolicy, trusted_workflow: '.github/workflows/other.yml' },
    { ...exampleManifestPolicy, trusted_source_sha: 'f'.repeat(40) },
    { ...exampleManifestPolicy, trusted_registries: ['other.example'] },
    { ...exampleManifestPolicy, trusted_registries: [] },
  ]) invalid('manifest', () => parseManifest(manifest(), policy));
  for (const registry of ['ghcr.io.attacker.example', 'https://ghcr.io', 'ghcr.io/user', 'ghcr.io:443', 'ghcr.io\n']) {
    const input = manifest();
    input.native_images[0]!.image.registry = registry;
    invalid('manifest', () => parseExample(input));
  }
  invalid('manifest', () => parseExample({ ...manifest(), trusted_registries: ['attacker.example'] }));
});
test('named backfill and maintenance policies are explicit represented metadata', () => {
  const input = manifest();
  input.search.families[0]!.compatibility = {
    class: 'COMPATIBLE_WITH_BACKFILL',
    backfill: { policy: 'REGISTERED', id: 'example-projection-repair', implementation_sha256: fixtureDigest('a'), input_schema_version: 1, checkpoint_schema_version: 1, gate_id: 'projection-repaired' },
  };
  assert.equal(parseExample(input).search.families[0]!.compatibility.class, 'COMPATIBLE_WITH_BACKFILL');
  input.search.families[0]!.operation = 'MAINTENANCE_REBUILD';
  input.search.families[0]!.incompatible_policy = 'REQUIRE_MAINTENANCE';
  input.search.families[0]!.compatibility = { ...input.search.families[0]!.compatibility, class: 'MAINTENANCE_REQUIRED', reason_code: 'mapping-incompatible' };
  assert.equal(parseExample(input).search.families[0]!.operation, 'MAINTENANCE_REBUILD');
  Object.assign(input.search.families[0]!.compatibility.backfill, { command: 'arbitrary-replay' });
  invalid('manifest', () => parseExample(input));
});
test('manifest and plan are never intents; kind relabeling cannot bypass strict shape', () => {
  invalid('intent', () => parseIntent(manifest()));
  invalid('intent', () => parseIntent(examplePlan));
  invalid('intent', () => parseIntent({ ...manifest(), kind: 'DEPLOYMENT_INTENT' }));
  invalid('intent', () => parseIntent({ ...exampleIntent(fixtureDigest('a')), ...manifest(), kind: 'DEPLOYMENT_INTENT' }));
});
test('examples cannot target either real stage, even with matching intent fields', () => {
  for (const stage of ['dev', 'prod'] as const) {
    const { plan, intent, expected } = bindingFixture();
    plan.target.stage = stage;
    intent.target.stage = stage;
    expected.target.stage = stage;
    invalid('plan', () => parsePlan(plan));
    invalid('intent', () => parseIntent(intent));
    invalid('intent binding', () => assertIntentBinding(intent, plan, expected));
  }
});

const planFailures: ReadonlyArray<readonly [string, (input: DeploymentPlan) => void]> = [
  ['phase order', input => { input.phases.reverse(); }],
  ['missing phase', input => { input.phases.pop(); }],
  ['duplicate dependency', input => { input.dependencies[1] = input.dependencies[0]!; }],
  ['forward dependency', input => { input.dependencies[0]!.requires_phases = ['commit_release']; }],
  ['self dependency', input => { input.dependencies[0]!.requires_phases = ['verify_release']; }],
  ['missing sequential dependency', input => { input.dependencies[1]!.requires_phases = []; }],
  ['unknown gate reference', input => { input.dependencies[0]!.requires_gates = ['not-a-gate']; }],
  ['duplicate gate', input => { input.gates.push(input.gates[0]!); }],
  ['missing safety gate', input => { input.gates.pop(); }],
  ['duplicate migration stream', input => { input.migration_diffs[1] = input.migration_diffs[0]!; }],
  ['unknown history marked compatible', input => { input.migration_diffs[0]!.history_status = 'UNKNOWN'; }],
  ['unsafe migration order', input => { input.migration_diffs[0]!.pending_versions = [2, 1]; }],
  ['understated compatibility', input => { input.migration_diffs[0]!.compatibility = { class: 'BLOCKED', backfill: { policy: 'NONE' }, reason_code: 'history-drift' }; }],
  ['unmarked destructive infra', input => { input.infrastructure_diff.changes[0]!.action = 'DELETE'; }],
  ['unmarked infra replacement', input => { input.infrastructure_diff.changes[0]!.requires_replacement = true; }],
  ['insufficient Postgres reserve', input => { input.headroom.postgres.reserve_connections = 37; }],
  ['insufficient host memory', input => { input.headroom.hosts[0]!.available_memory_mib = 1; }],
  ['insufficient WAL', input => { input.headroom.wal.available_bytes = 1; }],
  ['negative revision', input => { input.expected_environment_revision = -1; }],
  ['unsafe revision number', input => { input.expected_environment_revision = Number.MAX_SAFE_INTEGER + 1; }],
  ['invalid target stage', input => { Object.assign(input.target, { stage: 'PROD' }); }],
  ['unknown operation class', input => { Object.assign(input, { operation_class: 'RUN_SHELL' }); }],
  ['unknown plan compatibility', input => { Object.assign(input, { compatibility: 'UNKNOWN' }); }],
  ['unknown rollback strategy', input => { Object.assign(input, { rollback: { eligibility: 'INELIGIBLE', strategy: 'DOWNGRADE_SQL', reason_codes: ['unsafe'] } }); }],
];
for (const [name, change] of planFailures) {
  test(`plan rejects ${name}`, () => {
    const input = structuredClone(examplePlan);
    change(input);
    invalid('plan', () => parsePlan(input));
  });
}
test('blocked and pending plans are inspectable, never authorized', () => {
  for (const status of ['BLOCKED', 'PENDING'] as const) {
    const { plan, intent, expected } = bindingFixture();
    plan.gates[0] = { id: 'provenance', kind: 'PROVENANCE', status, reason_code: 'evidence-unavailable' };
    if (status === 'BLOCKED') {
      plan.compatibility = 'BLOCKED';
      plan.rollback = { eligibility: 'INELIGIBLE', strategy: 'REPAIR_FORWARD_ONLY', reason_codes: ['not-safe'] };
    }
    assert.deepEqual(parsePlan(plan), plan);
    intent.plan_sha256 = hashPlan(plan);
    expected.plan_sha256 = intent.plan_sha256;
    invalid('intent binding', () => assertIntentBinding(intent, plan, expected));
  }
});
test('plan backfill requires named BACKFILL gate and phase dependency', () => {
  const plan = structuredClone(examplePlan);
  plan.compatibility = 'COMPATIBLE_WITH_BACKFILL';
  plan.search_diffs[0]!.compatibility = {
    class: 'COMPATIBLE_WITH_BACKFILL',
    backfill: { policy: 'REGISTERED', id: 'example-repair', implementation_sha256: fixtureDigest('a'), input_schema_version: 1, checkpoint_schema_version: 1, gate_id: 'repaired' },
  };
  invalid('plan', () => parsePlan(plan));
  plan.gates.push({ id: 'repaired', kind: 'BACKFILL', status: 'PASSED', evidence_sha256: fixtureDigest('b') });
  invalid('plan', () => parsePlan(plan));
  plan.dependencies.find(dependency => dependency.phase === 'verify_candidates')!.requires_gates.push('repaired');
  assert.deepEqual(parsePlan(plan), plan);
});

test('backfill-class plan cannot omit the named backfill metadata', () => {
  const plan = structuredClone(examplePlan);
  plan.compatibility = 'COMPATIBLE_WITH_BACKFILL';
  invalid('plan', () => parsePlan(plan));
});
test('rollback eligibility binds the target artifact and queue safety gates', () => {
  const plan = structuredClone(examplePlan);
  assert.ok(plan.rollback.eligibility === 'ELIGIBLE');
  plan.rollback.target_manifest_sha256 = fixtureDigest('f');
  invalid('plan', () => parsePlan(plan));
  plan.rollback.target_manifest_sha256 = plan.current_manifest_sha256!;
  plan.rollback.required_gate_ids = ['rollback'];
  invalid('plan', () => parsePlan(plan));
  plan.rollback.required_gate_ids = ['rollback', 'queue-compatibility'];
  plan.operation_class = 'ROLLBACK';
  invalid('plan', () => parsePlan(plan));
  plan.rollback.target_manifest_sha256 = plan.manifest_sha256;
  assert.deepEqual(parsePlan(plan), plan);
});
test('multiarchitecture images are distinct variants, not duplicate components', () => {
  const input = manifest();
  input.native_images.push({
    ...input.native_images[0]!, architecture: 'x86_64', rust_target: 'x86_64-unknown-linux-gnu',
  });
  assert.equal(parseExample(input).native_images.length, 6);
});

for (const field of [
  'manifest_sha256', 'source_sha', 'expected_environment_revision', 'current_manifest_sha256', 'inventory_sha256',
  'configuration_sha256', 'infrastructure_context_sha256', 'operation_class', 'operation_id', 'nonce', 'plan_sha256',
] as const) {
  test(`intent exact binding rejects changed ${field}`, () => {
    const { plan, intent, expected } = bindingFixture();
    if (field === 'expected_environment_revision') intent[field] += 1;
    else if (field === 'source_sha') intent[field] = 'f'.repeat(40);
    else if (field === 'operation_class') intent[field] = 'MAINTENANCE';
    else if (field === 'operation_id') intent[field] = 'another-operation';
    else if (field === 'nonce') intent[field] = 'f'.repeat(64);
    else intent[field] = fixtureDigest('0');
    invalid('intent binding', () => assertIntentBinding(intent, plan, expected));
  });
}
for (const field of ['stage', 'account', 'region'] as const) {
  test(`target ${field} compared across intent, plan and independent trusted context`, () => {
    for (const change of ['intent', 'plan', 'trusted'] as const) {
      const { plan, intent, expected } = bindingFixture();
      const target = change === 'intent' ? intent.target : change === 'plan' ? plan.target : expected.target;
      if (field === 'stage') target.stage = 'local';
      if (field === 'account') target.account = '111111111111';
      if (field === 'region') target.region = 'us-east-1';
      invalid('intent binding', () => assertIntentBinding(intent, plan, expected));
    }
  });
}
for (const executable of ['controller', 'helper'] as const) {
  for (const field of ['sha256', 'source_sha', 'repository', 'workflow', 'run_id', 'run_attempt'] as const) {
    test(`trusted ${executable} ${field} must match exactly`, () => {
      const { plan, intent, expected } = bindingFixture();
      if (field === 'sha256') intent[executable][field] = fixtureDigest('f');
      if (field === 'source_sha') intent[executable][field] = 'f'.repeat(40);
      if (field === 'repository') intent[executable][field] = 'attacker/repository';
      if (field === 'workflow') intent[executable][field] = '.github/workflows/untrusted.yml';
      if (field === 'run_id' || field === 'run_attempt') intent[executable][field] += 1;
      invalid('intent binding', () => assertIntentBinding(intent, plan, expected));
    });
  }
}
test('changed plan body, changed live revision, and attacker-rehashed plan fail binding', () => {
  const { plan, intent, expected } = bindingFixture();
  plan.headroom.hosts[0]!.required_cpu_millis += 1;
  assert.deepEqual(parsePlan(plan), plan);
  invalid('intent binding', () => assertIntentBinding(intent, plan, expected));
  intent.plan_sha256 = hashPlan(plan);
  invalid('intent binding', () => assertIntentBinding(intent, plan, expected));
  const fresh = bindingFixture();
  fresh.expected.expected_environment_revision += 1;
  invalid('intent binding', () => assertIntentBinding(fresh.intent, fresh.plan, fresh.expected));
});
test('hasher failure or wrong digest gets only safe binding error', () => {
  const { plan, intent, expected } = bindingFixture();
  invalid('intent binding', () => assertIntentBinding(intent, plan, { ...expected, hash_plan: () => { throw new Error('secret-hasher-payload'); } }));
  invalid('intent binding', () => assertIntentBinding(intent, plan, { ...expected, hash_plan: () => 'not-a-digest' }));
});
test('object key order is immaterial; array order is not normalized away', () => {
  assert.deepEqual(parseExample(reordered(exampleManifest, true)), exampleManifest);
  const { plan, intent, expected } = bindingFixture();
  const reversed = parsePlan(reordered(plan, true));
  assert.equal(hashPlan(reversed), hashPlan(plan));
  assertIntentBinding(reordered(intent, true), reordered(plan, true), expected);
  plan.gates.reverse();
  assert.deepEqual(parsePlan(plan), plan);
  assert.notEqual(hashPlan(plan), expected.plan_sha256);
  invalid('intent binding', () => assertIntentBinding(intent, plan, expected));
});
test('exported structural schemas reject unknown keys at every object boundary', () => {
  for (const [schema, fixture] of [
    [ReleaseManifest, exampleManifest], [DeploymentPlan, examplePlan],
    [DeploymentIntent, exampleIntent(fixtureDigest('a'))], [MigrationMetadata, exampleManifest.migrations],
  ] as const) {
    assert.equal(schema.safeParse(fixture).success, true);
    function visit(value: unknown, path: string[]): void {
      if (value === null || typeof value !== 'object') return;
      if (!Array.isArray(value)) {
        const changed: unknown = structuredClone(fixture);
        let cursor = changed as Record<string, unknown>;
        for (const key of path) cursor = cursor[key] as Record<string, unknown>;
        cursor.unrecognized_control = 'do-not-echo';
        assert.equal(schema.safeParse(changed).success, false, path.join('.'));
      }
      for (const [key, child] of Object.entries(value)) visit(child, [...path, key]);
    }
    visit(fixture, []);
  }
});
test('generated schemas match the sole typed structural source, with no opaque objects', async () => {
  function visit(value: unknown): void {
    if (value === null || typeof value !== 'object') return;
    if (Array.isArray(value)) { value.forEach(visit); return; }
    const node = value as Record<string, unknown>;
    if (node.type === 'object') assert.equal(node.additionalProperties, false);
    Object.values(node).forEach(visit);
  }
  const documents = releaseSchemaDocuments();
  assert.equal(documents.length, 4);
  for (const { name, document } of documents) {
    visit(document);
    const generated = await readFile(new URL(`../../../schemas/${name}.schema.json`, import.meta.url), 'utf8');
    assert.equal(generated, `${JSON.stringify(document, null, 2)}\n`);
    assert.deepEqual(JSON.parse(generated), document);
  }
});
