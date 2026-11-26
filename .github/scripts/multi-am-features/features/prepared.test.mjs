import assert from 'node:assert/strict';
import test from 'node:test';
import {preparedJourney} from '../prepared_suite.mjs';

function fixture({consumed = true, readyIds = ['candidate']} = {}) {
  const actions = [];
  const records = [];
  const create = async (am, desktop, overrides) => {
    assert.equal(desktop, false);
    const sandboxId = overrides ? 'cold' : 'candidate';
    const record = {agentId: 'fixture-agent', sandboxId, operationId: 'fixture-operation'};
    records.push(record);
    actions.push('create-' + am + '-' + sandboxId);
    return record;
  };
  const adapter = {
    preparedReady: async () => readyIds,
    preparedEvidence: async ({agentId, sandboxId, operationId}) => {
      assert.equal(agentId, 'fixture-agent');
      assert.equal(operationId, 'fixture-operation');
      return {agent_ready: true, consumed: sandboxId !== 'cold' && consumed,
        installation_bound: true, schemas: [3, 4, 2]};
    },
    ownerOf: async () => ({agent_id: 'fixture-agent', am: 'b', epoch: 1,
      instance_id: 'fixture-manager', pod_uid: 'fixture-pod'}),
  };
  const results = [];
  const step = async (name, operation) => { results.push({name, ...await operation()}); };
  return {adapter, create, step, results, actions, records,
    cleanup: async record => actions.push('cleanup-' + record.sandboxId),
    openBoth: async () => actions.push('open-both'),
    complete: async (_, am) => actions.push('reply-' + am)};
}

test('cold comparison and two ingress checks join the exact prepared reservation to Ready and Send', async () => {
  const context = fixture();
  await preparedJourney(context);
  assert.deepEqual(context.results.map(row => row.name),
    ['cold_comparison', 'prepared_create_on_a', 'prepared_create_on_b']);
  assert.deepEqual(context.actions, [
    'create-a-cold', 'cleanup-cold',
    'create-a-candidate', 'open-both', 'reply-a', 'cleanup-candidate',
    'create-b-candidate', 'open-both', 'reply-a', 'cleanup-candidate',
  ]);
});

test('a ready reservation or a separately consumed reservation cannot count as a hit', async () => {
  for (const context of [fixture({consumed: false}), fixture({readyIds: ['different-candidate']})]) {
    await assert.rejects(preparedJourney(context), /prepared_admission_consumption_not_observed/);
    assert(!context.actions.includes('open-both'));
  }
});
