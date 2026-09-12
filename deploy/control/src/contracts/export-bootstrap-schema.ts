import { writeFileSync } from 'node:fs';
import { z } from 'zod';
import { GitHubBootstrap } from './bootstrap.js';

export function githubBootstrapJsonSchema() {
  return z.toJSONSchema(GitHubBootstrap, { target: 'draft-2020-12', reused: 'ref' });
}

// Run this compiled entrypoint explicitly; importing it never writes files.
if (import.meta.main) {
  writeFileSync(new URL('../../../../schemas/github-bootstrap.schema.json', import.meta.url),
    `${JSON.stringify(githubBootstrapJsonSchema(), null, 2)}\n`);
}
