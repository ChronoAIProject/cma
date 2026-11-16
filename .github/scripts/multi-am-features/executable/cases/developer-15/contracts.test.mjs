import test from 'node:test';
import assert from 'node:assert/strict';
import {fileSetup, observedPeer, hash} from './helpers.mjs';
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
