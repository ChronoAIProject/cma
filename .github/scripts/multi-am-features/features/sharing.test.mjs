import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import test from 'node:test';
import {shareAgentArtifact} from './sharing.mjs';

function scenario({duplicateRedemption = false, divergingArtifact = false} = {}) {
  const agentId = 'fixture-agent';
  const calls = [];
  const link = {id: 'fixture-link', agent_id: agentId, mode: 'view', revision: 2};
  let redeemed = 0, revoked = false;
  const artifact = {id: 'fixture-artifact', agent_id: agentId, created_at: 'fixture-time'};
  let published;
  const firstAdmission = new Map();
  const adapter = {ownerOf: async () => ({am: 'a', epoch: 2, agent_id: agentId, pod_uid: 'pod-a'})};
  const user = {request: async (am, method, route, options) => {
    calls.push([am, method, route]);
    if (method === 'POST' && route.endsWith('/artifacts')) {
      if (options.allowed.includes(409)) return {status: 409};
      const input = JSON.stringify(options.body);
      if (firstAdmission.has(options.key)) assert.equal(firstAdmission.get(options.key), input);
      else firstAdmission.set(options.key, input);
      const data = Buffer.from(options.body.content_base64, 'base64');
      const id = divergingArtifact && am === 'a' ? 'duplicate-artifact' : artifact.id;
      published = {...artifact, id, name: options.body.name, size_bytes: data.length,
        sha256: createHash('sha256').update(data).digest('hex')};
      return {body: published};
    }
    if (method === 'POST' && route.endsWith('/share-links')) return {body: link};
    if (method === 'GET' && route.endsWith('/share-links')) return {body: {items: [link]}};
    if (method === 'GET' && route.endsWith('/artifacts')) return {body: {items: [published]}};
    assert.equal(method, 'DELETE');
    assert.match(route, /fixture-link\?expected_revision=2$/);
    revoked = true;
    return {body: {...link, revoked_at: 'fixture-time'}};
  }};
  user.rawRequest = user.request;
  const outsider = {request: async (am, method, route, options) => {
    if (options?.allowed?.includes(404)) {
      if (redeemed === 2 && method === 'GET') assert(revoked);
      return {status: 404};
    }
    if (method === 'GET' && route.includes('/resource-access/')) {
      assert(!revoked);
      return {body: {capabilities: ['view_files']}};
    }
    if (method === 'GET' && route.endsWith('/' + artifact.id)) {
      assert(!revoked);
      return {body: published};
    }
    assert.equal(method, 'POST');
    assert.match(route, /fixture-link\/redeem$/);
    redeemed += 1;
    return {body: {...link, accepted_count: duplicateRedemption ? redeemed : 1}};
  }};
  outsider.rawRequest = outsider.request;
  const failures = [];
  const run = () => shareAgentArtifact({adapter, user, outsider, record: {agentId},
    recordFailure: async (stage, error) => failures.push([stage, error.message])});
  return {run, failures, calls, isRevoked: () => revoked};
}

test('both ingresses share one artifact receipt and redeem the same link only once', async () => {
  const {run, failures, calls, isRevoked} = scenario();
  await run();
  assert.deepEqual(calls.slice(0, 2).map(row => row[0]), ['b', 'a']);
  assert.deepEqual(failures, []);
  assert(isRevoked());
});

test('duplicate redemption fails the case and still revokes the owned fixture link', async () => {
  const {run, failures, calls, isRevoked} = scenario({duplicateRedemption: true});
  await assert.rejects(run, /share_redemption_replayed_twice/);
  assert.deepEqual(failures, [['primary', 'share_redemption_replayed_twice']]);
  assert.deepEqual(calls.slice(0, 2).map(row => row[0]), ['b', 'a']);
  assert(isRevoked());
});

test('competing uploads must return one artifact before any share is created', async () => {
  const {run, failures, calls, isRevoked} = scenario({divergingArtifact: true});
  await assert.rejects(run, /artifact_concurrent_receipt_diverged/);
  assert.deepEqual(calls.map(row => row[0]), ['b', 'a']);
  assert.deepEqual(failures, []);
  assert.equal(isRevoked(), false);
});
