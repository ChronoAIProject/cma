import test from 'node:test';
import assert from 'node:assert/strict';
import {configurationDenials} from './configuration_denials.mjs';

const fixture = (mode = 'refused') => {
  const state = {preferences: {revision: 4, title: 'owner', system_prompt: 'owner instructions'},
    settings: {model: 'fixture', reasoning_effort: 'high', permission_mode: 'guarded'}};
  const writes = [], evidence = [];
  return {record: {agentId: 'agent'}, writes, evidence,
    runCase: async (_name, run) => run(), recordState: async (_name, value) => evidence.push(value),
    api: {request: async (_am, _method, path) => ({body: structuredClone(state[path.split('/').at(-1)])})},
    outsider: {request: async (_am, method, _path, options) => {
      assert.deepEqual(options.allowed, [403, 404]);
      if (method === 'PATCH') {
        writes.push(options);
        if (mode === 'mutated') state.preferences.title = 'unauthorized';
      }
      return {status: mode === 'allowed' ? 200 : 404};
    }}};
};

test('authenticated outsider is refused changed mutations and owner state remains intact', async () => {
  const x = fixture(); await configurationDenials(x);
  assert.equal(x.writes.length, 3); assert.equal(x.evidence.length, 1);
  assert.equal(new Set(x.writes.map(row => row.key)).size, 3);
  assert.deepEqual(x.evidence[0].before, x.evidence[0].after);
});

test('success status or mutation behind a denial cannot count as authorization coverage', async () => {
  await assert.rejects(configurationDenials(fixture('allowed')), /foreign_configuration_read_allowed/);
  await assert.rejects(configurationDenials(fixture('mutated')), /foreign_configuration_changed_owner_state/);
});
