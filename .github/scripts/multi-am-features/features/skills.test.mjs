import assert from 'node:assert/strict';
import test from 'node:test';
import {installSkillCase} from './skills.mjs';

function fixture(effects = [{status: 'installed', version: '1.0'}]) {
  const calls = [];
  const id = 'two-am-before-restart';
  const admission = {items: [{id, version: '1.0', status: 'pending'}], pending: true};
  const api = {
    async request(am, method, path, intent) {
      assert.match(path, /\/skills$/);
      calls.push({am, method, intent});
      if (method === 'POST') return {body: admission};
      return {body: calls.length === 1 ? {items: [], can_install: true}
        : {items: [{id, version: '1.0', status: 'installed'}], pending: false}};
    },
  };
  const adapter = {async skillEffects(input) {
    assert.deepEqual(input, {agentId: 'fixture-agent', skillId: id});
    return effects;
  }};
  return {api, adapter, record: {agentId: 'fixture-agent'}, phase: 'before_restart', calls};
}

test('installation replays the unchanged receipt and reads one durable effect', async () => {
  const context = fixture();
  await installSkillCase(context);
  assert.deepEqual(context.calls.map(row => row.method), ['GET', 'POST', 'GET', 'POST']);
  assert.deepEqual(context.calls[1].intent, context.calls[3].intent);
});

test('duplicate durable installation fails even with one public skill row', async () => {
  const effect = {status: 'installed', version: '1.0'};
  await assert.rejects(installSkillCase(fixture([effect, effect])), /one_durable_skill_install_required/);
});
