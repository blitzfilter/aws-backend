import { z } from 'zod';

export const SchemaVersion = z.literal(1);
export const Stage = z.enum(['dev', 'prod', 'ephemeral', 'local', 'test']);
export type Stage = z.infer<typeof Stage>;
export const RealStage = z.enum(['dev', 'prod']);
export const Identifier = z.string().regex(/^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$/).max(80);
export const Sha = z.string().regex(/^[0-9a-f]{40}$/);
export const Digest = z.string().regex(/^sha256:[0-9a-f]{64}$/);
export const SqlxChecksum = z.string().regex(/^sha384:[0-9a-f]{96}$/);
export const Account = z.string().regex(/^[0-9]{12}$/);
export const Region = z.string().regex(/^[a-z]{2}(?:-gov)?-[a-z]+-[1-9][0-9]?$/);
export const Positive = z.number().int().positive().max(Number.MAX_SAFE_INTEGER);
export const Revision = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
export const Architecture = z.enum(['x86_64', 'arm64']);
export const Compatibility = z.enum(['COMPATIBLE', 'COMPATIBLE_WITH_BACKFILL', 'MAINTENANCE_REQUIRED', 'BLOCKED']);
export const SecretReference = z.strictObject({ provider: z.enum(['SECRETS_MANAGER', 'SSM_SECURE', 'PROTECTED_FILE']), id: Identifier, revision: Identifier });
export const DatabaseTarget = z.enum(['business', 'crawler']);
export const SafeKey = z.string().regex(/^[a-zA-Z0-9][a-zA-Z0-9_.-]*(?:\/[a-zA-Z0-9][a-zA-Z0-9_.-]*)*$/).max(512).refine(value => !value.split('/').some(part => part === '.' || part === '..'), 'unsafe key');
export const HttpsUrl = z.string().url().refine(value => {
  const url = new URL(value);
  return url.protocol === 'https:' && !url.username && !url.password && !url.hash;
}, 'HTTPS URL without credentials required');
export const WorkerScope = z.enum([
  'product-listing-opensearch', 'search-filter-projection', 'search-filter-percolator',
  'search-filter-match-notification', 'watchlist-notification', 'product-content-assessment',
  'product-embedding', 'product-translation', 'product-listing-normalization', 'notification-delivery',
]);
export type WorkerScope = z.infer<typeof WorkerScope>;
export const PhaseId = z.enum([
  'verify_release', 'acquire_operation', 'revalidate_plan', 'aws_prerequisites', 'host_preflight',
  'postgres_expand', 'opensearch_expand', 'prepare_cdc', 'prepare_compute', 'verify_candidates',
  'replace_worker_scopes', 'activate_lambda', 'switch_api', 'handover_jobs', 'verify_release_behavior', 'commit_release',
]);
export type PhaseId = z.infer<typeof PhaseId>;
export const isRealStage = (stage: Stage): boolean => stage === 'dev' || stage === 'prod';
