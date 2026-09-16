import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { scope, classify, compareUi, networkAssessment } from './cma-bridge-continuity-browser-contract.mjs';
import { installReadOnlyObserver } from './cma-bridge-continuity-browser-guard.mjs';

globalThis.fetch = () => { throw new Error('offline_validation_forbids_network'); };
const owned = { origin: 'https://fixture.invalid', agentId: 'agt_owned' };
const config = { ...owned, run: 'owned-replacement', expectedSubject: 'owner', markers: { manual: 'CMA-OWNED', trigger: 'CMA-TRIGGER' } };
const resources = { agentIds: ['agt_trigger', 'agt_owned'], sandboxId: 'sbx_owned', workspaceId: 'wks_owned' };
const journal = { schema: 'cma-public-canary-journal-v1', run: config.run, origin: owned.origin,
  owner: 'owner', result: { status: 'passed' }, resources, commands: {} };
const result = { status: 'passed', resources };
const now = (seconds) => `2026-09-16T00:00:${String(seconds).padStart(2, '0')}Z`;
const before = { documentId: 'document-one', timeOrigin: 1234, navigationCount: 1, navigationEntries: 1,
  path: '/agents/agt_owned', messages: [{ role: 'Agent', sha256: 'original' }], allowedMutations: 0,
  blockedMutations: 0, finalMarkerVisible: false, closed: false };
const after = { ...before, messages: [...before.messages, { role: 'Agent', sha256: 'final' }], finalMarkerVisible: true };
const selectedStream = { id: 1, kind: 'v2_agent_sse', method: 'GET', status: 200, sse: true,
  startedAt: now(1), finishedAt: now(15) };

test('scope derives only the successful replacement manual Agent and expected final marker', () => {
  const value = scope(config, journal, result, 'active-final-ops');
  assert.equal(value.agentId, 'agt_owned');
  assert.equal(value.finalMarker, 'CMA-OWNED CMA-ACTIVE-active-final-ops');
  assert.throws(() => scope(config, { ...journal, result: { status: 'failed' } }, result, 'active-final-ops'));
  assert.throws(() => scope(config, journal, { ...result, resources: { ...resources, agentIds: ['agt_trigger', 'agt_other'] } }, 'active-final-ops'));
});
test('only exact owned stream routes are classified; queries retain hashes rather than raw cursors', () => {
  const v2 = classify(`${owned.origin}/api/v2/agents/agt_owned/events?after_cursor=PRIVATE_CURSOR`, owned);
  assert.equal(v2.kind, 'v2_agent_sse');
  assert.equal(v2.cursorSha256.length, 64);
  assert.ok(!JSON.stringify(v2).includes('PRIVATE_CURSOR'));
  assert.equal(classify(`${owned.origin}/api/v1/agui/events?thread_id=agt_owned&starting_after=3`, owned).kind, 'legacy_agui_sse');
  assert.equal(classify(`${owned.origin}/api/v1/agui/events?thread_id=agt_other`, owned).kind, 'read');
  assert.equal(classify(`${owned.origin}/api/v2/agents/agt_owned/events/snapshot`, owned).kind, 'read');
});
test('UI pass requires same document and Agent, retained history, final marker and zero mutation attempts', () => {
  assert.equal(compareUi(before, after, owned).status, 'passed');
  for (const mutation of [{ timeOrigin: 4321 }, { navigationCount: 2 }, { path: '/agents/agt_other' },
    { messages: [{ role: 'Agent', sha256: 'replacement-history' }] }, { finalMarkerVisible: false },
    { allowedMutations: 1 }, { blockedMutations: 1 }, { closed: true }]) {
    assert.equal(compareUi(before, { ...after, ...mutation }, owned).status, 'failed');
  }
});
test('a fresh stream after the old one closes inside the armed interval is a recorded automatic reopen', () => {
  const next = { ...selectedStream, id: 2, startedAt: now(20), finishedAt: undefined };
  const value = networkAssessment([selectedStream, next], [], now(10), now(30));
  assert.equal(value.reconnectEvidence, 'observed');
  assert.deepEqual(value.automaticReopens[0].previous, 1);
  assert.equal(value.transportCoverage, 'fetch_sse_observed');
});
test('polling, failed reconnect attempts and closures before arming do not establish successful reconnection', () => {
  const next = { ...selectedStream, id: 2, startedAt: now(20), finishedAt: undefined };
  assert.equal(networkAssessment([{ ...selectedStream, kind: 'read' }], [], now(10), now(30)).reconnectEvidence, 'not_observed');
  assert.equal(networkAssessment([selectedStream, { ...next, status: 503 }], [], now(10), now(30)).reconnectEvidence, 'not_observed');
  assert.equal(networkAssessment([selectedStream, next], [], now(18), now(30)).reconnectEvidence, 'not_observed');
});
test('GET errors, network failures and typed SSE errors remain distinct evidence', () => {
  const value = networkAssessment([{ ...selectedStream, failedAt: now(15), failure: 'net::ERR_ABORTED' },
    { id: 2, kind: 'read', startedAt: now(16), status: 503 }],
  [{ selected: true, type: 'stream_frame', frameType: 'cma.agent.error', code: 'read_unavailable' }], now(10), now(30));
  assert.equal(value.httpFailures.length, 1);
  assert.equal(value.networkFailures.length, 1);
  assert.equal(value.streamErrors.length, 1);
});

function browserStub(nativeFetch) {
  const window = { fetch: nativeFetch };
  const context = vm.createContext({ window, location: { href: owned.origin + '/agents/agt_owned' },
    Request, Response, Headers, URL, TextDecoder, Date, TypeError });
  vm.runInContext(`(${installReadOnlyObserver.toString()})`, context)({ exactOrigin: owned.origin,
    credential: 'OFFLINE_TOKEN', documentId: 'offline-document', agentId: owned.agentId });
  return window;
}
test('guard blocks POST and auth mutation routes before native fetch; it never retries', async () => {
  let calls = 0;
  const window = browserStub(async () => { calls++; return new Response('{}'); });
  await assert.rejects(window.fetch(owned.origin + '/api/v2/responses', { method: 'POST' }));
  await assert.rejects(window.fetch(owned.origin + '/auth/logout'));
  assert.equal(calls, 0);
  assert.equal(window.__cmaReadOnlyObserver.blocked.length, 2);
});
test('saved bearer is exact-origin API/session only and requests reject redirects', async () => {
  const seen = [];
  const window = browserStub(async request => { seen.push(request); return new Response('{}'); });
  await window.fetch(owned.origin + '/api/v1/agents/agt_owned');
  await window.fetch('https://other.invalid/read', { headers: { authorization: 'Bearer something' } });
  await window.fetch(owned.origin + '/asset.js');
  assert.equal(seen[0].headers.get('authorization'), 'Bearer OFFLINE_TOKEN');
  assert.equal(seen[1].headers.get('authorization'), null);
  assert.equal(seen[2].headers.get('authorization'), null);
  assert.ok(seen.every(request => request.redirect === 'error'));
});
test('passive stream inspection preserves bytes and records v2 event-name errors without retaining payload text', async () => {
  const body = 'event: cma.agent.error\ndata: {"code":"read_unavailable","message":"PRIVATE_ERROR_TEXT"}\n\n';
  const window = browserStub(async () => new Response(body, { headers: { 'content-type': 'text/event-stream' } }));
  const response = await window.fetch(owned.origin + '/api/v2/agents/agt_owned/events');
  const reader = response.body.getReader();
  const first = await reader.read();
  assert.equal(new TextDecoder().decode(first.value), body);
  assert.equal((await reader.read()).done, true);
  const events = window.__cmaReadOnlyObserver.events;
  assert.equal(events.find(value => value.type === 'stream_frame').frameType, 'cma.agent.error');
  assert.equal(events.find(value => value.type === 'stream_frame').code, 'read_unavailable');
  assert.ok(!JSON.stringify(events).includes('PRIVATE_ERROR_TEXT'));
});
test('body read failures propagate unchanged and remain observable without a repair request', async () => {
  const failure = new Error('OFFLINE_NETWORK_FAILURE');
  let calls = 0;
  const window = browserStub(async () => {
    calls++;
    return new Response(new ReadableStream({ start(controller) { controller.error(failure); } }),
      { headers: { 'content-type': 'text/event-stream' } });
  });
  const response = await window.fetch(owned.origin + '/api/v2/agents/agt_owned/events');
  await assert.rejects(response.body.getReader().read(), error => error === failure);
  assert.equal(calls, 1);
  assert.ok(window.__cmaReadOnlyObserver.events.some(value => value.type === 'stream_read_error'));
});
