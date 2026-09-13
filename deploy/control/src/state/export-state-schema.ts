import { writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { z } from 'zod';
import { DeploymentState } from './model.js';

export function stateSchemaDocuments() {
  return [{ name: 'deployment-state', document: z.toJSONSchema(DeploymentState, { unrepresentable: 'throw', reused: 'ref' }) }];
}

// Compiled entrypoint only; importing never writes or contacts external systems.
if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  for (const { name, document } of stateSchemaDocuments()) {
    await writeFile(new URL(`../../../../schemas/${name}.schema.json`, import.meta.url), `${JSON.stringify(document, null, 2)}\n`, 'utf8');
  }
}
