import { canonicalHash } from '../src/contracts/hash.js';
import { PhaseId } from '../src/contracts/primitives.js';
import type { DeploymentIntent, DeploymentPlan, ReleaseManifest } from '../src/contracts/release.js';
import { acquireOperation, completeOperation, completePhase, createInitialState, observePhase, startPhase } from '../src/state/model.js';
import type { AcquireOperationCommand, CompletePhaseCommand, DeploymentState, ObservePhaseCommand, StartPhaseCommand } from '../src/state/model.js';
import { exampleIntent, exampleManifest, examplePlan } from './release-fixtures.js';

// Synthetic protocol inputs only. No approval, journal, read-back or remote effect exists.
export function journal(key: string) {
  return { key: `operations/${key}`, sha256: canonicalHash({ synthetic_journal: key }) };
}
export function stateFixture() {
  const manifest = structuredClone(exampleManifest);
  const plan = structuredClone(examplePlan);
  plan.manifest_sha256 = canonicalHash(manifest);
  plan.expected_environment_revision = 0;
  plan.current_manifest_sha256 = null;
  plan.rollback = { eligibility: 'INELIGIBLE', strategy: 'REPAIR_FORWARD_ONLY', reason_codes: ['initial-release'] };
  const intent = intentFor(plan);
  const state = createInitialState({ target: plan.target, example: true });
  return { state, manifest, plan, intent, acquire: acquisition(state, manifest, plan, intent) };
}
export function intentFor(plan: DeploymentPlan): DeploymentIntent {
  return {
    ...exampleIntent(canonicalHash(plan)), target: structuredClone(plan.target), example: plan.example,
    source_sha: plan.source_sha, manifest_sha256: plan.manifest_sha256,
    expected_environment_revision: plan.expected_environment_revision, current_manifest_sha256: plan.current_manifest_sha256,
    inventory_sha256: plan.inventory_sha256, configuration_sha256: plan.configuration_sha256,
    infrastructure_context_sha256: plan.infrastructure_context_sha256, operation_class: plan.operation_class,
  };
}
export function acquisition(state: DeploymentState, manifest: ReleaseManifest, plan: DeploymentPlan, intent: DeploymentIntent): AcquireOperationCommand {
  return { expected_revision: state.revision, manifest, plan, approved_intent: intent, journal: journal(`${intent.operation_id}/acquired`) };
}
export function ownerCommand(state: DeploymentState, key: string) {
  if (!state.operation) throw new Error('fixture requires operation');
  const { operation_id, nonce, plan_sha256, intent_sha256 } = state.operation;
  return { expected_revision: state.revision, operation_id, nonce, plan_sha256, intent_sha256, journal: journal(`${operation_id}/${key}`) };
}
export function startCommand(state: DeploymentState, phase: PhaseId): StartPhaseCommand {
  const attempt = state.operation!.phases.find(item => item.phase === phase)!.attempts.length + 1;
  return { ...ownerCommand(state, `${phase}/start-${attempt}`), phase, attempt };
}
export function completionCommand(state: DeploymentState, phase: PhaseId): CompletePhaseCommand {
  return { ...ownerCommand(state, `${phase}/completed`), phase };
}
export function observationCommand(state: DeploymentState, phase: PhaseId, outcome: 'APPLIED' | 'NOT_APPLIED' | 'UNKNOWN' = 'APPLIED'): ObservePhaseCommand {
  const record = state.operation!.phases.find(item => item.phase === phase)!;
  const attempt = record.attempts.at(-1)!;
  const { expected_revision, journal: evidence, ...owner } = ownerCommand(state, `${phase}/attempt-${attempt.attempt}/read-${attempt.observations.length + 1}`);
  return {
    expected_revision,
    observation: {
      ...owner, phase, attempt: attempt.attempt, start_sha256: attempt.started.journal.sha256, evidence,
      outcome, postconditions: outcome === 'APPLIED' ? 'VERIFIED' : 'UNVERIFIED',
      components: outcome === 'APPLIED' ? state.operation!.intended.filter(item => item.phase === phase).map(({ component, identity }) => ({ component, identity })) : [],
    },
  };
}
export function finishPhase(state: DeploymentState, phase: PhaseId): DeploymentState {
  state = startPhase(state, startCommand(state, phase));
  state = observePhase(state, observationCommand(state, phase));
  return completePhase(state, completionCommand(state, phase));
}
export function advanceTo(state: DeploymentState, phase: PhaseId): DeploymentState {
  for (const preceding of PhaseId.options.slice(0, PhaseId.options.indexOf(phase))) {
    if (state.operation!.phases.find(item => item.phase === preceding)!.status !== 'COMPLETED') state = finishPhase(state, preceding);
  }
  return state;
}
export function completedFixture() {
  const fixture = stateFixture();
  let state = acquireOperation(fixture.state, fixture.acquire);
  for (const phase of PhaseId.options) state = finishPhase(state, phase);
  state = completeOperation(state, ownerCommand(state, 'completed'));
  return { ...fixture, state };
}
