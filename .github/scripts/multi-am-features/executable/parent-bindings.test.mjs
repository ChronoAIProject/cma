import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import {disposableCustody, bindParentFixtures} from './parent-bindings.mjs';

const config = {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true, run_id: 'campaign'};

test('document custody records intent before resource and never claims final disposal', async () => {
  const events = [], database = disposableCustody(config, async event => events.push(event));
  const lease = await database.reserve({purpose: 'documents', caseId: 'C275:document'});
  await lease.retainIntent({method: 'POST', path: '/api/v1/documents', key: 'original'});
  await lease.retainDocument('acknowledged-id');
  const release = await lease.release(); await lease.release();
  assert.deepEqual(events.map(event => event.kind), ['disposable_reservation', 'disposable_intent',
    'disposable_document', 'disposable_reservation_closed']);
  assert.equal(release.final_cleanup, 'PENDING_PARENT_DATABASE_DISPOSAL');
  await assert.rejects(lease.retainDocument('late'), /released/);
});

test('failed custody sink cannot silently accept resource tracking', async () => {
  const database = disposableCustody(config, async () => {throw Error('disk_unavailable');});
  await assert.rejects(database.reserve({purpose: 'documents', caseId: 'C275:document'}), /disk_unavailable/);
});

test('parent bindings require exact actor subjects and preserve supplied authentic fixtures', async () => {
  const browser = {}, services = {record: async () => {}, executable: {fixtures: {browser}}};
  const shared = await bindParentFixtures({...config, owner_id: 'owner',
    actors: {recipient: {subject: 'recipient'}, outsider: {subject: 'outsider'}}},
  {services, adapter: {}}, {hasActor: name => name === 'recipient'});
  assert.equal(shared.fixtures.browser, browser);
  assert.deepEqual(shared.primitives.identities, {user: {actor: 'user', subject: 'owner'},
    recipient: {actor: 'recipient', subject: 'recipient'}});
  assert.equal(shared.fixtures['admin-fixtures'], undefined);
});

test('leased browser proves cookie subject and releases only owned contexts', async () => {
  const received = [], contexts = [];
  let parentClosed = false;
  const server = http.createServer((request, response) => {
    received.push({path: request.url, authorization: request.headers.authorization, cookie: request.headers.cookie});
    response.writeHead(200, {'content-type': 'application/json', 'x-request-id': 'session-proof'});
    response.end(JSON.stringify({authenticated: request.headers.cookie === 'session=private', subject: 'expected-owner'}));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = 'http://127.0.0.1:' + server.address().port;
  const browser = {async close() {parentClosed = true;}, async newContext(options) {
    const context = {route: async () => {}, closed: false, async close() {this.closed = true;},
      async newPage() {return {request: {async get(url) {
        const response = await fetch(url, {headers: {cookie: options.storageState.cookies
          .map(cookie => cookie.name + '=' + cookie.value).join('; ')}});
        return {status: () => response.status, json: () => response.json()};
      }}};}};
    contexts.push(context); return context;
  }};
  const identity = {subject: 'expected-owner', sessionOrigin: origin,
    storageState: {cookies: [{name: 'session', value: 'private', domain: '127.0.0.1', path: '/'}], origins: []}};
  const services = {record: async () => {}, executable: {browserLease: {
    scope: config.scope, parentOwnsBrowser: true, browser, webOrigin: origin,
    ams: {a: origin, b: 'http://127.0.0.1:1'}, actors: {user: identity, outsider: {...identity, subject: 'outsider'}},
    compiledSource: '1'.repeat(40), compiledManifest: 'sha256:' + '2'.repeat(64), resolveAgent: async () => null,
  }}};
  const adapter = {topology: async () => ({ams: {a: {ready: true, origin,
    pod_uid: 'pod-a', instance_id: 'instance-a', container_id: 'container-a'}}})};
  const shared = await bindParentFixtures(config, {services, adapter}, {hasActor: () => false});
  try {
    const session = await shared.fixtures.browser.open({actor: 'user', am: 'a'});
    assert.equal((await shared.fixtures.browser.evidence(session.page)).authentication, 'browser_session');
    await assert.rejects(shared.fixtures.browser.open({actor: 'outsider', am: 'a'}), /cookie_session_actor_mismatch/);
    assert.equal(contexts[1].closed, true);
    assert.deepEqual(received.map(row => row.authorization), [undefined, undefined]);
    assert.ok(received.every(row => row.path === '/auth/session' && row.cookie === 'session=private'));
  } finally {
    await shared.closeOwnedFixtures();
    server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
  }
  assert.ok(contexts.every(context => context.closed));
  assert.equal(parentClosed, false);
});
