import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { canonicalHash } from '../src/contracts/hash.js';
import { PhaseId } from '../src/contracts/primitives.js';
import {
  DeploymentState, acquireOperation, completeOperation, completePhase, createInitialState,
  markUncertain, observePhase, parseState, startPhase,
} from '../src/state/model.js';
import type { AcquireOperationCommand, ObservePhaseCommand } from '../src/state/model.js';
import { stateSchemaDocuments } from '../src/state/export-state-schema.js';
import {
  acquisition, advanceTo, completedFixture, completionCommand, finishPhase, intentFor,
  journal, observationCommand, ownerCommand, startCommand, stateFixture,
} from '../fixtures/state-fixtures.js';
import { fixtureDigest } from '../fixtures/release-fixtures.js';

function invalid(work: () => unknown, label = 'state transition'): void {
  assert.throws(work, (error: unknown) => {
    assert.ok(error instanceof Error);
    assert.equal(error.constructor, Error);
    assert.equal(error.message, `invalid ${label}`);
    assert.deepEqual(Object.keys(error), []);
    assert.equal(error.cause, undefined);
    return true;
  });
}
function unsupported(work: () => unknown): void {
  assert.throws(work, (error: unknown) => {
    assert.ok(error instanceof Error);
    assert.equal(error.constructor, Error);
    assert.equal(error.message, 'unsupported target transition');
    assert.deepEqual(Object.keys(error), []);
    assert.equal(error.cause, undefined);
    return true;
  });
}
function rebindManifest(command: AcquireOperationCommand): void {
  command.plan.manifest_sha256 = canonicalHash(command.manifest);
  const { operation_id, nonce } = command.approved_intent;
  command.approved_intent = { ...intentFor(command.plan), operation_id, nonce };
}
function acquired() {
  const fixture = stateFixture();
  return { ...fixture, state: acquireOperation(fixture.state, fixture.acquire) };
}
function started(phase: PhaseId = 'verify_release') {
  const fixture = acquired();
  const state = advanceTo(fixture.state, phase);
  return { ...fixture, state: startPhase(state, startCommand(state, phase)) };
}
function freeze(value: unknown): void {
  if (value !== null && typeof value === 'object') {
    Object.values(value).forEach(freeze);
    Object.freeze(value);
  }
}
function nextRelease(state: DeploymentState) {
  const fixture = stateFixture();
  const manifest = structuredClone(fixture.manifest);
  manifest.release_id = 'release-example-next';
  manifest.build.run_id += 1;
  for (const image of manifest.native_images) image.image.digest = fixtureDigest('5');
  const plan = {
    ...fixture.plan, manifest_sha256: canonicalHash(manifest), expected_environment_revision: state.revision,
    current_manifest_sha256: state.current_manifest_sha256,
  };
  const intent = { ...intentFor(plan), operation_id: 'example-operation-02', nonce: 'f'.repeat(64) };
  return acquisition(state, manifest, plan, intent);
}

// DEP04: strict, versioned operational records; no migration/defaulting of corruption.
test('DEP04 initial state has no invented actual versions or owner', () => {
  const fixture = stateFixture();
  assert.equal(fixture.state.revision, 0);
  assert.equal(fixture.state.operation, null);
  assert.equal(fixture.state.current_manifest_sha256, null);
  assert.deepEqual(fixture.state.actual, []);
  assert.deepEqual(parseState(JSON.parse(JSON.stringify(fixture.state))), fixture.state);
  assert.deepEqual(createInitialState({ target: fixture.plan.target, example: true }), fixture.state);
  invalid(() => createInitialState({ target: { ...fixture.plan.target, stage: 'prod' }, example: true }));
});
for (const [name, mutation] of [
  ['version', (state: Record<string, unknown>) => { state.schema_version = 2; }],
  ['unknown control', (state: Record<string, unknown>) => { state.receipt_handle = 'do-not-echo-secret'; }],
  ['revision type', (state: Record<string, unknown>) => { state.revision = '0'; }],
  ['negative revision', (state: Record<string, unknown>) => { state.revision = -1; }],
  ['fractional revision', (state: Record<string, unknown>) => { state.revision = 0.1; }],
  ['unsafe revision', (state: Record<string, unknown>) => { state.revision = Number.MAX_SAFE_INTEGER + 1; }],
  ['missing actual', (state: Record<string, unknown>) => { delete state.actual; }],
  ['unknown stage', (state: Record<string, unknown>) => { (state.target as Record<string, unknown>).stage = 'staging'; }],
  ['control byte', (state: Record<string, unknown>) => { (state.target as Record<string, unknown>).account = '000000000000\n'; }],
  ['fake completed manifest', (state: Record<string, unknown>) => { state.current_manifest_sha256 = fixtureDigest('a'); }],
] as const) {
  test(`DEP04 rejects ${name} with generic errors`, () => {
    const state = structuredClone(stateFixture().state) as Record<string, unknown>;
    mutation(state);
    invalid(() => parseState(state), 'deployment state');
  });
}
test('DEP04 malformed non-record inputs are safe errors', () => {
  for (const input of [null, undefined, [], 'secret-payload', 42, { kind: 'DEPLOYMENT_STATE' }]) {
    invalid(() => parseState(input), 'deployment state');
  }
});

// DEP06: ownership is a durable exact binding, not a lease or a plan rebase.
test('DEP06 acquire is deterministic, does not mutate, and records intent separately from state revision', () => {
  const fixture = stateFixture();
  const before = structuredClone(fixture);
  freeze(fixture);
  const state = acquireOperation(fixture.state, fixture.acquire);
  assert.deepEqual(state, acquireOperation(fixture.state, fixture.acquire));
  assert.deepEqual(fixture, before);
  assert.equal(state.revision, 1);
  assert.equal(state.operation!.approved_environment_revision, 0);
  assert.equal(state.operation!.nonce, fixture.intent.nonce);
  assert.equal(state.operation!.intent_sha256, canonicalHash(fixture.intent));
  assert.deepEqual(state.operation!.phases.map(phase => phase.phase), PhaseId.options);
  assert.ok(state.operation!.phases.every(phase => phase.status === 'PLANNED'));
  assert.equal(state.operation!.intended.length, 28);
  assert.deepEqual(state.actual, []);
  assert.equal(state.current_manifest_sha256, null);
});
test('DEP04 migrator is an explicit postgres tool identity, never an active native owner', () => {
  const fixture = acquired();
  const image = fixture.manifest.native_images.find(image => image.component === 'aura-historia-migrate')!;
  assert.deepEqual(fixture.state.operation!.intended.filter(item => item.component.kind === 'MIGRATOR'), [{
    component: { kind: 'MIGRATOR', architecture: image.architecture },
    identity: { source_sha: image.source_sha, definition_sha256: canonicalHash(image) }, phase: 'postgres_expand',
  }]);
  assert.deepEqual(fixture.state.operation!.intended.filter(item => item.component.kind === 'NATIVE').map(item => item.component), [
    { kind: 'NATIVE', component: 'aura-historia-api', architecture: 'arm64' },
    { kind: 'NATIVE', component: 'aura-historia-cron', architecture: 'arm64' },
    { kind: 'NATIVE', component: 'crawler', architecture: 'arm64' },
  ]);
  const missing = structuredClone(fixture.state);
  missing.operation!.intended = missing.operation!.intended.filter(item => item.component.kind !== 'MIGRATOR');
  invalid(() => parseState(missing), 'deployment state');
  const misplaced = structuredClone(fixture.state);
  misplaced.operation!.intended.find(item => item.component.kind === 'MIGRATOR')!.phase = 'handover_jobs';
  invalid(() => parseState(misplaced), 'deployment state');
  const daemon = structuredClone(fixture.state);
  Object.assign(daemon.operation!.intended.find(item => item.component.kind === 'MIGRATOR')!.component, {
    kind: 'NATIVE', component: 'aura-historia-migrate',
  });
  assert.equal(DeploymentState.safeParse(daemon).success, false);
  invalid(() => parseState(daemon), 'deployment state');
});
for (const component of ['aura-historia-api', 'aura-historia-worker', 'aura-historia-migrate', 'LAMBDA'] as const) {
  test(`DEP06 P2 multiple architecture selections are unsupported: ${component}`, () => {
    const fixture = stateFixture();
    if (component === 'LAMBDA') {
      const lambda = fixture.manifest.lambdas[0]!;
      fixture.manifest.lambdas.push({ ...structuredClone(lambda), architecture: 'arm64', rust_target: 'aarch64-unknown-linux-gnu' });
    } else {
      const image = fixture.manifest.native_images.find(image => image.component === component)!;
      fixture.manifest.native_images.push({
        ...structuredClone(image), architecture: 'x86_64', rust_target: 'x86_64-unknown-linux-gnu',
        image: { ...image.image, digest: fixtureDigest('6') },
      });
    }
    rebindManifest(fixture.acquire);
    const before = structuredClone(fixture);
    freeze(fixture);
    unsupported(() => acquireOperation(fixture.state, fixture.acquire));
    assert.deepEqual(fixture, before);
  });
  test(`DEP06 P2 architecture changes reject before acquisition and preserve old evidence: ${component}`, () => {
    const previous = completedFixture().state;
    const command = nextRelease(previous);
    if (component === 'LAMBDA') {
      Object.assign(command.manifest.lambdas[0]!, { architecture: 'arm64', rust_target: 'aarch64-unknown-linux-gnu' });
    } else {
      Object.assign(command.manifest.native_images.find(image => image.component === component)!, {
        architecture: 'x86_64', rust_target: 'x86_64-unknown-linux-gnu',
      });
    }
    rebindManifest(command);
    const before = structuredClone({ previous, command });
    freeze(previous); freeze(command);
    unsupported(() => acquireOperation(previous, command));
    assert.deepEqual({ previous, command }, before);
    assert.equal(previous.operation!.status, 'COMPLETED');
  });
}
test('DEP06 P2 ordinary same-key version upgrade preserves old evidence until new observations and completes', () => {
  const previous = completedFixture().state;
  const before = structuredClone(previous);
  const command = nextRelease(previous);
  freeze(previous); freeze(command);
  let state = acquireOperation(previous, command);
  assert.equal(state.revision, previous.revision + 1);
  assert.deepEqual(state.actual, previous.actual);
  assert.equal(state.operation!.approved_environment_revision, previous.revision);
  for (const phase of PhaseId.options) state = finishPhase(state, phase);
  state = completeOperation(state, ownerCommand(state, 'completed'));
  assert.equal(state.operation!.status, 'COMPLETED');
  assert.equal(state.current_manifest_sha256, command.plan.manifest_sha256);
  assert.deepEqual(previous, before);
});
test('DEP27 postgres completion needs observed tool and histories, not tool or histories alone', () => {
  const fixture = started('postgres_expand');
  const toolOnly = observationCommand(fixture.state, 'postgres_expand');
  toolOnly.observation.components = toolOnly.observation.components.filter(item => item.component.kind === 'MIGRATOR');
  const toolObserved = observePhase(fixture.state, toolOnly);
  invalid(() => completePhase(toolObserved, completionCommand(toolObserved, 'postgres_expand')));
  const historiesOnly = observationCommand(fixture.state, 'postgres_expand');
  historiesOnly.observation.components = historiesOnly.observation.components.filter(item => item.component.kind === 'POSTGRES');
  assert.equal(historiesOnly.observation.components.length, 2);
  let state = observePhase(fixture.state, historiesOnly);
  invalid(() => completePhase(state, completionCommand(state, 'postgres_expand')));
  assert.equal(state.operation!.status, 'OWNED');
  assert.equal(state.actual.some(item => item.component.kind === 'MIGRATOR'), false);
  const tool = state.operation!.intended.find(item => item.component.kind === 'MIGRATOR')!;
  for (const identity of [null, { ...tool.identity, definition_sha256: fixtureDigest('0') }]) {
    const readBack = observationCommand(state, 'postgres_expand');
    readBack.observation.components = [{ component: tool.component, identity }];
    state = observePhase(state, readBack);
    invalid(() => completePhase(state, completionCommand(state, 'postgres_expand')));
    invalid(() => completeOperation(state, ownerCommand(state, 'completed')));
  }
  const exact = observationCommand(state, 'postgres_expand');
  exact.observation.components = [{ component: tool.component, identity: tool.identity }];
  state = observePhase(state, exact);
  state = completePhase(state, completionCommand(state, 'postgres_expand'));
  assert.equal(state.operation!.phases.find(item => item.phase === 'postgres_expand')!.status, 'COMPLETED');
  assert.deepEqual(state.actual.find(item => item.component.kind === 'MIGRATOR')!.identity, tool.identity);
  assert.equal(state.actual.find(item => item.component.kind === 'MIGRATOR')!.observation.phase, 'postgres_expand');
});
test('DEP27 handover_jobs cannot claim or replace the verified migrator tool', () => {
  let state = started('handover_jobs').state;
  const tool = state.actual.find(item => item.component.kind === 'MIGRATOR')!;
  assert.equal(tool.observation.phase, 'postgres_expand');
  const command = observationCommand(state, 'handover_jobs');
  assert.deepEqual(command.observation.components.map(item => item.component), [
    { kind: 'NATIVE', component: 'aura-historia-cron', architecture: 'arm64' },
    { kind: 'NATIVE', component: 'crawler', architecture: 'arm64' },
  ]);
  const overclaim = structuredClone(command);
  overclaim.observation.components.push({ component: tool.component, identity: tool.identity });
  invalid(() => observePhase(state, overclaim));
  state = observePhase(state, command);
  state = completePhase(state, completionCommand(state, 'handover_jobs'));
  assert.deepEqual(state.actual.find(item => item.component.kind === 'MIGRATOR'), tool);
});
for (const field of [
  'transaction_mode', 'lock_timeout_ms', 'statement_timeout_ms', 'affected_capabilities',
  'required_extensions', 'old_new_compatibility_evidence_sha256',
] as const) {
  test(`DEP04/06 postgres identity and approval bind migration ${field}`, () => {
    const fixture = acquired();
    const original = fixture.state.operation!.intended.find(item => item.component.kind === 'POSTGRES' && item.component.stream === 'business')!;
    const changed = stateFixture();
    const stream = changed.manifest.migrations.streams.find(stream => stream.stream === 'business')!;
    const migration = stream.migrations[0]!;
    switch (field) {
      case 'transaction_mode': migration.transaction_mode = 'NONTRANSACTIONAL'; break;
      case 'lock_timeout_ms': migration.lock_timeout_ms += 1; break;
      case 'statement_timeout_ms': migration.statement_timeout_ms += 1; break;
      case 'affected_capabilities': migration.affected_capabilities.push('additional-capability'); break;
      case 'required_extensions': migration.required_extensions.push('additional_extension'); break;
      case 'old_new_compatibility_evidence_sha256': migration.old_new_compatibility_evidence_sha256 = fixtureDigest('0'); break;
    }
    invalid(() => acquireOperation(changed.state, changed.acquire));
    changed.plan.manifest_sha256 = canonicalHash(changed.manifest);
    changed.acquire.approved_intent = intentFor(changed.plan);
    const state = acquireOperation(changed.state, changed.acquire);
    const target = state.operation!.intended.find(item => item.component.kind === 'POSTGRES' && item.component.stream === 'business')!;
    assert.equal(target.identity.definition_sha256, canonicalHash(stream));
    assert.notEqual(target.identity.definition_sha256, original.identity.definition_sha256);
  });
}
test('DEP06 acquire requires an explicit fresh expected revision even for an exact replay', () => {
  const fixture = acquired();
  invalid(() => acquireOperation(fixture.state, fixture.acquire));
  const { expected_revision: _revision, ...missing } = fixture.acquire;
  invalid(() => acquireOperation(fixture.state, missing as AcquireOperationCommand));
  const command = { ...fixture.acquire, expected_revision: fixture.state.revision };
  assert.deepEqual(acquireOperation(fixture.state, command), fixture.state);
});
for (const field of ['stage', 'account', 'region'] as const) {
  test(`DEP06 acquire binds exact environment ${field}`, () => {
    const fixture = stateFixture();
    const command = structuredClone(fixture.acquire);
    const wrong = { stage: 'local', account: '111111111111', region: 'us-east-1' } as const;
    command.plan.target = { ...command.plan.target, [field]: wrong[field] };
    command.approved_intent = intentFor(command.plan);
    invalid(() => acquireOperation(fixture.state, command));
  });
}
for (const [name, change] of [
  ['manifest canonical identity', (command: AcquireOperationCommand) => { command.manifest.release_id = 'different-release'; }],
  ['manifest digest', (command: AcquireOperationCommand) => { command.approved_intent.manifest_sha256 = fixtureDigest('0'); }],
  ['plan body', (command: AcquireOperationCommand) => { command.plan.headroom.postgres.available_connections += 1; }],
  ['plan hash', (command: AcquireOperationCommand) => { command.approved_intent.plan_sha256 = fixtureDigest('0'); }],
  ['source', (command: AcquireOperationCommand) => { command.approved_intent.source_sha = '0'.repeat(40); }],
  ['inventory', (command: AcquireOperationCommand) => { command.approved_intent.inventory_sha256 = fixtureDigest('0'); }],
  ['configuration', (command: AcquireOperationCommand) => { command.approved_intent.configuration_sha256 = fixtureDigest('0'); }],
  ['infrastructure context', (command: AcquireOperationCommand) => { command.approved_intent.infrastructure_context_sha256 = fixtureDigest('0'); }],
  ['operation class', (command: AcquireOperationCommand) => { command.approved_intent.operation_class = 'REPAIR_FORWARD'; }],
  ['original revision', (command: AcquireOperationCommand) => { command.approved_intent.expected_environment_revision += 1; }],
  ['current manifest', (command: AcquireOperationCommand) => { command.plan.current_manifest_sha256 = fixtureDigest('0'); command.approved_intent = intentFor(command.plan); }],
  ['pending gate', (command: AcquireOperationCommand) => {
    const { id, kind } = command.plan.gates[0]!;
    command.plan.gates[0] = { id, kind, status: 'PENDING', reason_code: 'approval-missing' };
    command.approved_intent = intentFor(command.plan);
  }],
] as const) {
  test(`DEP06 acquisition rejects changed ${name}`, () => {
    const fixture = stateFixture();
    const command = structuredClone(fixture.acquire);
    change(command);
    invalid(() => acquireOperation(fixture.state, command));
  });
}
test('DEP06 second owner and changed nonce/plan/intent cannot acquire an owned environment', () => {
  const fixture = started();
  for (const field of ['operation_id', 'nonce', 'plan_sha256', 'controller', 'helper'] as const) {
    const command = structuredClone({ ...fixture.acquire, expected_revision: fixture.state.revision });
    if (field === 'operation_id') command.approved_intent.operation_id = 'another-owner';
    else if (field === 'nonce') command.approved_intent.nonce = 'd'.repeat(64);
    else if (field === 'plan_sha256') {
      command.plan.headroom.postgres.available_connections += 1;
      command.approved_intent.plan_sha256 = canonicalHash(command.plan);
    } else command.approved_intent[field].sha256 = fixtureDigest('0');
    invalid(() => acquireOperation(fixture.state, command));
  }
});
test('DEP06 same operation reconciles uncertainty without rebasing approved revision or restarting effects', () => {
  const fixture = started();
  const command = observationCommand(fixture.state, 'verify_release', 'UNKNOWN');
  const state = observePhase(fixture.state, command);
  const reconciliation = { ...fixture.acquire, expected_revision: state.revision };
  assert.deepEqual(acquireOperation(state, reconciliation), state);
  assert.equal(state.operation!.approved_environment_revision, fixture.intent.expected_environment_revision);
  assert.ok(state.revision > state.operation!.approved_environment_revision);
  const rebase = structuredClone(reconciliation);
  rebase.plan.expected_environment_revision = state.revision;
  rebase.approved_intent = intentFor(rebase.plan);
  invalid(() => acquireOperation(state, rebase));
  invalid(() => acquireOperation(state, { ...reconciliation, journal: journal('different-acquisition') }));
  invalid(() => parseState({ ...state, lease_expires_at: '2000-01-01T00:00:00Z' }), 'deployment state');
});

// DEP07 is only the protocol boundary. This deliberately does NOT simulate/prove S3 CAS.
test('DEP07 pure competing proposals need adapter CAS; a chosen state fences the losing owner', () => {
  const fixture = stateFixture();
  const other = structuredClone(fixture.acquire);
  other.approved_intent.operation_id = 'second-owner';
  other.approved_intent.nonce = 'd'.repeat(64);
  const firstProposal = acquireOperation(fixture.state, fixture.acquire);
  const secondProposal = acquireOperation(fixture.state, other);
  assert.equal(firstProposal.revision, secondProposal.revision);
  assert.notEqual(firstProposal.operation!.nonce, secondProposal.operation!.nonce);
  invalid(() => acquireOperation(firstProposal, { ...other, expected_revision: firstProposal.revision }));
  invalid(() => startPhase(firstProposal, startCommand(secondProposal, 'verify_release')));
});
test('DEP06 every mutation fences stale revision, operation, nonce, plan and intent digests', () => {
  const planned = acquired().state;
  const fixture = started();
  const observed = observePhase(fixture.state, observationCommand(fixture.state, 'verify_release'));
  const completed = completedFixture().state;
  for (const [field, value] of [
    ['expected_revision', -1], ['operation_id', 'other-operation'], ['nonce', '0'.repeat(64)],
    ['plan_sha256', fixtureDigest('0')], ['intent_sha256', fixtureDigest('0')],
  ] as const) {
    const start = startCommand(planned, 'verify_release');
    Reflect.set(start, field, field === 'expected_revision' ? planned.revision - 1 : value);
    invalid(() => startPhase(planned, start));
    const phase = completionCommand(observed, 'verify_release');
    Reflect.set(phase, field, field === 'expected_revision' ? observed.revision - 1 : value);
    invalid(() => completePhase(observed, phase));
    const operation = ownerCommand(completed, 'completed');
    Reflect.set(operation, field, field === 'expected_revision' ? completed.revision - 1 : value);
    invalid(() => completeOperation(completed, operation));
    const { components: _components, outcome: _outcome, postconditions: _postconditions, ...reference } = observationCommand(fixture.state, 'verify_release').observation;
    const uncertain = { expected_revision: fixture.state.revision, ...reference };
    Reflect.set(uncertain, field, field === 'expected_revision' ? fixture.state.revision - 1 : value);
    invalid(() => markUncertain(fixture.state, uncertain));
  }
  const observation = observationCommand(fixture.state, 'verify_release');
  observation.expected_revision -= 1;
  invalid(() => observePhase(fixture.state, observation));
});

// DEP27: crash/unknown-outcome reconciliation needs terminal, identity-bound evidence.
test('DEP27 STARTED cannot complete or start twice, even with identical start identity', () => {
  const fixture = acquired();
  const command = startCommand(fixture.state, 'verify_release');
  const state = startPhase(fixture.state, command);
  invalid(() => startPhase(state, { ...command, expected_revision: state.revision }));
  invalid(() => startPhase(state, startCommand(state, 'verify_release')));
  invalid(() => completePhase(state, completionCommand(state, 'verify_release')));
  invalid(() => completeOperation(state, ownerCommand(state, 'completed')));
});
test('DEP27 explicit uncertainty and UNKNOWN read-back keep owner, cannot complete or retry', () => {
  const fixture = started();
  const { components: _components, outcome: _outcome, postconditions: _postconditions, ...reference } = observationCommand(fixture.state, 'verify_release').observation;
  const command = { expected_revision: fixture.state.revision, ...reference };
  let state = markUncertain(fixture.state, command);
  assert.equal(state.operation!.status, 'OWNED');
  assert.equal(state.operation!.phases[0]!.status, 'UNCERTAIN');
  assert.deepEqual(markUncertain(state, { ...command, expected_revision: state.revision }), state);
  state = observePhase(state, observationCommand(state, 'verify_release', 'UNKNOWN'));
  invalid(() => startPhase(state, startCommand(state, 'verify_release')));
  invalid(() => completePhase(state, completionCommand(state, 'verify_release')));
  invalid(() => completeOperation(state, ownerCommand(state, 'completed')));
  const other = structuredClone({ ...fixture.acquire, expected_revision: state.revision });
  other.approved_intent.operation_id = 'timeout-takeover';
  other.approved_intent.nonce = 'd'.repeat(64);
  invalid(() => acquireOperation(state, other));
});
for (const [name, mutate] of [
  ['operation', (command: ObservePhaseCommand) => { command.observation.operation_id = 'wrong-operation'; }],
  ['nonce', (command: ObservePhaseCommand) => { command.observation.nonce = '0'.repeat(64); }],
  ['plan', (command: ObservePhaseCommand) => { command.observation.plan_sha256 = fixtureDigest('0'); }],
  ['intent', (command: ObservePhaseCommand) => { command.observation.intent_sha256 = fixtureDigest('0'); }],
  ['attempt', (command: ObservePhaseCommand) => { command.observation.attempt += 1; }],
  ['start digest', (command: ObservePhaseCommand) => { command.observation.start_sha256 = fixtureDigest('0'); }],
  ['phase', (command: ObservePhaseCommand) => { command.observation.phase = 'switch_api'; }],
  ['missing evidence', (command: ObservePhaseCommand) => { Reflect.deleteProperty(command.observation, 'evidence'); }],
  ['empty reference', (command: ObservePhaseCommand) => { command.observation.evidence.key = ''; }],
  ['traversal reference', (command: ObservePhaseCommand) => { command.observation.evidence.key = 'operations/../secret'; }],
  ['missing evidence digest', (command: ObservePhaseCommand) => { Reflect.deleteProperty(command.observation.evidence, 'sha256'); }],
  ['unknown control', (command: ObservePhaseCommand) => { Object.assign(command.observation, { receipt_handle: 'do-not-echo' }); }],
  ['nonterminal outcome', (command: ObservePhaseCommand) => { Object.assign(command.observation, { outcome: 'HEALTHY' }); }],
  ['unfounded postconditions', (command: ObservePhaseCommand) => { command.observation.outcome = 'UNKNOWN'; }],
] as const) {
  test(`DEP27 read-back rejects ${name}`, () => {
    const fixture = started();
    const command = observationCommand(fixture.state, 'verify_release');
    mutate(command);
    invalid(() => observePhase(fixture.state, command));
  });
}
test('DEP27 NOT_APPLIED retains ownership and permits only a new journaled attempt', () => {
  const fixture = started();
  let state = observePhase(fixture.state, observationCommand(fixture.state, 'verify_release', 'UNKNOWN'));
  const notApplied = observationCommand(state, 'verify_release', 'NOT_APPLIED');
  state = observePhase(state, notApplied);
  assert.equal(state.operation!.phases[0]!.status, 'FAILED');
  assert.equal(state.operation!.status, 'OWNED');
  invalid(() => completePhase(state, completionCommand(state, 'verify_release')));
  const retry = startCommand(state, 'verify_release');
  invalid(() => startPhase(state, { ...retry, journal: state.operation!.phases[0]!.attempts[0]!.started.journal }));
  state = startPhase(state, retry);
  assert.equal(state.operation!.phases[0]!.attempts.length, 2);
  invalid(() => observePhase(state, { ...notApplied, expected_revision: state.revision }));
  state = observePhase(state, observationCommand(state, 'verify_release'));
  state = completePhase(state, completionCommand(state, 'verify_release'));
  assert.equal(state.operation!.phases[0]!.status, 'COMPLETED');
  assert.equal(state.operation!.phases[0]!.attempts[0]!.observations.at(-1)!.observation.outcome, 'NOT_APPLIED');
});
test('DEP27 P1 switch_api attempt 2 cannot reuse correct targets from NOT_APPLIED attempt 1', () => {
  let state = started('switch_api').state;
  const failed = observationCommand(state, 'switch_api');
  failed.observation.outcome = 'NOT_APPLIED';
  failed.observation.postconditions = 'UNVERIFIED';
  assert.equal(failed.observation.components.length, 1);
  state = observePhase(state, failed);
  const mixedActual = structuredClone(state.actual);
  state = startPhase(state, startCommand(state, 'switch_api'));
  const appliedWithoutIdentities = observationCommand(state, 'switch_api');
  assert.equal(appliedWithoutIdentities.observation.attempt, 2);
  appliedWithoutIdentities.observation.components = [];
  state = observePhase(state, appliedWithoutIdentities);
  assert.deepEqual(state.actual, mixedActual);
  assert.equal(state.operation!.status, 'OWNED');
  invalid(() => completePhase(state, completionCommand(state, 'switch_api')));
  invalid(() => completeOperation(state, ownerCommand(state, 'completed')));
  const forged = structuredClone(state);
  const phase = forged.operation!.phases.find(phase => phase.phase === 'switch_api')!;
  phase.status = 'COMPLETED';
  phase.completed = { revision: ++forged.revision, journal: completionCommand(state, 'switch_api').journal };
  invalid(() => parseState(forged), 'deployment state');
  state = observePhase(state, observationCommand(state, 'switch_api'));
  state = completePhase(state, completionCommand(state, 'switch_api'));
  const actual = state.actual.find(item => item.component.kind === 'NATIVE' && item.component.component === 'aura-historia-api')!;
  assert.equal(actual.observation.phase, 'switch_api');
  assert.equal(actual.observation.attempt, 2);
  assert.equal(actual.observation.start_sha256, appliedWithoutIdentities.observation.start_sha256);
  assert.equal(state.operation!.phases.find(phase => phase.phase === 'switch_api')!.status, 'COMPLETED');
});
for (const outcome of ['APPLIED', 'UNKNOWN'] as const) {
  test(`DEP27 P1 current-attempt ${outcome}/UNVERIFIED identities need their own verified read-back`, () => {
    let state = started('switch_api').state;
    const unverified = observationCommand(state, 'switch_api', outcome);
    unverified.observation.postconditions = 'UNVERIFIED';
    unverified.observation.components = observationCommand(state, 'switch_api').observation.components;
    state = observePhase(state, unverified);
    const terminal = observationCommand(state, 'switch_api');
    terminal.observation.components = [];
    state = observePhase(state, terminal);
    invalid(() => completePhase(state, completionCommand(state, 'switch_api')));
    state = observePhase(state, observationCommand(state, 'switch_api'));
    state = completePhase(state, completionCommand(state, 'switch_api'));
    assert.equal(state.operation!.phases.find(phase => phase.phase === 'switch_api')!.status, 'COMPLETED');
  });
}
test('DEP27 P1 final verification retry cannot reuse failed-attempt identities despite completed effect phases', () => {
  let state = started('verify_release_behavior').state;
  const target = state.operation!.intended[0]!;
  const failed = observationCommand(state, 'verify_release_behavior', 'NOT_APPLIED');
  failed.observation.components = [{ component: target.component, identity: target.identity }];
  state = observePhase(state, failed);
  state = startPhase(state, startCommand(state, 'verify_release_behavior'));
  state = observePhase(state, observationCommand(state, 'verify_release_behavior'));
  invalid(() => completePhase(state, completionCommand(state, 'verify_release_behavior')));
  const verified = observationCommand(state, 'verify_release_behavior');
  verified.observation.components = [{ component: target.component, identity: target.identity }];
  state = observePhase(state, verified);
  state = completePhase(state, completionCommand(state, 'verify_release_behavior'));
  state = finishPhase(state, 'commit_release');
  state = completeOperation(state, ownerCommand(state, 'completed'));
  assert.equal(state.operation!.status, 'COMPLETED');
});
test('DEP27 P1 final commit rejects equal target identities backed only by unverified phase evidence', () => {
  const state = completedFixture().state;
  const behavior = state.operation!.phases.find(phase => phase.phase === 'verify_release_behavior')!;
  const attempt = behavior.attempts[0]!;
  const first = attempt.observations[0]!.observation;
  const actual = state.actual.find(item => item.component.kind === 'NATIVE' && item.component.component === 'aura-historia-api')!;
  first.postconditions = 'UNVERIFIED';
  first.components = [{ component: actual.component, identity: actual.identity }];
  const { components: _components, outcome: _outcome, postconditions: _postconditions, ...reference } = first;
  actual.observation = reference;
  // Structurally valid journal: a later verified empty read-back does not upgrade
  // the earlier component evidence. Keep all event revisions contiguous.
  attempt.observations.push({ revision: behavior.completed!.revision, observation: observationCommand(state, 'verify_release_behavior').observation });
  behavior.completed!.revision += 1;
  const commit = state.operation!.phases.find(phase => phase.phase === 'commit_release')!;
  commit.attempts[0]!.started.revision += 1;
  commit.attempts[0]!.observations[0]!.revision += 1;
  commit.completed!.revision += 1;
  state.operation!.completed!.revision += 1;
  state.revision += 1;
  assert.ok(state.operation!.phases.every(phase => phase.status === 'COMPLETED'));
  assert.equal(DeploymentState.safeParse(state).success, true);
  invalid(() => parseState(state), 'deployment state');
  invalid(() => completeOperation(state, ownerCommand(state, 'completed')));
});
test('DEP27 APPLIED needs verified postconditions; read-back may improve but never rerun or contradict effect', () => {
  const fixture = started();
  const observation = observationCommand(fixture.state, 'verify_release');
  observation.observation.postconditions = 'UNVERIFIED';
  let state = observePhase(fixture.state, observation);
  invalid(() => completePhase(state, completionCommand(state, 'verify_release')));
  invalid(() => startPhase(state, startCommand(state, 'verify_release')));
  invalid(() => observePhase(state, observationCommand(state, 'verify_release', 'NOT_APPLIED')));
  invalid(() => observePhase(state, observationCommand(state, 'verify_release', 'UNKNOWN')));
  state = observePhase(state, observationCommand(state, 'verify_release'));
  state = completePhase(state, completionCommand(state, 'verify_release'));
  assert.equal(state.operation!.phases[0]!.status, 'COMPLETED');
});
test('DEP27 identical observations/completions are no-ops, changed immutable evidence is rejected', () => {
  const fixture = started();
  const observation = observationCommand(fixture.state, 'verify_release');
  let state = observePhase(fixture.state, observation);
  assert.deepEqual(observePhase(state, { ...observation, expected_revision: state.revision }), state);
  const changed = structuredClone({ ...observation, expected_revision: state.revision });
  changed.observation.postconditions = 'UNVERIFIED';
  invalid(() => observePhase(state, changed));
  const completion = completionCommand(state, 'verify_release');
  state = completePhase(state, completion);
  assert.deepEqual(completePhase(state, { ...completion, expected_revision: state.revision }), state);
  assert.deepEqual(observePhase(state, { ...observation, expected_revision: state.revision }), state);
  invalid(() => completePhase(state, { ...completion, expected_revision: state.revision, journal: journal('changed-completion') }));
});
test('DEP27 canonical phase order is mandatory, including persisted journals', () => {
  const fixture = acquired();
  invalid(() => startPhase(fixture.state, startCommand(fixture.state, 'acquire_operation')));
  const state = structuredClone(fixture.state);
  state.operation!.phases.reverse();
  invalid(() => parseState(state), 'deployment state');
  const command = structuredClone(fixture.acquire);
  command.plan.phases.reverse();
  command.approved_intent.plan_sha256 = canonicalHash(command.plan);
  invalid(() => acquireOperation(stateFixture().state, command));
});
test('DEP27 persisted ownership, revision gaps, status and observation corruption fail closed', () => {
  const fixture = started();
  const observed = observePhase(fixture.state, observationCommand(fixture.state, 'verify_release'));
  for (const mutate of [
    (state: DeploymentState) => { state.revision += 1; },
    (state: DeploymentState) => { state.operation = null; },
    (state: DeploymentState) => { state.operation!.nonce = '0'.repeat(64); },
    (state: DeploymentState) => { state.operation!.approved_environment_revision = state.revision; },
    (state: DeploymentState) => { state.operation!.phases[0]!.status = 'COMPLETED'; },
    (state: DeploymentState) => { state.operation!.phases[0]!.attempts[0]!.observations[0]!.revision += 1; },
    (state: DeploymentState) => { state.operation!.intended.pop(); },
    (state: DeploymentState) => { state.operation!.intended.push(state.operation!.intended[0]!); },
    (state: DeploymentState) => { state.operation!.intended[0]!.phase = 'verify_release'; },
  ]) {
    const state = structuredClone(observed);
    mutate(state);
    invalid(() => parseState(state), 'deployment state');
  }
});

test('DEP04/27 partial worker observations preserve mixed actual identities and previous completed manifest', () => {
  const previous = completedFixture().state;
  let state = acquireOperation(previous, nextRelease(previous));
  assert.deepEqual(state.actual, previous.actual);
  state = advanceTo(state, 'replace_worker_scopes');
  state = startPhase(state, startCommand(state, 'replace_worker_scopes'));
  const observation = observationCommand(state, 'replace_worker_scopes');
  observation.observation.components = observation.observation.components.slice(0, 1);
  state = observePhase(state, observation);
  const desired = state.operation!.intended.filter(item => item.component.kind === 'WORKER');
  const actualWorkers = state.actual.filter(item => item.component.kind === 'WORKER');
  assert.deepEqual(actualWorkers[0]!.identity, desired[0]!.identity);
  assert.notDeepEqual(actualWorkers[1]!.identity, desired[1]!.identity);
  assert.deepEqual(state.actual.find(item => item.component.kind === 'NATIVE' && item.component.component === 'aura-historia-api'),
    previous.actual.find(item => item.component.kind === 'NATIVE' && item.component.component === 'aura-historia-api'));
  assert.equal(state.current_manifest_sha256, previous.current_manifest_sha256);
  assert.equal(state.operation!.status, 'OWNED');
  invalid(() => completePhase(state, completionCommand(state, 'replace_worker_scopes')));
  invalid(() => completeOperation(state, ownerCommand(state, 'completed')));
});
test('DEP04/27 healthy API alone neither records an API version nor rewrites jobs/other components', () => {
  const previous = completedFixture().state;
  let state = acquireOperation(previous, nextRelease(previous));
  state = advanceTo(state, 'switch_api');
  state = startPhase(state, startCommand(state, 'switch_api'));
  const before = structuredClone(state.actual);
  const healthyOnly = observationCommand(state, 'switch_api');
  healthyOnly.observation.components = [];
  state = observePhase(state, healthyOnly);
  assert.deepEqual(state.actual, before);
  invalid(() => completePhase(state, completionCommand(state, 'switch_api')));
  const overclaim = observationCommand(state, 'switch_api');
  const worker = state.operation!.intended.find(item => item.component.kind === 'WORKER')!;
  overclaim.observation.components.push({ component: worker.component, identity: worker.identity });
  invalid(() => observePhase(state, overclaim));
  state = observePhase(state, observationCommand(state, 'switch_api'));
  state = completePhase(state, completionCommand(state, 'switch_api'));
  const cron = (value: DeploymentState) => value.actual.find(item => item.component.kind === 'NATIVE' && item.component.component === 'aura-historia-cron');
  assert.deepEqual(cron(state), cron(previous));
  assert.notDeepEqual(cron(state)!.identity, state.operation!.intended.find(item => item.component.kind === 'NATIVE' && item.component.component === 'aura-historia-cron')!.identity);
  invalid(() => completeOperation(state, ownerCommand(state, 'completed')));
});
for (const outcome of ['APPLIED', 'UNKNOWN'] as const) {
  test(`DEP27 ${outcome} with unknown/mismatched actual target cannot finish or release ownership`, () => {
    const fixture = started('verify_release_behavior');
    let state = fixture.state;
    const target = state.operation!.intended[0]!;
    const observation = observationCommand(state, 'verify_release_behavior', outcome);
    observation.observation.components = [{ component: target.component, identity: null }];
    state = observePhase(state, observation);
    assert.equal(state.actual.find(item => canonicalHash(item.component) === canonicalHash(target.component))!.identity, null);
    invalid(() => completePhase(state, completionCommand(state, 'verify_release_behavior')));
    invalid(() => completeOperation(state, ownerCommand(state, 'completed')));
    assert.equal(state.operation!.status, 'OWNED');
    const wrong = observationCommand(state, 'verify_release_behavior');
    wrong.observation.components = [{ component: target.component, identity: { ...target.identity, definition_sha256: fixtureDigest('0') } }];
    state = observePhase(state, wrong);
    invalid(() => completePhase(state, completionCommand(state, 'verify_release_behavior')));
    const exact = observationCommand(state, 'verify_release_behavior');
    exact.observation.components = [{ component: target.component, identity: target.identity }];
    state = observePhase(state, exact);
    state = completePhase(state, completionCommand(state, 'verify_release_behavior'));
    invalid(() => completeOperation(state, ownerCommand(state, 'completed')));
    state = finishPhase(state, 'commit_release');
    state = completeOperation(state, ownerCommand(state, 'completed'));
    assert.equal(state.operation!.status, 'COMPLETED');
  });
}
test('DEP06/27 final release requires all verified phases and targets, exact replay never reacquires', () => {
  const fixture = completedFixture();
  const state = fixture.state;
  assert.equal(state.operation!.status, 'COMPLETED');
  assert.equal(state.current_manifest_sha256, fixture.plan.manifest_sha256);
  assert.ok(state.operation!.phases.every(phase => phase.status === 'COMPLETED'));
  assert.equal(state.actual.length, state.operation!.intended.length);
  assert.equal(state.revision, 50);
  assert.equal(state.operation!.approved_environment_revision, 0);
  assert.deepEqual(parseState(JSON.parse(JSON.stringify(state))), state);
  assert.deepEqual(completeOperation(state, ownerCommand(state, 'completed')), state);
  assert.deepEqual(acquireOperation(state, { ...fixture.acquire, expected_revision: state.revision }), state);
  invalid(() => startPhase(state, startCommand(state, 'verify_release')));
  const missing = structuredClone(state);
  missing.actual.pop();
  invalid(() => parseState(missing), 'deployment state');
  const wrong = structuredClone(state);
  wrong.actual[0]!.identity = null;
  invalid(() => completeOperation(wrong, ownerCommand(wrong, 'completed')));
  const next = nextRelease(state);
  next.approved_intent.nonce = state.operation!.nonce;
  invalid(() => acquireOperation(state, next));
});
test('DEP04/27 rollback approval cannot reset ownership, revision, journal or mixed actual state', () => {
  const previous = completedFixture().state;
  const command = nextRelease(previous);
  command.plan.operation_class = 'ROLLBACK';
  command.plan.rollback = { eligibility: 'ELIGIBLE', target_manifest_sha256: command.plan.manifest_sha256, required_gate_ids: ['rollback', 'queue-compatibility'] };
  command.approved_intent = { ...intentFor(command.plan), operation_id: 'rollback-operation', nonce: 'd'.repeat(64) };
  const state = acquireOperation(previous, command);
  assert.equal(state.revision, previous.revision + 1);
  assert.deepEqual(state.actual, previous.actual);
  assert.equal(state.current_manifest_sha256, previous.current_manifest_sha256);
  invalid(() => completeOperation(state, ownerCommand(state, 'completed')));
});
test('DEP27 transitions leave frozen caller state and commands untouched', () => {
  let state = acquired().state;
  for (const phase of PhaseId.options) {
    freeze(state);
    const start = startCommand(state, phase); freeze(start);
    state = startPhase(state, start);
    freeze(state);
    const observation = observationCommand(state, phase); freeze(observation);
    state = observePhase(state, observation);
    freeze(state);
    const completion = completionCommand(state, phase); freeze(completion);
    state = completePhase(state, completion);
  }
  freeze(state);
  const completion = ownerCommand(state, 'completed'); freeze(completion);
  state = completeOperation(state, completion);
  assert.equal(state.operation!.status, 'COMPLETED');
});

test('DEP04 strict Zod rejects unknown keys at every persisted object boundary', () => {
  const fixture = completedFixture().state;
  assert.equal(DeploymentState.safeParse(fixture).success, true);
  function visit(value: unknown, path: string[]): void {
    if (value === null || typeof value !== 'object') return;
    if (!Array.isArray(value)) {
      const changed: unknown = structuredClone(fixture);
      let cursor = changed as Record<string, unknown>;
      for (const key of path) cursor = cursor[key] as Record<string, unknown>;
      cursor.unrecognized_control = 'do-not-echo';
      assert.equal(DeploymentState.safeParse(changed).success, false, path.join('.'));
    }
    for (const [key, child] of Object.entries(value)) visit(child, [...path, key]);
  }
  visit(fixture, []);
});
test('DEP04 generated state JSON schema exactly matches Zod and has no opaque object controls', async () => {
  const documents = stateSchemaDocuments();
  assert.equal(documents.length, 1);
  function visit(value: unknown): void {
    if (value === null || typeof value !== 'object') return;
    if (Array.isArray(value)) { value.forEach(visit); return; }
    const node = value as Record<string, unknown>;
    if (node.type === 'object') assert.equal(node.additionalProperties, false);
    assert.notEqual(node.additionalProperties, true);
    Object.values(node).forEach(visit);
  }
  const { name, document } = documents[0]!;
  visit(document);
  const generated = await readFile(new URL(`../../../schemas/${name}.schema.json`, import.meta.url), 'utf8');
  assert.equal(generated, `${JSON.stringify(document, null, 2)}\n`);
  assert.deepEqual(JSON.parse(generated), document);
});
