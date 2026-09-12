import assert from 'node:assert/strict';
import test from 'node:test';
import { byteDigest, canonicalBytes, canonicalHash, verifyBytes } from '../src/contracts/hash.js';
import { parseCalVer } from '../src/contracts/tag.js';
import { ControlError, failureResult, redact } from '../src/contracts/result.js';

test('canonical JSON has stable object order, byte identity, and meaningful array order', () => {
  assert.equal(Buffer.from(canonicalBytes({ z: 2, a: { b: 1 } })).toString(), '{"a":{"b":1},"z":2}');
  assert.equal(canonicalHash({ a: 1, b: [2, 3] }), canonicalHash({ b: [2, 3], a: 1 }));
  assert.notEqual(canonicalHash([1, 2]), canonicalHash([2, 1]));
  assert.equal(byteDigest(Buffer.from('abc')), 'sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
  assert.notEqual(byteDigest(Buffer.from('{}\n')), byteDigest(Buffer.from('{}')));
  assert.throws(() => verifyBytes(Buffer.from('tampered'), byteDigest(Buffer.from('original'))));
  verifyBytes(Buffer.from('valid'), byteDigest(Buffer.from('valid')));
});
for (const invalid of [undefined, NaN, Infinity, 1.5, Number.MAX_SAFE_INTEGER + 1, new Date(), [undefined], Array(2), { a: undefined }, { toJSON: () => ({}) }]) {
  test(`reject noncanonical input ${typeof invalid}`, () => assert.throws(() => canonicalHash(invalid)));
}
test('reject cyclic/accessor/symbol objects', () => {
  const cycle: { self?: unknown } = {}; cycle.self = cycle;
  assert.throws(() => canonicalHash(cycle));
  assert.throws(() => canonicalHash(Object.defineProperty({}, 'x', { enumerable: true, get() { throw new Error('secret'); } })));
  assert.throws(() => canonicalHash({ [Symbol('x')]: 1 }));
});
test('canonical arrays reject holes with extra keys, getters, symbols and subclasses', () => {
  const sparse = Object.assign(Array(1), { extra: 1 });
  let accessed = false;
  const accessor = Object.defineProperty([], '0', { enumerable: true, get() { accessed = true; return 1; } });
  const symbol = Object.assign([1], { [Symbol('extra')]: 2 });
  class OtherArray extends Array<number> {}
  for (const value of [sparse, accessor, symbol, new OtherArray(1)]) assert.throws(() => canonicalHash(value));
  assert.equal(accessed, false);
});
for (const valid of ['20260912-1200', '20240229-2359', '20000229-0000']) {
  test(`accept UTC tag ${valid}`, () => assert.equal(parseCalVer(valid).getUTCSeconds(), 0));
}
for (const invalid of ['20260229-0000', '21000229-0000', '20261301-0000', '20260431-0000', '20260912-2400', '20260912-0060', '20260912-1200Z', '20260912-1200\n', '20260912-1200;id', '../20260912-1200', '2026912-1200']) {
  test(`reject malformed CalVer ${JSON.stringify(invalid)}`, () => assert.throws(() => parseCalVer(invalid)));
}
for (const input of [
  { password: 'sensitive-fixture', authorization: 'sensitive-fixture', cookie: 'sensitive-fixture' },
  { database_url: 'postgres://user:sensitive-fixture@fixture/db', private_key: 'sensitive-fixture' },
  { receipt_handle: 'sensitive-fixture', queue_body: 'sensitive-fixture', user_record: 'sensitive-fixture' },
  { provider_error: 'sensitive-fixture', token: 'sensitive-fixture', environment: ['sensitive-fixture'] },
  new Error('sensitive-fixture'), 'sensitive-fixture', { 'sensitive-fixture': 123 },
]) {
  test('redact arbitrary secrets, unknown fields and provider errors', () => {
    assert.equal(redact(input), '[REDACTED]');
    assert.doesNotMatch(JSON.stringify(failureResult(input)), /sensitive-fixture/);
  });
}
test('unknown outcome asks reconcile, not blind retry', () => {
  assert.equal(failureResult(new ControlError('UNCERTAIN_REMOTE_OUTCOME')).retry, 'RECONCILE');
  assert.equal(failureResult(new ControlError('RETRYABLE_DEPENDENCY')).retry, 'DO_NOT_RETRY');
});
