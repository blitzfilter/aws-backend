import { z } from 'zod';
import {
  Account, Architecture, Compatibility, DatabaseTarget, Digest, Identifier, PhaseId,
  Positive, Region, Revision, SafeKey, SchemaVersion, Sha, SqlxChecksum, Stage, WorkerScope,
  isRealStage,
} from './primitives.js';

// These schemas are the structural JSON Schema source. The parse functions additionally
// enforce cross-record/collection invariants, SafeKey refinements and trust policy.
// Neither shape, digest equality nor claimed provenance verifies attestations or approval.
const Repository = z.string().regex(/^[A-Za-z0-9][A-Za-z0-9_.-]*\/[A-Za-z0-9][A-Za-z0-9_.-]*$/).max(200);
const Workflow = z.string().regex(/^\.github\/workflows\/[a-zA-Z0-9_-]+\.ya?ml$/).max(160);
const Registry = z.string().regex(/^[a-z0-9]+(?:[.-][a-z0-9]+)*\.[a-z]{2,}$/).max(253);
const Version = z.string().regex(/^[0-9]+\.[0-9]+\.[0-9]+$/).max(32);
const Bucket = z.string().regex(/^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$/);
const ObjectVersion = z.string().regex(/^[A-Za-z0-9_+/.=-]+$/).max(1024);
const Target = z.strictObject({ stage: Stage, account: Account, region: Region });
const RustTarget = z.enum([
  'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu',
  'x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl',
]);
const NativeComponent = z.enum([
  'aura-historia-api', 'aura-historia-worker', 'aura-historia-cron', 'crawler', 'aura-historia-migrate',
]);
const LambdaComponent = z.enum([
  'cloudwatch-log-retention-lambda', 'cognito-post-confirmation', 'shopify-lambda', 'stripe-lambda', 'fxrate-lambda',
]);
const SearchFamily = z.enum(['PRODUCT_LISTINGS', 'USER_SEARCH_FILTERS']);
const Language = z.enum(['de', 'en', 'es', 'fr', 'it']);
const Capability = z.strictObject({ id: Identifier, minimum_version: Positive });
const ObjectReference = z.strictObject({ bucket: Bucket, key: SafeKey, version_id: ObjectVersion, sha256: Digest })
  .describe('Immutable object reference, not fetch authorization. Semantic checks reject null versions and keys outside releases/<source_sha>/.');
const BuildIdentity = z.strictObject({
  repository: Repository, workflow: Workflow, run_id: Positive, run_attempt: Positive, source_sha: Sha,
});
const ExecutableIdentity = BuildIdentity.extend({ sha256: Digest });
const NoBackfill = z.strictObject({ policy: z.literal('NONE') });
const RegisteredBackfill = z.strictObject({
  policy: z.literal('REGISTERED'), id: Identifier, implementation_sha256: Digest,
  input_schema_version: Positive, checkpoint_schema_version: Positive, gate_id: Identifier,
}).describe('Named implementation only; a future trusted registry must resolve and approve this identity. No replay command or arbitrary parameters.');
const Backfill = z.discriminatedUnion('policy', [NoBackfill, RegisteredBackfill]);
const CompatibilityPolicy = z.discriminatedUnion('class', [
  z.strictObject({ class: Compatibility.extract(['COMPATIBLE']), backfill: NoBackfill }),
  z.strictObject({ class: Compatibility.extract(['COMPATIBLE_WITH_BACKFILL']), backfill: RegisteredBackfill }),
  z.strictObject({ class: Compatibility.extract(['MAINTENANCE_REQUIRED']), backfill: Backfill, reason_code: Identifier }),
  z.strictObject({ class: Compatibility.extract(['BLOCKED']), backfill: Backfill, reason_code: Identifier }),
]);
const MigrationTimeoutMs = Positive.max(2_147_483_647)
  .describe('Positive PostgreSQL timeout in milliseconds, at most INT_MAX. Zero (disabled timeout) is not allowed.');
const ExtensionName = z.string().regex(/^[a-z][a-z0-9_-]*$/).max(63);
const Migration = z.strictObject({
  version: Positive, name: Identifier, sql_sha256: Digest, sqlx_checksum: SqlxChecksum,
  transaction_mode: z.enum(['TRANSACTIONAL', 'NONTRANSACTIONAL']),
  lock_timeout_ms: MigrationTimeoutMs, statement_timeout_ms: MigrationTimeoutMs,
  affected_capabilities: z.array(Identifier), required_extensions: z.array(ExtensionName),
  old_new_compatibility_evidence_sha256: Digest,
  change: z.enum(['EXPAND', 'CONTRACT', 'DATA_BACKFILL']), compatibility: CompatibilityPolicy,
  rollback: z.enum(['PREVIOUS_BINARY_COMPATIBLE', 'REPAIR_FORWARD_ONLY']),
}).describe('Explicit execution controls and author-declared dependencies; empty lists mean explicitly none. Old/new compatibility requires separate evidence identity, never SQL inference. An evidence digest is not evidence verification.');
export const MigrationMetadata = z.strictObject({
  schema_version: SchemaVersion,
  streams: z.array(z.strictObject({
    stream: DatabaseTarget, source_sha: Sha, artifact: ObjectReference,
    migrations: z.array(Migration).min(1),
  })).length(2),
}).describe('Two separate SQLx histories: business and crawler. parseMigrationMetadata also checks unique streams/dependency lists, ascending versions, source-key binding and explicit compatibility. SHA-384 is SQLx history identity; SHA-256 is artifact/evidence identity. Execution controls and old/new compatibility evidence are mandatory per migration.');
export type MigrationMetadata = z.infer<typeof MigrationMetadata>;

const SearchOperation = z.enum(['NO_CHANGE', 'ADDITIVE', 'MAINTENANCE_REBUILD', 'BLOCKED']);
const IncompatibleSearchPolicy = z.enum(['BLOCK', 'REQUIRE_MAINTENANCE']);
const SearchChange = {
  family: SearchFamily, operation: SearchOperation, compatibility: CompatibilityPolicy,
  incompatible_policy: IncompatibleSearchPolicy,
};
const QueueContract = z.strictObject({
  scope: WorkerScope,
  accepted_versions: z.array(Positive).min(1), emitted_versions: z.array(Positive).min(1),
  schemas: z.array(z.strictObject({ version: Positive, sha256: Digest })).min(1),
  tombstone: z.strictObject({ policy: z.literal('CONTENT_FREE_PROJECTION_DELETED'), minimum_schema_version: Positive }),
  rollback: z.strictObject({
    policy: z.enum(['PREVIOUS_CONSUMERS_ACCEPT_EMITTED', 'REPAIR_FORWARD_ONLY']),
    required_accepted_versions: z.array(Positive).min(1), preserve_queued_messages: z.literal(true),
  }),
});
export const ReleaseManifest = z.strictObject({
  schema_version: SchemaVersion, kind: z.literal('RELEASE_MANIFEST'), example: z.boolean(),
  release_id: Identifier, source_sha: Sha,
  build: BuildIdentity.extend({
    provenance: z.strictObject({ kind: z.literal('GITHUB_ATTESTATION'), artifact: ObjectReference }),
  }),
  toolchain: z.strictObject({
    rust: Version, node: Version, typescript: Version, sqlx: Version,
    cargo_lock_sha256: Digest, deployment_lock_sha256: Digest,
  }),
  targets: z.array(z.strictObject({ architecture: Architecture, rust_target: RustTarget })).min(1),
  native_images: z.array(z.strictObject({
    component: NativeComponent, architecture: Architecture, rust_target: RustTarget, source_sha: Sha,
    image: z.strictObject({
      registry: Registry,
      repository: SafeKey.regex(/^[a-z0-9]+(?:[._-][a-z0-9]+)*(?:\/[a-z0-9]+(?:[._-][a-z0-9]+)*)*$/),
      digest: Digest,
    }),
  })).min(NativeComponent.options.length),
  lambdas: z.array(z.strictObject({
    component: LambdaComponent, architecture: Architecture, rust_target: RustTarget, source_sha: Sha,
    runtime: z.literal('provided.al2023'), artifact: ObjectReference,
  })).min(5),
  mail_bundle: z.strictObject({
    source_sha: Sha, artifact: ObjectReference, prefix: SafeKey,
    prefix_semantics: z.literal('PREPEND_TARGET_STAGE'),
    templates: z.array(z.strictObject({ template: SafeKey, language: Language, key: SafeKey, sha256: Digest })).min(1),
  }).describe('Environment neutral: prefix MUST be <source_sha>/mjml; template key MUST be <template>/<language>.html. Activation prepends the separately approved target stage. Archive digest covers exact bundle bytes.'),
  migrations: MigrationMetadata,
  search: z.strictObject({
    engine: z.strictObject({ version: Version, required_plugins: z.array(Identifier).min(1) }),
    families: z.array(z.strictObject({
      ...SearchChange, source_sha: Sha, definition: ObjectReference,
      analysis_assets: z.array(z.strictObject({ name: Identifier, artifact: ObjectReference })).min(1),
      operation_metadata: z.strictObject({ schema_version: SchemaVersion, artifact: ObjectReference }),
      projection: z.strictObject({
        schema_version: Positive, versioning: z.literal('EXTERNAL'),
        tombstone: z.literal('CONTENT_FREE_PROJECTION_DELETED'), rollback: z.literal('REPAIR_FORWARD_ONLY'),
      }),
    })).length(2),
  }),
  infrastructure: z.strictObject({
    source_sha: Sha, source: ObjectReference, assembly: ObjectReference,
    toolchain: z.strictObject({ node: Version, typescript: Version, cdk: Version, lock_sha256: Digest }),
    context: z.strictObject({ kind: z.literal('NONSECRET_CONTEXT'), schema_version: SchemaVersion, sha256: Digest }),
  }),
  required_capabilities: z.strictObject({ controller: z.array(Capability).min(1), platform: z.array(Capability).min(1) }),
  queues: z.array(QueueContract).length(WorkerScope.options.length),
}).describe('Environment-neutral immutable build record, NOT deployment intent or attestation verification. No self digest. parseManifest checks structure, source policy and internal consistency, not full bundle completeness or build availability. Callers must separately run verifyManifestCatalog(manifest, trustedCatalog). JSON Schema alone does not enforce semantic invariants.');
export type ReleaseManifest = z.infer<typeof ReleaseManifest>;

const OperationClass = z.enum(['STANDARD', 'MAINTENANCE', 'REPAIR_FORWARD', 'ROLLBACK']);
const PlanBinding = {
  target: Target, manifest_sha256: Digest, source_sha: Sha,
  expected_environment_revision: Revision, current_manifest_sha256: Digest.nullable(),
  inventory_sha256: Digest, configuration_sha256: Digest, infrastructure_context_sha256: Digest,
  operation_class: OperationClass,
};
const GateKind = z.enum([
  'PROVENANCE', 'BOOTSTRAP', 'MIGRATION_HISTORY', 'QUEUE_COMPATIBILITY', 'SEARCH_COMPATIBILITY',
  'BACKFILL', 'CAPACITY', 'ROLLBACK', 'CAPABILITIES',
]);
const GateIdentity = { id: Identifier, kind: GateKind };
const Gate = z.discriminatedUnion('status', [
  z.strictObject({ ...GateIdentity, status: z.literal('PASSED'), evidence_sha256: Digest }),
  z.strictObject({ ...GateIdentity, status: z.literal('PENDING'), reason_code: Identifier }),
  z.strictObject({ ...GateIdentity, status: z.literal('BLOCKED'), reason_code: Identifier }),
]);
export const DeploymentPlan = z.strictObject({
  schema_version: SchemaVersion, kind: z.literal('DEPLOYMENT_PLAN'), example: z.boolean(), ...PlanBinding,
  compatibility: Compatibility,
  infrastructure_diff: z.strictObject({
    current_assembly_sha256: Digest.nullable(), desired_assembly_sha256: Digest,
    changes: z.array(z.strictObject({
      stack: z.enum(['DATA', 'COMPUTE', 'API', 'OBSERVABILITY']),
      action: z.enum(['CREATE', 'UPDATE', 'DELETE']), change_set_sha256: Digest,
      requires_replacement: z.boolean(), compatibility: CompatibilityPolicy,
    })),
  }),
  migration_diffs: z.array(z.strictObject({
    stream: DatabaseTarget, observed_history_sha256: Digest, desired_metadata_sha256: Digest,
    pending_versions: z.array(Positive), history_status: z.enum(['MATCHED', 'UNKNOWN', 'DRIFTED']),
    compatibility: CompatibilityPolicy,
  })).length(2),
  search_diffs: z.array(z.strictObject({
    ...SearchChange, current_definition_sha256: Digest.nullable(), desired_definition_sha256: Digest,
  })).length(2),
  headroom: z.strictObject({
    postgres: z.strictObject({ available_connections: Revision, required_connections: Revision, reserve_connections: Revision }),
    wal: z.strictObject({ available_bytes: Revision, required_bytes: Revision }),
    hosts: z.array(z.strictObject({
      host_id: Identifier, available_cpu_millis: Revision, required_cpu_millis: Revision,
      available_memory_mib: Revision, required_memory_mib: Revision,
      available_disk_mib: Revision, required_disk_mib: Revision,
    })).min(1),
  }),
  gates: z.array(Gate).min(1),
  dependencies: z.array(z.strictObject({
    phase: PhaseId, requires_phases: z.array(PhaseId), requires_gates: z.array(Identifier),
  })).length(PhaseId.options.length),
  rollback: z.discriminatedUnion('eligibility', [
    z.strictObject({
      eligibility: z.literal('ELIGIBLE'), target_manifest_sha256: Digest,
      required_gate_ids: z.array(Identifier).min(1),
    }),
    z.strictObject({ eligibility: z.literal('INELIGIBLE'), strategy: z.literal('REPAIR_FORWARD_ONLY'), reason_codes: z.array(Identifier).min(1) }),
  ]),
  phases: z.array(PhaseId).length(PhaseId.options.length),
}).describe('Plan digest is external. parsePlan checks ordered phases/dependencies, unique metadata, named backfills, gate references, rollback target binding and conservative compatibility. Blocked/pending plans may be inspected but cannot pass assertIntentBinding.');
export type DeploymentPlan = z.infer<typeof DeploymentPlan>;

export const DeploymentIntent = z.strictObject({
  schema_version: SchemaVersion, kind: z.literal('DEPLOYMENT_INTENT'), example: z.boolean(), ...PlanBinding,
  operation_id: Identifier, nonce: z.string().regex(/^[0-9a-f]{64}$/), plan_sha256: Digest,
  controller: ExecutableIdentity, helper: ExecutableIdentity,
}).describe('Exact operation authorization claim, not proof of approval. assertIntentBinding requires independent trusted context and a canonical plan hasher. Protected approval-store identity, provenance verification and atomic nonce/revision enforcement remain mandatory outside this pure module.');
export type DeploymentIntent = z.infer<typeof DeploymentIntent>;

const ManifestPolicy = z.strictObject({
  trusted_repository: Repository, trusted_workflow: Workflow, trusted_registries: z.array(Registry).min(1),
  trusted_source_sha: Sha, allowExamples: z.boolean().optional(),
});
export type ManifestPolicy = Omit<z.infer<typeof ManifestPolicy>, 'trusted_registries'> & { trusted_registries: readonly string[] };
const IntentContext = DeploymentIntent.omit({ schema_version: true, kind: true, example: true });
export type TrustedIntentContext = z.infer<typeof IntentContext> & {
  // Inject the integrator-owned canonical SHA-256 function, never a function from JSON.
  // plan_sha256 above must come from the protected approval, not this calculation.
  hash_plan: (plan: DeploymentPlan) => string;
};

function requireValid(condition: boolean): asserts condition {
  if (!condition) throw new Error('invalid contract');
}
function unique<T>(items: readonly T[], key: (item: T) => string | number): void {
  requireValid(new Set(items.map(key)).size === items.length);
}
function sameMembers<T>(items: readonly T[], expected: readonly T[]): void {
  unique(items, item => String(item));
  requireValid(items.length === expected.length && expected.every(item => items.includes(item)));
}
function ascending(versions: readonly number[]): void {
  requireValid(versions.every((version, index) => index === 0 || version > versions[index - 1]!));
}
function cleanStrings(input: unknown): void {
  // Shared regex primitives use JS end anchors; also reject trailing newlines/control bytes.
  if (typeof input === 'string') requireValid(!/[\u0000-\u001f\u007f]/.test(input));
  else if (Array.isArray(input)) input.forEach(cleanStrings);
  else if (input !== null && typeof input === 'object') Object.values(input).forEach(cleanStrings);
}
function safely<T>(label: string, work: () => T): T {
  try {
    const value = work();
    cleanStrings(value);
    return value;
  } catch {
    // No raw Zod errors, causes, registry names, keys, payloads or attacker fields escape.
    throw new Error(`invalid ${label}`);
  }
}
function objectSource(object: z.infer<typeof ObjectReference>, sha: string): void {
  requireValid(object.version_id !== 'null' && object.key.startsWith(`releases/${sha}/`));
}
function checkSearch(change: z.infer<typeof DeploymentPlan>['search_diffs'][number] | ReleaseManifest['search']['families'][number]): void {
  if (change.operation === 'BLOCKED') requireValid(change.compatibility.class === 'BLOCKED');
  if (change.operation === 'MAINTENANCE_REBUILD') {
    requireValid(change.incompatible_policy === 'REQUIRE_MAINTENANCE');
    requireValid(['MAINTENANCE_REQUIRED', 'BLOCKED'].includes(change.compatibility.class));
    requireValid(change.compatibility.backfill.policy === 'REGISTERED');
  }
  if (change.operation === 'NO_CHANGE') requireValid(change.compatibility.class === 'COMPATIBLE');
}
function checkMigrations(metadata: MigrationMetadata): void {
  sameMembers(metadata.streams.map(stream => stream.stream), DatabaseTarget.options);
  for (const stream of metadata.streams) {
    objectSource(stream.artifact, stream.source_sha);
    ascending(stream.migrations.map(migration => migration.version));
    unique(stream.migrations, migration => migration.name);
    for (const migration of stream.migrations) {
      unique(migration.affected_capabilities, capability => capability);
      unique(migration.required_extensions, extension => extension);
      if (migration.change === 'CONTRACT') {
        requireValid(['MAINTENANCE_REQUIRED', 'BLOCKED'].includes(migration.compatibility.class));
        requireValid(migration.rollback === 'REPAIR_FORWARD_ONLY');
      }
      if (migration.change === 'DATA_BACKFILL') requireValid(migration.compatibility.backfill.policy === 'REGISTERED');
      if (migration.compatibility.class === 'BLOCKED') requireValid(migration.rollback === 'REPAIR_FORWARD_ONLY');
    }
  }
}
export function parseMigrationMetadata(input: unknown): MigrationMetadata {
  return safely('migration metadata', () => {
    const metadata = MigrationMetadata.parse(input);
    checkMigrations(metadata);
    return metadata;
  });
}

/**
 * Structural/source-policy validation and internal consistency, not full bundle completeness.
 * Callers must separately run the integrator-owned verifyManifestCatalog(manifest, trustedCatalog)
 * gate for catalog completeness and build availability, including the iteration-05 migrator.
 * Neither gate replaces cryptographic provenance verification or protected approval.
 */
export function parseManifest(input: unknown, policy: ManifestPolicy): ReleaseManifest {
  return safely('manifest', () => {
    const trusted = ManifestPolicy.parse(policy);
    cleanStrings(trusted);
    const manifest = ReleaseManifest.parse(input);
    requireValid(!manifest.example || trusted.allowExamples === true);
    requireValid(manifest.source_sha === trusted.trusted_source_sha);
    requireValid(manifest.build.repository === trusted.trusted_repository && manifest.build.workflow === trusted.trusted_workflow);
    requireValid(manifest.build.source_sha === manifest.source_sha);
    objectSource(manifest.build.provenance.artifact, manifest.source_sha);
    unique(manifest.targets, target => target.rust_target);
    for (const target of manifest.targets) {
      requireValid(target.rust_target.startsWith(target.architecture === 'arm64' ? 'aarch64-' : 'x86_64-'));
    }
    const components = [...manifest.native_images, ...manifest.lambdas];
    unique(components, component => `${component.component}/${component.architecture}`);
    for (const component of components) {
      requireValid(component.source_sha === manifest.source_sha);
      requireValid(manifest.targets.some(target => target.rust_target === component.rust_target && target.architecture === component.architecture));
    }
    requireValid(NativeComponent.options.every(component => manifest.native_images.some(image => image.component === component)));
    requireValid(LambdaComponent.options.every(component => manifest.lambdas.some(lambda => lambda.component === component)));
    for (const native of manifest.native_images) requireValid(trusted.trusted_registries.includes(native.image.registry));
    for (const lambda of manifest.lambdas) objectSource(lambda.artifact, manifest.source_sha);
    const mail = manifest.mail_bundle;
    requireValid(mail.source_sha === manifest.source_sha && mail.prefix === `${manifest.source_sha}/mjml`);
    objectSource(mail.artifact, manifest.source_sha);
    unique(mail.templates, template => template.key);
    for (const template of mail.templates) requireValid(template.key === `${template.template}/${template.language}.html`);
    checkMigrations(manifest.migrations);
    requireValid(manifest.migrations.streams.every(stream => stream.source_sha === manifest.source_sha));
    sameMembers(manifest.search.families.map(family => family.family), SearchFamily.options);
    unique(manifest.search.engine.required_plugins, plugin => plugin);
    for (const family of manifest.search.families) {
      checkSearch(family);
      requireValid(family.source_sha === manifest.source_sha);
      objectSource(family.definition, manifest.source_sha);
      objectSource(family.operation_metadata.artifact, manifest.source_sha);
      unique(family.analysis_assets, asset => asset.name);
      unique(family.analysis_assets, asset => `${asset.artifact.bucket}/${asset.artifact.key}/${asset.artifact.version_id}`);
      for (const asset of family.analysis_assets) objectSource(asset.artifact, manifest.source_sha);
    }
    const infra = manifest.infrastructure;
    requireValid(infra.source_sha === manifest.source_sha);
    objectSource(infra.source, manifest.source_sha);
    objectSource(infra.assembly, manifest.source_sha);
    requireValid(infra.toolchain.node === manifest.toolchain.node && infra.toolchain.typescript === manifest.toolchain.typescript);
    for (const capabilities of Object.values(manifest.required_capabilities)) unique(capabilities, capability => capability.id);
    sameMembers(manifest.queues.map(queue => queue.scope), WorkerScope.options);
    for (const queue of manifest.queues) {
      unique(queue.accepted_versions, version => version);
      unique(queue.emitted_versions, version => version);
      unique(queue.schemas, schema => schema.version);
      unique(queue.rollback.required_accepted_versions, version => version);
      sameMembers(queue.schemas.map(schema => schema.version), queue.accepted_versions);
      requireValid(queue.emitted_versions.every(version => queue.accepted_versions.includes(version)));
      requireValid(queue.accepted_versions.every(version => version >= queue.tombstone.minimum_schema_version));
      if (queue.rollback.policy === 'PREVIOUS_CONSUMERS_ACCEPT_EMITTED') {
        requireValid(queue.emitted_versions.every(version => queue.rollback.required_accepted_versions.includes(version)));
      }
    }
    return manifest;
  });
}

const requiredGateKinds = GateKind.options.filter(kind => kind !== 'BACKFILL');
function sufficientHeadroom(plan: DeploymentPlan): boolean {
  const { postgres, wal, hosts } = plan.headroom;
  return postgres.required_connections <= postgres.available_connections - postgres.reserve_connections
    && wal.required_bytes <= wal.available_bytes
    && hosts.every(host => host.required_cpu_millis <= host.available_cpu_millis
      && host.required_memory_mib <= host.available_memory_mib && host.required_disk_mib <= host.available_disk_mib);
}
function checkPlan(plan: DeploymentPlan): void {
  requireValid(!plan.example || !isRealStage(plan.target.stage));
  sameMembers(plan.migration_diffs.map(diff => diff.stream), DatabaseTarget.options);
  sameMembers(plan.search_diffs.map(diff => diff.family), SearchFamily.options);
  unique(plan.infrastructure_diff.changes, change => change.stack);
  unique(plan.headroom.hosts, host => host.host_id);
  unique(plan.gates, gate => gate.id);
  requireValid(requiredGateKinds.every(kind => plan.gates.some(gate => gate.kind === kind)));
  requireValid(plan.phases.every((phase, index) => phase === PhaseId.options[index]));
  sameMembers(plan.dependencies.map(dependency => dependency.phase), plan.phases);
  const gateIds = plan.gates.map(gate => gate.id);
  for (const dependency of plan.dependencies) {
    unique(dependency.requires_phases, phase => phase);
    unique(dependency.requires_gates, gate => gate);
    requireValid(dependency.requires_phases.every(phase => plan.phases.indexOf(phase) < plan.phases.indexOf(dependency.phase)));
    requireValid(dependency.requires_gates.every(gate => gateIds.includes(gate)));
    const position = plan.phases.indexOf(dependency.phase);
    if (position > 0) requireValid(dependency.requires_phases.includes(plan.phases[position - 1]!));
  }
  for (const diff of plan.migration_diffs) ascending(diff.pending_versions);
  for (const diff of plan.search_diffs) checkSearch(diff);
  const policies = [
    ...plan.infrastructure_diff.changes.map(diff => diff.compatibility),
    ...plan.migration_diffs.map(diff => diff.compatibility),
    ...plan.search_diffs.map(diff => diff.compatibility),
  ];
  const ranks = { COMPATIBLE: 0, COMPATIBLE_WITH_BACKFILL: 1, MAINTENANCE_REQUIRED: 2, BLOCKED: 3 };
  requireValid(policies.every(policy => ranks[policy.class] <= ranks[plan.compatibility]));
  for (const policy of policies) {
    if (policy.backfill.policy === 'REGISTERED') {
      const gateId = policy.backfill.gate_id;
      requireValid(plan.gates.some(gate => gate.id === gateId && gate.kind === 'BACKFILL'));
      requireValid(plan.dependencies.some(dependency => dependency.requires_gates.includes(gateId)));
    }
  }
  if (plan.compatibility === 'COMPATIBLE_WITH_BACKFILL') {
    requireValid(policies.some(policy => policy.backfill.policy === 'REGISTERED'));
  }
  if (plan.migration_diffs.some(diff => diff.history_status !== 'MATCHED')
    || plan.gates.some(gate => gate.status === 'BLOCKED') || !sufficientHeadroom(plan)) {
    requireValid(plan.compatibility === 'BLOCKED');
  }
  if (plan.infrastructure_diff.changes.some(diff => diff.action === 'DELETE' || diff.requires_replacement)) {
    requireValid(['MAINTENANCE_REQUIRED', 'BLOCKED'].includes(plan.compatibility));
  }
  if (plan.rollback.eligibility === 'ELIGIBLE') {
    unique(plan.rollback.required_gate_ids, id => id);
    requireValid(plan.rollback.required_gate_ids.every(id => gateIds.includes(id)));
    requireValid(plan.current_manifest_sha256 !== null && plan.compatibility !== 'BLOCKED');
    requireValid(plan.rollback.target_manifest_sha256 === (plan.operation_class === 'ROLLBACK'
      ? plan.manifest_sha256 : plan.current_manifest_sha256));
    const rollbackGates = plan.rollback.required_gate_ids;
    requireValid(['ROLLBACK', 'QUEUE_COMPATIBILITY'].every(kind =>
      plan.gates.some(gate => gate.kind === kind && rollbackGates.includes(gate.id)),
    ));
  }
  if (plan.operation_class === 'ROLLBACK') requireValid(plan.rollback.eligibility === 'ELIGIBLE');
  if (plan.compatibility === 'MAINTENANCE_REQUIRED') requireValid(plan.operation_class === 'MAINTENANCE');
}
export function parsePlan(input: unknown): DeploymentPlan {
  return safely('plan', () => {
    const plan = DeploymentPlan.parse(input);
    checkPlan(plan);
    return plan;
  });
}
export function parseIntent(input: unknown): DeploymentIntent {
  return safely('intent', () => {
    const intent = DeploymentIntent.parse(input);
    requireValid(!intent.example || !isRealStage(intent.target.stage));
    return intent;
  });
}

const boundFields = [
  'manifest_sha256', 'source_sha', 'expected_environment_revision', 'current_manifest_sha256',
  'inventory_sha256', 'configuration_sha256', 'infrastructure_context_sha256', 'operation_class',
] as const;
const executableFields = ['sha256', 'source_sha', 'repository', 'workflow', 'run_id', 'run_attempt'] as const;
export function assertIntentBinding(intentInput: unknown, planInput: unknown, expected: TrustedIntentContext): void {
  safely('intent binding', () => {
    const intent = parseIntent(intentInput);
    const plan = parsePlan(planInput);
    const { hash_plan, ...context } = expected;
    const trusted = IntentContext.parse(context);
    cleanStrings(trusted);
    requireValid(intent.example === plan.example);
    for (const field of boundFields) requireValid(intent[field] === plan[field] && intent[field] === trusted[field]);
    for (const field of ['stage', 'account', 'region'] as const) {
      requireValid(intent.target[field] === plan.target[field] && intent.target[field] === trusted.target[field]);
    }
    for (const field of ['operation_id', 'nonce', 'plan_sha256'] as const) requireValid(intent[field] === trusted[field]);
    for (const executable of ['controller', 'helper'] as const) {
      for (const field of executableFields) requireValid(intent[executable][field] === trusted[executable][field]);
    }
    const actualDigest = Digest.parse(hash_plan(plan));
    cleanStrings(actualDigest);
    requireValid(actualDigest === trusted.plan_sha256);
    requireValid(plan.compatibility !== 'BLOCKED' && plan.gates.every(gate => gate.status === 'PASSED'));
    requireValid(sufficientHeadroom(plan));
  });
}
