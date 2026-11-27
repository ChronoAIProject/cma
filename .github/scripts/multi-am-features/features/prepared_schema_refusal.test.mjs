import assert from 'node:assert/strict';
import test from 'node:test';
import {preparedSchemaRefusal} from './prepared_schema_refusal.mjs';

function fixture({aborts = 1, refusals = {a: 1, b: 0}, partial = false, restoreFails = false} = {}) {
  const calls = [];
  const kept = [];
  const record = {operationId: 'op-1'};
  const owner = {agent_id: 'agent-1', am: 'a', pod_uid: 'pod-1', epoch: 12};
  const before = {agentId: 'agent-1', sandboxId: 'sandbox-1', schema: 1, receipt_count: 2};
  return {calls, kept, adapter: {
    preparedReady: async () => [before.sandboxId],
    preparedSchemaBarrier: async ({arm}) => calls.push(arm ? 'arm' : 'disarm'),
    preparedSchemaRace: async ({action, operationId, reservations}) => {
      assert.deepEqual([operationId, reservations], ['op-1', ['sandbox-1']]);
      calls.push(action);
      if (action === 'inject') return before;
      if (action === 'read') return {...before, schema: 999, conversation_unpublished: true,
        ready_fences_absent: !partial};
      if (restoreFails) throw Error('restore_failure');
      return {restored: 1};
    },
    ownerOf: async () => owner,
    preparedSchemaRefusals: async () => refusals,
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
  assert.equal(result.owner_epoch, 12);
  assert.doesNotMatch(JSON.stringify(result), /agent-1|sandbox-1/);
});

test('the schema case fails on a second abort, an unrelated refusal or a partial publication', async () => {
  for (const [context, error] of [[fixture({aborts: 2}), /abort_not_attributed/],
    [fixture({refusals: {a: 1, b: 1}}), /refusal_wrong_manager/],
    [fixture({partial: true}), /partial_publication/]]) {
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
