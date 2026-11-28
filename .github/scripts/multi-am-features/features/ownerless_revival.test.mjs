import assert from 'node:assert/strict';
import test from 'node:test';
import {ownerlessRevival} from './ownerless_revival.mjs';

function fixture({liveLease = false, retainedOwner = true, replayChanges = false,
  duplicate = false, changesWorktree = false} = {}) {
  const agentId = 'fixture-agent', sandboxId = 'fixture-sandbox';
  const record = {agentId, sandboxId, runtime: {image: 'fixture-image', pod_uid: 'pod-before'}};
  const calls = [];
  const event = (seq, kind) => ({seq, sandbox_id: sandboxId, kind});
  const events = [event(1, {type: 'created'})];
  let status = 'ready', sandboxStatus = 'active', replayKey, restarted = false;
  const adapter = {
    ownerOf: async input => {
      assert.deepEqual(input, {agentId});
      return status === 'paused' && !retainedOwner ? null
        : {am: 'a', agent_id: agentId, epoch: replayKey ? 5 : 4, pod_uid: 'manager-a'};
    },
    liveOwnerLease: async input => {
      assert.deepEqual(input, {agentId});
      calls.push('lease');
      assert.equal(status, 'paused');
      return liveLease;
    },
    runtimeFor: async () => ({agent_id: agentId, sandbox_id: sandboxId, ready: true,
      image: 'fixture-image', pod_uid: replayKey ? 'pod-after' : 'pod-before'}),
    topology: async () => ({shared_database: true, coordination_enabled: true,
      ams: {a: {ready: true, pod_uid: restarted ? 'new-manager-a' : 'manager-a'},
        b: {ready: true, pod_uid: 'manager-b'}}}),
  };
  const api = {request: async (am, method, path, options) => {
    if (path.endsWith('/pause')) {
      assert.equal(method, 'POST');
      calls.push('pause-' + am);
      status = 'paused'; sandboxStatus = 'paused';
      return {status: 202, body: {id: sandboxId, status: 'pausing'}, timing: {am, request_id: 'pause-receipt'}};
    }
    if (path.endsWith('/revive')) {
      assert.equal(method, 'POST');
      const isReplay = replayKey !== undefined;
      calls.push((isReplay ? 'replay-' : 'revive-') + am);
      if (isReplay) assert.equal(options.key, replayKey);
      else {
        replayKey = options.key;
        status = 'ready'; sandboxStatus = 'active';
        events.push(event(2, {type: 'status_changed', from: 'paused', to: 'reviving'}),
          event(3, {type: 'revived'}));
        if (duplicate) events.push(event(4, {type: 'revived'}));
      }
      return {status: 202, body: {id: sandboxId, status: 'reviving', revision: isReplay && replayChanges ? 7 : 6},
        timing: {am, request_id: 'revive-' + am}};
    }
    if (path.includes('/events?')) {
      assert.equal(method, 'GET');
      const after = Number(new URL(path, 'https://example.test').searchParams.get('after'));
      return {body: events.filter(row => row.seq > after)};
    }
    assert.fail(`Unexpected routed request ${method} ${path}`);
  }, rawRequest: async (_am, method, path) => {
    assert.equal(method, 'GET');
    assert.equal(path, `/api/v1/agents/${agentId}`);
    return {body: {id: agentId, status, sandbox: {id: sandboxId, status: sandboxStatus},
      workspace: {id: 'fixture-workspace'},
      worktree: {id: changesWorktree && replayKey ? 'other-worktree' : 'fixture-worktree'}}};
  }};
  return {calls, context: {api, adapter, record,
    restartOwner: async owner => {
      assert.equal(status, 'paused');
      assert.equal(owner.epoch, 4);
      calls.push('restart-' + owner.am);
      restarted = true;
    },
    waitFor: async (read, accept, {code}) => {
      const value = await read();
      assert(accept(value), code);
      return value;
    },
    openBoth: async value => {
      assert.equal(value, record);
      calls.push('open-both');
      return {both_am_details_and_transcripts: true};
    },
    complete: async (value, am) => {
      assert.equal(value, record);
      calls.push('complete-' + am);
    },
  }};
}

test('revival begins without an active lease and preserves one receipt and one durable effect', async () => {
  const {context, calls} = fixture();
  const observed = await ownerlessRevival(context);
  assert.deepEqual(calls, ['pause-b', 'restart-a', 'lease', 'lease', 'revive-b', 'replay-a', 'open-both', 'complete-b']);
  assert.equal(observed.durable_admissions, 1);
  assert.equal(observed.durable_completions, 1);
  assert.equal(observed.owner_epoch, 5);
  assert.equal(observed.retained_owner_restarted, true);
});

test('an Agent that lost its lease during pause does not need another Manager restart', async () => {
  const {context, calls} = fixture({retainedOwner: false});
  const observed = await ownerlessRevival(context);
  assert.equal(observed.retained_owner_restarted, false);
  assert(!calls.includes('restart-a'));
});

test('revival refuses an active lease, changed replay, changed lineage or duplicate completion', async () => {
  for (const [options, expected] of [
    [{liveLease: true}, /revival_ownerless_pause_missing/],
    [{replayChanges: true}, /revival_receipt_changed_on_peer_replay/],
    [{changesWorktree: true}, /revival_hierarchy_changed/],
    [{duplicate: true}, /revival_durable_effect_missing_or_duplicated/],
  ]) {
    const {context} = fixture(options);
    await assert.rejects(ownerlessRevival(context), expected);
  }
});
