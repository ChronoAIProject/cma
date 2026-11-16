import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import WebSocket, {WebSocketServer} from 'ws';
import {createBrowserAdapter, sessionStorageForProxy} from './browser.mjs';
import {validServingReceipt} from './serving.mjs';

const topology = (a, b) => ({shared_database: true, coordination_enabled: true,
  ams: Object.fromEntries(Object.entries({a, b}).map(([name, origin]) => [name,
    {ready: true, draining: false, instance_id: 'i' + name, pod_uid: 'p' + name, container_id: 'c' + name, origin}]))});

async function serve(handler) {
  const server = http.createServer(handler);
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  return {server, origin: 'http://127.0.0.1:' + server.address().port,
    close: async () => {server.closeAllConnections(); await new Promise(resolve => server.close(resolve));}};
}

test('normalized browser paths cannot replace the parent-owned upstream origin', async () => {
  const hits = [];
  const upstream = await serve((request, response) => {hits.push(['upstream', request.url]); response.end('upstream');});
  const trap = await serve((request, response) => {hits.push(['trap', request.url]); response.end('trap');});
  const fixture = createBrowserAdapter({browser: {newContext: async () => ({route: async () => {}, newPage: async () => ({}), close: async () => {}})},
    webOrigin: upstream.origin, ams: {a: upstream.origin, b: trap.origin}, actors: {user: {bearer: 'private-test-marker'}},
    adapter: {topology: async () => topology(upstream.origin, trap.origin)}, record: async () => {},
    compiledSource: '1'.repeat(40), compiledManifest: 'sha256:' + '2'.repeat(64), resolveAgent: async () => null});
  try {
    const session = await fixture.open({actor: 'user', am: 'a'});
    const authority = new URL(trap.origin).host;
    // Send raw request targets: fetch/URL construction normalizes away the attack.
    for (const path of ['/safe/..//' + authority + '/capture', '/%2e%2e//' + authority + '/capture', '/\\' + authority + '/capture']) {
      const status = await new Promise((resolve, reject) => {
        const request = http.request(session.origin, {path, headers: {cookie: 'test-session-marker'}}, response => {
          response.resume(); response.once('end', () => resolve(response.statusCode));
        });
        request.once('error', reject); request.end();
      });
      assert.equal(status, 502, path);
    }
    assert.deepEqual(hits, []);
    await session.close();
    await assert.rejects(fixture.evidence(session.page), /browser_proxy_target/);
  } finally {await fixture.close(); await Promise.all([upstream.close(), trap.close()]);}
});

test('browser proxy switches ingress without changing context, keeps attribution and strips credentials', async () => {
  const calls = [], contexts = [], recorded = [];
  const make = am => serve((request, response) => {
    calls.push({am, auth: request.headers.authorization, cookie: request.headers.cookie});
    response.writeHead(200, {'Content-Type': 'application/json', 'x-request-id': am + '-request'});
    response.end(JSON.stringify({am}));
  });
  const a = await make('a'), b = await make('b'), web = await serve((_request, response) => response.end('compiled asset'));
  const browser = {async newContext(options) {
    const context = {options, route: async () => {}, newPage: async () => ({}), close: async () => {context.closed = true;}};
    contexts.push(context); return context;
  }};
  const adapter = {ownerOf: async ({agentId}) => ({agent_id: agentId, am: 'a', epoch: 1, instance_id: 'ia', pod_uid: 'pa'}),
    topology: async () => topology(a.origin, b.origin)};
  // Configuration forwards are intentionally stale; live topology chooses the
  // actual peer origin after every owner observation and restart.
  const fixture = createBrowserAdapter({browser, webOrigin: web.origin, ams: {a: b.origin, b: a.origin},
    actors: {user: {bearer: 'private-marker'}}, adapter, record: async value => recorded.push(value),
    compiledSource: '1'.repeat(40), compiledManifest: 'sha256:' + '2'.repeat(64),
    resolveAgent: async ({path}) => path === '/api/v1/agents/owned' ? 'owned' : null});
  try {
    const session = await fixture.open({actor: 'user', am: 'a'});
    await fetch(session.origin + '/auth/session');
    await fetch(session.origin + '/agent-manager/openapi.json');
    await fetch(session.origin + '/api/v1/agents/provenance');
    await session.switchAM('b');
    await fetch(session.origin + '/api/v1/agents/provenance');
    await session.switchAM('a');
    await fetch(session.origin + '/api/agent-manager/v1/agents/owned', {headers: {Cookie: 'browser-private'}});
    await fetch(session.origin + '/api/agent-manager/v1/agents/owned', {method: 'POST'});
    const evidence = await fixture.evidence(session.page);
    assert.deepEqual(calls.map(call => call.am), ['a', 'a', 'a', 'b', 'a', 'b']);
    assert.equal(contexts.length, 1);
    assert.equal(calls[4].cookie, undefined);
    assert.equal(calls[4].auth, 'Bearer private-marker');
    assert.equal(evidence.requests[4].owner_stable, true);
    assert.ok(evidence.requests.every(validServingReceipt));
    assert.equal(JSON.stringify([evidence, recorded]).includes('private-marker'), false);
    await session.close(); assert.equal(contexts[0].closed, true);
    await fixture.evidence(session.page);
  } finally {await fixture.close(); await Promise.all([a.close(), b.close(), web.close()]);}
});

test('a cookie session cannot use an injected bearer to prove authentication', async () => {
  let contexts = 0;
  const fixture = createBrowserAdapter({browser: {async newContext() {contexts++; throw Error('must_not_open');}},
    webOrigin: 'http://127.0.0.1:10001', ams: {a: 'http://127.0.0.1:10002', b: 'http://127.0.0.1:10003'},
    actors: {user: {bearer: 'private', storageState: {cookies: [], origins: []}, sessionOrigin: 'https://api.test'}},
    adapter: {}, record: async () => {}, compiledSource: '1'.repeat(40),
    compiledManifest: 'sha256:' + '2'.repeat(64), resolveAgent: async () => null});
  await assert.rejects(fixture.open({actor: 'user', am: 'a'}), /browser_authentication_ambiguous/);
  assert.equal(contexts, 0);
});

test('session cookie relocation preserves custody flags and isolates provider cookies', () => {
  const state = {cookies: [
    {name: 'session', value: 'private', domain: 'api.localhost.direct', path: '/', secure: true, httpOnly: true, sameSite: 'Lax', expires: 2000000000},
    {name: 'provider', value: 'private-provider', domain: 'nyxid.localhost.direct', path: '/', secure: true},
  ], origins: [{origin: 'https://api.localhost.direct', localStorage: [{name: 'draft', value: 'private-draft'}]}]};
  const transformed = sessionStorageForProxy(state, 'https://api.localhost.direct', 'http://127.0.0.1:23456');
  assert.deepEqual(transformed.cookies[0], {...state.cookies[0], domain: '127.0.0.1'});
  assert.deepEqual(transformed.cookies[1], state.cookies[1]);
  assert.equal(transformed.origins[0].origin, 'http://127.0.0.1:23456');
  assert.equal(state.cookies[0].domain, 'api.localhost.direct');
  assert.throws(() => sessionStorageForProxy(state, undefined, 'http://127.0.0.1:23456'), /session_origin/);
});

test('leased browser upgrade forwards actual binary frames through selected AM without mocking', async () => {
  const upstream = await serve((_request, response) => response.end()), peer = await serve((_request, response) => response.end());
  const wsServer = new WebSocketServer({server: upstream.server});
  wsServer.on('headers', headers => headers.push('x-request-id: actual-upgrade'));
  wsServer.on('connection', socket => socket.on('message', data => socket.send(data)));
  const fixture = createBrowserAdapter({browser: {newContext: async () => ({route: async () => {}, newPage: async () => ({}), close: async () => {}})},
    webOrigin: peer.origin, ams: {a: upstream.origin, b: peer.origin}, actors: {user: {bearer: 'private'}},
    adapter: {topology: async () => topology(upstream.origin, peer.origin)}, record: async () => {}, compiledSource: '1'.repeat(40),
    compiledManifest: 'sha256:' + '2'.repeat(64), resolveAgent: async () => null});
  try {
    const session = await fixture.open({actor: 'user', am: 'a'});
    const socket = new WebSocket(session.origin.replace('http:', 'ws:') + '/api/agent-manager/v1/socket');
    await new Promise((resolve, reject) => {socket.once('open', resolve); socket.once('error', reject);});
    const pending = new Promise((resolve, reject) => {socket.once('message', resolve); socket.once('error', reject);});
    socket.send(Buffer.from([0, 1, 127, 255]));
    assert.deepEqual(await pending, Buffer.from([0, 1, 127, 255]));
    const evidence = await fixture.evidence(session.page);
    assert.equal(evidence.requests[0].request_id, 'actual-upgrade');
    assert.equal(evidence.requests[0].transport, 'websocket');
    assert.ok(validServingReceipt(evidence.requests[0]));
    socket.terminate();
  } finally {
    await fixture.close(); for (const client of wsServer.clients) client.terminate();
    await new Promise(resolve => wsServer.close(resolve)); await Promise.all([upstream.close(), peer.close()]);
  }
});

test('missing response correlation preserves one accepted write and fails evidence', async () => {
  let writes = 0; const records = [];
  const upstream = await serve((_request, response) => {writes++; response.writeHead(202, {'content-type': 'application/json'});
    response.end(JSON.stringify({receipt: {command_id: 'original-command'}, provider_secret: 'must-not-retain'}));});
  const unused = await serve((_request, response) => response.end());
  const fixture = createBrowserAdapter({browser: {newContext: async () => ({route: async () => {}, newPage: async () => ({}), close: async () => {}})},
    webOrigin: unused.origin, ams: {a: upstream.origin, b: unused.origin}, actors: {user: {bearer: 'private'}},
    adapter: {topology: async () => topology(upstream.origin, unused.origin)}, record: async row => records.push(row), compiledSource: '1'.repeat(40),
    compiledManifest: 'sha256:' + '2'.repeat(64), resolveAgent: async () => null});
  try {
    const session = await fixture.open({actor: 'user', am: 'a'});
    const response = await fetch(session.origin + '/api/v1/agents', {method: 'POST'});
    assert.equal(response.status, 202);
    await assert.rejects(fixture.evidence(session.page), /request_id/);
    assert.equal(writes, 1);
    const retained = records.find(row => row.kind === 'browser_mutation_response');
    assert.equal(retained.response.body.receipt.command_id, 'original-command');
    assert.equal(retained.complete, true);
    assert.equal(JSON.stringify(records).includes('must-not-retain'), false);
  } finally {await fixture.close(); await Promise.all([upstream.close(), unused.close()]);}
});

test('a process replacement after accepted write fails attribution without replay', async () => {
  let writes = 0, observations = 0; const records = [];
  const upstream = await serve((_request, response) => {
    writes++; response.writeHead(202, {'x-request-id': 'accepted-once'}); response.end('accepted');
  });
  const peer = await serve((_request, response) => response.end());
  const fixture = createBrowserAdapter({browser: {newContext: async () => ({route: async () => {}, newPage: async () => ({}), close: async () => {}})},
    webOrigin: peer.origin, ams: {a: upstream.origin, b: peer.origin}, actors: {user: {bearer: 'private'}},
    adapter: {topology: async () => {
      const value = topology(upstream.origin, peer.origin);
      if (++observations > 1) value.ams.a.container_id = 'replaced-container';
      return value;
    }}, record: async row => records.push(row), compiledSource: '1'.repeat(40),
    compiledManifest: 'sha256:' + '2'.repeat(64), resolveAgent: async () => null});
  try {
    const session = await fixture.open({actor: 'user', am: 'a'});
    const response = await fetch(session.origin + '/api/v1/agents', {method: 'POST'});
    assert.equal(response.status, 202);
    await assert.rejects(fixture.evidence(session.page), /serving_binding_changed/);
    assert.equal(writes, 1);
    const receipt = records.find(row => row.kind === 'browser_http').receipt;
    assert.equal(receipt.request_id, 'accepted-once');
    assert.equal(receipt.attribution.verdict, 'UNPROVED');
    assert.notEqual(receipt.serving.container_id, receipt.serving_after.container_id);
  } finally {await fixture.close(); await Promise.all([upstream.close(), peer.close()]);}
});

test('closing a held read is allowed while a truncated accepted mutation remains a failure', async () => {
  for (const method of ['GET', 'POST']) {
    let requests = 0;
    const upstream = await serve((_request, response) => {
      requests++; response.writeHead(200, {'content-type': 'application/json', 'x-request-id': 'held-response'});
      response.write('{"id":"incomplete');
    });
    const peer = await serve((_request, response) => response.end());
    const fixture = createBrowserAdapter({browser: {newContext: async () => ({route: async () => {}, newPage: async () => ({}), close: async () => {}})},
      webOrigin: peer.origin, ams: {a: upstream.origin, b: peer.origin}, actors: {user: {bearer: 'private'}},
      adapter: {topology: async () => topology(upstream.origin, peer.origin)}, record: async () => {},
      compiledSource: '1'.repeat(40), compiledManifest: 'sha256:' + '2'.repeat(64), resolveAgent: async () => null});
    try {
      const session = await fixture.open({actor: 'user', am: 'a'});
      const response = await fetch(session.origin + '/api/v1/agents', {method});
      assert.equal(response.status, 200);
      await session.close();
      if (method === 'POST') await assert.rejects(fixture.evidence(session.page), /mutation/);
      else assert.equal((await fixture.evidence(session.page)).requests.length, 1);
      assert.equal(requests, 1);
    } finally {await fixture.close(); await Promise.all([upstream.close(), peer.close()]);}
  }
});
