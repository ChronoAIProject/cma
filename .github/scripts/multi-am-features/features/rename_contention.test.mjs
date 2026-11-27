import assert from 'node:assert/strict';
import test from 'node:test';
import {competingRenames, renameAfterPublicationAbort} from './rename_contention.mjs';

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

function abortFixture({abortOn = 'a', consumed = true, duplicate = false, rejected = false} = {}) {
  let title = 'Original', revision = 4, armed = false;
  const operations = [];
  const api = {router: {dispatch: (_id, run) => run('b')},
    async rawRequest(am, method, path, options) {
      assert.equal(path, '/api/v1/agents/agent/preferences');
      operations.push([am, method]);
      if (method === 'PATCH') {
        assert.equal(am, 'b');
        assert.equal(armed, true);
        assert.equal(options.body.expected_revision, revision);
        if (rejected) throw Error('rename_fixture_request_failed');
        title = options.body.title;
        revision += 1;
      }
      return {status: 200, body: {title, revision}};
    }};
  const adapter = {
    ownerOf: async () => ({am: 'a', epoch: 2, agent_id: 'agent', pod_uid: 'owner'}),
    renamePublicationAbort: async ({arm}) => {
      armed = arm;
      return {count: arm || !consumed ? 7 : 8};
    },
    renamePublicationEvidence: async () => {
      const evidence = {a: 0, b: 0};
      if (revision > 4) evidence[abortOn] += 1;
      return evidence;
    },
    titleEffects: async ({title: expected}) => {
      assert.equal(expected, title);
      return duplicate ? [12, 13] : [12];
    },
  };
  return {api, adapter, record: {agentId: 'agent'}, operations, isArmed: () => armed};
}

test('peer Rename requires one aborted owner transaction and one visible publication', async () => {
  const context = abortFixture();
  await renameAfterPublicationAbort(context);
  assert.deepEqual(context.operations, [['a', 'GET'], ['b', 'PATCH'], ['a', 'GET'], ['b', 'GET']]);
  assert.equal(context.isArmed(), false);
});

test('Rename rejects unrelated aborts and duplicates and disarms after a failed request', async () => {
  await assert.rejects(renameAfterPublicationAbort(abortFixture({abortOn: 'b'})),
    /rename_publication_abort_not_attributed_to_owner/);
  await assert.rejects(renameAfterPublicationAbort(abortFixture({consumed: false})),
    /rename_publication_abort_not_consumed_once/);
  await assert.rejects(renameAfterPublicationAbort(abortFixture({duplicate: true})),
    /rename_abort_retry_publication_not_unique/);
  const context = abortFixture({rejected: true});
  await assert.rejects(renameAfterPublicationAbort(context), /rename_fixture_request_failed/);
  assert.equal(context.isArmed(), false);
});
