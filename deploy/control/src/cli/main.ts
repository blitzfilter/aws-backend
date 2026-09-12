#!/usr/bin/env node
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { z } from 'zod';
import { validateCatalog } from '../catalog.js';
import { Digest, Identifier, PhaseId, RealStage, Sha } from '../contracts/primitives.js';
import { ControlError, ResultCodes, failureResult } from '../contracts/result.js';

const commands = {
  'release assemble': { '--source-sha': Sha },
  'release verify': { '--manifest-digest': Digest },
  'deployment plan': { '--stage': RealStage, '--manifest-digest': Digest },
  'deployment apply': { '--operation-id': Identifier, '--approved-plan-digest': Digest },
  'deployment status': { '--stage': RealStage },
  'deployment reconcile': { '--operation-id': Identifier },
  'deployment rollback-plan': { '--stage': RealStage, '--target-manifest-digest': Digest },
  'host-inspect': { '--stage': RealStage },
  'host-operation start': { '--operation-id': Identifier, '--phase': PhaseId },
  'host-operation status': { '--operation-id': Identifier },
  'host-operation reconcile': { '--operation-id': Identifier },
} satisfies Record<string, Record<string, z.ZodType>>;
export const help = [
  'aura-deploy — default-off hybrid deployment contracts',
  '',
  'release validate-catalog  (implemented; local source inspection only)',
  ...Object.entries(commands).map(([name, flags]) => `${name} ${Object.keys(flags).map(flag => `${flag} <value>`).join(' ')}  [NOT_IMPLEMENTED]`),
  '',
  'No AWS/host mutation adapter is installed. Valid unsupported commands exit 8.',
  'Never pass credentials, rendered configuration, queue bodies or receipt handles.',
].join('\n');

export function run(argv: readonly string[]): { exitCode: number; output: string } {
  try {
    if (argv.length === 0 || (argv.length === 1 && ['help', '--help', '-h'].includes(argv[0]!))) return { exitCode: 0, output: help };
    if (argv.length === 2 && argv[0] === 'release' && argv[1] === 'validate-catalog') return { exitCode: 0, output: JSON.stringify(validateCatalog()) };
    const length = argv[0] === 'host-inspect' ? 1 : 2;
    const command = argv.slice(0, length).join(' ');
    const flags = commands[command as keyof typeof commands] as Record<string, z.ZodType> | undefined;
    if (!flags) throw new ControlError('INVALID_INPUT');
    if (argv.length === length + 1 && argv[length] === '--help') return { exitCode: 0, output: help };
    const remaining = argv.slice(length);
    if (remaining.length !== Object.keys(flags).length * 2) throw new ControlError('INVALID_INPUT');
    const seen = new Set<string>();
    for (let index = 0; index < remaining.length; index += 2) {
      const flag = remaining[index]!; const value = remaining[index + 1]!;
      if (seen.has(flag) || !Object.hasOwn(flags, flag) || /[\u0000-\u0020\u007f]/.test(value) || !flags[flag]!.safeParse(value).success) throw new ControlError('INVALID_INPUT');
      seen.add(flag);
    }
    throw new ControlError('NOT_IMPLEMENTED');
  } catch (error) {
    const result = failureResult(error);
    return { exitCode: ResultCodes[result.code], output: JSON.stringify(result) };
  }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  const result = run(process.argv.slice(2));
  process.stdout.write(`${result.output}\n`);
  process.exitCode = result.exitCode;
}
