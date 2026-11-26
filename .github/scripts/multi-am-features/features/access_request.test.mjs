import assert from 'node:assert/strict';
import test from 'node:test';
import {hash} from '../cma/core.mjs';
import {accessRequest} from './access_request.mjs';

function fixture({reopenDenied = false, wrongToolReceipt = false} = {}) {
  const record = {agentId: 'fixture-agent', sandboxId: 'fixture-sandbox'};
  const requestId = 'aar_' + 'a'.repeat(64);
  const visited = [], seenKeys = [];
  let armed, request, denyKey, response, streamsClosed = 0;
  const adapter = {ownerOf: async () => ({agent_id: record.agentId, am: 'a', epoch: 1, pod_uid: 'owner-pod'}),
    prepareTurn: async input => { armed = input; seenKeys.push(input.accessRequest); },
    releaseTurn: async () => {},
    oracle: async ({caseId}) => ({case_id: caseId, access_tool_calls: 1, request_count: 2,
      completed_effect_count: 2, active_streams: 0, access_receipt: {
        request_sha256: hash(wrongToolReceipt ? 'another-request' : requestId), status: request.status,
      }}),
  };
  const api = {send: async (am, agentId, input, key) => {
    assert.equal(agentId, record.agentId);
    assert.equal(key, armed.caseId);
    assert(input.includes(armed.marker));
    const status = request?.status === 'denied' && !reopenDenied ? 'denied' : 'pending';
    request = {id: requestId, agent_id: agentId, reason: 'Synthetic consent check ' + armed.accessRequest,
      services: ['fixture-access-service'], status};
    response = {id: 'fixture-response', conversation: {id: agentId}, status: 'completed', output: [armed.marker]};
    visited.push(am);
    return {responseId: response.id, abort: () => { streamsClosed += 1; }, done: Promise.resolve()};
  }, request: async () => ({body: response})};
  const user = {request: async (am, method, path, options) => {
    visited.push(am);
    if (path.endsWith('fixture-sandbox')) return {body: {id: record.sandboxId, nyxid_services: ['baseline-service']}};
    if (method === 'GET') return {body: {items: request ? [{...request}] : []}};
    assert.equal(path, `/api/v1/agents/${record.agentId}/access-requests/${requestId}/deny`);
    if (denyKey) assert.equal(options.key, denyKey);
    else denyKey = options.key;
    request.status = 'denied';
    return {body: {...request}};
  }};
  const outsider = {request: async (am, _method, _path, options) => {
    visited.push(am);
    assert.deepEqual(options.allowed, [404]);
    return {status: 404, body: {code: 'agent_access_not_found'}};
  }};
  return {context: {api, adapter, user, outsider, record}, visited, seenKeys, closed: () => streamsClosed};
}

test('the peer denies the exact proposal once and the runtime replay observes that decision', async () => {
  const {context, visited, seenKeys, closed} = fixture();
  await accessRequest(context);
  assert.deepEqual(new Set(visited), new Set(['a', 'b']));
  assert.equal(seenKeys.length, 2);
  assert.equal(seenKeys[0], seenKeys[1]);
  assert.equal(closed(), 2);
});

test('a wrong tool receipt or a reopened denied request fails the regression', async () => {
  const wrong = fixture({wrongToolReceipt: true});
  await assert.rejects(accessRequest(wrong.context), /agent_access_pending_tool_receipt_unbound/);
  assert.equal(wrong.closed(), 1);
  const reopened = fixture({reopenDenied: true});
  await assert.rejects(accessRequest(reopened.context), /agent_access_denied_tool_receipt_unbound/);
  assert.equal(reopened.closed(), 2);
});
