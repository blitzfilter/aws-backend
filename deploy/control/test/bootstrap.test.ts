import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { GitHubBootstrap, githubBootstrapStatus, matchesGitHubDeploymentRefPolicy, parseGitHubBootstrap } from '../src/contracts/bootstrap.js';
import { githubBootstrapJsonSchema } from '../src/contracts/export-bootstrap-schema.js';

const tagPattern = '[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]-[0-9][0-9][0-9][0-9]';
const names = ['aws-dev', 'aws-prod', 'prod-recovery'] as const;
const observed = {
  observed_at: '2026-09-12T12:30:00Z',
  environment_protection_available: true,
  tag_rulesets_available: true,
};
const customSubject = {
  mode: 'CUSTOMIZED', use_default: false, include_claim_keys: ['repo', 'context'],
  verified: true, verified_at: observed.observed_at,
};

// All numeric identities and the recovery workflow are synthetic, only in example/test context.
function fixture(): GitHubBootstrap {
  function environment<N extends typeof names[number]>(name: N) {
    const prod = name === 'aws-prod';
    return {
      reviewers: [{ type: 'USER', id: 101 }, { type: 'TEAM', id: 201 }],
      prevent_self_review: true,
      admin_bypass: false,
      deployment_ref: { type: prod ? 'TAG' : 'BRANCH', pattern: prod ? tagPattern : 'develop' },
      workflow: {
        path: name === 'prod-recovery' ? '.github/workflows/test-recovery.yml' : '.github/workflows/deploy.yml',
        ref_pattern: prod ? `refs/tags/${tagPattern}` : 'refs/heads/develop',
      },
      oidc: {
        issuer: 'https://token.actions.githubusercontent.com',
        aud: 'sts.amazonaws.com',
        sub: `repo:aura-historia/backend:environment:${name}`,
      },
    } as const;
  }
  // A fresh mutable JSON value avoids sharing fixture arrays between tests.
  return JSON.parse(JSON.stringify({
    schema_version: 1,
    example: true,
    repository: 'aura-historia/backend',
    default_branch: 'develop',
    merge: { allow_squash_merge: true, allow_merge_commit: false, allow_rebase_merge: false },
    branch_protection: {
      ref: 'refs/heads/develop', require_pull_request: true, require_up_to_date_branch: true,
      required_checks: [{
        context: 'Lint Workspace', app_id: 301, workflow_path: '.github/workflows/integrate.yml',
        workflow_name: 'Integrate (CI)', job_id: 'lint',
      }],
      allow_force_pushes: false, allow_deletions: false, bypass_actors: [],
    },
    environments: {
      'aws-dev': environment('aws-dev'),
      'aws-prod': environment('aws-prod'),
      'prod-recovery': environment('prod-recovery'),
    },
    oidc_subject_configuration: { mode: 'DEFAULT', use_default: true },
    tag_protection: {
      format: 'UTC_CALVER', timezone: 'UTC',
      creation_ruleset: {
        enforcement: 'ACTIVE', target: 'TAG', include: [`refs/tags/${tagPattern}`], exclude: [],
        restrict_creations: true, authorized_actors: [{ type: 'TEAM', id: 201 }, { type: 'INTEGRATION', id: 401 }],
      },
      immutability_ruleset: {
        enforcement: 'ACTIVE', target: 'TAG', include: [`refs/tags/${tagPattern}`], exclude: [],
        deny_updates: true, deny_deletions: true, bypass_actors: [],
      },
    },
    external_observations: null,
  }));
}

type Path = readonly (string | number)[];
function changed(input: unknown, path: Path, value: unknown): unknown {
  const copy = structuredClone(input) as Record<string | number, unknown>;
  let parent = copy;
  for (const key of path.slice(0, -1)) parent = parent[key] as typeof parent;
  const key = path.at(-1)!;
  if (value === undefined) delete parent[key];
  else parent[key] = value;
  return copy;
}
function invalid(input: unknown): void {
  assert.equal(GitHubBootstrap.safeParse(input).success, false);
  assert.throws(() => parseGitHubBootstrap(input, 'test'), {
    name: 'Error', message: 'Invalid GitHub bootstrap specification',
  });
}

test('synthetic policy is valid only in an explicit non-real example context', () => {
  const spec = fixture();
  assert.deepEqual(parseGitHubBootstrap(spec, 'test'), spec);
  for (const stage of ['dev', 'prod'] as const) {
    assert.throws(() => parseGitHubBootstrap(spec, stage), /Invalid GitHub bootstrap specification/);
    assert.equal(githubBootstrapStatus(spec, stage).spec_valid, false);
  }
  assert.throws(() => parseGitHubBootstrap(spec), /Invalid GitHub bootstrap specification/);
});

test('committed draft has no fabricated identities and stays invalid even with example flag removed', () => {
  const draft = JSON.parse(readFileSync(new URL('../../../bootstrap/github.example.json', import.meta.url), 'utf8'));
  assert.equal(draft.example, true);
  for (const name of names) {
    assert.deepEqual(draft.environments[name].reviewers, []);
    assert.equal(draft.environments[name].workflow.path, null);
  }
  assert.deepEqual(draft.branch_protection.required_checks, []);
  assert.deepEqual(draft.tag_protection.creation_ruleset.authorized_actors, []);
  assert.equal(draft.oidc_subject_configuration, null);
  assert.equal(draft.external_observations, null);
  invalid(draft);
  invalid(changed(draft, ['example'], false));
  assert.equal(githubBootstrapStatus(draft).production_enabled, false);
});

const invalidCases: { name: string; path: Path; value: unknown }[] = [
  { name: 'missing version', path: ['schema_version'], value: undefined },
  { name: 'unsupported version', path: ['schema_version'], value: 2 },
  { name: 'missing example flag', path: ['example'], value: undefined },
  { name: 'repository typo', path: ['repository'], value: 'aura-historia/backed' },
  { name: 'wrong default branch', path: ['default_branch'], value: 'prod' },
  { name: 'squash disabled', path: ['merge', 'allow_squash_merge'], value: false },
  { name: 'merge commits enabled', path: ['merge', 'allow_merge_commit'], value: true },
  { name: 'rebase enabled', path: ['merge', 'allow_rebase_merge'], value: true },
  { name: 'wrong protected branch', path: ['branch_protection', 'ref'], value: 'refs/heads/prod' },
  { name: 'PRs not required', path: ['branch_protection', 'require_pull_request'], value: false },
  { name: 'stale branch allowed', path: ['branch_protection', 'require_up_to_date_branch'], value: false },
  { name: 'no required checks', path: ['branch_protection', 'required_checks'], value: [] },
  { name: 'wildcard check', path: ['branch_protection', 'required_checks', 0, 'context'], value: 'Lint *' },
  { name: 'wildcard app', path: ['branch_protection', 'required_checks', 0, 'app_id'], value: -1 },
  { name: 'missing app', path: ['branch_protection', 'required_checks', 0, 'app_id'], value: undefined },
  { name: 'workflow traversal', path: ['branch_protection', 'required_checks', 0, 'workflow_path'], value: '.github/workflows/../integrate.yml' },
  { name: 'wildcard workflow name', path: ['branch_protection', 'required_checks', 0, 'workflow_name'], value: 'Integrate *' },
  { name: 'wildcard workflow path', path: ['branch_protection', 'required_checks', 0, 'workflow_path'], value: '.github/workflows/*.yml' },
  { name: 'missing workflow job', path: ['branch_protection', 'required_checks', 0, 'job_id'], value: undefined },
  { name: 'wildcard job', path: ['branch_protection', 'required_checks', 0, 'job_id'], value: '*' },
  { name: 'branch force push', path: ['branch_protection', 'allow_force_pushes'], value: true },
  { name: 'branch deletion', path: ['branch_protection', 'allow_deletions'], value: true },
  { name: 'branch bypass', path: ['branch_protection', 'bypass_actors'], value: [{ type: 'TEAM', id: 201 }] },
  { name: 'missing recovery environment', path: ['environments', 'prod-recovery'], value: undefined },
  { name: 'environment typo', path: ['environments', 'aws-prd'], value: fixture().environments['aws-prod'] },
  { name: 'wrong tag format', path: ['tag_protection', 'format'], value: 'SEMVER' },
  { name: 'local tag time', path: ['tag_protection', 'timezone'], value: 'Europe/Berlin' },
  { name: 'unrestricted tag creation', path: ['tag_protection', 'creation_ruleset', 'restrict_creations'], value: false },
  { name: 'no tag creators', path: ['tag_protection', 'creation_ruleset', 'authorized_actors'], value: [] },
  { name: 'unsupported USER ruleset bypass', path: ['tag_protection', 'creation_ruleset', 'authorized_actors', 0, 'type'], value: 'USER' },
  { name: 'broad admin creator', path: ['tag_protection', 'creation_ruleset', 'authorized_actors', 0, 'type'], value: 'ORGANIZATION_ADMIN' },
  { name: 'missing creator ID', path: ['tag_protection', 'creation_ruleset', 'authorized_actors', 0, 'id'], value: null },
  { name: 'tag updates allowed', path: ['tag_protection', 'immutability_ruleset', 'deny_updates'], value: false },
  { name: 'tag deletes allowed', path: ['tag_protection', 'immutability_ruleset', 'deny_deletions'], value: false },
  { name: 'shared tag bypass', path: ['tag_protection', 'bypass_actors'], value: [{ type: 'TEAM', id: 201 }] },
  { name: 'creation authorization bypasses update/delete', path: ['tag_protection', 'immutability_ruleset', 'bypass_actors'], value: fixture().tag_protection.creation_ruleset.authorized_actors },
  { name: 'unknown subject configuration', path: ['oidc_subject_configuration'], value: null },
  { name: 'default template mismatch', path: ['oidc_subject_configuration', 'use_default'], value: false },
  { name: 'missing availability', path: ['external_observations'], value: { observed_at: observed.observed_at } },
  { name: 'self-claimed readiness', path: ['bootstrap_ready'], value: true },
  { name: 'self-enabled production', path: ['production_enabled'], value: true },
];
for (const { name, path, value } of invalidCases) {
  test(`rejects ${name}`, () => invalid(changed(fixture(), path, value)));
}

for (const name of names) {
  const cases: { field: Path; value: unknown }[] = [
    { field: ['reviewers'], value: [] },
    { field: ['reviewers', 0, 'id'], value: undefined },
    { field: ['reviewers', 0, 'id'], value: '101' },
    { field: ['reviewers', 0, 'id'], value: 0 },
    { field: ['reviewers', 0, 'id'], value: 1.5 },
    { field: ['reviewers', 0, 'id'], value: Number.MAX_SAFE_INTEGER + 1 },
    { field: ['reviewers', 0, 'type'], value: 'user' },
    { field: ['reviewers', 0, 'type'], value: 'INTEGRATION' },
    { field: ['prevent_self_review'], value: false },
    { field: ['admin_bypass'], value: true },
    { field: ['deployment_ref', 'type'], value: name === 'aws-prod' ? 'BRANCH' : 'TAG' },
    { field: ['deployment_ref', 'pattern'], value: '*' },
    { field: ['deployment_ref', 'pattern'], value: 'prod' },
    { field: ['workflow', 'path'], value: null },
    { field: ['workflow', 'ref_pattern'], value: 'refs/heads/prod' },
    { field: ['workflow', 'ref_pattern'], value: 'refs/heads/*' },
    { field: ['oidc', 'issuer'], value: 'https://example.test' },
    { field: ['oidc', 'aud'], value: '*' },
    { field: ['oidc', 'aud'], value: 'https://sts.amazonaws.com' },
    { field: ['oidc', 'sub'], value: 'repo:aura-historia/backend:*' },
    { field: ['oidc', 'sub'], value: 'repo:other/backend:environment:aws-prod' },
    { field: ['oidc', 'sub'], value: 'repo:aura-historia/backend:ref:refs/heads/develop' },
  ];
  test(`${name} requires exact reviewers, branch/tag/workflow restrictions and OIDC claims`, () => {
    for (const { field, value } of cases) invalid(changed(fixture(), ['environments', name, ...field], value));
    const otherName = name === 'aws-prod' ? 'aws-dev' : 'aws-prod';
    invalid(changed(fixture(), ['environments', name, 'oidc', 'sub'], fixture().environments[otherName].oidc.sub));
    const reviewer = fixture().environments[name].reviewers[0]!;
    invalid(changed(fixture(), ['environments', name, 'reviewers'], [reviewer, reviewer]));
    invalid(changed(fixture(), ['environments', name, 'reviewers'], Array.from({ length: 7 }, (_, id) => ({ type: 'USER', id: id + 1 }))));
  });
}

for (const ruleset of ['creation_ruleset', 'immutability_ruleset'] as const) {
  test(`${ruleset} must actively cover all CalVer tags without exclusions`, () => {
    for (const [field, value] of [
      ['enforcement', 'DISABLED'], ['enforcement', 'EVALUATE'], ['target', 'BRANCH'],
      ['include', ['refs/tags/*']], ['include', []], ['exclude', ['refs/tags/20260912-1230']],
    ] as const) invalid(changed(fixture(), ['tag_protection', ruleset, field], value));
  });
}

test('duplicate creators and ambiguous check producers are rejected', () => {
  const spec = fixture();
  const actor = spec.tag_protection.creation_ruleset.authorized_actors[0]!;
  invalid(changed(spec, ['tag_protection', 'creation_ruleset', 'authorized_actors'], [actor, actor]));
  const check = spec.branch_protection.required_checks[0]!;
  invalid(changed(spec, ['branch_protection', 'required_checks'], [check, check]));
  invalid(changed(spec, ['branch_protection', 'required_checks'], [check, { ...check, app_id: 302 }]));
});

test('custom subjects require explicit verification of the supported repo/context template', () => {
  const spec = changed(fixture(), ['oidc_subject_configuration'], customSubject);
  assert.equal(parseGitHubBootstrap(spec, 'test').oidc_subject_configuration.mode, 'CUSTOMIZED');
  for (const [field, value] of [
    ['verified', undefined], ['verified', false], ['verified_at', null], ['use_default', true],
    ['include_claim_keys', ['repo']], ['include_claim_keys', ['context', 'repo']],
    ['include_claim_keys', ['repo', 'context', 'job_workflow_ref']],
  ] as const) invalid(changed(spec, ['oidc_subject_configuration', field], value));
  invalid(changed(spec, ['environments', 'aws-prod', 'oidc', 'sub'], 'repo:aura-historia/backend:environment:*'));
  assert.equal(githubBootstrapStatus(spec, 'test').bootstrap_ready, false);
});

test('observation and custom-verification times require exact real UTC dates', () => {
  const spec = changed(changed(fixture(), ['external_observations'], observed), ['oidc_subject_configuration'], customSubject);
  assert.equal(parseGitHubBootstrap(spec, 'test').external_observations?.observed_at, observed.observed_at);
  for (const value of [
    '2026-02-29T12:30:00Z', '2026-02-30T12:30:00Z', '2026-13-01T12:30:00Z',
    '2026-09-12T24:00:00Z', '2026-09-12T12:60:00Z', '2026-09-12T12:30:60Z',
    '2026-09-12T12:30:00+00:00', '2026-09-12T12:30:00-02:00', '2026-09-12T12:30:00',
    '2026-09-12T12:30:00.000Z', '20260912-1230', '2026-09-12T12:30:00Z\n', '', null,
  ]) {
    invalid(changed(spec, ['external_observations', 'observed_at'], value));
    invalid(changed(spec, ['oidc_subject_configuration', 'verified_at'], value));
  }
  assert.equal(parseGitHubBootstrap(changed(spec, ['external_observations', 'observed_at'], '2024-02-29T23:59:59Z'), 'test').schema_version, 1);
});

test('CalVer matching uses calendar semantics, not just the GitHub tag glob', () => {
  const spec = fixture();
  for (const tag of ['20000229-0000', '20240229-2359', '20260912-1230', '24000229-2359', '99991231-2359']) {
    assert.equal(matchesGitHubDeploymentRefPolicy(spec, 'aws-prod', `refs/tags/${tag}`, 'test'), true);
  }
  for (const tag of [
    '19991231-2359', '00000101-0000', '20260229-0000', '21000229-0000', '20260431-0000',
    '20261301-0000', '20260001-0000', '20260900-0000', '20260912-2400', '20260912-1260',
    '20260912-123000', '20260912-1230Z', '20260912-1230+0000', '20260912-1230\n',
    'v20260912-1230', '2026-09-12T12:30:00Z', '20260912-1230/extra', '20260912-1230 ', '*', '',
  ]) assert.equal(matchesGitHubDeploymentRefPolicy(spec, 'aws-prod', `refs/tags/${tag}`, 'test'), false, tag);
  assert.equal(matchesGitHubDeploymentRefPolicy(spec, 'aws-prod', 'refs/heads/20260912-1230', 'test'), false);
  assert.equal(matchesGitHubDeploymentRefPolicy(spec, 'aws-prod', 'refs/heads/prod', 'test'), false);
  for (const name of ['aws-dev', 'prod-recovery'] as const) {
    assert.equal(matchesGitHubDeploymentRefPolicy(spec, name, 'refs/heads/develop', 'test'), true);
    for (const ref of ['develop', 'refs/tags/develop', 'refs/heads/prod', 'refs/heads/develop/other', 'refs/tags/20260912-1230']) {
      assert.equal(matchesGitHubDeploymentRefPolicy(spec, name, ref, 'test'), false);
    }
  }
  assert.equal(matchesGitHubDeploymentRefPolicy(spec, 'aws-prod', 'refs/tags/20260912-1230'), false);
});

test('missing, unavailable and even fully asserted observations never enable bootstrap or production', () => {
  for (const [observation, reason] of [
    [null, 'EXTERNAL_OBSERVATIONS_REQUIRED'],
    [{ ...observed, environment_protection_available: false }, 'PROTECTION_UNAVAILABLE'],
    [{ ...observed, tag_rulesets_available: false }, 'PROTECTION_UNAVAILABLE'],
    [observed, 'LIVE_VERIFICATION_REQUIRED'],
  ] as const) {
    assert.deepEqual(githubBootstrapStatus(changed(fixture(), ['external_observations'], observation), 'test'), {
      spec_valid: true, bootstrap_ready: false, production_enabled: false, reason,
    });
  }
  assert.deepEqual(githubBootstrapStatus(null), {
    spec_valid: false, bootstrap_ready: false, production_enabled: false, reason: 'INVALID_SPEC',
  });
});

test('unknown fields and misspelled keys fail at every object boundary', () => {
  const spec = changed(changed(fixture(), ['external_observations'], observed), ['oidc_subject_configuration'], customSubject);
  function check(value: unknown, path: Path): void {
    if (!value || typeof value !== 'object') return;
    if (!Array.isArray(value)) invalid(changed(spec, [...path, 'typo'], true));
    for (const [key, child] of Object.entries(value)) check(child, [...path, key]);
  }
  check(spec, []);
});

test('safe boundaries never expose input, Zod issues, causes or thrown getter contents', () => {
  const marker = 'sensitive-input-marker';
  const getterInput = Object.defineProperty({}, 'schema_version', { get() { throw new Error(marker); } });
  for (const input of [null, false, [], marker, { [marker]: `https://user:${marker}@invalid.test` }, getterInput]) {
    assert.throws(() => parseGitHubBootstrap(input, 'test'), error => {
      assert(error instanceof Error);
      assert.equal(error.constructor, Error);
      assert.equal(error.message, 'Invalid GitHub bootstrap specification');
      assert.equal(Object.hasOwn(error, 'cause'), false);
      assert.equal(Object.hasOwn(error, 'issues'), false);
      assert.equal(`${error.stack}${JSON.stringify(error)}`.includes(marker), false);
      return true;
    });
    assert.equal(JSON.stringify(githubBootstrapStatus(input, 'test')).includes(marker), false);
    assert.equal(matchesGitHubDeploymentRefPolicy(input, 'aws-prod', 'refs/tags/20260912-1230', 'test'), false);
  }
});

test('parsing and status/ref evaluation preserve inputs, make no fetch calls and log nothing', t => {
  const fetch = t.mock.method(globalThis, 'fetch', () => { throw new Error('Network forbidden'); });
  const logs = ['log', 'error', 'warn'].map(method => t.mock.method(console, method as 'log', () => {}));
  const spec = fixture();
  const before = structuredClone(spec);
  assert.deepEqual(parseGitHubBootstrap(spec, 'test'), before);
  githubBootstrapStatus(spec, 'test');
  matchesGitHubDeploymentRefPolicy(spec, 'aws-prod', 'refs/tags/20260912-1230', 'test');
  githubBootstrapStatus({ secret: 'must-not-log' }, 'test');
  assert.deepEqual(spec, before);
  assert.equal(fetch.mock.callCount(), 0);
  for (const log of logs) assert.equal(log.mock.callCount(), 0);
});

test('generated schema is byte-deterministic, current and strict at all object boundaries', () => {
  const schema = githubBootstrapJsonSchema();
  const expected = `${JSON.stringify(schema, null, 2)}\n`;
  assert.equal(`${JSON.stringify(githubBootstrapJsonSchema(), null, 2)}\n`, expected);
  assert.equal(readFileSync(new URL('../../../schemas/github-bootstrap.schema.json', import.meta.url), 'utf8'), expected);
  assert.equal(schema.$schema, 'https://json-schema.org/draft/2020-12/schema');
  function check(value: unknown): number {
    if (!value || typeof value !== 'object') return 0;
    const object = value as Record<string, unknown>;
    if (object.type === 'object') assert.equal(object.additionalProperties, false);
    return Number(object.type === 'object') + Object.values(object).reduce<number>((sum, child) => sum + check(child), 0);
  }
  assert(check(schema) > 10);
});
