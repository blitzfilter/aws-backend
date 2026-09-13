import { PhaseId, WorkerScope } from '../src/contracts/primitives.js';
import type { DeploymentIntent, DeploymentPlan, ManifestPolicy, ReleaseManifest } from '../src/contracts/release.js';

// Deliberately synthetic identities and checksums; no artifact, provenance, approval,
// capacity, engine version or platform capability here is evidence for any live stage.
export const fixtureDigest = (digit: string): string => `sha256:${digit.repeat(64)}`;
export const fixtureSource = 'a'.repeat(40);
const artifact = (key: string) => ({
  bucket: 'example-release-artifacts', key: `releases/${fixtureSource}/${key}`,
  version_id: 'example-version-1', sha256: fixtureDigest('b'),
});
const compatible = { class: 'COMPATIBLE', backfill: { policy: 'NONE' } } as const;
const migrations = (versions: readonly number[], name: string, extensions: readonly string[]) => versions.map((version, index) => ({
  version, name: `${name}-${index + 1}`, sql_sha256: fixtureDigest('c'), sqlx_checksum: `sha384:${'d'.repeat(96)}`,
  transaction_mode: 'TRANSACTIONAL' as const, lock_timeout_ms: 5000, statement_timeout_ms: 60000,
  affected_capabilities: [`${name}-schema`], required_extensions: [...extensions],
  old_new_compatibility_evidence_sha256: fixtureDigest('8'),
  change: 'EXPAND' as const, compatibility: compatible, rollback: 'PREVIOUS_BINARY_COMPATIBLE' as const,
}));

export const exampleManifest: ReleaseManifest = {
  schema_version: 1, kind: 'RELEASE_MANIFEST', example: true,
  release_id: 'release-example-2026-09-12-01', source_sha: fixtureSource,
  build: {
    repository: 'example-owner/example-repository', workflow: '.github/workflows/build-release.yml',
    run_id: 1001, run_attempt: 1, source_sha: fixtureSource,
    provenance: { kind: 'GITHUB_ATTESTATION', artifact: artifact('provenance/build.json') },
  },
  toolchain: {
    rust: '1.98.0', node: '26.0.0', typescript: '6.0.3', sqlx: '0.9.0',
    cargo_lock_sha256: fixtureDigest('1'), deployment_lock_sha256: fixtureDigest('2'),
  },
  targets: [
    { architecture: 'x86_64', rust_target: 'x86_64-unknown-linux-gnu' },
    { architecture: 'arm64', rust_target: 'aarch64-unknown-linux-gnu' },
  ],
  native_images: (['aura-historia-api', 'aura-historia-worker', 'aura-historia-cron', 'crawler', 'aura-historia-migrate'] as const).map(component => ({
    component, architecture: 'arm64', rust_target: 'aarch64-unknown-linux-gnu', source_sha: fixtureSource,
    image: { registry: 'ghcr.io', repository: `example-owner/${component}`, digest: fixtureDigest('3') },
  })),
  lambdas: (['cloudwatch-log-retention-lambda', 'cognito-post-confirmation', 'shopify-lambda', 'stripe-lambda', 'fxrate-lambda'] as const).map(component => ({
    component, architecture: 'x86_64', rust_target: 'x86_64-unknown-linux-gnu', source_sha: fixtureSource,
    runtime: 'provided.al2023', artifact: artifact(`lambda/${component}.zip`),
  })),
  mail_bundle: {
    source_sha: fixtureSource, artifact: artifact('mail/templates.tar'),
    prefix: `${fixtureSource}/mjml`, prefix_semantics: 'PREPEND_TARGET_STAGE',
    templates: [
      'partnership-application/approval', 'partnership-application/rejection', 'search-filter/match',
      'watchlist/product-update/availability', 'watchlist/product-update/price',
    ].flatMap(template =>
      (['de', 'en', 'es', 'fr', 'it'] as const).map(language => ({
        template, language, key: `${template}/${language}.html`, sha256: fixtureDigest('4'),
      }))),
  },
  migrations: {
    schema_version: 1,
    streams: [
      { stream: 'business', source_sha: fixtureSource, artifact: artifact('migrations/business.tar'), migrations: migrations([20260725090000], 'example-business', ['pg_trgm', 'unaccent', 'pg_ttl_index']) },
      {
        stream: 'crawler', source_sha: fixtureSource, artifact: artifact('migrations/crawler.tar'),
        migrations: migrations([20260101000000, 20260514000000, 20260711000000, 20260817000000, 20260831000000, 20260831000001], 'example-crawler', []),
      },
    ],
  },
  search: {
    engine: { version: '3.0.0', required_plugins: ['opensearch-knn'] },
    families: (['PRODUCT_LISTINGS', 'USER_SEARCH_FILTERS'] as const).map((family, index) => ({
      family, source_sha: fixtureSource, definition: artifact(`search/family-${index + 1}/definition.json`),
      analysis_assets: ['english-synonyms', 'german-synonyms', 'french-synonyms', 'spanish-synonyms', 'italian-synonyms'].map(name => ({
        name, artifact: artifact(`search/analysis/${name}.txt`),
      })),
      operation: 'ADDITIVE', incompatible_policy: 'BLOCK', compatibility: compatible,
      operation_metadata: { schema_version: 1, artifact: artifact(`search/family-${index + 1}/operation.json`) },
      projection: { schema_version: 2, versioning: 'EXTERNAL', tombstone: 'CONTENT_FREE_PROJECTION_DELETED', rollback: 'REPAIR_FORWARD_ONLY' },
    })),
  },
  infrastructure: {
    source_sha: fixtureSource, source: artifact('infrastructure/source.tar'), assembly: artifact('infrastructure/assembly.tar'),
    toolchain: { node: '26.0.0', typescript: '6.0.3', cdk: '2.200.0', lock_sha256: fixtureDigest('5') },
    context: { kind: 'NONSECRET_CONTEXT', schema_version: 1, sha256: fixtureDigest('6') },
  },
  required_capabilities: {
    controller: [{ id: 'exact-intent-binding', minimum_version: 1 }],
    platform: [{ id: 'immutable-artifact-staging', minimum_version: 1 }, { id: 'postgres-sqlx-migrate', minimum_version: 1 }],
  },
  queues: WorkerScope.options.map(scope => ({
    scope, accepted_versions: [2], emitted_versions: [2], schemas: [{ version: 2, sha256: fixtureDigest('7') }],
    tombstone: { policy: 'CONTENT_FREE_PROJECTION_DELETED', minimum_schema_version: 2 },
    rollback: { policy: 'PREVIOUS_CONSUMERS_ACCEPT_EMITTED', required_accepted_versions: [2], preserve_queued_messages: true },
  })),
};

export const exampleManifestPolicy: ManifestPolicy = {
  trusted_repository: exampleManifest.build.repository, trusted_workflow: exampleManifest.build.workflow,
  trusted_registries: ['ghcr.io'], trusted_source_sha: fixtureSource, allowExamples: true,
};
const gateKinds = [
  'PROVENANCE', 'BOOTSTRAP', 'MIGRATION_HISTORY', 'QUEUE_COMPATIBILITY', 'SEARCH_COMPATIBILITY',
  'CAPACITY', 'ROLLBACK', 'CAPABILITIES',
] as const;
const gates: DeploymentPlan['gates'] = gateKinds.map(kind => ({
  id: kind.toLowerCase().replaceAll('_', '-'), kind, status: 'PASSED', evidence_sha256: fixtureDigest('8'),
}));
export const examplePlan: DeploymentPlan = {
  schema_version: 1, kind: 'DEPLOYMENT_PLAN', example: true,
  target: { stage: 'test', account: '000000000000', region: 'eu-central-1' },
  manifest_sha256: fixtureDigest('9'), source_sha: fixtureSource,
  expected_environment_revision: 7, current_manifest_sha256: fixtureDigest('a'),
  inventory_sha256: fixtureDigest('b'), configuration_sha256: fixtureDigest('c'), infrastructure_context_sha256: fixtureDigest('6'),
  operation_class: 'STANDARD', compatibility: 'COMPATIBLE',
  infrastructure_diff: {
    current_assembly_sha256: fixtureDigest('d'), desired_assembly_sha256: exampleManifest.infrastructure.assembly.sha256,
    changes: [{ stack: 'COMPUTE', action: 'UPDATE', change_set_sha256: fixtureDigest('e'), requires_replacement: false, compatibility: compatible }],
  },
  migration_diffs: exampleManifest.migrations.streams.map(stream => ({
    stream: stream.stream, observed_history_sha256: fixtureDigest('f'), desired_metadata_sha256: fixtureDigest('1'),
    pending_versions: [], history_status: 'MATCHED', compatibility: compatible,
  })),
  search_diffs: exampleManifest.search.families.map(family => ({
    family: family.family, current_definition_sha256: fixtureDigest('2'), desired_definition_sha256: family.definition.sha256,
    operation: 'ADDITIVE', incompatible_policy: 'BLOCK', compatibility: compatible,
  })),
  headroom: {
    postgres: { available_connections: 100, required_connections: 64, reserve_connections: 16 },
    wal: { available_bytes: 1073741824, required_bytes: 536870912 },
    hosts: [{
      host_id: 'example-host', available_cpu_millis: 8000, required_cpu_millis: 4000,
      available_memory_mib: 16384, required_memory_mib: 8192, available_disk_mib: 102400, required_disk_mib: 51200,
    }],
  },
  gates,
  dependencies: PhaseId.options.map((phase, index) => ({
    phase, requires_phases: index === 0 ? [] : [PhaseId.options[index - 1]!],
    requires_gates: phase === 'revalidate_plan' ? gates.map(gate => gate.id) : [],
  })),
  rollback: { eligibility: 'ELIGIBLE', target_manifest_sha256: fixtureDigest('a'), required_gate_ids: ['rollback', 'queue-compatibility'] },
  phases: [...PhaseId.options],
};

export function exampleIntent(planSha256: string): DeploymentIntent {
  return {
    schema_version: 1, kind: 'DEPLOYMENT_INTENT', example: true,
    target: structuredClone(examplePlan.target), manifest_sha256: examplePlan.manifest_sha256, source_sha: fixtureSource,
    expected_environment_revision: examplePlan.expected_environment_revision, current_manifest_sha256: examplePlan.current_manifest_sha256,
    inventory_sha256: examplePlan.inventory_sha256, configuration_sha256: examplePlan.configuration_sha256,
    infrastructure_context_sha256: examplePlan.infrastructure_context_sha256, operation_class: examplePlan.operation_class,
    operation_id: 'example-operation-01', nonce: 'e'.repeat(64), plan_sha256: planSha256,
    controller: {
      sha256: fixtureDigest('3'), source_sha: 'b'.repeat(40), repository: 'example-owner/trusted-control',
      workflow: '.github/workflows/controller.yml', run_id: 2001, run_attempt: 1,
    },
    helper: {
      sha256: fixtureDigest('4'), source_sha: 'c'.repeat(40), repository: 'example-owner/trusted-platform',
      workflow: '.github/workflows/helper.yml', run_id: 3001, run_attempt: 2,
    },
  };
}
