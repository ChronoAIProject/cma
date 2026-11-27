import assert from 'node:assert/strict';
import {mkdtemp, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import test from 'node:test';
import {cleanupFixture} from '../cma/cleanup.mjs';
import {observeAtomicPause, retainedLifecycle, verifySiblingRecovery} from './lifecycle_atomicity.mjs';

const ready = {status: 'ready', revision: 5, last_seq: 4};
const paused = {status: 'paused', revision: 6, last_seq: 5};
const snapshot = (status, revision, children) => ({status, revision, children});
const before = snapshot('active', 9, [ready, ready]);

async function sample(states) {
  const sequence = [...states];
  return observeAtomicPause({before, read: async () => sequence.shift(), waitFor: async (read, done, {code}) => {
    while (sequence.length) {
      const observed = await read();
      if (done(observed)) return observed;
    }
    throw new Error(code);
  }});
}

test('both children publish their pause status and log progression before the parent settles', async () => {
  assert.deepEqual(await sample([snapshot('pausing', 10, [ready, ready]),
    snapshot('pausing', 11, [paused, paused]), snapshot('paused', 12, [paused, paused])]),
  {snapshot_samples: 3, paused_agents: 2});
});

test('a mixed snapshot, parent-first publication, missing lineage and stale child logs all fail', async () => {
  for (const [state, expected] of [
    [snapshot('pausing', 10, [paused, ready]), /pause_published_partial_hierarchy/],
    [snapshot('paused', 11, [ready, ready]), /pause_published_parent_before_children/],
    [snapshot('pausing', 10, [ready, null]), /pause_snapshot_lineage_changed/],
    [snapshot('paused', 11, [paused, {...paused, last_seq: ready.last_seq}]), /pause_atomic_publication_missing/],
  ]) await assert.rejects(sample([state]), expected);
});

test('missing Workspace identity cannot fall through to new Sandbox admission', async () => {
  const create = () => assert.fail('public create received an unbound Workspace');
  await assert.rejects(retainedLifecycle({record: {sandboxId: 'sandbox'}, create}), /pause_parent_lineage_required/);
});

test('sibling Open and both public operation receipts must point to the same Agent', async () => {
  const requests = [];
  const child = {operationId: 'operation', agentId: 'second'};
  const api = {rawRequest: async (am, method, path) => {
    requests.push(am);
    assert.equal(method, 'GET');
    assert.equal(path, '/api/v1/agent-lifecycle/operation');
    return {body: {id: 'operation', agent_id: 'second', status: 'succeeded'}};
  }};
  const adapter = {lifecycleSnapshot: async value => {
    assert.deepEqual(value, {sandboxId: 'sandbox', agentIds: ['first', 'second']});
    return before;
  }};
  const result = await verifySiblingRecovery({api, adapter, record: {sandboxId: 'sandbox', agentId: 'first'},
    sibling: child, openBoth: async value => {
      assert.equal(value, child);
      return {both_am_details_and_transcripts: true};
    }});
  assert.deepEqual(requests, ['a', 'b']);
  assert.equal(result.restored_agent_count, 2);
});

test('shared Sandbox cleanup deletes only the sibling and requires its exact Deleted readback', async () => {
  const output_dir = await mkdtemp(join(tmpdir(), 'shared-agent-cleanup-'));
  const record = {agentId: 'second', name: 'second', sandboxId: 'sandbox', operationId: 'operation',
    am: 'a', sharedSandbox: true, ready: true, key: 'cleanup-key'};
  const calls = [];
  let deleted = false;
  const api = {rawRequest: async (am, method, path) => {
    assert.equal(am, 'b');
    calls.push([method, path]);
    if (path.endsWith('/agent-lifecycle/operation')) return {status: 200,
      body: {id: 'operation', agent_id: 'second'}};
    assert.equal(path, '/api/v1/agents/second');
    if (method === 'DELETE') deleted = true;
    return {status: 200, body: {id: 'second', title: 'second', sandbox: {id: 'sandbox'},
      status: deleted ? 'deleted' : 'stopped'}};
  }};
  try {
    await cleanupFixture({record, api, config: {output_dir}, result: {cleanup: []}});
    assert.equal(record.deleted, true);
    assert.deepEqual(calls, [['GET', '/api/v1/agents/second'], ['GET', '/api/v1/agent-lifecycle/operation'],
      ['DELETE', '/api/v1/agents/second'], ['GET', '/api/v1/agents/second']]);
  } finally {
    await rm(output_dir, {recursive: true, force: true});
  }
});

test('a still-Starting Agent can be cleaned up after independent lifecycle readback', async () => {
  const output_dir = await mkdtemp(join(tmpdir(), 'starting-agent-cleanup-'));
  const record = {agentId: 'agent', name: 'probe', sandboxId: 'sandbox', operationId: 'operation',
    am: 'a', sharedSandbox: true, key: 'cleanup-key'};
  let status = 'starting';
  const mutations = [];
  const api = {rawRequest: async (_, method, path) => {
    if (path === '/api/v1/agent-lifecycle/operation') {
      return {status: 200, body: {id: 'operation', agent_id: 'agent'}};
    }
    if (method === 'POST') {
      assert.equal(path, '/api/v1/agents/agent/stop');
      status = 'stopped';
    }
    if (method === 'DELETE') status = 'deleted';
    if (method !== 'GET') mutations.push(method);
    return {status: 200, body: {id: 'agent', title: 'probe', sandbox: {id: 'sandbox'}, status}};
  }};
  try {
    await cleanupFixture({record, api, config: {output_dir}, result: {cleanup: []}});
    assert.deepEqual(mutations, ['POST', 'DELETE']);
    assert.equal(record.deleted, true);
  } finally {
    await rm(output_dir, {recursive: true, force: true});
  }
});
