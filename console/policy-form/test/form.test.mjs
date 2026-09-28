import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setMode, denyHost, changes, assertSafeNumbers } from '../policy-form.mjs';
const schema = JSON.parse(readFileSync(new URL('../../../docs/contracts/source-policy.schema.json', import.meta.url).pathname));
const policy = {
  policy_mode: 'observe', constraints: [{kind: 'access_rule', host: '*.example.test', action: 'allow'}],
  scopes: [{match: '/work/narrow'}, {match: '/work', policy_mode: 'strict', constraints: []}],
  terms: [{host:'example.test', reference:'agreement'}], record_internal_prefixes: ['https://internal.example.test/'],
  principals: [{principal:'person', os_user:100}], allow_private_hosts: true
};
test('mode changes preserve all other fields and array order without mutating the snapshot', () => {
  const before = structuredClone(policy);
  const edited = setMode(policy, 0, 'prefer');
  assert.deepEqual(policy, before);
  assert.deepEqual(edited, {...policy, scopes: [{...policy.scopes[0], policy_mode:'prefer'}, policy.scopes[1]]});
  assert.deepEqual(setMode(edited, 0, ''), policy);
  assert.ok(schema.$defs.PolicyMode.oneOf.some(m => m.const === 'prefer'));
});
test('first denied host materialises inherited constraints without reordering existing rules', () => {
  const edited = denyHost(policy, 0, 'tracker.example.test');
  assert.deepEqual(edited.scopes[0].constraints, [...policy.constraints, {kind:'denied_source_host', host:'tracker.example.test'}]);
  assert.deepEqual(edited.constraints, policy.constraints);
  assert.deepEqual(edited.scopes[1].constraints, []);
  assert.equal(policy.scopes[0].constraints, undefined);
});
test('review includes field removal, array order, and fields beyond the simple controls', () => {
  const after = structuredClone(policy);
  delete after.allow_private_hosts;
  after.scopes.reverse();
  after.terms[0].reference = 'changed';
  assert.deepEqual(changes(policy, after).map(c => c.path), ['/scopes', '/terms', '/allow_private_hosts']);
  assert.deepEqual(changes(policy, structuredClone(policy)), []);
});

test('unsafe integers cannot silently round during unrelated edits', () => {
  assert.throws(() => assertSafeNumbers(JSON.parse('{"micros":9007199254740993}')), /exact integer precision/);
  assert.throws(() => assertSafeNumbers({micros:1e400}), /exact integer precision/);
  assert.doesNotThrow(() => assertSafeNumbers({micros:9007199254740991}));
});
