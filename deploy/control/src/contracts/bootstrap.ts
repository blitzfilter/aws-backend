import { z } from 'zod';
import { Positive, SchemaVersion, Stage, isRealStage } from './primitives.js';
import { parseCalVer } from './tag.js';

// GitHub's fnmatch selector cannot validate calendar dates. Ref checks also need parseCalVer.
const CalVerTagPattern = '[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]-[0-9][0-9][0-9][0-9]';
const WorkflowPath = z.string().regex(/^\.github\/workflows\/[a-zA-Z0-9][a-zA-Z0-9_-]*\.ya?ml$/).max(160);
const ExactCheckName = z.string().min(1).max(200).regex(/^[a-zA-Z0-9][a-zA-Z0-9 ._()/:-]*[a-zA-Z0-9)]$|^[a-zA-Z0-9]$/);
const UtcTimestamp = z.iso.datetime({ precision: 0 });
const NoActors = z.tuple([]);
const Reviewer = z.strictObject({ type: z.enum(['USER', 'TEAM']), id: Positive });
const Reviewers = z.array(Reviewer).min(1).max(6).refine(
  actors => new Set(actors.map(actor => `${actor.type}:${actor.id}`)).size === actors.length,
  'Invalid reviewers',
);
// Native rulesets accept teams and integrations, not individual USER bypass entries.
const CreationActor = z.strictObject({ type: z.enum(['TEAM', 'INTEGRATION']), id: Positive });
const CreationActors = z.array(CreationActor).min(1).refine(
  actors => new Set(actors.map(actor => `${actor.type}:${actor.id}`)).size === actors.length,
  'Invalid creation actors',
);
const RequiredCheck = z.strictObject({
  context: ExactCheckName,
  app_id: Positive,
  workflow_path: WorkflowPath,
  workflow_name: ExactCheckName,
  job_id: z.string().regex(/^[a-zA-Z_][a-zA-Z0-9_-]*$/).max(100),
}).describe('Exact expected check producer; workflow/app association still needs external verification.');
const RequiredChecks = z.array(RequiredCheck).min(1).refine(
  checks => new Set(checks.map(check => check.context)).size === checks.length,
  'Ambiguous required checks',
);

const SubjectConfiguration = z.discriminatedUnion('mode', [
  z.strictObject({ mode: z.literal('DEFAULT'), use_default: z.literal(true) }),
  z.strictObject({
    mode: z.literal('CUSTOMIZED'),
    use_default: z.literal(false),
    // V1 supports only this exact custom template, not arbitrary claim substitution.
    include_claim_keys: z.tuple([z.literal('repo'), z.literal('context')]),
    verified: z.literal(true),
    verified_at: UtcTimestamp,
  }),
]).describe('CUSTOMIZED requires explicit configuration verification. Source assertions are not live proof.');

function environment<N extends 'aws-dev' | 'aws-prod' | 'prod-recovery'>(name: N) {
  const prod = name === 'aws-prod';
  return z.strictObject({
    reviewers: Reviewers,
    prevent_self_review: z.literal(true),
    admin_bypass: z.literal(false),
    deployment_ref: z.strictObject({
      type: z.literal(prod ? 'TAG' : 'BRANCH'),
      pattern: z.literal(prod ? CalVerTagPattern : 'develop'),
    }),
    workflow: z.strictObject({
      path: WorkflowPath,
      ref_pattern: z.literal(prod ? `refs/tags/${CalVerTagPattern}` : 'refs/heads/develop'),
    }),
    oidc: z.strictObject({
      issuer: z.literal('https://token.actions.githubusercontent.com'),
      aud: z.literal('sts.amazonaws.com'),
      sub: z.literal(`repo:aura-historia/backend:environment:${name}`),
    }),
  });
}

const TagRuleset = z.strictObject({
  enforcement: z.literal('ACTIVE'),
  target: z.literal('TAG'),
  include: z.tuple([z.literal(`refs/tags/${CalVerTagPattern}`)]),
  exclude: z.tuple([]),
});

export const GitHubBootstrap = z.strictObject({
  schema_version: SchemaVersion,
  example: z.boolean(),
  repository: z.literal('aura-historia/backend'),
  default_branch: z.literal('develop'),
  merge: z.strictObject({
    allow_squash_merge: z.literal(true),
    allow_merge_commit: z.literal(false),
    allow_rebase_merge: z.literal(false),
  }),
  branch_protection: z.strictObject({
    ref: z.literal('refs/heads/develop'),
    require_pull_request: z.literal(true),
    require_up_to_date_branch: z.literal(true),
    required_checks: RequiredChecks,
    allow_force_pushes: z.literal(false),
    allow_deletions: z.literal(false),
    bypass_actors: NoActors,
  }),
  environments: z.strictObject({
    'aws-dev': environment('aws-dev'),
    'aws-prod': environment('aws-prod'),
    'prod-recovery': environment('prod-recovery'),
  }),
  oidc_subject_configuration: SubjectConfiguration,
  tag_protection: z.strictObject({
    format: z.literal('UTC_CALVER'),
    timezone: z.literal('UTC'),
    creation_ruleset: TagRuleset.extend({
      restrict_creations: z.literal(true),
      authorized_actors: CreationActors,
    }),
    immutability_ruleset: TagRuleset.extend({
      deny_updates: z.literal(true),
      deny_deletions: z.literal(true),
      bypass_actors: NoActors,
    }),
  }).describe('Two separate rulesets. Creation authorization MUST NOT bypass update/delete denial.'),
  external_observations: z.strictObject({
    observed_at: UtcTimestamp,
    environment_protection_available: z.boolean(),
    tag_rulesets_available: z.boolean(),
  }).nullable().describe('Untrusted observation summary, never approval or evidence validation.'),
}).describe('Schema v1 offline owner inputs only. Semantic parsing and later live verification remain mandatory.');
export type GitHubBootstrap = z.infer<typeof GitHubBootstrap>;

/** Safe boundary: no Zod issues, input keys, provider bodies or getter exceptions escape. */
export function parseGitHubBootstrap(input: unknown, stage: Stage = 'prod'): GitHubBootstrap {
  try {
    const target = Stage.parse(stage);
    const result = GitHubBootstrap.safeParse(input);
    if (result.success && !(isRealStage(target) && result.data.example)) return result.data;
  } catch {
    // Deliberately discard untrusted error details.
  }
  throw new Error('Invalid GitHub bootstrap specification');
}

/** A ref-policy match is NOT deployment authorization or bootstrap readiness. */
export function matchesGitHubDeploymentRefPolicy(
  input: unknown,
  environmentName: keyof GitHubBootstrap['environments'],
  ref: string,
  stage: Stage = 'prod',
): boolean {
  try {
    const spec = parseGitHubBootstrap(input, stage);
    if (!Object.hasOwn(spec.environments, environmentName)) return false;
    if (environmentName !== 'aws-prod') return ref === 'refs/heads/develop';
    if (!ref.startsWith('refs/tags/')) return false;
    parseCalVer(ref.slice('refs/tags/'.length));
    return true;
  } catch {
    return false;
  }
}

/** Iteration 12 must validate actual setup separately; no source-controlled flag can enable it. */
export function githubBootstrapStatus(input: unknown, stage: Stage = 'prod') {
  const disabled = { bootstrap_ready: false, production_enabled: false } as const;
  try {
    const spec = parseGitHubBootstrap(input, stage);
    const observed = spec.external_observations;
    const reason = observed === null ? 'EXTERNAL_OBSERVATIONS_REQUIRED'
      : !observed.environment_protection_available || !observed.tag_rulesets_available ? 'PROTECTION_UNAVAILABLE'
        : 'LIVE_VERIFICATION_REQUIRED';
    return { ...disabled, spec_valid: true, reason } as const;
  } catch {
    return { ...disabled, spec_valid: false, reason: 'INVALID_SPEC' } as const;
  }
}
