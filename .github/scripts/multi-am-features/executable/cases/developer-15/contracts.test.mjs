import test from 'node:test';
import assert from 'node:assert/strict';
import {fileSetup, observedPeer, hash, setup} from './helpers.mjs';
import {cases} from './index.mjs';
import {loadRegistry} from '../../registry.mjs';

test('six exact file rows retain all original steps', async () => {
  const registry = await loadRegistry(); assert.equal(cases.length, 6);
  for (const definition of cases) {
    assert.equal(registry.owners.get(definition.id), 'developer-15');
    assert.deepEqual(definition.steps, registry.rows.get(definition.id).required_steps);
  }
});

test('file setup fails closed before creating on missing or wrong runtime', async () => {
  let requests = 0; const ctx = {request: async () => {requests++;}};
  await assert.rejects(fileSetup(ctx, {}), /file_runtime_fixture_unavailable/);
  await assert.rejects(fileSetup(ctx, {fileAgent: {ownedByRun: true, agentId: 'a', actor: 'user', runtimeImage: 'exact'},
    runtimeFor: async () => ({ready: true, image: 'wrong', pod_uid: 'pod'})}), /file_runtime_image_unbound/);
  assert.equal(requests, 0);
});

test('unknown directory creation keeps exact cleanup custody and does not replay create', async () => {
  const deferred = [], requests = []; let root;
  const ctx = {defer: (_name, callback) => deferred.push(callback), async request(value) {
    requests.push(value);
    if (value.method === 'POST') {root = value.body.path; throw Error('lost_ack');}
    if (value.method === 'DELETE') return {status: 200, body: {path: root, deleted: true}};
    return {status: 200, body: {entries: []}};
  }};
  await assert.rejects(fileSetup(ctx, {fileAgent: {ownedByRun: true, agentId: 'a', actor: 'user', runtimeImage: 'exact'},
    runtimeFor: async () => ({ready: true, image: 'exact', pod_uid: 'pod'})}), /lost_ack/);
  assert.equal(deferred.length, 1); await deferred[0]();
  assert.equal(requests.filter(value => value.method === 'POST').length, 1);
  const deleted = requests.find(value => value.method === 'DELETE');
  assert.equal(new URL(deleted.path, 'http://local').searchParams.get('path'), root);
  assert.equal(new URL(deleted.path, 'http://local').searchParams.get('recursive'), 'true');
});

test('peer read selects opposite of actual browser receipt rather than requested case AM', async () => {
  const ctx = {am: 'a', fixture: () => ({evidence: async () => ({requests: [
    {method: 'POST', am: 'b', agent_id_sha256: hash('agent')},
  ]})})};
  assert.equal(await observedPeer(ctx, {agentId: 'agent', page: {}}), 'a');
});

test('final browser evidence observes failures discovered while closing the session', async () => {
  const cleanups = [], calls = []; let closed = false;
  const browser = {open: async () => ({page: {}, close: async () => {closed = true; calls.push('close');}}),
    evidence: async () => {calls.push('evidence'); if (closed) throw Error('late_unknown_mutation'); return {};}};
  const fixture = {actor: 'user', agentId: 'agent', cleanup: async () => calls.push('cleanup')};
  const ctx = {am: 'a', fixture: name => name === 'browser' ? browser : {take: async () => fixture},
    defer: (_label, work) => cleanups.push(work), observe: (_label, work) => work()};
  await setup(ctx);
  await assert.rejects(cleanups.pop()(), /late_unknown_mutation/);
  await cleanups.pop()();
  assert.deepEqual(calls, ['close', 'evidence', 'cleanup']);
});
