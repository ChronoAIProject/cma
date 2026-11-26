import assert from 'node:assert/strict';
import test from 'node:test';
import {installSkillCase} from './skills.mjs';

function fixture({effects = [{status: 'installed', version: '1.0'}], staleCatalog = false,
  changes = [{seq: 1, status: 'pending'}, {seq: 2, status: 'installed'}]} = {}) {
  const calls = [];
  const probes = [];
  let armed;
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
    async send(_am, agentId, input, key) {
      assert.equal(agentId, 'fixture-agent');
      assert.equal(key, armed.caseId);
      assert.ok(input.includes(armed.marker));
      assert.ok(!input.includes(id));
      return {responseId: 'response', error: null, events: [{type: 'response.completed'}],
        abort() {}, done: Promise.resolve()};
    },
    async rawRequest(_am, method, path) {
      assert.equal(method, 'GET');
      if (path === '/api/v1/agents/fixture-agent') return {body: {id: 'fixture-agent', status: 'ready'}};
      assert.equal(path, '/api/v2/responses/response');
      return {body: {id: 'response', status: 'completed', conversation: {id: 'fixture-agent'},
        output: [{text: 'WS4 complete ' + armed.marker}]}};
    },
  };
  const adapter = {
    async prepareTurn(value) { armed = value; probes.push(value.skill); },
    async releaseTurn({caseId}) { assert.equal(caseId, armed.caseId); },
    async oracle({caseId}) {
      assert.equal(caseId, armed.caseId);
      const present = !staleCatalog && calls.some(row => row.method === 'POST');
      return {case_id: caseId, request_count: 1, completed_effect_count: 1, active_streams: 0,
        skill_matches: Number(present === armed.skill.present)};
    },
    async skillEffects(input) {
      assert.deepEqual(input, {agentId: 'fixture-agent', skillId: id});
      return {agentFound: true, entries: effects, changes};
    },
  };
  return {api, adapter, record: {agentId: 'fixture-agent'}, phase: 'before_restart', calls, probes};
}

test('installation replays the unchanged receipt and reads one durable effect', async () => {
  const context = fixture();
  await installSkillCase(context);
  assert.deepEqual(context.calls.map(row => row.method), ['GET', 'POST', 'GET', 'POST']);
  assert.deepEqual(context.calls[1].intent, context.calls[3].intent);
  assert.deepEqual(context.probes.map(probe => probe.present), [false, true]);
});

test('duplicate durable installation fails even with one public skill row', async () => {
  const effect = {status: 'installed', version: '1.0'};
  await assert.rejects(installSkillCase(fixture({effects: [effect, effect]})), /one_durable_skill_install_required/);
  await assert.rejects(installSkillCase(fixture({changes: [{seq: 1, status: 'pending'},
    {seq: 2, status: 'installed'}, {seq: 3, status: 'installed'}]})), /one_durable_skill_install_required/);
});

test('the installed status cannot pass when the native runtime retains the original skill catalog', async () => {
  await assert.rejects(installSkillCase(fixture({staleCatalog: true})), /native_skill_catalog_mismatch/);
});
