import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync, realpathSync } from 'node:fs';
import { resolve, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { z } from 'zod';
import { Architecture, DatabaseTarget, Identifier, SafeKey, SchemaVersion, WorkerScope } from './contracts/primitives.js';
import type { ReleaseManifest } from './contracts/release.js';
import { ControlError } from './contracts/result.js';

const Binary = z.strictObject({ id: Identifier, package: Identifier, binary: Identifier, path: SafeKey, status: z.enum(['IMPLEMENTED', 'NOT_IMPLEMENTED']) });
export const Catalog = z.strictObject({
  schema_version: SchemaVersion,
  native: z.array(Binary).min(1),
  lambdas: z.array(Binary.extend({ architecture: Architecture, database: z.boolean() })).min(1),
  excluded_tools: z.array(z.strictObject({ package: Identifier, binary: Identifier })),
  workers: z.array(z.strictObject({ scope: WorkerScope, table: z.string().regex(/^[a-z_]+$/), operations: z.array(z.enum(['INSERT', 'UPDATE', 'DELETE'])).min(1) })),
  mail: z.strictObject({ root: SafeKey, groups: z.array(SafeKey).min(1), languages: z.array(z.enum(['de', 'en', 'es', 'fr', 'it'])).min(1) }),
  migrations: z.array(z.strictObject({ target: DatabaseTarget, path: SafeKey })).length(2),
  search: z.array(z.strictObject({ family: z.enum(['PRODUCT_LISTINGS', 'USER_SEARCH_FILTERS']), definition: SafeKey, physical_baseline: z.string().regex(/^[a-z][a-z0-9_-]*$/) })).length(2),
  analysis: z.array(z.strictObject({ id: Identifier, path: SafeKey, node_path: SafeKey })).min(1),
  release_inputs: z.array(z.string().regex(/^(?:[a-zA-Z0-9_.-]+\/?)+$/)).min(1),
});
export type Catalog = z.infer<typeof Catalog>;
export const workspaceRoot = fileURLToPath(new URL('../../../../', import.meta.url));
const requireMatch = (condition: boolean): void => { if (!condition) throw new ControlError('PRECONDITION_FAILED'); };
const unique = (values: readonly string[]): void => requireMatch(new Set(values).size === values.length);
const sameSet = (a: readonly string[], b: readonly string[]): void => { unique(a); unique(b); requireMatch(a.length === b.length && a.every(item => b.includes(item))); };

export function parseCatalog(input: unknown): Catalog {
  const result = Catalog.safeParse(input);
  if (!result.success) throw new ControlError('INVALID_INPUT');
  const catalog = result.data;
  unique([...catalog.native, ...catalog.lambdas].map(item => item.id));
  unique([...catalog.native, ...catalog.lambdas, ...catalog.excluded_tools].map(item => `${item.package}/${item.binary}`));
  sameSet(catalog.workers.map(item => item.scope), WorkerScope.options);
  for (const worker of catalog.workers) unique(worker.operations);
  unique(catalog.mail.groups); unique(catalog.mail.languages);
  sameSet(catalog.migrations.map(item => item.target), DatabaseTarget.options);
  sameSet(catalog.search.map(item => item.family), ['PRODUCT_LISTINGS', 'USER_SEARCH_FILTERS']);
  unique(catalog.analysis.map(item => item.id)); unique(catalog.analysis.map(item => item.node_path));
  for (const path of [...catalog.release_inputs, ...catalog.native.map(item => item.path), ...catalog.lambdas.map(item => item.path)]) {
    requireMatch(!/[\u0000-\u001f\u007f]/.test(path) && !path.split('/').some(part => part === '..' || part === '.' || part === ''));
  }
  return catalog;
}
export const readCatalog = (): Catalog => parseCatalog(JSON.parse(readFileSync(resolve(workspaceRoot, 'deploy/catalog.json'), 'utf8')));

export function mailKeys(catalog: Catalog): string[] {
  return catalog.mail.groups.flatMap(group => catalog.mail.languages.map(language => `${group}/${language}.html`));
}

/** Mandatory after parseManifest, using the installed trusted catalog, never one supplied by the builder. */
export function verifyManifestCatalog(manifest: ReleaseManifest, trustedCatalog: Catalog): void {
  const catalog = parseCatalog(trustedCatalog);
  if ([...catalog.native, ...catalog.lambdas].some(item => item.status !== 'IMPLEMENTED')) throw new ControlError('COMPATIBILITY_BLOCKED');
  sameSet([...new Set(manifest.native_images.map(item => item.component))], catalog.native.map(item => item.id));
  sameSet(manifest.lambdas.map(item => item.component), catalog.lambdas.map(item => item.id));
  for (const binary of catalog.lambdas) requireMatch(manifest.lambdas.some(item => item.component === binary.id && item.architecture === binary.architecture));
  sameSet(manifest.mail_bundle.templates.map(item => item.key), mailKeys(catalog));
  sameSet(manifest.search.families.map(item => item.family), catalog.search.map(item => item.family));
  for (const family of manifest.search.families) sameSet(family.analysis_assets.map(item => item.name), catalog.analysis.map(item => item.id));
  sameSet(manifest.migrations.streams.map(item => item.stream), catalog.migrations.map(item => item.target));
  sameSet(manifest.queues.map(item => item.scope), catalog.workers.map(item => item.scope));
}

function checkedPath(root: string, path: string): string {
  const actual = realpathSync(resolve(root, path));
  requireMatch(actual.startsWith(`${realpathSync(root)}${sep}`));
  return actual;
}
function files(root: string, path: string): string[] {
  return readdirSync(checkedPath(root, path), { withFileTypes: true }).flatMap(entry => {
    const child = `${path}/${entry.name}`;
    requireMatch(!entry.isSymbolicLink());
    return entry.isDirectory() ? files(root, child) : [child];
  });
}
function unwrap(node: ts.Expression): ts.Expression {
  return ts.isAsExpression(node) || ts.isSatisfiesExpression(node) || ts.isParenthesizedExpression(node) ? unwrap(node.expression) : node;
}
function sourceDefinition(source: string, name: string): ts.Expression {
  const file = ts.createSourceFile('catalog.ts', source, ts.ScriptTarget.Latest, true);
  let result: ts.Expression | undefined;
  const visit = (node: ts.Node): void => {
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.name.text === name && node.initializer) result = unwrap(node.initializer);
    ts.forEachChild(node, visit);
  };
  visit(file);
  if (!result) throw new ControlError('PRECONDITION_FAILED');
  return result;
}
export function cdkCatalog(source: string): { binary: string; database: boolean }[] {
  const expression = sourceDefinition(source, 'LAMBDA_DEFINITIONS');
  if (!ts.isCallExpression(expression) || !expression.arguments[0]) throw new ControlError('PRECONDITION_FAILED');
  const definitions = unwrap(expression.arguments[0]);
  if (!ts.isObjectLiteralExpression(definitions)) throw new ControlError('PRECONDITION_FAILED');
  return definitions.properties.map(property => {
    if (!ts.isPropertyAssignment(property)) throw new ControlError('PRECONDITION_FAILED');
    const definition = unwrap(property.initializer);
    if (!ts.isObjectLiteralExpression(definition)) throw new ControlError('PRECONDITION_FAILED');
    let binary: string | undefined; let database = false;
    for (const field of definition.properties) {
      if (!ts.isPropertyAssignment(field)) throw new ControlError('PRECONDITION_FAILED');
      const name = field.name.getText();
      if (name === 'binaryName') {
        if (!ts.isStringLiteral(field.initializer)) throw new ControlError('PRECONDITION_FAILED');
        binary = field.initializer.text;
      }
      if (name === 'postgres') { requireMatch(field.initializer.kind === ts.SyntaxKind.TrueKeyword); database = true; }
    }
    if (!binary) throw new ControlError('PRECONDITION_FAILED');
    return { binary, database };
  });
}
export function validateCatalog(root = workspaceRoot): { code: 'OK'; native: number; lambdas: number; workers: number; mail: number; incomplete: string[] } {
  const catalog = parseCatalog(JSON.parse(readFileSync(checkedPath(root, 'deploy/catalog.json'), 'utf8')));
  const output = execFileSync('cargo', ['metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'], { cwd: root, encoding: 'utf8', timeout: 60000, maxBuffer: 16 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] });
  const metadata = z.object({ packages: z.array(z.object({ name: z.string(), manifest_path: z.string(), targets: z.array(z.object({ name: z.string(), kind: z.array(z.string()) })) })) }).parse(JSON.parse(output));
  const actualBins = metadata.packages.flatMap(pkg => pkg.targets.filter(target => target.kind.includes('bin')).map(target => `${pkg.name}/${target.name}`));
  const implemented = [...catalog.native, ...catalog.lambdas].filter(item => item.status === 'IMPLEMENTED');
  sameSet(actualBins, [...implemented, ...catalog.excluded_tools].map(item => `${item.package}/${item.binary}`));
  for (const binary of implemented) {
    const pkg = metadata.packages.find(pkg => pkg.name === binary.package);
    requireMatch(pkg !== undefined && relative(root, pkg.manifest_path) === `${binary.path}/Cargo.toml`);
  }
  for (const binary of catalog.native.filter(item => item.status === 'NOT_IMPLEMENTED')) requireMatch(!existsSync(resolve(root, binary.path, 'Cargo.toml')));
  const cdk = cdkCatalog(readFileSync(checkedPath(root, 'infra/src/constructs/lambdas.ts'), 'utf8'));
  sameSet(cdk.map(item => item.binary), catalog.lambdas.map(item => item.binary));
  for (const item of cdk) requireMatch(catalog.lambdas.some(binary => binary.binary === item.binary && binary.database === item.database));
  const scopes = sourceDefinition(readFileSync(checkedPath(root, 'infra/src/worker-queue-config.ts'), 'utf8'), 'WORKER_SCOPES');
  if (!ts.isArrayLiteralExpression(scopes)) throw new ControlError('PRECONDITION_FAILED');
  sameSet(scopes.elements.map(item => { if (!ts.isStringLiteral(item)) throw new ControlError('PRECONDITION_FAILED'); return item.text; }), catalog.workers.map(item => item.scope));
  sameSet(files(root, catalog.mail.root).filter(path => path.endsWith('.mjml')), mailKeys(catalog).map(key => `${catalog.mail.root}/${key.replace(/\.html$/, '.mjml')}`));
  sameSet(files(root, 'opensearch/mappings').filter(path => path.endsWith('.json')), catalog.search.map(item => item.definition));
  sameSet(files(root, 'opensearch/analysis').filter(path => path.endsWith('.txt')), catalog.analysis.map(item => item.path));
  for (const search of catalog.search) {
    const definition = JSON.parse(readFileSync(checkedPath(root, search.definition), 'utf8')) as unknown;
    const references: string[] = [];
    const scan = (value: unknown): void => {
      if (value && typeof value === 'object') for (const [key, child] of Object.entries(value)) {
        if (key === 'synonyms_path') { requireMatch(typeof child === 'string'); references.push(child as string); } else scan(child);
      }
    };
    scan(definition);
    sameSet(references, catalog.analysis.map(item => item.node_path));
  }
  for (const stream of catalog.migrations) requireMatch(files(root, stream.path).some(path => /\/[0-9]{14}_[a-z0-9_]+\.sql$/.test(path)));
  for (const path of catalog.release_inputs) checkedPath(root, path);
  return { code: 'OK', native: implemented.length - catalog.lambdas.length, lambdas: catalog.lambdas.length, workers: catalog.workers.length, mail: mailKeys(catalog).length, incomplete: catalog.native.filter(item => item.status !== 'IMPLEMENTED').map(item => item.id) };
}
