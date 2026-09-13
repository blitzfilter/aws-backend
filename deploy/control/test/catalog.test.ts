import assert from 'node:assert/strict';
import test from 'node:test';
import { cdkCatalog, parseCatalog, readCatalog, validateCatalog, verifyManifestCatalog } from '../src/catalog.js';
import { exampleManifest } from '../fixtures/release-fixtures.js';
import { run } from '../src/cli/main.js';

const implementedFixtureCatalog = () => {
  const catalog = readCatalog();
  for (const component of catalog.native) component.status = 'IMPLEMENTED';
  return catalog;
};

test('actual source catalog matches Cargo, CDK, all mail and search inputs', () => {
  assert.deepEqual(validateCatalog(), { code: 'OK', native: 4, lambdas: 5, workers: 10, mail: 25, incomplete: ['aura-historia-migrate'] });
});
test('incomplete migrator capability cannot authorize release', () => assert.throws(() => verifyManifestCatalog(exampleManifest, readCatalog()), /COMPATIBILITY_BLOCKED/));
test('complete synthetic manifest matches catalog identities, not artifact existence', () => verifyManifestCatalog(exampleManifest, implementedFixtureCatalog()));
for (const alter of [
  (m: typeof exampleManifest) => { m.mail_bundle.templates.pop(); },
  (m: typeof exampleManifest) => { m.mail_bundle.templates[0]!.key = 'unknown/en.html'; },
  (m: typeof exampleManifest) => { m.search.families[0]!.analysis_assets.pop(); },
  (m: typeof exampleManifest) => { m.search.families[1]!.analysis_assets[0]!.name = 'unknown-synonyms'; },
  (m: typeof exampleManifest) => { m.lambdas[0]!.architecture = 'arm64'; },
  (m: typeof exampleManifest) => { m.native_images.pop(); },
]) {
  test('DEP09 absent or substituted required bundle component is rejected', () => {
    const manifest = structuredClone(exampleManifest); alter(manifest);
    assert.throws(() => verifyManifestCatalog(manifest, implementedFixtureCatalog()));
  });
}
test('catalog rejects duplicate targets, unknown scope, unsafe paths and unknown controls', () => {
  const catalog = readCatalog();
  assert.throws(() => parseCatalog({ ...catalog, command: 'untrusted' }));
  assert.throws(() => parseCatalog({ ...catalog, native: [...catalog.native, catalog.native[0]] }));
  assert.throws(() => parseCatalog({ ...catalog, release_inputs: ['../outside'] }));
  assert.throws(() => parseCatalog({ ...catalog, workers: [{ scope: 'not-a-scope', table: 'users', operations: ['INSERT'] }] }));
});
test('CDK extraction never executes source and rejects computed binary names', () => {
  assert.deepEqual(cdkCatalog('const LAMBDA_DEFINITIONS = defineLambdaDefinitions({foo:{binaryName:"safe",postgres:true}} as const);'), [{ binary: 'safe', database: true }]);
  assert.throws(() => cdkCatalog('const LAMBDA_DEFINITIONS = defineLambdaDefinitions({foo:{binaryName:process.exit()}} as const);'));
});
for (const argv of [
  ['deployment', 'apply', '--operation-id', 'example-op', '--approved-plan-digest', `sha256:${'a'.repeat(64)}`],
  ['host-operation', 'start', '--operation-id', 'example-op', '--phase', 'switch_api'],
  ['host-inspect', '--stage', 'dev'],
  ['release', 'verify', '--manifest-digest', `sha256:${'b'.repeat(64)}`],
]) {
  test('valid unimplemented command explicitly fails; never simulated success', () => assert.equal(run(argv).exitCode, 8));
}
for (const argv of [
  ['host-operation', 'start', '--operation-id', 'op;whoami', '--phase', 'switch_api'],
  ['host-inspect', '--stage', 'prod\n'],
  ['release', 'validate-catalog', '--shell', 'sensitive-fixture'],
  ['deployment', 'status', '--stage', 'unknown'],
  ['deployment', 'apply', '--operation-id', 'one', '--operation-id', 'two'],
  ['host-operation', 'start', '--operation-id', '../escape', '--phase', 'switch_api'],
  ['host-operation', 'start', '--operation-id', 'op', '--phase', 'purge'],
]) {
  test('DEP08 invalid CLI input is rejected and never echoed', () => {
    assert.equal(run(argv).exitCode, 2);
    assert.doesNotMatch(run(argv).output, /sensitive-fixture|whoami|escape|purge/);
  });
}
