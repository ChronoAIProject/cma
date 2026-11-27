import assert from 'node:assert/strict';
import test from 'node:test';
import {NonOwnerRouter} from '../routing.mjs';
import {provenanceKnownAbort} from './provenance_progress.mjs';

function fixture({changedTransaction = false, abortCount = 1} = {}) {
  const record = {agentId: 'agent', sandboxId: 'sandbox', operationId: 'operation'};
  const owner = {am: 'a', agent_id: record.agentId, epoch: 4, instance_id: 'owner', pod_uid: 'owner-pod'};
  const original = {agent_id: record.agentId, creation_origin: 'manual', created_at: '2026-01-01T00:00:00Z'};
  const writes = [], actions = [], proofs = [];
  let active = false, revision = 11, prompt = null, calls = 0;
  const adapter = {
    ownerOf: async () => owner,
    topology: async () => ({shared_database: true, coordination_enabled: true, ams: {
      a: {...owner, ready: true}, b: {instance_id: 'peer', pod_uid: 'peer-pod', ready: true},
    }}),
    peerWriteEvidence: async args => {
      assert.equal(args.am, owner.am);
      assert.equal(args.epoch, owner.epoch);
      assert.equal(args.operation, 'preferences');
      return {matches: 1};
    },
    async provenanceBackfill({action, ...scope}) {
      assert.deepEqual(scope, record);
      actions.push(action);
      if (action === 'arm') {
        active = true;
        return {armed: true, count: 20, agent_revision: revision};
      }
      if (action === 'sample') {
        const replaced = ++calls === 2 && changedTransaction;
        return {transaction_sha256: (replaced ? 'b' : 'a').repeat(64)};
      }
      if (action === 'observe') {
        assert.ok(active && prompt);
        return {recorded: true, sweep_finished: true, agent_revision: revision};
      }
      assert.equal(action, 'restore');
      active = false;
      return {restored: true, count: 20 + abortCount};
    },
  };
  const api = {
    router: new NonOwnerRouter({adapter, record: proof => proofs.push(proof)}),
    async rawRequest(am, method, route, options) {
      if (method === 'PATCH') {
        assert.equal(am, 'b');
        assert.equal(options.body.expected_revision, revision);
        prompt = options.body.system_prompt;
        writes.push(prompt);
        revision += 1;
      }
      const body = route.endsWith('/provenance')
        ? {...original, creation_origin: active ? 'legacy_unknown' : 'manual'}
        : {system_prompt: prompt, prompt_pending: false, prompt_supported: true, revision};
      return {status: 200, body, timing: {method, route: '/api/v1/agents/{id}/preferences', request_id: 'receipt'}};
    },
  };
  return {api, adapter, record, actions, writes, proofs, read: () => ({active, prompt})};
}

test('provenance abort requires one durable observation and restores the original prompt and creation', async () => {
  const state = fixture();
  const outcome = await provenanceKnownAbort(state);
  assert.equal(outcome.known_aborts, 1);
  assert.equal(outcome.overlapping_transaction_sha256, 'a'.repeat(64));
  assert.equal(state.proofs[0].verified_owner_write, true);
  assert.deepEqual(state.actions, ['arm', 'sample', 'sample', 'observe', 'restore']);
  assert.equal(state.writes.length, 2);
  assert.deepEqual(state.read(), {active: false, prompt: null});
});

test('provenance abort refuses a replacement transaction and restores both values', async () => {
  const state = fixture({changedTransaction: true});
  await assert.rejects(provenanceKnownAbort(state), /provenance_progress_peer_write_outside_transaction/);
  assert.deepEqual(state.actions, ['arm', 'sample', 'sample', 'restore']);
  assert.deepEqual(state.read(), {active: false, prompt: null});
});

test('provenance abort refuses ambiguous failpoint accounting after restoration', async () => {
  const state = fixture({abortCount: 2});
  await assert.rejects(provenanceKnownAbort(state), /provenance_progress_abort_not_consumed_once/);
  assert.deepEqual(state.read(), {active: false, prompt: null});
});
