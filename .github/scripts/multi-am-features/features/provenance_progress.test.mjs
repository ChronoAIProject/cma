import assert from 'node:assert/strict';
import test from 'node:test';
import {NonOwnerRouter} from '../routing.mjs';
import {provenanceKnownAbort, provenanceUnknownCommit} from './provenance_progress.mjs';

function fixture({changedTransaction = false, aborted = true, unboundSample} = {}) {
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
        return {armed: true, configured: true, agent_revision: revision};
      }
      if (action === 'sample') {
        if (unboundSample) return unboundSample;
        const replaced = ++calls === 2 && changedTransaction;
        return {transaction_sha256: (replaced ? 'b' : 'a').repeat(64), commit_transaction_sha256: 'd'.repeat(64)};
      }
      if (action === 'release') {
        assert.equal(transaction, 'd'.repeat(64));
        return {released: true};
      }
      if (action === 'observe') {
        assert.ok(active && prompt);
        return {recorded: true, sweep_finished: true, agent_revision: revision};
      }
      if (action === 'verify') {
        assert.equal(transaction, 'd'.repeat(64));
        return {aborted};
      }
      assert.equal(action, 'restore');
      active = false;
      return {restored: true, configured: false};
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
  assert.deepEqual(state.actions, ['arm', 'sample', 'sample', 'release', 'verify', 'observe', 'restore']);
  assert.equal(state.writes.length, 2);
  assert.deepEqual(state.read(), {active: false, prompt: null});
});

test('provenance abort refuses a replacement transaction and restores both values', async () => {
  const state = fixture({changedTransaction: true});
  await assert.rejects(provenanceKnownAbort(state), /provenance_progress_peer_write_outside_transaction/);
  assert.deepEqual(state.actions, ['arm', 'sample', 'sample', 'restore']);
  assert.deepEqual(state.read(), {active: false, prompt: null});
});

test('provenance requires an abort attributed to the held transaction', async t => {
  let now = 0;
  const state = fixture({aborted: false});
  t.mock.method(performance, 'now', () => now += state.actions.includes('verify') ? 21_000 : 1);
  await assert.rejects(provenanceKnownAbort(state), /provenance_progress_abort_transaction_unproven/);
  assert.deepEqual(state.actions, ['arm', 'sample', 'sample', 'release', 'verify', 'restore']);
  assert.deepEqual(state.read(), {active: false, prompt: null});
});

test('provenance requires a held progress claim before submitting the peer write', async t => {
  let now = 0;
  t.mock.method(performance, 'now', () => now += 21_000);
  const state = fixture({unboundSample: {transaction_sha256: null}});
  await assert.rejects(provenanceKnownAbort(state), /provenance_progress_claim_not_held/);
  assert.deepEqual(state.actions, ['arm', 'sample', 'restore']);
  assert.deepEqual(state.writes, []);
  assert.deepEqual(state.read(), {active: false, prompt: null});
});

test('provenance adapter failures identify the control and still restore the original state', async t => {
  const state = fixture();
  const control = state.adapter.provenanceBackfill;
  const cause = new Error('private control failure');
  t.mock.method(state.adapter, 'provenanceBackfill', async request => {
    if (request.action === 'sample') throw cause;
    return control(request);
  });
  await assert.rejects(provenanceKnownAbort(state), {
    message: 'provenance_progress_control_sample_failed', cause,
  });
  assert.deepEqual(state.actions, ['arm', 'restore']);
  assert.deepEqual(state.writes, []);
  assert.deepEqual(state.read(), {active: false, prompt: null});
});

test('provenance peer refusal is not replayed and restores its held transaction', async t => {
  const state = fixture();
  const request = state.api.rawRequest;
  const failures = [];
  const refusal = Object.assign(Error('api_status_503'), {response: {status: 503,
    body: {code: 'agent_owner_unavailable'}, timing: {am: 'b', method: 'PATCH',
      route: '/api/v1/agents/{id}/preferences', request_id: 'fixture-refusal'}}});
  const writes = [];
  t.mock.method(state.api, 'rawRequest', async (am, method, route, options) => {
    if (method !== 'PATCH') return request(am, method, route, options);
    writes.push({am, route, options});
    throw refusal;
  });
  await assert.rejects(provenanceKnownAbort({...state,
    recordFailure: (stage, error) => failures.push({stage, error})}), error => error === refusal);
  assert.equal(writes.length, 1);
  assert.equal(writes[0].am, 'b');
  assert.equal(writes[0].options.body.expected_revision, 11);
  assert.deepEqual(state.actions, ['arm', 'sample', 'restore']);
  assert.deepEqual(state.read(), {active: false, prompt: null});
  assert.deepEqual(state.writes, []);
  assert.deepEqual(failures, [{stage: 'primary', error: refusal}]);
  assert.equal(state.proofs.length, 1);
  assert.equal(state.proofs[0].verdict, 'FAIL');
  assert.equal(state.proofs[0].owner, 'a');
  assert.equal(state.proofs[0].ingress, 'b');
  assert.equal(state.proofs[0].owner_epoch, 4);
  assert.equal(state.proofs[0].request_id, 'fixture-refusal');
});

test('provenance preserves the peer refusal when disarming its transaction also fails', async t => {
  const state = fixture();
  const request = state.api.rawRequest, control = state.adapter.provenanceBackfill;
  const refusal = Object.assign(Error('api_status_503'), {response: {status: 503}});
  const restoration = Error('fixture_restore_failed');
  const failures = [];
  let writes = 0, restores = 0;
  t.mock.method(state.api, 'rawRequest', async (am, method, route, options) => {
    if (method !== 'PATCH') return request(am, method, route, options);
    writes += 1;
    throw refusal;
  });
  t.mock.method(state.adapter, 'provenanceBackfill', async options => {
    if (options.action !== 'restore') return control(options);
    restores += 1;
    throw restoration;
  });
  await assert.rejects(provenanceKnownAbort({...state,
    recordFailure: (stage, error) => failures.push({stage, error})}), error => error === refusal);
  assert.equal(writes, 1);
  assert.equal(restores, 1);
  assert.equal(failures[0].stage, 'primary');
  assert.equal(failures[0].error, refusal);
  const retained = failures.find(row => row.stage === 'restoration');
  assert.equal(retained.error.message, 'provenance_progress_control_restore_failed');
  assert.equal(retained.error.cause, restoration);
  assert.deepEqual(state.writes, []);
  assert.deepEqual(state.read(), {active: true, prompt: null});
});

function uncertainFixture({reusedSweep = false, lost = 5, confirmed = false} = {}) {
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
          return {commit_armed: true};
        case 'settle':
          return {worker_unknown: true, commit_acknowledgements_lost: lost};
        case 'observe':
          return {recorded: true, sweep_finished: true, new_sweep_receipts: 1,
            sweep_sha256: reusedSweep ? previousSweep : 'c'.repeat(64)};
        case 'observe_commit':
          if (confirmed) throw Error('provenance_progress_commit_attribution_failed');
          return {worker_unknown: true, commit_acknowledgements_lost: lost};
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
  assert.equal(result.worker_commit_outcome, 'unknown');
  assert.equal(result.new_sweep_receipts, 1);
  assert.equal(result.original_creation_restored, true);
  assert.deepEqual(fixture.actions, ['arm', 'sample', 'commit', 'settle', 'observe', 'observe_commit', 'restore']);
  assert.deepEqual(fixture.state(), {active: false, originalReadbacks: 4});
});

test('provenance commit acknowledgement loss refuses reused sweeps, extra fault hits and a confirmed send', async () => {
  for (const [options, error] of [[{reusedSweep: true}, /provenance_progress_commit_sweep_reused/],
    [{lost: 10}, /provenance_progress_commit_receipt_unbound/],
    [{confirmed: true}, /provenance_progress_control_observe_commit_failed/]]) {
    const fixture = uncertainFixture(options);
    await assert.rejects(provenanceUnknownCommit(fixture), error);
    assert.equal(fixture.actions.at(-1), 'restore');
    assert.equal(fixture.state().active, false);
  }
});
