import assert from 'node:assert/strict';
import test from 'node:test';
import {preparedParentPauseRefusal, preparedSchemaRefusal} from './prepared_schema_refusal.mjs';

function fixture({aborts = 1, refusals = {a: 1, b: 0}, partial = false, restoreFails = false, changedNativeEffect = false,
  barrier = 'worktree_schema'} = {}) {
  const calls = [];
  const kept = [];
  const record = {operationId: 'op-1'};
  const owner = {agent_id: 'agent-1', am: 'a', pod_uid: 'pod-1', epoch: 12};
  const before = {agentId: 'agent-1', sandboxId: 'sandbox-1', schema: 1, receipt_count: 2};
  return {calls, kept, barrier, adapter: {
    preparedReady: async () => [before.sandboxId],
    preparedRuntimeEvidence: async ({sandboxId, operationId}) => {
      assert.equal(sandboxId, before.sandboxId);
      assert.ok(operationId === undefined || operationId === record.operationId);
      return {format: 'indexed', pod_sha256: 'a'.repeat(64),
        receipts: ['b', 'c', operationId && changedNativeEffect ? 'e' : 'd'].map(value => value.repeat(64))};
    },
    preparedSchemaBarrier: async ({arm}) => calls.push(arm ? 'arm' : 'disarm'),
    preparedSchemaRace: async scope => {
      const {action, ...lineage} = scope;
      assert.deepEqual(lineage, {operationId: 'op-1', reservations: ['sandbox-1'], barrier});
      calls.push(action);
      if (action === 'inject') return before;
      if (action === 'read') return {...before, schema: barrier === 'worktree_schema' ? 999 : before.schema,
        cleanup_marked: barrier === 'worktree_cleanup', work_lease_changed: barrier === 'work_lease',
        conversation_unpublished: true, ready_fences_absent: !partial};
      if (restoreFails) throw Error('restore_failure');
      return {restored: 1};
    },
    ownerOf: async () => owner,
    preparedSchemaRefusals: async scope => {
      assert.deepEqual(scope, {agentId: before.agentId, barrier});
      return refusals;
    },
    publicationEvidence: async () => ({a: {known_aborted_cas: {publish_connection: aborts}}, b: {}}),
  }, admit: async () => {calls.push('admit'); return record;},
  cleanup: async value => {assert.equal(value, record); calls.push('cleanup');},
  recordFailure: async (stage, error) => kept.push([stage, error.message])};
}

test('a live prepared Worktree refusal proves the old owner and cleans up the fixture', async () => {
  const context = fixture();
  const result = await preparedSchemaRefusal(context);
  assert.deepEqual(context.calls, ['arm', 'admit', 'inject', 'disarm', 'read', 'restore', 'disarm', 'cleanup']);
  assert.equal(result.prepared_schema_refused, true);
  assert.equal(result.startup_receipt_count, 2);
  assert.equal(result.runtime_receipt_count, 3);
  assert.equal(result.owner_epoch, 12);
  assert.doesNotMatch(JSON.stringify(result), /agent-1|sandbox-1/);
});

test('a live prepared cleanup marker refuses publication without changing the Worktree schema', async () => {
  const context = fixture({barrier: 'worktree_cleanup'});
  const result = await preparedSchemaRefusal(context);
  assert.equal(result.prepared_cleanup_refused, true);
  assert.equal(result.prepared_schema_refused, false);
  assert.equal(result.startup_receipt_count, 2);
  assert.deepEqual(context.calls, ['arm', 'admit', 'inject', 'disarm', 'read', 'restore', 'disarm', 'cleanup']);
});

test('the exact Agent work lease can change without committing Ready or its open receipt', async () => {
  const context = fixture({barrier: 'work_lease'});
  const result = await preparedSchemaRefusal(context);
  assert.equal(result.prepared_work_lease_refused, true);
  assert.equal(result.prepared_schema_refused, false);
  assert.equal(result.prepared_cleanup_refused, false);
  assert.equal(result.startup_receipt_count, 2);
  assert.deepEqual(context.calls, ['arm', 'admit', 'inject', 'disarm', 'read', 'restore', 'disarm', 'cleanup']);
});

test('the schema case fails on a second abort, an unrelated refusal or a partial publication', async () => {
  for (const [context, error] of [[fixture({aborts: 2}), /abort_not_attributed/],
    [fixture({refusals: {a: 1, b: 1}}), /refusal_wrong_manager/],
    [fixture({partial: true}), /partial_publication/],
    [fixture({changedNativeEffect: true}), /runtime_mutation_receipts_changed/]]) {
    await assert.rejects(preparedSchemaRefusal(context), error);
    assert.deepEqual(context.calls.slice(-2), ['restore', 'disarm']);
  }
});

test('cleanup retains a schema restoration failure alongside the assertion failure', async () => {
  const context = fixture({partial: true, restoreFails: true});
  await assert.rejects(preparedSchemaRefusal(context), /partial_publication/);
  assert.ok(context.kept.some(row => row[0] === 'restoration' && row[1] === 'restore_failure'));
  assert.equal(context.calls.at(-1), 'disarm');
});


test('the public pause must commit during the exact prepared Ready transaction', async () => {
  const before = {agentId: 'agent-1', sandboxId: 'sandbox-1', state: 'starting', step: 'open_conversation',
    sandboxStatus: 'active', receipt_count: 2, conversation_unpublished: true, ready_fences_absent: true,
    transactionSha256: 'a'.repeat(64)};
  const record = {operationId: 'op-1'};
  const calls = [];
  let paused = false, wrongTransaction = false, committed = false;
  const context = {api: {
    request: async (ingress, method, path, options) => {
      assert.deepEqual([ingress, method, path], ['b', 'POST', '/api/v1/sandboxes/sandbox-1/pause']);
      assert.equal(options.direct, true);
      assert.deepEqual(options.allowed, [202]);
      paused = true;
      calls.push('pause');
      return {status: 202, body: {id: 'sandbox-1', status: 'pausing'},
        timing: {am: 'b', request_id: 'pause-request'}};
    },
  }, adapter: {
    preparedReady: async () => ['sandbox-1'],
    preparedSchemaBarrier: async value => {
      calls.push(value.arm ? 'arm' : 'disarm');
      assert.deepEqual(value, value.arm ? {arm: true, parentPause: true} : {arm: false});
    },
    preparedSchemaRace: async ({action, ...scope}) => {
      assert.deepEqual(scope, {operationId: 'op-1', reservations: ['sandbox-1'], barrier: 'parent_pause'});
      calls.push(action);
      if (!paused) return before;
      return {...before, sandboxStatus: 'pausing', transactionSha256: wrongTransaction ? 'b'.repeat(64) :
        before.transactionSha256};
    },
    ownerOf: async () => ({agent_id: 'agent-1', am: 'a', pod_uid: 'pod-1', epoch: 12}),
    publicationEvidence: async () => ({a: {known_aborted_cas: {publish_connection: 1},
      prepared_commit_work_lease_sha256: committed ? ['c'.repeat(64)] : []}, b: {}}),
  }, admit: async () => {calls.push('admit'); return record;},
  cleanup: async observed => {assert.equal(observed, record); calls.push('cleanup');}};

  const result = await preparedParentPauseRefusal(context);
  assert.deepEqual(calls, ['arm', 'admit', 'sample', 'pause', 'sample', 'disarm', 'read', 'cleanup']);
  assert.equal(result.prepared_parent_pause_refused, true);
  assert.equal(result.owner_epoch, 12);
  assert.doesNotMatch(JSON.stringify(result), /agent-1|sandbox-1|pause-request/);

  paused = false;
  wrongTransaction = true;
  await assert.rejects(preparedParentPauseRefusal(context), /prepared_parent_pause_order_unproven/);
  assert.equal(calls.at(-1), 'disarm');

  paused = false;
  wrongTransaction = false;
  committed = true;
  await assert.rejects(preparedParentPauseRefusal(context), /prepared_parent_pause_publication_unproven/);
  assert.equal(calls.at(-1), 'disarm');
});
