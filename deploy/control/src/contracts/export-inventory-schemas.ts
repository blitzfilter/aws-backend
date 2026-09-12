import { writeFileSync } from 'node:fs';
import { z } from 'zod';
import { Inventory, RuntimeConfiguration } from './inventory.js';

export function inventoryJsonSchemas() {
  const options = { target: 'draft-2020-12', reused: 'ref' } as const;
  return {
    'inventory.schema.json': z.toJSONSchema(Inventory, options),
    'runtime-configuration.schema.json': z.toJSONSchema(RuntimeConfiguration, options),
  };
}

// Run the compiled entrypoint from deploy/control/dist/src/contracts.
if (import.meta.main) {
  for (const [name, schema] of Object.entries(inventoryJsonSchemas())) {
    writeFileSync(new URL(`../../../../schemas/${name}`, import.meta.url), `${JSON.stringify(schema, null, 2)}\n`);
  }
}
