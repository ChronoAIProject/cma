import test from 'node:test';
import assert from 'node:assert/strict';
import {NonOwnerRouter} from './routing.mjs';
import {coverage, loadInventory} from './coverage.mjs';
import {readConfig} from './config.mjs';
import {RequestPacer} from './pacing.mjs';
import {API, assertOwner, hash} from './cma/core.mjs';

test('command polling follows its Agent receipt to the current non-owner', async () => {
  const api = new API({bearer: 'command-routing-fixture'});
  const calls = [];
  api.routedAgents.add('agent');
  api.router = {dispatch: async (agent, run) => { assert.equal(agent, 'agent'); return run('b'); }};
  api.rawRequest = async (am, method, path) => {
    calls.push({am, method, path});
    return {body: method === 'POST' ? {control: {receipt: {command_id: 'command'}}} : {status: 'completed'}};
  };
  await api.request('a', 'POST', '/api/v2/agents/agent/responses/response/controls');
  await api.request('a', 'GET', '/api/v2/commands/command');
  assert.equal(calls.length, 2);
  assert.deepEqual(calls.map(row => row.am), ['b', 'b']);
});

test('Agent-bound admin Sandbox writes follow the current non-owner without rebinding another Agent', async () => {
  const api = new API({bearer: 'sandbox-routing-fixture'}), calls = [];
  api.bindSandbox('sandbox', 'agent');
  assert.throws(() => api.bindSandbox('sandbox', 'another-agent'), /sandbox_agent_binding_changed/);
  api.router = {dispatch: async (agent, run) => { assert.equal(agent, 'agent'); return run('b'); }};
  api.rawRequest = async (am, method, path) => { calls.push({am, method, path}); return {body: {}}; };
  await api.request('a', 'POST', '/api/v1/admin/sandboxes/sandbox/always-on');
  await api.request('a', 'GET', '/api/v1/admin/sandboxes/sandbox');
  assert.deepEqual(calls.map(row => row.am), ['b', 'b']);
});

test('known admission replay follows its original Agent to the current non-owner', async () => {
  const api = new API({bearer: 'admission-routing-fixture'});
  assert.throws(() => api.bindAdmission('known-key', 'agent'), /admission_agent_binding_invalid/);
  api.routedAgents.add('agent'); api.bindAdmission('known-key', 'agent');
  api.routedAgents.add('another-agent');
  assert.throws(() => api.bindAdmission('known-key', 'another-agent'), /admission_agent_binding_changed/);
  api.router = {dispatch: async (agent, run) => { assert.equal(agent, 'agent'); return run('b'); }};
  const calls = [];
  api.rawRequest = async (am, method, path, options) => {
    calls.push({am, method, path, key: options.key}); return {body: {id: 'known-operation'}};
  };
  const result = await api.request('a', 'POST', '/api/v1/agents', {key: 'known-key'});
  assert.equal(result.body.id, 'known-operation');
  assert.deepEqual(calls, [{am: 'b', method: 'POST', path: '/api/v1/agents', key: 'known-key'}]);
});

test('requests sharing a credential reserve distinct slots across AM clients and preserve independent actors', async () => {
  const waits = [];
  const pacer = new RequestPacer({now: () => 100, sleep: async ms => { waits.push(ms); }});
  await Promise.all([pacer.wait('owner', 'POST'), pacer.wait('owner', 'POST'), pacer.wait('owner', 'POST')]);
  assert.deepEqual(waits, [6200, 12400]);
  await pacer.wait('other-owner', 'POST');
  await pacer.wait('owner', 'GET');
  assert.deepEqual(waits, [6200, 12400]);
  await pacer.wait('owner', 'GET');
  assert.equal(waits.at(-1), 120);
  assert.ok([...pacer.next.keys()].every(key => !key.includes('owner')));
});

test('client config accepts split UTF-8 over a pipe and rejects overlong or truncated input', async () => {
  const bytes = Buffer.from(JSON.stringify({owner: 'fixture-测试'}));
  assert.deepEqual(await readConfig([...bytes].map(x => Buffer.from([x]))), {owner: 'fixture-测试'});
  await assert.rejects(readConfig([Buffer.alloc(64 * 1024 + 1)]), /client_config_bound/);
  await assert.rejects(readConfig([Buffer.from('{"owner":')]), SyntaxError);
});

const owner = {agent_id: 'agent', am: 'a', epoch: 3, instance_id: 'instance-a', pod_uid: 'pod-a'};
const topology = {shared_database: true, coordination_enabled: true, ams: {
  a: {ready: true, instance_id: 'instance-a', pod_uid: 'pod-a'},
  b: {ready: true, instance_id: 'instance-b', pod_uid: 'pod-b'},
}};
const setup = (changes = {}) => {
  const proofs = [];
  const adapter = {ownerOf: async () => owner, topology: async () => topology, ...changes};
  return {proofs, router: new NonOwnerRouter({adapter, record: p => proofs.push(p)})};
};
test('uses the live non-owner and records independent process identities', async () => {
  const {router, proofs} = setup();
  const response = {status: 202, timing: {request_id: 'fixture-request'}};
  assert.equal(await router.dispatch('agent', async am => { assert.equal(am, 'b'); return response; }), response);
  assert.equal(proofs[0].verdict, 'PASS');
  assert.notEqual(proofs[0].owner_instance_sha256, proofs[0].ingress_instance_sha256);
  assert.ok(!JSON.stringify(proofs).includes('pod-a'));
});
test('writer route needs one matching event from the owning process', async () => {
  const requestId = '5ecbd0e3-4d26-465d-9595-7ba12271cf05';
  const request = {status: 202, timing: {request_id: requestId, method: 'POST',
    route: '/api/v1/agents/{id}/skills'}};
  for (const matches of [0, 1, 2]) {
    let calls = 0;
    const {router, proofs} = setup({peerWriteEvidence: async value => {
      assert.deepEqual(value, {am: owner.am, requestHash: hash(requestId), epoch: owner.epoch,
        operation: 'skill_install'});
      return {matches};
    }});
    const submitted = router.dispatch('agent', async () => { calls++; return request; });
    if (matches === 1) {
      assert.equal(await submitted, request);
      assert.equal(proofs[0].verified_owner_write, true);
    } else {
      await assert.rejects(submitted, /verified_owner_write_missing_or_duplicated/);
      assert.equal(proofs[0].verdict, 'FAIL');
    }
    assert.equal(calls, 1);
  }
});
test('refuses absent leases and a mismatched topology before executing', async () => {
  let calls = 0;
  for (const changes of [{ownerOf: async () => null}, {topology: async () => ({...topology, shared_database: false})}]) {
    await assert.rejects(setup(changes).router.dispatch('agent', async () => calls++));
  }
  assert.equal(calls, 0);
});
test('owner movement during a write fails attribution without replay', async () => {
  let reads = 0, calls = 0;
  const {router, proofs} = setup({ownerOf: async () => ++reads === 1 ? owner : {...owner, am: 'b', epoch: 4}});
  await assert.rejects(router.dispatch('agent', async () => {
    calls++; return {status: 202, timing: {request_id: 'fixture-request'}};
  }), /owner_changed/);
  assert.equal(calls, 1); assert.equal(proofs[0].verdict, 'FAIL');
});
test('missing request correlation fails attribution after exactly one mutation', async () => {
  for (const requestId of [undefined, null, '', '   ', 42]) {
    let calls = 0;
    const {router, proofs} = setup();
    await assert.rejects(router.dispatch('agent', async () => {
      calls++; return {status: 202, timing: {request_id: requestId}};
    }), /response_request_id_required/);
    assert.equal(calls, 1);
    assert.equal(proofs.length, 1);
    assert.equal(proofs[0].verdict, 'FAIL');
  }
});
test('zero and invalid ownership epochs never execute a mutation', async () => {
  let calls = 0;
  for (const epoch of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, '1', undefined]) {
    const {router, proofs} = setup({ownerOf: async () => ({...owner, epoch})});
    assert.throws(() => assertOwner({...owner, epoch}, 'agent'), /owner_identity_missing/);
    await assert.rejects(router.dispatch('agent', async () => { calls++; }), /live_owner_required/);
    assert.equal(proofs.length, 0);
  }
  assert.equal(calls, 0);
});
test('uncertain mutation is recorded once and never retried', async () => {
  let calls = 0; const {router, proofs} = setup();
  await assert.rejects(router.dispatch('agent', async () => { calls++; throw Error('connection lost'); }));
  assert.equal(calls, 1); assert.equal(proofs[0].verdict, 'FAIL');
});
test('all inventory identities are retained', async () => {
  const inventory = await loadInventory(new URL('./inventory/', import.meta.url).pathname);
  assert.equal(inventory.filter(x => x.role === 'user').length, 120);
  assert.equal(inventory.filter(x => x.role === 'admin').length, 76);
  assert.equal(inventory.filter(x => x.role === 'eg').length, 79);
  assert.equal(inventory.length, 275);
  assert.ok(inventory.filter(x => x.role === 'eg').every(x => Array.isArray(x.actor_roles)
    && x.steps.length > 0 && x.minimum_runs === 3));
});
const features = [{id: 'user.send', role: 'user'}, {id: 'admin.settings', role: 'admin'}];
test('missing mapping and not-run cases cannot reduce the denominator', () => {
  const result = coverage(features, {'user.send': {cases: ['send']}}, []);
  assert.equal(result.total, 2); assert.equal(result.verdict, 'FAIL');
  assert.deepEqual(result.counts, {PASS: 0, FAIL: 0, BLOCKED: 1, NOT_RUN: 1});
});
test('passing HTTP is insufficient without non-owner evidence and all cases', () => {
  const mappings = {'user.send': {cases: ['send'], routing: 'non_owner'},
    'admin.settings': {cases: ['cas', 'stale'], routing: 'both'}};
  const rows = [{case: 'send', iteration: 1, verdict: 'PASS'}, {case: 'cas', iteration: 1, verdict: 'PASS', ams: ['a', 'b']}];
  assert.equal(coverage(features, mappings, rows).verdict, 'FAIL');
  rows[0].non_owner = true; rows.push({case: 'stale', iteration: 1, verdict: 'PASS', ams: ['a', 'b']});
  assert.equal(coverage(features, mappings, rows).verdict, 'PASS');
});
test('failed repetition and missing browser observation stay failing', () => {
  const mappings = {'user.send': {cases: ['send'], minimum_runs: 3, browser: true}};
  const rows = Array.from({length: 3}, (_, i) => ({case: 'send', iteration: i + 1, verdict: 'PASS'}));
  assert.equal(coverage(features.slice(0, 1), mappings, rows).verdict, 'FAIL');
  rows.forEach(row => row.real_browser = true); rows[1].verdict = 'FAIL';
  assert.equal(coverage(features.slice(0, 1), mappings, rows).verdict, 'FAIL');
});

test('duplicate observations cannot replace distinct runs before and after restart', () => {
  const selected = features.slice(0, 1);
  const mapping = {'user.send': {cases: ['send'], minimum_runs: 3,
    phases: ['before_restart', 'after_restart'], routing: 'non_owner'}};
  const make = (phase, iteration) => ({case: 'send', verdict: 'PASS', phase, iteration, non_owner: true});
  const before = [1, 2, 3].map(i => make('before_restart', i));
  assert.equal(coverage(selected, mapping, before.concat(before)).verdict, 'FAIL');
  const after = [1, 1, 2].map(i => make('after_restart', i));
  assert.equal(coverage(selected, mapping, before.concat(after)).verdict, 'FAIL');
  after.push(make('after_restart', 3));
  assert.equal(coverage(selected, mapping, before.concat(after)).verdict, 'PASS');
});

test('both-process features require both AMs in every counted iteration and restart phase', () => {
  const selected = features.slice(1);
  const mapping = {'admin.settings': {cases: ['cas'], minimum_runs: 3,
    phases: ['before_restart', 'after_restart'], routing: 'both'}};
  const make = (phase, iteration, ams) => ({case: 'cas', verdict: 'PASS', phase, iteration, ams});
  const before = [1, 2, 3].map(i => make('before_restart', i, ['a', 'b']));
  const after = [1, 2, 3].map(i => make('after_restart', i, ['a']));
  assert.equal(coverage(selected, mapping, before.concat(after)).verdict, 'FAIL');
  after.push(make('after_restart', 1, ['b']), make('after_restart', 2, ['b']));
  assert.equal(coverage(selected, mapping, before.concat(after)).verdict, 'FAIL');
  after.push(make('after_restart', 3, ['b']));
  assert.equal(coverage(selected, mapping, before.concat(after)).verdict, 'PASS');
});
