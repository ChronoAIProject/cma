import assert from 'node:assert/strict';
import test from 'node:test';
import {preparedJourney} from '../prepared_suite.mjs';

function fixture({consumed = true, readyIds = ['candidate-a', 'candidate-b'], repeatReservation = false,
  schemas = [4, 3, 1], readyFences = [1, 1, 1, 1], starting = true, ready = true, effects = 1} = {}) {
  const actions = [];
  const records = [];
  const create = async (am, desktop, overrides, onAgentId) => {
    assert.equal(desktop, false);
    const sandboxId = overrides?.profile ? 'cold-' + am : 'candidate-' + (repeatReservation ? 'a' : am);
    const record = {agentId: 'fixture-agent', sandboxId, operationId: 'fixture-operation'};
    records.push(record);
    actions.push('create-' + am + '-' + sandboxId);
    onAgentId?.(record);
    return record;
  };
  const adapter = {
    preparedReady: async () => readyIds,
    preparedEvidence: async ({agentId, sandboxId, operationId}) => {
      assert.equal(agentId, 'fixture-agent');
      assert.equal(operationId, 'fixture-operation');
      return {agent_ready: true, consumed: !sandboxId.startsWith('cold-') && consumed,
        installation_bound: true, schemas, ready_fences: readyFences};
    },
    ownerOf: async () => ({agent_id: 'fixture-agent', am: 'b', epoch: 1,
      instance_id: 'fixture-manager', pod_uid: 'fixture-pod'}),
  };
  const results = [];
  const step = async (name, operation) => { results.push({name, ...await operation()}); };
  const browser = {forIngress: async (am, run) => run({
    async openStarting(agentId) {
      assert.equal(agentId, 'fixture-agent');
      actions.push('starting-' + am);
      return starting;
    },
    async sendAndRead(agentId) {
      assert.equal(agentId, 'fixture-agent');
      actions.push('reply-' + am);
      return {effects, ready_observed: ready, answer_sha256: 'a'.repeat(64)};
    },
  })};
  return {adapter, create, step, results, actions, records, browser,
    cleanup: async record => actions.push('cleanup-' + record.sandboxId),
    openBoth: async () => actions.push('open-both')};
}

test('cold comparison and two ingress checks join the exact prepared reservation to Ready and Send', async () => {
  const context = fixture();
  await preparedJourney(context);
  assert.deepEqual(context.results.map(row => row.name),
    ['cold_comparison_on_a', 'prepared_create_on_a', 'cold_comparison_on_b', 'prepared_create_on_b']);
  assert.deepEqual(context.actions, [
    'create-a-cold-a', 'cleanup-cold-a',
    'create-a-candidate-a', 'starting-a', 'open-both', 'reply-a', 'cleanup-candidate-a',
    'create-b-cold-b', 'cleanup-cold-b',
    'create-b-candidate-b', 'starting-b', 'open-both', 'reply-b', 'cleanup-candidate-b',
  ]);
  assert.equal(context.results[1].atomic_publication_fence_count, 4);
  assert.equal(context.results[1].browser_starting_read, true);
  assert.equal(context.results[1].browser_ready_read, true);
  assert.deepEqual(context.results[1].parent_schema_versions, [4, 3, 1]);
});

test('a real prepared hit still needs the Starting browser read and a single reply', async () => {
  await assert.rejects(preparedJourney(fixture({starting: false})), /prepared_browser_starting_read_unproven/);
  await assert.rejects(preparedJourney(fixture({ready: false})), /prepared_browser_reply_unproven/);
  await assert.rejects(preparedJourney(fixture({effects: 2})), /prepared_browser_reply_unproven/);
});

test('a Ready Agent cannot stand in for four committed publication fences and supported parents', async () => {
  for (const readyFences of [[1, 1, 0, 1], [1, 1, 1, 2], [1, 1, 1]]) {
    const context = fixture({readyFences});
    await assert.rejects(preparedJourney(context), /prepared_open_ready_atomic_publication_unproven/);
    assert(!context.actions.includes('open-both'));
  }
  for (const schemas of [[4, 3, 3], [4, 8, 1], [4, '3', 1]]) {
    const context = fixture({schemas});
    await assert.rejects(preparedJourney(context), /prepared_parent_schemas_unbound/);
    assert(!context.actions.includes('open-both'));
  }
});

test('the two ingresses must consume distinct reservations', async () => {
  const context = fixture({repeatReservation: true});
  await assert.rejects(preparedJourney(context), /prepared_admission_consumption_not_observed/);
  assert.equal(context.actions.filter(action => action === 'open-both').length, 1);
});

test('a ready reservation or a separately consumed reservation cannot count as a hit', async () => {
  for (const context of [fixture({consumed: false}), fixture({readyIds: ['different-candidate']})]) {
    await assert.rejects(preparedJourney(context), /prepared_admission_consumption_not_observed/);
    assert(!context.actions.includes('open-both'));
  }
});
