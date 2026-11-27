import assert from 'node:assert/strict';
import test from 'node:test';
import {retirementRetention} from './retirement_retention.mjs';

function fixture({foreignDeleteWrites = false, deadlineFuture = true, duplicateRetirement = false,
  duplicateStop = false, stopReplayWrites = false, duplicateDelete = false, deleteReplayWrites = false,
  missingTerminalRow = false, staleManager = false, changedProvenance = false, missingTitle = false} = {}) {
  const target = {agentId: 'fixture-agent', sandboxId: 'fixture-sandbox', operationId: 'fixture-create'};
  const record = {...target, generatedTitleProof: {digest: 'd'.repeat(64), turnSequence: 5}};
  const creationDigest = 'c'.repeat(64);
  const sandboxPath = `/api/v1/sandboxes/${record.sandboxId}`;
  const agentPath = `/api/v1/agents/${record.agentId}`;
  const deleteKey = 'fixture-delete-key';
  const attempts = [];
  let retired = false, stopped = false, deleteEffect = false, ownerCleanupCount = 0, stopKey, revision = 1;
  let armed = false, readsAfterArm = 0;
  const outsider = {request: async (am, method, path, options) => {
    assert.equal(method, 'DELETE');
    assert.deepEqual(options.allowed, [404]);
    attempts.push([am, path]);
    if (foreignDeleteWrites) deleteEffect = true;
    return {status: 404};
  }};
  const adapter = {ownerOf: async ({agentId}) => ({agent_id: agentId, am: 'a', epoch: 7, pod_uid: 'owner-pod'}),
    titleIntent: async ({agentId}) => {
      assert.equal(agentId, target.agentId);
      assert(retired && !armed);
      return {accepted: !missingTitle, request_sha256: record.generatedTitleProof.digest, turn_sequence: 5};
    },
    retirementPurge: async ({agentId, sandboxId, operationId, arm}) => {
      assert.deepEqual({agentId, sandboxId, operationId}, target);
      assert(retired);
      if (arm) {
        assert(!armed);
        armed = true;
        return {armed: true, agents: 1, creation_sha256: creationDigest};
      }
      assert(armed);
      const present = ++readsAfterArm === 1;
      return {present, agents: present ? 1 : 0, agent_events: present && !missingTerminalRow,
        sandbox_events: present, workspaces: present, worktrees: present,
        creation_sha256: changedProvenance && !present ? 'e'.repeat(64) : creationDigest};
    },
    lifecycleSnapshot: async input => {
    assert.deepEqual(input, {sandboxId: record.sandboxId, agentIds: [record.agentId]});
    return retired ? {status: 'retired', revision: 4, runtime_detached: true, purge_deadline_future: deadlineFuture,
      children: [{status: 'deleted', last_seq: 7, revision}]}
      : {status: deleteEffect ? 'retiring' : 'active',
        children: [{status: stopped ? 'stopped' : 'ready', last_seq: stopped ? 5 : 4, revision}]};
  }};
  const user = {request: async (am, method, path, options) => {
    if (method === 'DELETE') {
      assert(retired);
      assert.equal(path, agentPath);
      assert.equal(am, 'a');
      assert.equal(options.key, deleteKey);
      assert.deepEqual(options.allowed, [202]);
      if (deleteReplayWrites) revision += 1;
      return {status: 202, body: null};
    }
    if (path === agentPath + '/stop') {
      assert.equal(method, 'POST');
      if (stopKey) assert.equal(options.key, stopKey);
      if (options.body.reason) {
        assert.equal(am, 'b');
        assert.deepEqual(options.allowed, [422]);
        return {status: 422, body: {code: 'idempotency_key_reused'}};
      }
      if (stopKey) {
        assert.equal(am, 'a');
        if (stopReplayWrites) revision += 1;
      } else {
        assert.equal(am, 'b');
        stopKey = options.key;
        stopped = true;
        revision += 1;
      }
      return {status: 202};
    }
    assert.equal(method, 'GET');
    if (path === agentPath + '/events?after=4') {
      const event = {seq: 5, kind: {type: 'stopped', reason: 'owner'}};
      return {body: {items: duplicateStop ? [event, {...event, seq: 6}] : [event]}};
    }
    if (path === agentPath + '/events?after=5') {
      assert(retired);
      const event = {seq: 7, kind: {type: 'deleted'}};
      return {body: {items: duplicateDelete ? [event, {...event, seq: 8}] : [event]}};
    }
    if (!retired && path === agentPath) {
      assert(stopped);
      return {body: {id: record.agentId, sandbox: {id: record.sandboxId}, status: 'stopped', stop_reason: 'owner'}};
    }
    assert(retired);
    if (path === agentPath) return {body: {id: record.agentId, status: 'deleted'}};
    if (path === sandboxPath) return {body: {id: record.sandboxId, status: 'retired'}};
    assert.equal(path, `${sandboxPath}/events?limit=200`);
    const event = {sandbox_id: record.sandboxId, seq: 16, kind: {type: 'status_changed', to: 'retired'}};
    return {body: duplicateRetirement ? [event, {...event, seq: 17}] : [event]};
  }};
  user.rawRequest = async (am, method, path, options) => {
    assert.equal(readsAfterArm, 2);
    assert.equal(method, 'GET');
    assert.ok([sandboxPath, agentPath].includes(path));
    assert.deepEqual(options.allowed, [404]);
    return {status: staleManager && am === 'b' ? 200 : 404};
  };
  return {attempts, count: () => ownerCleanupCount, context: {adapter, user, outsider, record, cleanup: async value => {
    assert.equal(value, record);
    ownerCleanupCount += 1;
    retired = true;
    revision += 1;
    return {deleteKey, deleteReceipt: {status: 202, body: null, timing: {am: 'b'}}};
  }, waitForPurge: async (read, accept) => {
    assert.equal(accept(await read()), false);
    assert.equal(accept(await read()), true);
  }}};
}

test('both Managers refuse foreign cleanup and retain one exact terminal publication', async () => {
  const {context, attempts, count} = fixture();
  const proof = await retirementRetention(context);
  assert.equal(proof.terminal_event_count, 1);
  assert.equal(proof.stop_events, 1);
  assert.equal(proof.delete_events, 1);
  assert.equal(proof.stop_ingress, 'b');
  assert.equal(proof.observed_future_purge_deadline, true);
  assert.equal(proof.retained_title_custody, true);
  assert.equal(proof.retained_creation_provenance, true);
  assert.equal(proof.eligible_purge, true);
  assert.equal(proof.purged_agents, 1);
  assert.equal(proof.purge_snapshot_samples, 2);
  assert.equal(attempts.length, 4);
  assert.equal(count(), 1);
});

test('a foreign side effect, early purge deadline or duplicate retirement fails', async () => {
  for (const [input, expected] of [
    [{foreignDeleteWrites: true}, /retirement_foreign_delete_changed_state/],
    [{duplicateStop: true}, /stop_event_missing_or_duplicated/],
    [{stopReplayWrites: true}, /stop_replay_mutated_terminal_agent/],
    [{duplicateDelete: true}, /delete_event_missing_or_duplicated/],
    [{deleteReplayWrites: true}, /delete_replay_mutated_terminal_lineage/],
    [{deadlineFuture: false}, /retirement_retained_state_or_deadline_missing/],
    [{duplicateRetirement: true}, /retirement_history_missing_or_duplicated/],
    [{missingTerminalRow: true}, /retirement_purge_partially_removed_lineage/],
    [{missingTitle: true}, /retirement_title_custody_changed/],
    [{changedProvenance: true}, /retirement_purge_changed_creation_provenance/],
    [{staleManager: true}, /retirement_purge_public_readback_present/],
  ]) {
    const {context} = fixture(input);
    await assert.rejects(retirementRetention(context), expected);
  }
});
