import assert from 'node:assert/strict';
import test from 'node:test';
import {retirementRetention} from './retirement_retention.mjs';

function fixture({foreignDeleteWrites = false, deadlineFuture = true, duplicateRetirement = false} = {}) {
  const record = {agentId: 'fixture-agent', sandboxId: 'fixture-sandbox'};
  const sandboxPath = `/api/v1/sandboxes/${record.sandboxId}`;
  const agentPath = `/api/v1/agents/${record.agentId}`;
  const attempts = [];
  let retired = false, deleteEffect = false, ownerCleanupCount = 0;
  const outsider = {request: async (am, method, path, options) => {
    assert.equal(method, 'DELETE');
    assert.deepEqual(options.allowed, [404]);
    attempts.push([am, path]);
    if (foreignDeleteWrites) deleteEffect = true;
    return {status: 404};
  }};
  const adapter = {lifecycleSnapshot: async input => {
    assert.deepEqual(input, {sandboxId: record.sandboxId, agentIds: [record.agentId]});
    return retired ? {status: 'retired', runtime_detached: true, purge_deadline_future: deadlineFuture,
      children: [{status: 'deleted', last_seq: 7}]}
      : {status: deleteEffect ? 'retiring' : 'active', children: [{status: 'ready', last_seq: 4}]};
  }};
  const user = {request: async (_am, method, path) => {
    assert.equal(method, 'GET');
    assert(retired);
    if (path === agentPath) return {body: {id: record.agentId, status: 'deleted'}};
    if (path === sandboxPath) return {body: {id: record.sandboxId, status: 'retired'}};
    assert.equal(path, `${sandboxPath}/events?limit=200`);
    const event = {sandbox_id: record.sandboxId, seq: 16, kind: {type: 'status_changed', to: 'retired'}};
    return {body: duplicateRetirement ? [event, {...event, seq: 17}] : [event]};
  }};
  return {attempts, count: () => ownerCleanupCount, context: {adapter, user, outsider, record, cleanup: async value => {
    assert.equal(value, record);
    ownerCleanupCount += 1;
    retired = true;
  }}};
}

test('both Managers refuse foreign cleanup and retain one exact terminal publication', async () => {
  const {context, attempts, count} = fixture();
  const proof = await retirementRetention(context);
  assert.equal(proof.terminal_event_count, 1);
  assert.equal(proof.observed_future_purge_deadline, true);
  assert.equal(attempts.length, 4);
  assert.equal(count(), 1);
});

test('a foreign side effect, early purge deadline or duplicate retirement fails', async () => {
  for (const [input, expected] of [
    [{foreignDeleteWrites: true}, /retirement_foreign_delete_changed_state/],
    [{deadlineFuture: false}, /retirement_retained_state_or_deadline_missing/],
    [{duplicateRetirement: true}, /retirement_history_missing_or_duplicated/],
  ]) {
    const {context} = fixture(input);
    await assert.rejects(retirementRetention(context), expected);
  }
});
