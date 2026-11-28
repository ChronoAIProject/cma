import assert from 'node:assert/strict';
import test from 'node:test';
import {installSkillCase} from './skills.mjs';

function fixture({effects = [{status: 'installed', version: '1.0'}], staleCatalog = false, duplicateRename = false,
  changes = [{seq: 1, status: 'pending'}, {seq: 2, status: 'installed'}],
  publicationRetry = false, abortOn = 'a'} = {}) {
  const calls = [];
  const probes = [], renames = [];
  let armed, abortArmed = false;
  let preferences = {title: 'Original title', revision: 1};
  const id = publicationRetry ? 'two-am-retry-before-restart' : 'two-am-before-restart';
  const admission = {items: [{id, version: '1.0', status: 'pending'}], pending: true};
  const api = {
    router: {dispatch: async (agentId, operation) => {
      assert.equal(agentId, 'fixture-agent');
      return operation('b');
    }},
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
    async rawRequest(am, method, path, intent) {
      if (path.endsWith('/skills')) {
        if (publicationRetry) {
          assert.equal(abortArmed, true);
          assert.equal(am, 'b');
        }
        return api.request(am, method, path, intent);
      }
      if (path.endsWith('/preferences')) {
        if (method === 'PATCH') {
          assert.equal(am, 'a');
          assert.equal(intent.body.expected_revision, preferences.revision);
          preferences = {title: intent.body.title, revision: preferences.revision + 1};
          renames.push(preferences.title);
        }
        return {body: preferences};
      }
      assert.equal(method, 'GET');
      if (path === '/api/v1/agents/fixture-agent') return {body: {id: 'fixture-agent', status: 'ready'}};
      assert.equal(path, '/api/v2/responses/response');
      return {body: {id: 'response', status: 'completed', conversation: {id: 'fixture-agent'},
        output: [{text: 'WS4 complete ' + armed.marker}]}};
    },
  };
  const adapter = {
    async ownerOf({agentId}) { return {agent_id: agentId, am: 'a', pod_uid: 'pod', epoch: 1}; },
    async writerPublicationAbort({arm}) {
      abortArmed = arm;
      return {count: arm ? 2 : 3};
    },
    async writerPublicationEvidence(input) {
      assert.deepEqual(input, {agentId: 'fixture-agent', operation: 'skill_install'});
      const evidence = {a: 0, b: 0};
      if (calls.some(row => row.method === 'POST')) evidence[abortOn] += 1;
      return evidence;
    },
    async titleEffects({agentId, title}) {
      assert.equal(agentId, 'fixture-agent');
      assert.equal(title, preferences.title);
      return duplicateRename ? [4, 5] : [4];
    },
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
  return {api, adapter, record: {agentId: 'fixture-agent'}, phase: 'before_restart', calls, probes, renames,
    isAbortArmed: () => abortArmed};
}

test('installation replays the unchanged receipt and reads one durable effect', async () => {
  const context = fixture();
  await installSkillCase(context);
  assert.deepEqual(context.calls.map(row => row.method), ['GET', 'POST', 'GET', 'POST']);
  assert.deepEqual(context.calls[1].intent, context.calls[3].intent);
  assert.equal(context.calls[1].am, 'b');
  assert.match(context.renames[0], /^Skill publication /);
  assert.equal(context.renames[1], 'Original title');
  assert.deepEqual(context.probes.map(probe => probe.present), [false, true]);
});

test('a duplicated rename fails the concurrent case and restores the original title', async () => {
  const context = fixture({duplicateRename: true});
  await assert.rejects(installSkillCase(context), /skill_publication_rename_effect_unconfirmed/);
  assert.equal(context.renames.at(-1), 'Original title');
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

test('the skill intent retries one abort at the owner and installs once', async () => {
  const context = fixture({publicationRetry: true});
  await installSkillCase(context, true);
  assert.equal(context.isAbortArmed(), false);
  assert.equal(context.calls[1].am, 'b');
  assert.deepEqual(context.calls[1].intent.body, context.calls[3].intent.body);
  assert.deepEqual(context.probes.map(probe => probe.present), [false, true]);
});

test('a skill receipt cannot use evidence from the non-owner', async () => {
  const context = fixture({publicationRetry: true, abortOn: 'b'});
  await assert.rejects(installSkillCase(context, true), /writer_publication_abort_not_attributed_to_owner/);
  assert.equal(context.isAbortArmed(), false);
});
