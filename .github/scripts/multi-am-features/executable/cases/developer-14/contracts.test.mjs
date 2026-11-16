import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {cases, setups} from './index.mjs';
import {freshAgent} from './setups.mjs';
import {chooseSettings} from './settings.mjs';
import {promptEffect} from './prompt.mjs';
import {sha, request, conflictPreferences} from './common.mjs';
import {Blocked} from '../../runner.mjs';

test('assigned three cases implement every canonical step and expose concrete setups', async () => {
  const catalog = JSON.parse(await readFile(new URL('../../catalog.json', import.meta.url)));
  const assignment = JSON.parse(await readFile(new URL('../../assignments/developer-14.json', import.meta.url)));
  assert.deepEqual(cases.map(c => c.id), assignment.case_ids);
  for (const definition of cases) {
    assert.deepEqual(definition.steps, catalog.rows.find(r => r.case_id === definition.id).required_steps);
    assert.equal(definition.scope, 'agent');
    assert.deepEqual(definition.fixtures, ['user-fixtures', 'browser']);
  }
  assert.equal(Object.keys(setups).length, 4);
});

function nativeFixture({lostAck = false, unknownAck = false, wrongRuntime = false, wrongOwner = false} = {}) {
  const calls = [], deferred = [], observations = [];
  let createBody, released = 0, deleted = false;
  const ctx = {row: {case_id: 'synthetic-setup-test'}, am: 'a',
    defer(label, work) { deferred.push({label, work}); },
    async observe(label, work) { const value = await work(); observations.push({label, value}); return value; },
    async request(options) {
      calls.push(options);
      if (options.method === 'POST') {
        assert.ok(deferred.some(x => x.label === 'configuration_public_agent_cleanup'));
        createBody = options.body;
        if (lostAck || unknownAck) throw Object.assign(Error('request_attribution_missing'),
          unknownAck ? {} : {response: {status: 202, body: {id: 'op_test'}}});
        return {status: 202, body: {id: 'op_test'}};
      }
      if (options.path.includes('/agent-lifecycle/')) return {status: 200, body: {id: 'op_test', agent_id: 'agt_test', status: 'succeeded'}};
      if (options.path.endsWith('/preferences')) return {status: 200, body: {prompt_supported: true, prompt_pending: false, revision: 0}};
      if (options.path.includes('/agents/')) return {status: 200, body: {id: 'agt_test', status: 'ready', sandbox: {id: 'sb_test', status: 'active'}}};
      if (options.method === 'DELETE') { deleted = true; return {status: 202, body: {id: 'sb_test', status: 'retiring'}}; }
      return {status: 200, body: {id: 'sb_test', owner: wrongOwner ? 'foreign' : 'test-owner',
        name: createBody.name + ' sandbox op_test', status: deleted ? 'retired' : 'active'}};
    }};
  const bindings = {scope: 'LOCAL_DISPOSABLE_TWO_AM',
    async acquireVariant() { return {scope: 'LOCAL_DISPOSABLE_TWO_AM', variant: 'supported', exclusive: true,
      runtimeImage: 'fixture@sha256:' + 'a'.repeat(64), runtimeSource: 'b'.repeat(40), owner: 'test-owner',
      async release() { released++; }}; },
    async runtimeFor() { return {agent_id: 'agt_test', ready: true, pod_uid: 'synthetic-pod',
      image: 'fixture@sha256:' + (wrongRuntime ? 'c' : 'a').repeat(64)}; },
    async registerWitness() {}, provider: {prepareTurn() {}, releaseTurn() {}, oracle() {}}};
  return {ctx, bindings, calls, deferred, observations, released: () => released,
    async finish() { const errors = []; for (const item of [...deferred].reverse()) {
      try { await item.work(); } catch (error) { errors.push(error.message); }
    } return errors; }};
}

test('missing native binding is BLOCKED before creating an Agent', async () => {
  const x = nativeFixture();
  await assert.rejects(freshAgent(x.ctx, {}), error => error instanceof Blocked);
  assert.equal(x.calls.length, 0);
});

test('public fresh setup joins real runtime and registers cleanup before creation', async () => {
  const x = nativeFixture(), resource = await freshAgent(x.ctx, x.bindings);
  assert.equal(resource.agentId, 'agt_test'); assert.equal(typeof resource.cleanup, 'function');
  assert.equal(x.calls.filter(c => c.method === 'POST').length, 1);
  assert.ok(x.observations.some(o => o.label === 'configuration_runtime_binding'));
  await resource.cleanup(); assert.deepEqual(await x.finish(), []);
  assert.equal(x.calls.filter(c => c.method === 'DELETE').length, 1);
  assert.equal(x.released(), 1);
});

test('accepted creation attribution failure keeps original receipt and cleans without retry', async () => {
  const x = nativeFixture({lostAck: true});
  await assert.rejects(freshAgent(x.ctx, x.bindings), /request_attribution_missing/);
  assert.deepEqual(await x.finish(), []);
  assert.equal(x.calls.filter(c => c.method === 'POST').length, 1);
  assert.equal(x.calls.filter(c => c.method === 'DELETE').length, 1);
});

test('unknown create result is retained as unresolved cleanup, never rekeyed', async () => {
  const x = nativeFixture({unknownAck: true});
  await assert.rejects(freshAgent(x.ctx, x.bindings), /request_attribution_missing/);
  assert.deepEqual(await x.finish(), ['configuration_unknown_create_outcome_requires_parent_cleanup']);
  assert.equal(x.calls.length, 1); assert.equal(x.released(), 1);
});

test('wrong runtime refuses setup while still cleaning owned resources', async () => {
  const x = nativeFixture({wrongRuntime: true});
  await assert.rejects(freshAgent(x.ctx, x.bindings), /configuration_runtime_identity_mismatch/);
  assert.deepEqual(await x.finish(), []);
});

test('public cleanup refuses foreign Sandbox before retirement mutation', async () => {
  const x = nativeFixture({wrongOwner: true}); await freshAgent(x.ctx, x.bindings);
  assert.deepEqual(await x.finish(), ['configuration_cleanup_owner_mismatch']);
  assert.equal(x.calls.filter(c => c.method === 'DELETE').length, 0);
});

test('configuration writes omit explicit AM so routing selects observed non-owner', async () => {
  let actual;
  await request({am: 'a', request: async options => { actual = options; }}, {agentId: 'agt_test'}, 'PATCH', '/preferences', {title: 'Changed'});
  assert.equal(actual.agentId, 'agt_test'); assert.equal('am' in actual, false);
});

test('model selection excludes unavailable options and changes actual supported settings', () => {
  const initial = {model: 'one', reasoning_effort: 'low', permission_mode: 'guarded'};
  const choice = chooseSettings(initial, [{id: 'one', available: true}, {id: 'two', available: false}, {id: 'three', available: true}]);
  assert.deepEqual(choice, {model: 'three', reasoning_effort: 'high', permission_mode: 'autonomous'});
  assert.throws(() => chooseSettings(initial, [{id: 'one', available: false}]), Blocked);
  assert.throws(() => chooseSettings(initial, [{id: 'one', available: true}]), /alternate_available_model_required/);
});

test('prompt effect needs actual instruction marker hash and one matching provider request', async () => {
  const marker = 'PROMPT-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee', expected = 'Instructions ' + marker;
  const ctx = {assert: (v, code) => assert.ok(v, code), observe: async (_label, work) => work()};
  const resource = {controls: {requestMetadata: async () => [{case_id_sha256: sha('case'), instruction_marker_sha256: []}]}};
  await assert.rejects(promptEffect(ctx, resource, {caseId: 'case'}, expected), /not_in_actual_next_turn/);
  resource.controls.requestMetadata = async () => [{case_id_sha256: sha('case'), instruction_marker_sha256: [sha(marker)]}];
  await promptEffect(ctx, resource, {caseId: 'case'}, expected);
});

test('revision barrier commits competing write before dispatching the unchanged browser PATCH once', async () => {
  const events = []; let handler, resolveResponse, result;
  const body = {expected_revision: 4, title: 'stale draft'};
  const actualRequest = {method: () => 'PATCH', postDataJSON: () => body};
  const response = {status: () => 409, request: () => actualRequest, json: async () => ({code: 'agent_operation_in_flight'})};
  const page = {
    async route(_pattern, value) { handler = value; },
    async unroute() { events.push('unroute'); },
    waitForResponse() { return new Promise(resolve => { resolveResponse = resolve; }); },
  };
  const editor = {save: {async click() {
    result = handler({request: () => actualRequest,
      async continue() { events.push('dispatch-original'); resolveResponse(response); },
      async abort() { throw Error('unexpected-abort'); }});
  }}};
  const observed = await conflictPreferences(page, editor, async captured => {
    assert.equal(captured, body); events.push('concurrent-commit'); return {revision: 5};
  });
  await result;
  assert.deepEqual(events, ['concurrent-commit', 'dispatch-original', 'unroute']);
  assert.equal(observed.conflict.request, body); assert.equal(observed.conflict.status, 409);
  assert.equal(observed.winner.revision, 5);
});
