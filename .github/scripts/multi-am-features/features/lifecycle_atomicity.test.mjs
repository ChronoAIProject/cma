import assert from 'node:assert/strict';
import {mkdtemp, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import test from 'node:test';
import {cleanupFixture} from '../cma/cleanup.mjs';
import {assertRetainedHome, observeAtomicPause, retainedLifecycle, verifySiblingRecovery} from './lifecycle_atomicity.mjs';

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

test('a paused Sandbox preserves the runtime home, source generation and both exact conversations', () => {
  const children = ['first', 'second'].map(agent_id => ({agent_id, conversation_id: 'conversation-' + agent_id}));
  const prior = {...before, children, provision_generation: 8, checkpoint_epoch: 0};
  const priorRuntime = {ready: true, home_identity: 'home', pod_uid: 'old-pod', pvc_names: ['claim']};
  const retained = {identity: 'home', source_generation: 8};
  const observed = {status: 'paused', runtime_detached: true, checkpoint_epoch: 0, retained_home: retained,
    children: children.map(child => ({...child, status: 'paused'}))};
  const result = assertRetainedHome(prior, observed, priorRuntime);
  assert.equal(result.source_generation, 8);
  assert.equal(result.retained_conversations, 2);
  assert.match(result.retained_home_sha256, /^[a-f0-9]{64}$/);
  for (const overrides of [{retained_home: {...retained, source_generation: 7}},
    {retained_home: {...retained, identity: 'replacement'}}, {runtime_detached: false}, {checkpoint_epoch: 1}]) {
    assert.throws(() => assertRetainedHome(prior, {...observed, ...overrides}, priorRuntime), /pause_retained_home_unbound/);
  }
  assert.throws(() => assertRetainedHome(prior, {...observed, children: observed.children.toReversed()}, priorRuntime),
    /pause_retained_home_child_lineage_changed/);
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
    assert.deepEqual(calls, [['GET', '/api/v1/agent-lifecycle/operation'], ['GET', '/api/v1/agents/second'],
      ['DELETE', '/api/v1/agents/second'], ['GET', '/api/v1/agents/second']]);
  } finally {
    await rm(output_dir, {recursive: true, force: true});
  }
});

test('cleanup discovers a late assignment and retires the original home with its Agent still Starting', async () => {
  const output_dir = await mkdtemp(join(tmpdir(), 'starting-agent-cleanup-'));
  const record = {name: 'probe', operationId: 'operation', am: 'a', key: 'cleanup-key'};
  let status = 'starting';
  let retired = false;
  const mutations = [];
  const api = {request: async (_, method, path) => {
    if (path === '/api/v1/agent-lifecycle/operation') {
      return {status: 200, body: {id: 'operation', agent_id: 'agent', phase: 'sandbox_ready'}};
    }
    if (method !== 'GET') mutations.push([method, path]);
    if (path === '/api/v1/sandboxes/sandbox') {
      if (method === 'DELETE') retired = true;
      return {status: 200, body: {id: 'sandbox', owner: 'fixture-owner', name: 'probe sandbox operation',
        status: retired ? 'retired' : 'provisioning'}};
    }
    if (path.startsWith('/api/v1/agents?')) {
      return {status: 200, body: {items: [{id: 'agent', sandbox_id: 'sandbox', status}]}};
    }
    if (method === 'POST') {
      assert.equal(path, '/api/v1/agents/agent/stop');
      status = 'stopped';
    }
    if (method === 'DELETE') status = 'deleted';
    return {status: 200, body: {id: 'agent', title: 'probe', sandbox: {id: 'sandbox'}, status}};
  }};
  try {
    await cleanupFixture({record, api, config: {output_dir, owner_id: 'fixture-owner'}, result: {cleanup: []}});
    assert.deepEqual(mutations, [['POST', '/api/v1/agents/agent/stop'], ['DELETE', '/api/v1/agents/agent'],
      ['DELETE', '/api/v1/sandboxes/sandbox']]);
    assert.equal(record.agentId, 'agent');
    assert.equal(record.sandboxId, 'sandbox');
    assert.equal(record.deleted, true);
  } finally {
    await rm(output_dir, {recursive: true, force: true});
  }
});

test('cleanup cancels an acknowledged unassigned creation and checks its durable settlement', async () => {
  const output_dir = await mkdtemp(join(tmpdir(), 'unallocated-creation-cleanup-'));
  const operation = '/api/v1/agent-lifecycle/operation';
  try {
    for (const confirmed of [true, false]) {
      const record = {name: 'probe', operationId: 'operation', am: 'a', key: 'cleanup-key'};
      const calls = [];
      const api = {request: async (am, method, path) => {
        assert.equal(am, 'b');
        calls.push([method, path]);
        if (path === operation + '/cancel') {
          assert.equal(method, 'POST');
          return confirmed ? {status: 200, body: {id: 'operation', status: 'failed'}}
            : {status: 409, body: {code: 'creation_cancellation_unavailable'}};
        }
        assert.equal(path, operation);
        return {status: 200, body: {id: 'operation', status: confirmed && calls.length === 3 ? 'failed' : 'running'}};
      }};
      const cleanup = cleanupFixture({record, api, config: {output_dir}, result: {cleanup: []}});
      if (confirmed) await cleanup;
      else await assert.rejects(cleanup, /cleanup_lifecycle_mismatch/);
      assert.equal(record.deleted, confirmed ? true : undefined);
      assert.deepEqual(calls, [['GET', operation], ['POST', operation + '/cancel'], ['GET', operation]]);
    }
  } finally {
    await rm(output_dir, {recursive: true, force: true});
  }
});
