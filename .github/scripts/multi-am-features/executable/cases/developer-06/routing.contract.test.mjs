import test from 'node:test';
import assert from 'node:assert/strict';
import { api, hash, switchAfterWrite } from './helpers.mjs';
import { projectReceipt, validateReceipt, validatePeerReads } from './receipts.mjs';

const source = 'a'.repeat(40);
function receipt({ method = 'POST', ingress = 'b', owner = 'a', kind = 'non_owner' } = {}) {
  return {
    schema: 'multi-am-browser-evidence-v1',
    real_ui: true,
    compiled_source: source,
    compiled_manifest: 'sha256:' + 'b'.repeat(64),
    secrets_redacted: true,
    requests: [
      {
        method,
        am: ingress,
        serving_am: ingress,
        status: 200,
        request_id: 'write',
        elapsed_ms: 1,
        agent_id_sha256: hash('agent'),
        owner,
        owner_epoch: 2,
        owner_stable: true,
        owner_proof: {
          agent_sha256: hash('agent'),
          owner,
          ingress,
          kind,
          method,
          owner_epoch: 2,
          request_id: 'write',
          verdict: 'PASS',
        },
      },
    ],
  };
}
test('owner-side GET needs exact peer-read proof; owner-side POST never passes', () => {
  validateReceipt(receipt(), source, 'agent');
  validateReceipt(receipt({ method: 'GET', ingress: 'a', kind: 'peer_read' }), source, 'agent');
  assert.throws(() => validateReceipt(receipt({ ingress: 'a' }), source), /owner_proof/);
  assert.throws(
    () => validateReceipt(receipt({ method: 'POST', ingress: 'a', kind: 'peer_read' }), source),
    /owner_proof/,
  );
  const changed = receipt({ method: 'GET', ingress: 'a', kind: 'peer_read' });
  changed.requests[0].owner_proof.method = 'HEAD';
  assert.throws(() => validateReceipt(changed, source), /owner_proof/);
});
test('projection retains runner correlation while dropping nested sensitive extensions', () => {
  const value = receipt();
  value.requests[0].owner_proof.authorization = 'do-not-retain';
  value.requests[0].headers = { authorization: 'do-not-retain' };
  const projected = projectReceipt(value);
  validateReceipt(projected, source, 'agent');
  assert.equal(projected.schema, 'multi-am-browser-evidence-v1');
  assert.equal(projected.requests[0].owner_proof.request_id, 'write');
  assert.equal(JSON.stringify(projected).includes('do-not-retain'), false);
});
test('projection retains safe serving attribution while dropping adapter extensions', () => {
  const value = receipt();
  const serving = {
    schema: 'c275-serving-v1',
    am: 'b',
    origin: 'http://127.0.0.1:10002',
    pod_uid: 'pod-b',
    instance_id: 'instance-b',
    container_id: 'container-b',
    ready: true,
    observed_at: '2026-11-16T10:00:00.000Z',
    private_forward_token: 'do-not-retain',
  };
  value.requests[0] = {
    ...value.requests[0],
    serving,
    serving_after: { ...serving, observed_at: '2026-11-16T10:00:00.003Z' },
    started_at: '2026-11-16T10:00:00.001Z',
    ended_at: '2026-11-16T10:00:00.002Z',
    attribution: {
      verdict: 'PASS',
      basis: 'pod_forward_before_after',
      error: 'do-not-retain',
    },
    adapter_extension: 'do-not-retain',
  };
  const projected = projectReceipt(value);
  const request = projected.requests[0];
  assert.deepEqual(request.serving, {
    schema: 'c275-serving-v1',
    am: 'b',
    origin: 'http://127.0.0.1:10002',
    pod_uid: 'pod-b',
    instance_id: 'instance-b',
    container_id: 'container-b',
    ready: true,
    observed_at: '2026-11-16T10:00:00.000Z',
  });
  assert.deepEqual(request.serving_after, {
    ...request.serving,
    observed_at: '2026-11-16T10:00:00.003Z',
  });
  assert.deepEqual(request.attribution, { verdict: 'PASS', basis: 'pod_forward_before_after' });
  assert.equal(request.started_at, '2026-11-16T10:00:00.001Z');
  assert.equal(request.ended_at, '2026-11-16T10:00:00.002Z');
  assert.equal(JSON.stringify(projected).includes('do-not-retain'), false);
  // An uncertain post-write observation must not become a success through
  // projection, nor acquire a synthetic after snapshot from the earlier one.
  delete value.requests[0].serving_after;
  value.requests[0].attribution.verdict = 'UNPROVED';
  const uncertain = projectReceipt(value).requests[0];
  assert.equal(uncertain.serving_after, undefined);
  assert.equal(uncertain.attribution.verdict, 'UNPROVED');
  assert.equal('error' in uncertain.attribution, false);
});
test('peer switch follows actual write on B even when scheduled ingress is A', async () => {
  const value = receipt(),
    calls = [];
  const session = { page: {}, switchAM: async (am) => calls.push(am) };
  const ctx = {
    am: 'a',
    state: { b: session },
    fixture: (name) =>
      name === 'browser' ? { evidence: async () => value } : { webSource: source },
  };
  assert.equal(await switchAfterWrite(ctx), 'a');
  assert.deepEqual(calls, ['a']);
  assert.throws(() => validatePeerReads(value, session.peerReads), /peer_read_observed/);
  value.requests.push({ ...value.requests[0], method: 'GET', request_id: 'wrong' });
  assert.throws(() => validatePeerReads(value, session.peerReads), /peer_read_observed/);
  value.requests.push({
    ...value.requests[0],
    method: 'GET',
    am: 'a',
    serving_am: 'a',
    request_id: 'read',
  });
  validatePeerReads(value, session.peerReads);
});
test('Agent mutations omit default AM; peer reads choose opposite actual write', async () => {
  const calls = [],
    value = receipt();
  const ctx = {
    am: 'a',
    state: { sessions: [{ session: { page: {} } }] },
    fixture: () => ({ evidence: async () => value }),
    request: async (r) => {
      calls.push(r);
      return { timing: { am: r.am ?? 'b', request_id: 'accepted' } };
    },
  };
  await api(ctx, '/api/v1/agents/agent', { method: 'PATCH', agentId: 'agent' });
  await api(ctx, '/api/v1/agents/agent', { agentId: 'agent' });
  await api(ctx, '/api/v1/agents/agent', { agentId: 'agent', am: 'b' });
  assert.equal('am' in calls[0], false);
  assert.equal(calls[1].am, 'a');
  assert.equal(calls[1].peerRead, true);
  assert.equal(calls[2].am, 'b');
  assert.equal(calls.length, 3);
});
