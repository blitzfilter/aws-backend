import { createHash, timingSafeEqual } from 'node:crypto';

export function byteDigest(bytes: Uint8Array): string {
  return `sha256:${createHash('sha256').update(bytes).digest('hex')}`;
}

/** Canonical control JSON: UTF-16 key order, ordered arrays, compact UTF-8, no newline. */
export function canonicalBytes(value: unknown): Uint8Array {
  const ancestors = new Set<object>();
  const visit = (item: unknown, depth: number): string => {
    if (depth > 64) throw new Error('invalid canonical JSON');
    if (item === null || typeof item === 'boolean' || typeof item === 'string') return JSON.stringify(item);
    if (typeof item === 'number') {
      if (!Number.isFinite(item) || !Number.isSafeInteger(item)) throw new Error('invalid canonical JSON number');
      return JSON.stringify(item);
    }
    if (typeof item !== 'object' || ancestors.has(item)) throw new Error('invalid canonical JSON');
    ancestors.add(item);
    try {
      if (Array.isArray(item)) {
        if (Object.getPrototypeOf(item) !== Array.prototype || Reflect.ownKeys(item).length !== item.length + 1) throw new Error('invalid canonical JSON array');
        const entries: string[] = [];
        for (let index = 0; index < item.length; index++) {
          const property = Object.getOwnPropertyDescriptor(item, String(index));
          if (!property || !('value' in property) || !property.enumerable) throw new Error('invalid canonical JSON array');
          entries.push(visit(property.value, depth + 1));
        }
        return `[${entries.join(',')}]`;
      }
      if (![Object.prototype, null].includes(Object.getPrototypeOf(item))) throw new Error('invalid canonical JSON object');
      if (Object.getOwnPropertySymbols(item).length) throw new Error('invalid canonical JSON object');
      const entries = Object.keys(item).sort().map(key => {
        const property = Object.getOwnPropertyDescriptor(item, key);
        if (!property || !('value' in property)) throw new Error('invalid canonical JSON property');
        return `${JSON.stringify(key)}:${visit(property.value, depth + 1)}`;
      });
      return `{${entries.join(',')}}`;
    } finally {
      ancestors.delete(item);
    }
  };
  const bytes = Buffer.from(visit(value, 0), 'utf8');
  if (bytes.length > 1024 * 1024) throw new Error('canonical JSON too large');
  return bytes;
}

export const canonicalHash = (value: unknown): string => byteDigest(canonicalBytes(value));

export function verifyBytes(bytes: Uint8Array, digest: string): void {
  if (!/^sha256:[0-9a-f]{64}$/.test(digest) || digest.length !== 71) throw new Error('invalid digest');
  const actual = Buffer.from(byteDigest(bytes), 'ascii');
  if (!timingSafeEqual(actual, Buffer.from(digest, 'ascii'))) throw new Error('digest mismatch');
}
