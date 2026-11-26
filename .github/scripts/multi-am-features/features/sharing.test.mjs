import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import test from 'node:test';
import {shareAgentArtifact} from './sharing.mjs';

test('duplicate redemption fails the case and still revokes the owned fixture link', async () => {
  const agentId = 'fixture-agent';
  const calls = [];
  const link = {id: 'fixture-link', agent_id: agentId, mode: 'view', revision: 2};
  let redeemed = 0, revoked = false;
  const artifact = {id: 'fixture-artifact', agent_id: agentId, created_at: 'fixture-time'};
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
      return {body: {...artifact, size_bytes: data.length, sha256: createHash('sha256').update(data).digest('hex')}};
    }
    if (method === 'POST' && route.endsWith('/share-links')) return {body: link};
    if (method === 'GET' && route.endsWith('/share-links')) return {body: {items: [link]}};
    assert.equal(method, 'DELETE');
    assert.match(route, /fixture-link\?expected_revision=2$/);
    revoked = true;
    return {body: {...link, revoked_at: 'fixture-time'}};
  }};
  const outsider = {request: async (am, method, route, options) => {
    if (options?.allowed?.includes(404)) {
      if (redeemed === 2) assert(revoked);
      return {status: 404};
    }
    assert.equal(method, 'POST');
    assert.match(route, /fixture-link\/redeem$/);
    return {body: {...link, accepted_count: ++redeemed}};
  }};
  const failures = [];
  await assert.rejects(shareAgentArtifact({adapter, user, outsider, record: {agentId},
    recordFailure: async (stage, error) => failures.push([stage, error.message])}), /share_redemption_replayed_twice/);
  assert.deepEqual(failures, [['primary', 'share_redemption_replayed_twice']]);
  assert.deepEqual(calls.slice(0, 2).map(row => row[0]), ['b', 'a']);
  assert(revoked);
});
