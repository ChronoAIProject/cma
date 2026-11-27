import assert from 'node:assert/strict';
import test from 'node:test';
import {NonOwnerRouter} from '../routing.mjs';
import {provenanceKnownAbort, provenanceUnknownCommit} from './provenance_progress.mjs';

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
    async provenanceBackfill({action, transaction, ...scope}) {
      assert.deepEqual(scope, record);
      actions.push(action);
      if (action === 'arm') {
        active = true;
        return {armed: true, count: 20, agent_revision: revision};
      }
      if (action === 'sample') {
        const replaced = ++calls === 2 && changedTransaction;
        return {transaction_sha256: (replaced ? 'b' : 'a').repeat(64), commit_transaction_sha256: 'd'.repeat(64)};
      }
      if (action === 'observe') {
        assert.ok(active && prompt);
        return {recorded: true, sweep_finished: true, agent_revision: revision};
      }
      if (action === 'verify') {
        assert.equal(transaction, 'd'.repeat(64));
        return {aborted: true};
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
  assert.deepEqual(state.actions, ['arm', 'sample', 'sample', 'observe', 'verify', 'restore']);
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

function uncertainFixture({reusedSweep = false, lost = 5} = {}) {
  const record = {agentId: 'agent', sandboxId: 'sandbox', operationId: 'operation'};
  const transaction = 'a'.repeat(64), previousSweep = 'b'.repeat(64);
  let active = false, originalReadbacks = 0;
  const actions = [];
  const adapter = {
    async provenanceBackfill({action, unknown, transaction: target, ...scope}) {
      assert.equal(unknown, true);
      assert.deepEqual(scope, record);
      actions.push(action);
      switch (action) {
        case 'arm':
          active = true;
          return {armed: true, configured: true, previous_sweep_sha256: previousSweep};
        case 'sample':
          return {commit_transaction_sha256: transaction};
        case 'commit':
          assert.equal(target, transaction);
          return {commit_acknowledgements_lost: lost};
        case 'observe':
          return {recorded: true, sweep_finished: true, sweep_sha256: reusedSweep ? previousSweep : 'c'.repeat(64)};
        case 'restore':
          active = false;
          return {restored: true, configured: false};
        default:
          assert.fail(action);
      }
    },
  };
  const api = {
    async rawRequest(am, method, path) {
      assert.ok(['a', 'b'].includes(am));
      assert.equal(method, 'GET');
      assert.equal(path, '/api/v1/agents/agent/provenance');
      if (!active) originalReadbacks += 1;
      return {body: {agent_id: record.agentId, created_at: '2026-01-01T00:00:00Z',
        creation_origin: active ? 'legacy_unknown' : 'manual'}};
    },
  };
  return {adapter, api, record, actions, state: () => ({active, originalReadbacks})};
}

test('provenance commit acknowledgement loss qualifies one new durable sweep', async () => {
  const fixture = uncertainFixture();
  const result = await provenanceUnknownCommit(fixture);
  assert.equal(result.commit_acknowledgements_lost, 5);
  assert.equal(result.original_creation_restored, true);
  assert.deepEqual(fixture.actions, ['arm', 'sample', 'commit', 'observe', 'restore']);
  assert.deepEqual(fixture.state(), {active: false, originalReadbacks: 4});
});

test('provenance commit acknowledgement loss refuses a reused sweep and an exhausted budget', async () => {
  for (const [options, error] of [[{reusedSweep: true}, /provenance_progress_commit_sweep_reused/],
    [{lost: 10}, /provenance_progress_commit_receipt_unbound/]]) {
    const fixture = uncertainFixture(options);
    await assert.rejects(provenanceUnknownCommit(fixture), error);
    assert.equal(fixture.actions.at(-1), 'restore');
    assert.equal(fixture.state().active, false);
  }
});
