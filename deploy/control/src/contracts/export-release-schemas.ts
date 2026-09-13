import { writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { z } from 'zod';
import { DeploymentIntent, DeploymentPlan, MigrationMetadata, ReleaseManifest } from './release.js';

export function releaseSchemaDocuments() {
  const schemas = {
    'release-manifest': ReleaseManifest,
    'deployment-plan': DeploymentPlan,
    'deployment-intent': DeploymentIntent,
    'migration-metadata': MigrationMetadata,
  };
  return Object.entries(schemas).map(([name, schema]) => ({
    name,
    document: z.toJSONSchema(schema, { unrepresentable: 'throw' }),
  }));
}

// Run compiled entrypoint after the package build. No writes merely by importing it.
if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  const directory = new URL('../../../../schemas/', import.meta.url);
  await Promise.all(releaseSchemaDocuments().map(({ name, document }) =>
    writeFile(new URL(`${name}.schema.json`, directory), `${JSON.stringify(document, null, 2)}\n`, 'utf8'),
  ));
}
