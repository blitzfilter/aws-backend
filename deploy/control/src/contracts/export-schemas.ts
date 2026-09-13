import { writeFile } from 'node:fs/promises';
import { z } from 'zod';
import { Catalog } from '../catalog.js';
import { releaseSchemaDocuments } from './export-release-schemas.js';
import { inventoryJsonSchemas } from './export-inventory-schemas.js';
import { stateSchemaDocuments } from '../state/export-state-schema.js';
import { githubBootstrapJsonSchema } from './export-bootstrap-schema.js';

const schemas = {
  ...Object.fromEntries([...releaseSchemaDocuments(), ...stateSchemaDocuments()].map(({ name, document }) => [`${name}.schema.json`, document])),
  ...inventoryJsonSchemas(),
  'catalog.schema.json': z.toJSONSchema(Catalog, { unrepresentable: 'throw' }),
    'github-bootstrap.schema.json': githubBootstrapJsonSchema(),
};
for (const [name, document] of Object.entries(schemas)) {
  await writeFile(new URL(`../../../../schemas/${name}`, import.meta.url), `${JSON.stringify(document, null, 2)}\n`, 'utf8');
}
