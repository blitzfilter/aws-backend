import { z } from 'zod';
import { canonicalHash } from '../contracts/hash.js';
import { Architecture, Digest, PhaseId, Positive, Revision, SafeKey, SchemaVersion, Sha, WorkerScope, isRealStage } from '../contracts/primitives.js';
import { DeploymentIntent, DeploymentPlan, ReleaseManifest, assertIntentBinding, parseIntent, parsePlan } from '../contracts/release.js';

// Pure operational protocol, not an approval, storage or remote-execution adapter.
// The caller MUST independently authenticate the approved intent, verify manifest
// provenance/artifacts and authenticate read-back evidence. Parsing is NOT authorization.
// Manifest/plan/intent identities here use canonicalHash; artifact byte hashes do not.
// Persist each returned revision with the loaded storage token (e.g. ETag CAS) BEFORE
// executing a newly started effect. A successful start is not permission to execute twice.
// No clock, timeout takeover, rollback reset, credentials, payloads or receipt handles.
const JournalReference = z.strictObject({ key: SafeKey, sha256: Digest })
  .describe('Immutable content-free journal/evidence reference in an adapter-owned namespace; not a URL or fetch authorization.');
const NativeName = ReleaseManifest.shape.native_images.element.shape.component.exclude(['aura-historia-worker', 'aura-historia-migrate']);
const LambdaName = ReleaseManifest.shape.lambdas.element.shape.component;
const SearchFamily = ReleaseManifest.shape.search.shape.families.element.shape.family;
const Database = ReleaseManifest.shape.migrations.shape.streams.element.shape.stream;
const Stack = DeploymentPlan.shape.infrastructure_diff.shape.changes.element.shape.stack;
const Component = z.discriminatedUnion('kind', [
  z.strictObject({ kind: z.literal('NATIVE'), component: NativeName, architecture: Architecture }),
  z.strictObject({ kind: z.literal('WORKER'), scope: WorkerScope, architecture: Architecture }),
  z.strictObject({ kind: z.literal('MIGRATOR'), architecture: Architecture })
    .describe('Verified aura-historia-migrate tool image for postgres_expand; not a running daemon, singleton or job owner.'),
  z.strictObject({ kind: z.literal('LAMBDA'), component: LambdaName, architecture: Architecture }),
  z.strictObject({ kind: z.literal('POSTGRES'), stream: Database }),
  z.strictObject({ kind: z.literal('SEARCH'), family: SearchFamily }),
  z.strictObject({ kind: z.literal('INFRASTRUCTURE'), stack: Stack }),
  z.strictObject({ kind: z.literal('MAIL') }),
]);
type Component = z.infer<typeof Component>;
const ComponentIdentity = z.strictObject({ source_sha: Sha, definition_sha256: Digest })
  .describe('Canonical hash of the complete immutable manifest component entry, not an artifact byte digest. Worker identity hashes { image: native entry, queue: scope contract }. Infrastructure identity hashes the infrastructure entry, separately observed for every stack.');
const IntendedComponent = z.strictObject({ component: Component, identity: ComponentIdentity, phase: PhaseId });
const ComponentReadBack = z.strictObject({ component: Component, identity: ComponentIdentity.nullable() })
  .describe('An independently read-back identity; null means unknown. Planned targets are never observations.');
const OwnerIdentity = DeploymentIntent.pick({ operation_id: true, nonce: true, plan_sha256: true })
  .extend({ intent_sha256: Digest });
const ObservationIdentity = OwnerIdentity.extend({ phase: PhaseId, attempt: Positive, start_sha256: Digest });
const ObservationReference = ObservationIdentity.extend({ evidence: JournalReference });
const PhaseObservation = ObservationReference.extend({
  outcome: z.enum(['APPLIED', 'NOT_APPLIED', 'UNKNOWN']),
  postconditions: z.enum(['VERIFIED', 'UNVERIFIED']),
  components: z.array(ComponentReadBack).max(256),
}).describe('Read-back facts are retained for every outcome. Completion evidence must itself be APPLIED/VERIFIED and bound to the completing phase, current attempt and start journal digest.');
const ActualComponent = ComponentReadBack.extend({ observation: ObservationReference });
const JournalEvent = z.strictObject({ revision: Revision, journal: JournalReference });
const Attempt = z.strictObject({
  attempt: Positive, started: JournalEvent,
  observations: z.array(z.strictObject({ revision: Revision, observation: PhaseObservation })),
});
const Phase = z.strictObject({
  phase: PhaseId, status: z.enum(['PLANNED', 'STARTED', 'OBSERVED', 'COMPLETED', 'FAILED', 'UNCERTAIN']),
  attempts: z.array(Attempt), completed: JournalEvent.nullable(),
});
const Operation = OwnerIdentity.extend({
  status: z.enum(['OWNED', 'COMPLETED']),
  approved_environment_revision: Revision, approved_current_manifest_sha256: Digest.nullable(),
  manifest_sha256: Digest, source_sha: Sha,
  acquired: JournalReference, intended: z.array(IntendedComponent).min(1).max(256)
    .describe('One architecture per logical native component, worker scope, migrator and Lambda. Plans have no reviewed variant selector or retirement protocol: multiple variants and changes to an existing component-key set are unsupported before acquisition.'),
  phases: z.array(Phase).length(PhaseId.options.length), completed: JournalEvent.nullable(),
});
type Operation = z.infer<typeof Operation>;
type Phase = z.infer<typeof Phase>;
type PhaseObservation = z.infer<typeof PhaseObservation>;
type ActualComponent = z.infer<typeof ActualComponent>;

export const DeploymentState = z.strictObject({
  schema_version: SchemaVersion, kind: z.literal('DEPLOYMENT_STATE'), example: z.boolean(),
  target: DeploymentPlan.shape.target, revision: Revision,
  current_manifest_sha256: Digest.nullable()
    .describe('Last fully completed release ONLY. During an owned operation, actual components may be mixed or unknown.'),
  actual: z.array(ActualComponent).max(256)
    .describe('Last read-back per component, including failed/unverified attempts; never dropped to accommodate target changes. These facts alone do not prove completion: it needs APPLIED/VERIFIED current-attempt evidence from the required phase, or a successful final verification phase. In-flight effects may have changed since read-back.'),
  operation: Operation.nullable()
    .describe('OWNED retains the durable nonce through failures and uncertainty. COMPLETED releases ownership but retains the last binding/journal for exact replay.'),
}).describe('Strict operational state v1. Use parseState as well as JSON Schema: ordering, revision continuity, ownership, evidence and target-equality checks are semantic. No timed owner stealing. No storage CAS or approval verification is implemented here.');
export type DeploymentState = z.infer<typeof DeploymentState>;

const InitialCommand = z.strictObject({ target: DeploymentPlan.shape.target, example: z.boolean() });
const AcquireCommand = z.strictObject({
  expected_revision: Revision, approved_intent: DeploymentIntent, plan: DeploymentPlan,
  manifest: ReleaseManifest, journal: JournalReference,
});
export type AcquireOperationCommand = z.infer<typeof AcquireCommand>;
const OperationCommand = OwnerIdentity.extend({ expected_revision: Revision, journal: JournalReference });
export type CompleteOperationCommand = z.infer<typeof OperationCommand>;
const PhaseCommand = OperationCommand.extend({ phase: PhaseId });
export type CompletePhaseCommand = z.infer<typeof PhaseCommand>;
const StartCommand = PhaseCommand.extend({ attempt: Positive });
export type StartPhaseCommand = z.infer<typeof StartCommand>;
const ObserveCommand = z.strictObject({ expected_revision: Revision, observation: PhaseObservation });
export type ObservePhaseCommand = z.infer<typeof ObserveCommand>;
const UncertainCommand = ObservationReference.extend({ expected_revision: Revision });
export type MarkUncertainCommand = z.infer<typeof UncertainCommand>;

function requireValid(condition: boolean): asserts condition {
  if (!condition) throw new Error('invalid state transition');
}
class UnsupportedTargetTransition extends Error {}
function safely<T>(label: string, work: () => T): T {
  try { return work(); } catch (error) {
    throw new Error(error instanceof UnsupportedTargetTransition ? 'unsupported target transition' : `invalid ${label}`);
  }
}
function cleanStrings(value: unknown): void {
  if (typeof value === 'string') requireValid(!/[\u0000-\u001f\u007f]/.test(value));
  else if (Array.isArray(value)) value.forEach(cleanStrings);
  else if (value !== null && typeof value === 'object') Object.values(value).forEach(cleanStrings);
}
function same(a: unknown, b: unknown): boolean { return canonicalHash(a) === canonicalHash(b); }
function componentKey(component: Component): string { return canonicalHash(component); }
function uniqueComponents(items: readonly { component: Component }[]): void {
  requireValid(new Set(items.map(item => componentKey(item.component))).size === items.length);
}
function singleArchitectureSelection(items: readonly { component: Component }[]): boolean {
  const logicalKeys = items.map(({ component }) => {
    if ('architecture' in component) {
      const { architecture: _architecture, ...logicalComponent } = component;
      return canonicalHash(logicalComponent);
    }
    return componentKey(component);
  });
  return new Set(logicalKeys).size === items.length;
}
function requireSupportedTargetTransition(state: DeploymentState, intended: Operation['intended']): void {
  uniqueComponents(intended);
  const keys = new Set(intended.map(item => componentKey(item.component)));
  if (!singleArchitectureSelection(intended) || (state.operation !== null
    && (state.actual.length !== keys.size || state.actual.some(item => !keys.has(componentKey(item.component)))))) {
    throw new UnsupportedTargetTransition();
  }
}
function ownerMatches(a: z.infer<typeof OwnerIdentity>, b: z.infer<typeof OwnerIdentity>): boolean {
  return a.operation_id === b.operation_id && a.nonce === b.nonce
    && a.plan_sha256 === b.plan_sha256 && a.intent_sha256 === b.intent_sha256;
}
function componentPhase(component: Component): PhaseId {
  switch (component.kind) {
    case 'INFRASTRUCTURE': return 'aws_prerequisites';
    case 'MIGRATOR':
    case 'POSTGRES': return 'postgres_expand';
    case 'SEARCH': return 'opensearch_expand';
    case 'MAIL': return 'prepare_compute';
    case 'WORKER': return 'replace_worker_scopes';
    case 'LAMBDA': return 'activate_lambda';
    case 'NATIVE': return component.component === 'aura-historia-api' ? 'switch_api' : 'handover_jobs';
  }
}
function intendedComponents(manifest: ReleaseManifest): Operation['intended'] {
  const intended: Operation['intended'] = [];
  const add = (component: Component, definition: unknown): void => {
    intended.push({ component, identity: { source_sha: manifest.source_sha, definition_sha256: canonicalHash(definition) }, phase: componentPhase(component) });
  };
  for (const image of manifest.native_images) {
    if (image.component === 'aura-historia-worker') {
      for (const queue of manifest.queues) add({ kind: 'WORKER', scope: queue.scope, architecture: image.architecture }, { image, queue });
    } else if (image.component === 'aura-historia-migrate') {
      add({ kind: 'MIGRATOR', architecture: image.architecture }, image);
    } else add({ kind: 'NATIVE', component: image.component, architecture: image.architecture }, image);
  }
  for (const lambda of manifest.lambdas) add({ kind: 'LAMBDA', component: lambda.component, architecture: lambda.architecture }, lambda);
  for (const stream of manifest.migrations.streams) add({ kind: 'POSTGRES', stream: stream.stream }, stream);
  for (const family of manifest.search.families) add({ kind: 'SEARCH', family: family.family }, family);
  for (const stack of Stack.options) add({ kind: 'INFRASTRUCTURE', stack }, manifest.infrastructure);
  add({ kind: 'MAIL' }, manifest.mail_bundle);
  return intended;
}
function checkIntended(operation: Operation): void {
  uniqueComponents(operation.intended);
  requireValid(singleArchitectureSelection(operation.intended));
  const components = operation.intended.map(item => item.component);
  for (const name of NativeName.options) {
    requireValid(components.some(component => component.kind === 'NATIVE' && component.component === name));
  }
  requireValid(components.some(component => component.kind === 'MIGRATOR'));
  for (const name of LambdaName.options) requireValid(components.some(component => component.kind === 'LAMBDA' && component.component === name));
  for (const scope of WorkerScope.options) requireValid(components.some(component => component.kind === 'WORKER' && component.scope === scope));
  for (const stream of Database.options) requireValid(components.some(component => component.kind === 'POSTGRES' && component.stream === stream));
  for (const family of SearchFamily.options) requireValid(components.some(component => component.kind === 'SEARCH' && component.family === family));
  for (const stack of Stack.options) requireValid(components.some(component => component.kind === 'INFRASTRUCTURE' && component.stack === stack));
  requireValid(components.some(component => component.kind === 'MAIL'));
  requireValid(operation.intended.every(item => item.phase === componentPhase(item.component) && item.identity.source_sha === operation.source_sha));
}
function actualFrom(observation: PhaseObservation): ActualComponent[] {
  const { outcome: _outcome, postconditions: _postconditions, components, ...reference } = observation;
  return components.map(component => ({ ...component, observation: reference }));
}
function verifiedInCurrentAttempt(actual: ActualComponent, operation: Operation, phase: Phase): boolean {
  const attempt = phase.attempts.at(-1);
  return attempt !== undefined && applied(attempt.observations.at(-1)?.observation)
    && ownerMatches(actual.observation, operation) && actual.observation.phase === phase.phase
    && actual.observation.attempt === attempt.attempt && actual.observation.start_sha256 === attempt.started.journal.sha256
    && attempt.observations.some(({ observation }) => applied(observation)
      && actualFrom(observation).some(candidate => same(candidate, actual)));
}
function targetsKnown(actual: readonly ActualComponent[], operation: Operation, completingPhase?: PhaseId): boolean {
  const allTargets = completingPhase === undefined || completingPhase === 'verify_release_behavior' || completingPhase === 'commit_release';
  const required = operation.intended.filter(item => allTargets || item.phase === completingPhase);
  return required.every(target => {
    const item = actual.find(item => same(item.component, target.component));
    if (!item || item.identity === null || !same(item.identity, target.identity)) return false;
    const evidencePhase = operation.phases.find(phase => phase.phase === item.observation.phase);
    if (!evidencePhase || (evidencePhase.status !== 'COMPLETED' && evidencePhase.phase !== completingPhase)) return false;
    // Final checks may retain successful effect-phase evidence or use later verified
    // behavior read-back. Neither owner equality nor an old attempt upgrades evidence.
    if (allTargets) {
      if (evidencePhase.phase !== target.phase && evidencePhase.phase !== 'verify_release_behavior') return false;
    } else if (evidencePhase.phase !== completingPhase) return false;
    return verifiedInCurrentAttempt(item, operation, evidencePhase);
  });
}
function checkObservation(observation: PhaseObservation, operation: Operation, phase: Phase, attempt: z.infer<typeof Attempt>): void {
  requireValid(ownerMatches(observation, operation) && observation.phase === phase.phase
    && observation.attempt === attempt.attempt && observation.start_sha256 === attempt.started.journal.sha256);
  requireValid(observation.outcome === 'APPLIED' || observation.postconditions === 'UNVERIFIED');
  uniqueComponents(observation.components);
  requireValid(observation.components.every(item => operation.intended.some(target => same(target.component, item.component)
    && (target.phase === phase.phase || phase.phase === 'verify_release_behavior'))));
}
function checkFollowup(previous: PhaseObservation | undefined, next: PhaseObservation): void {
  // APPLIED may need more postcondition/identity read-back, never another execution.
  if (previous && previous.outcome !== 'UNKNOWN') requireValid(previous.outcome === 'APPLIED' && next.outcome === 'APPLIED');
}
function applied(observation: PhaseObservation | undefined): boolean {
  return observation?.outcome === 'APPLIED' && observation.postconditions === 'VERIFIED';
}
function lastObservation(phase: Phase): PhaseObservation | undefined {
  return phase.attempts.at(-1)?.observations.at(-1)?.observation;
}
function checkPhaseTargets(actual: readonly ActualComponent[], operation: Operation, phase: PhaseId): void {
  requireValid(targetsKnown(actual, operation, phase));
}
function checkState(state: DeploymentState): void {
  cleanStrings(state);
  requireValid(!state.example || !isRealStage(state.target.stage));
  uniqueComponents(state.actual);
  const operation = state.operation;
  if (!operation) {
    requireValid(state.revision === 0 && state.current_manifest_sha256 === null && state.actual.length === 0);
    return;
  }
  checkIntended(operation);
  const intendedKeys = new Set(operation.intended.map(item => componentKey(item.component)));
  requireValid(state.actual.every(item => intendedKeys.has(componentKey(item.component))));
  let revision = operation.approved_environment_revision + 1;
  requireValid(revision <= state.revision);
  const observed = new Map<string, ActualComponent>();
  const starts = new Set<string>();
  const evidence = new Set<string>();
  let precedingComplete = true;
  for (const [index, phase] of operation.phases.entries()) {
    requireValid(phase.phase === PhaseId.options[index]);
    if (!precedingComplete) requireValid(phase.status === 'PLANNED');
    precedingComplete = phase.status === 'COMPLETED';
    for (const [index, attempt] of phase.attempts.entries()) {
      requireValid(attempt.attempt === index + 1 && attempt.started.revision === ++revision);
      requireValid(!starts.has(attempt.started.journal.key) && !starts.has(attempt.started.journal.sha256));
      starts.add(attempt.started.journal.key); starts.add(attempt.started.journal.sha256);
      if (index > 0) requireValid(phase.attempts[index - 1]!.observations.at(-1)?.observation.outcome === 'NOT_APPLIED');
      let previous: PhaseObservation | undefined;
      for (const entry of attempt.observations) {
        requireValid(entry.revision === ++revision);
        checkObservation(entry.observation, operation, phase, attempt);
        checkFollowup(previous, entry.observation);
        const reference = entry.observation.evidence;
        requireValid(!evidence.has(reference.key) && !evidence.has(reference.sha256));
        evidence.add(reference.key); evidence.add(reference.sha256);
        previous = entry.observation;
        for (const actual of actualFrom(entry.observation)) observed.set(componentKey(actual.component), actual);
      }
    }
    const last = lastObservation(phase);
    const expectedStatus = phase.attempts.length === 0 ? 'PLANNED' : !last ? 'STARTED'
      : last.outcome === 'UNKNOWN' ? 'UNCERTAIN' : last.outcome === 'NOT_APPLIED' ? 'FAILED' : 'OBSERVED';
    if (phase.completed) {
      requireValid(phase.status === 'COMPLETED' && applied(last) && phase.completed.revision === ++revision);
      checkPhaseTargets([...observed.values()], operation, phase.phase);
    } else requireValid(phase.status === expectedStatus);
  }
  for (const actual of observed.values()) requireValid(state.actual.some(item => same(item, actual)));
  for (const actual of state.actual) {
    if (actual.observation.operation_id === operation.operation_id || actual.observation.nonce === operation.nonce) {
      requireValid(same(observed.get(componentKey(actual.component)) ?? null, actual));
    }
  }
  if (operation.status === 'COMPLETED') {
    requireValid(operation.completed !== null && operation.completed.revision === ++revision);
    requireValid(operation.phases.every(phase => phase.status === 'COMPLETED'));
    requireValid(state.actual.length === operation.intended.length && targetsKnown(state.actual, operation));
    requireValid(state.current_manifest_sha256 === operation.manifest_sha256);
  } else {
    requireValid(operation.completed === null && state.current_manifest_sha256 === operation.approved_current_manifest_sha256);
  }
  requireValid(revision === state.revision);
}

/** Safe generic error only; never return raw Zod errors, input fields or causes. */
export function parseState(input: unknown): DeploymentState {
  return safely('deployment state', () => {
    const state = DeploymentState.parse(input);
    checkState(state);
    return state;
  });
}
export function createInitialState(input: z.infer<typeof InitialCommand>): DeploymentState {
  return safely('state transition', () => {
    const command = InitialCommand.parse(input);
    return parseState({ schema_version: 1, kind: 'DEPLOYMENT_STATE', ...command, revision: 0, current_manifest_sha256: null, actual: [], operation: null });
  });
}
function current(input: unknown, expected: number): DeploymentState {
  const state = parseState(input);
  requireValid(state.revision === expected);
  return state;
}
function owned(state: DeploymentState, identity: z.infer<typeof OwnerIdentity>, allowCompleted = false): Operation {
  const operation = state.operation;
  requireValid(operation !== null && ownerMatches(operation, identity) && (allowCompleted || operation.status === 'OWNED'));
  return operation;
}
function selectedPhase(operation: Operation, phaseId: PhaseId): Phase {
  const index = PhaseId.options.indexOf(phaseId);
  requireValid(operation.phases.slice(0, index).every(phase => phase.status === 'COMPLETED'));
  return operation.phases[index]!;
}
function advance(state: DeploymentState): DeploymentState {
  state.revision += 1;
  return parseState(state);
}

/** approved_intent must already be independently authenticated. Reconciliation uses
 * today's expected_revision but the ORIGINAL approved revision/plan/intent/nonce.
 * Even exact idempotent replays require a fresh conditional revision token.
 * Architecture/key-set changes and multiple variants are unsupported: no reviewed
 * retirement/target-selection protocol exists. Reject before changing ownership. */
export function acquireOperation(input: unknown, commandInput: AcquireOperationCommand): DeploymentState {
  return safely('state transition', () => {
    const command = AcquireCommand.parse(commandInput);
    cleanStrings(command);
    const state = current(input, command.expected_revision);
    const intent = parseIntent(command.approved_intent);
    const plan = parsePlan(command.plan);
    const { schema_version: _version, kind: _kind, example: _example, ...binding } = intent;
    // This checks exact bindings/readiness, NOT approval: trust is the adapter's precondition.
    assertIntentBinding(intent, plan, { ...binding, hash_plan: canonicalHash });
    requireValid(same(state.target, intent.target) && state.example === intent.example && command.manifest.example === intent.example);
    requireValid(canonicalHash(command.manifest) === intent.manifest_sha256 && command.manifest.source_sha === intent.source_sha);
    const intended = intendedComponents(command.manifest);
    const identity = { operation_id: intent.operation_id, nonce: intent.nonce, plan_sha256: intent.plan_sha256, intent_sha256: canonicalHash(intent) };
    const previous = state.operation;
    if (previous && (previous.status === 'OWNED' || previous.operation_id === intent.operation_id || previous.nonce === intent.nonce)) {
      requireValid(ownerMatches(previous, identity) && previous.approved_environment_revision === intent.expected_environment_revision
        && previous.approved_current_manifest_sha256 === intent.current_manifest_sha256
        && previous.manifest_sha256 === intent.manifest_sha256 && previous.source_sha === intent.source_sha
        && same(previous.intended, intended) && same(previous.acquired, command.journal));
      return state;
    }
    requireValid(intent.expected_environment_revision === state.revision && intent.current_manifest_sha256 === state.current_manifest_sha256);
    requireSupportedTargetTransition(state, intended);
    state.operation = {
      ...identity, status: 'OWNED', approved_environment_revision: intent.expected_environment_revision,
      approved_current_manifest_sha256: intent.current_manifest_sha256, manifest_sha256: intent.manifest_sha256, source_sha: intent.source_sha,
      acquired: command.journal, intended,
      phases: PhaseId.options.map(phase => ({ phase, status: 'PLANNED', attempts: [], completed: null })), completed: null,
    };
    return advance(state);
  });
}

/** STARTED/UNCERTAIN are never retry permission, even for an identical start request. */
export function startPhase(input: unknown, commandInput: StartPhaseCommand): DeploymentState {
  return safely('state transition', () => {
    const command = StartCommand.parse(commandInput);
    cleanStrings(command);
    const state = current(input, command.expected_revision);
    const operation = owned(state, command);
    const phase = selectedPhase(operation, command.phase);
    requireValid(phase.status === 'PLANNED' || (phase.status === 'FAILED' && lastObservation(phase)?.outcome === 'NOT_APPLIED'));
    requireValid(command.attempt === phase.attempts.length + 1);
    phase.attempts.push({ attempt: command.attempt, started: { revision: state.revision + 1, journal: command.journal }, observations: [] });
    phase.status = 'STARTED';
    return advance(state);
  });
}
export function observePhase(input: unknown, commandInput: ObservePhaseCommand): DeploymentState {
  return safely('state transition', () => {
    const command = ObserveCommand.parse(commandInput);
    cleanStrings(command);
    const state = current(input, command.expected_revision);
    const observation = command.observation;
    const operation = owned(state, observation);
    const phase = selectedPhase(operation, observation.phase);
    const attempt = phase.attempts.at(-1);
    requireValid(attempt !== undefined);
    checkObservation(observation, operation, phase, attempt);
    if (attempt.observations.some(entry => same(entry.observation, observation))) return state;
    requireValid(phase.status !== 'COMPLETED');
    checkFollowup(lastObservation(phase), observation);
    // One immutable evidence reference cannot be rebound to a different assertion.
    requireValid(!operation.phases.some(item => item.attempts.some(attempt => attempt.observations.some(entry =>
      entry.observation.evidence.key === observation.evidence.key || entry.observation.evidence.sha256 === observation.evidence.sha256))));
    attempt.observations.push({ revision: state.revision + 1, observation });
    phase.status = observation.outcome === 'UNKNOWN' ? 'UNCERTAIN' : observation.outcome === 'NOT_APPLIED' ? 'FAILED' : 'OBSERVED';
    for (const actual of actualFrom(observation)) {
      const index = state.actual.findIndex(item => same(item.component, actual.component));
      if (index < 0) state.actual.push(actual); else state.actual[index] = actual;
    }
    return advance(state);
  });
}
export function markUncertain(input: unknown, commandInput: MarkUncertainCommand): DeploymentState {
  return safely('state transition', () => {
    const { expected_revision, ...reference } = UncertainCommand.parse(commandInput);
    return observePhase(input, { expected_revision, observation: { ...reference, outcome: 'UNKNOWN', postconditions: 'UNVERIFIED', components: [] } });
  });
}
export function completePhase(input: unknown, commandInput: CompletePhaseCommand): DeploymentState {
  return safely('state transition', () => {
    const command = PhaseCommand.parse(commandInput);
    cleanStrings(command);
    const state = current(input, command.expected_revision);
    const operation = owned(state, command);
    const phase = selectedPhase(operation, command.phase);
    if (phase.completed) { requireValid(same(phase.completed.journal, command.journal)); return state; }
    requireValid(phase.status === 'OBSERVED' && applied(lastObservation(phase)));
    checkPhaseTargets(state.actual, operation, phase.phase);
    phase.status = 'COMPLETED';
    phase.completed = { revision: state.revision + 1, journal: command.journal };
    return advance(state);
  });
}
export function completeOperation(input: unknown, commandInput: CompleteOperationCommand): DeploymentState {
  return safely('state transition', () => {
    const command = OperationCommand.parse(commandInput);
    cleanStrings(command);
    const state = current(input, command.expected_revision);
    const operation = owned(state, command, true);
    if (operation.completed) { requireValid(same(operation.completed.journal, command.journal)); return state; }
    requireValid(operation.phases.every(phase => phase.status === 'COMPLETED'));
    requireValid(state.actual.length === operation.intended.length && targetsKnown(state.actual, operation));
    operation.status = 'COMPLETED';
    operation.completed = { revision: state.revision + 1, journal: command.journal };
    state.current_manifest_sha256 = operation.manifest_sha256;
    return advance(state);
  });
}
