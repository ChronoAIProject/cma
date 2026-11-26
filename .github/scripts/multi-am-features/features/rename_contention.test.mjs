import assert from 'node:assert/strict';
import test from 'node:test';
import {competingRenames} from './rename_contention.mjs';

function fixture({first = 'a', publishLoser = false, staleReason = 'stale_revision'} = {}) {
  const original = {title: 'Original', revision: 4};
  let visible = original;
  const writes = new Map(), events = new Map();
  const route = '/api/v1/agents/agent/preferences';
  const api = {router: {dispatch: (_id, run) => run('b')},
    async rawRequest(am, method, path, options) {
      assert.equal(path, route);
      if (method === 'GET') return {body: visible};
      assert.equal(method, 'PATCH');
      assert.equal(options.body.expected_revision, original.revision);
      assert.equal(options.prefer, 'cma-settings-reasons');
      return new Promise(resolve => {
        writes.set(am, {resolve, title: options.body.title});
        if (writes.size === 2) {
          const winner = writes.get(first), loser = writes.get(first === 'a' ? 'b' : 'a');
          visible = {title: winner.title, revision: original.revision + 1};
          events.set(winner.title, [12]);
          if (publishLoser) events.set(loser.title, [13]);
          winner.resolve({status: 200, body: visible});
          loser.resolve({status: 409, body: {settings_update_reason: staleReason}});
        }
      });
    }};
  const adapter = {ownerOf: async () => ({am: 'a', epoch: 2, agent_id: 'agent', pod_uid: 'owner'}),
    titleEffects: async ({title}) => events.get(title) ?? []};
  return {api, adapter, record: {agentId: 'agent'}};
}

test('both direct and peer ingress can win under one exact revision', async () => {
  for (const first of ['a', 'b']) {
    const context = fixture({first});
    let state;
    await competingRenames({...context, recordState: (_, record) => { state = record; }});
    assert.deepEqual(state, {owner: 'a', ingress: 'b', winning_ingress: first,
      expected_revision: 4, final_revision: 5, publications: 1});
  }
});

test('an unchanged revision response or stray losing publication fails', async () => {
  await assert.rejects(competingRenames(fixture({staleReason: 'conflict'})), /rename_race_receipt_or_stale_reason_wrong/);
  await assert.rejects(competingRenames(fixture({publishLoser: true})), /rename_race_durable_effect_count_wrong/);
});
