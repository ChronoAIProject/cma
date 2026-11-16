import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {randomBytes} from 'node:crypto';
import {Blocked, executeCase} from '../../runner.mjs';
import {loadRegistry} from '../../registry.mjs';
import {fixturesForCase} from '../../setups.mjs';
import {egRequest} from './helpers.mjs';
import {createEgTransport, channelEditSetup, parentEgSetup, setups} from './setup.mjs';

async function fixture(t, options = {}) {
  const requests = [], observations = [], records = new Map(), secrets = [randomBytes(32).toString('hex')];
  const image = 'fixture@sha256:' + 'a'.repeat(64), source = 'b'.repeat(40);
  let createCount = 0, released = 0;
  const members = {};
  for (const slot of ['a', 'b']) {
    const server = createServer(async (req, res) => {
      let body = ''; for await (const part of req) body += part;
      const value = body ? JSON.parse(body) : undefined;
      requests.push({method: req.method, path: req.url, slot, recordId: value?.record_id});
      let status = 200, result;
      if (req.url === '/v1/channels' && req.method === 'POST') {
        createCount++;
        secrets.push(value.credentials.agent_key, value.credentials.channel_key, value.credentials.hmac_secrets[0].secret);
        const record = {record_id: value.record_id, channel_id: 'synthetic-channel', version: 1,
          definition: {creation: {ready: true}, name: value.name}};
        records.set(value.record_id, record); result = record; status = 201;
        if (options.loseCreate) {req.socket.destroy(); return;}
      } else if (req.url.startsWith('/v1/channel-records/')) {
        result = records.get(req.url.split('/').at(-1));
        if (!result) {result = {code: 'not_found'}; status = 404;}
      } else if (req.method === 'DELETE') {
        if (options.failDelete) {status = 409; result = {code: 'changed_revision'};}
        else {records.clear(); status = 204;}
      } else if (req.url === '/v1/redirect') {status = 302; res.setHeader('location', options.externalOrigin);}
      else result = {slot};
      if (req.url === '/v1/pending') {status = 409; result = {detail: 'routed_reply_in_progress', retry_after_seconds: 2}; res.setHeader('retry-after', '2');}
      res.writeHead(status, {'content-type': 'application/json', 'x-request-id': slot + '-' + requests.length});
      res.end(status === 204 ? undefined : JSON.stringify(result));
    });
    server.listen(0, '127.0.0.1'); await once(server, 'listening');
    t.after(() => new Promise(resolve => {server.close(resolve); server.closeAllConnections();}));
    members[slot] = {origin: `http://127.0.0.1:${server.address().port}`, instance: 'process-' + slot,
      instance_id: 'process-' + slot, pod_uid: 'pod-' + slot, container_id: 'container-' + slot,
      ready: true, image, source};
  }
  const parent = {scope: 'LOCAL_DISPOSABLE_TWO_AM', synthetic: true, shared_database: true,
    expectedImage: image, expectedSource: source, topology: async () => members,
    authorize: async () => ({authorization: 'Bearer ' + secrets[0]}),
    witness: async input => ({slot: input.replica, instance: 'process-' + input.replica,
      requestId: input.responseRequestId}),
    leaseAuthority: async () => ({local: true, synthetic: true, agentKey: secrets[0],
      profile: {metadata: {owner: {kind: 'human', subject: 'synthetic-owner'}}},
      provider: {slug: 'synthetic-provider'}, close: async () => {released++;}})};
  const cleanups = [];
  const ctx = {row: {case_id: 'C275:EG-022'}, phase: 'before_restart', iteration: 1, am: 'a',
    defer: (label, run) => cleanups.push({label, run}),
    observe: async (label, run) => {const value = await run(); observations.push({label, value}); return value;}};
  return {parent, ctx, members, observations, requests, records, secrets,
    counts: () => ({createCount, released}), cleanup: async () => {
      const errors = [];
      for (const {run} of cleanups.reverse()) {try {await run();} catch (e) {errors.push(e);}}
      if (errors.length) throw new AggregateError(errors, 'cleanup_failed');
    }};
}

test('setup transport uses observed distinct EG forwards and retains only safe receipts', async t => {
  const f = await fixture(t), records = [];
  const transport = await createEgTransport(f.parent, async value => records.push(value));
  for (const replica of ['a', 'b']) {
    const result = await transport.request({path: '/v1/channel-capabilities', replica});
    assert.equal(result.body.slot, replica); assert.equal(result.instance, 'process-' + replica);
    assert.match(result.requestId, new RegExp('^' + replica + '-'));
  }
  assert.equal(new Set(records.map(x => x.instance_sha256)).size, 2);
  assert.ok(records.every(x => x.scope === 'eg' && !('owner_proof' in x) && !('am' in x)));
  for (const secret of f.secrets) assert.ok(!JSON.stringify(records).includes(secret));
});

test('unavailable topology blocks before network and foreign origins are rejected', async t => {
  const f = await fixture(t);
  await assert.rejects(createEgTransport({}), Blocked);
  await assert.rejects(createEgTransport({...f.parent, topology: async () => ({...f.members,
    b: {...f.members.b, ready: false}})}), Blocked);
  await assert.rejects(createEgTransport({...f.parent, topology: async () => ({...f.members,
    b: {...f.members.b, origin: 'https://example.com'}})}), /loopback_origin/);
  assert.equal(f.requests.length, 0);
});

test('wrong replica witness and redirects cannot claim attributed HTTP success', async t => {
  const f = await fixture(t, {externalOrigin: 'https://example.com/'});
  const wrong = await createEgTransport({...f.parent, witness: async () => ({slot: 'a', instance: 'process-b', requestId: 'wrong'})});
  await assert.rejects(wrong.request({path: '/v1/channel-capabilities'}), /actual_ingress/);
  const transport = await createEgTransport(f.parent);
  await assert.rejects(transport.request({path: '/v1/redirect'}), /redirect_refused/);
  await assert.rejects(transport.request({path: '/v1/../../api/credentials'}), /eg_http_origin/);
  assert.equal(f.requests.length, 2);
});

test('channel setup performs one reserved create then exact public cleanup on both EGs', async t => {
  const f = await fixture(t);
  const lease = await channelEditSetup(f.ctx, {eg: f.parent});
  assert.equal(lease.channelId, 'synthetic-channel'); assert.ok(!('credentials' in lease.updateBody));
  assert.equal(f.records.size, 1); assert.equal(f.counts().createCount, 1);
  await lease.cleanup(); await f.cleanup();
  assert.deepEqual(f.counts(), {createCount: 1, released: 1}); assert.equal(f.records.size, 0);
  const deletion = f.requests.find(x => x.method === 'DELETE');
  assert.equal(deletion.path, '/v1/channels/synthetic-channel?expected_version=1');
  assert.ok(f.requests.filter(x => x.method === 'GET').every(x => x.slot === 'b'));
  for (const secret of f.secrets) assert.ok(!JSON.stringify(f.observations).includes(secret));
});

test('lost accepted create response keeps reserved cleanup custody without resubmission', async t => {
  const f = await fixture(t, {loseCreate: true});
  await assert.rejects(channelEditSetup(f.ctx, {eg: f.parent}), error => error.message === 'eg_http_unconfirmed'
    && error.outcome === 'unknown');
  assert.equal(f.counts().createCount, 1); assert.equal(f.records.size, 1);
  await f.cleanup();
  assert.equal(f.counts().createCount, 1); assert.equal(f.records.size, 0);
});

test('revision conflict during cleanup retains failure and never retries DELETE', async t => {
  const f = await fixture(t, {failDelete: true});
  const lease = await channelEditSetup(f.ctx, {eg: f.parent});
  await assert.rejects(lease.cleanup(), /eg_http_status_409/);
  await assert.rejects(f.cleanup(), AggregateError);
  assert.equal(f.requests.filter(x => x.method === 'DELETE').length, 1);
  assert.equal(f.records.size, 1);
});

test('named remaining setups call the parent factory, validate the concrete lease and clean it once', async () => {
  const cleanups = [], calls = [];
  const method = async () => ({});
  const lease = {
    local: true, synthetic: true, replicas: [{ready: true}, {ready: true}],
    channelId: 'channel', channelName: 'channel', expected: {},
    request: method, counters: method, close: async () => calls.push('close'),
  };
  const ctx = {row: {case_id: 'C275:EG-019'}, phase: 'before_restart', iteration: 1, am: 'a',
    defer: (_label, operation) => cleanups.push(operation)};
  const primitives = {eg: {acquire: async options => {calls.push(options); return lease;}}};
  const incomplete = {...lease}; delete incomplete.channelName;
  primitives.eg.acquire = async options => {calls.push(options); return incomplete;};
  await assert.rejects(parentEgSetup(ctx, primitives, 'channel_capabilities'), /eg_fixture_field_channelName/);
  // Shape refusal must leave the acquired lease in cleanup custody.
  assert.equal(calls.length, 1);
  await cleanups.at(-1)();
  const complete = {...lease, close: async () => calls.push('closed')};
  primitives.eg.acquire = async options => {calls.push(options); return complete;};
  const result = await setups.channel_capabilities(ctx, primitives);
  assert.notEqual(result, complete);
  assert.equal(result.channelId, complete.channelId);
  assert.equal(typeof result.request, 'function');
  assert.equal(typeof result.cleanup, 'function');
  await cleanups.at(-1)();
  assert.deepEqual(calls.at(-1), 'closed');
  await result.cleanup(); await result.close();
  assert.equal(calls.filter(value => value === 'closed').length, 1);
});

test('all remaining cases reach their named parent factory through the real registry dispatcher', async () => {
  const scenarios = {18: 'channel_inventory', 19: 'channel_capabilities', 20: 'channel_create', 21: 'channel_create_uncertain', 31: 'channel_delete',
    23: 'producer_edit', 24: 'channel_admission', 25: 'consumer_profile', 26: 'multi_consumer_routing',
    27: 'channel_tests', 28: 'channel_activity', 29: 'outbound_prepare', 30: 'outbound_send', 35: 'relay_verification',
    32: 'relay_telegram', 33: 'relay_discord', 34: 'relay_whatsapp', 36: 'relay_lark', 37: 'relay_feishu', 38: 'relay_slack', 39: 'native_telegram',
    44: 'managed_connection_inventory', 45: 'connection_browser_setup', 46: 'connection_cancel',
    47: 'consumer_inventory', 48: 'consumer_create', 49: 'bot_father_profile',
    55: 'bot_father_actions', 70: 'console_session', 71: 'console_inspection', 72: 'console_source_lifecycle'};
  const registry = await loadRegistry();
  for (const [number, scenario] of Object.entries(scenarios)) {
    const id = 'C275:EG-' + number.padStart(3, '0'), definition = registry.cases.get(id);
    const calls = [];
    const shared = {fixtures: Object.fromEntries(definition.fixtures.map(name => [name, {}])),
      primitives: {eg: {acquire: async input => {calls.push(input); throw new Blocked('parent_fixture_unavailable');}}}};
    const result = await executeCase(definition, registry.rows.get(id), {phase: 'steady', iteration: 2, am: 'b',
      source: 'a'.repeat(40), image: 'fixture@sha256:' + 'b'.repeat(64),
      fixtures: fixturesForCase(registry, definition, shared)});
    assert.equal(result.status, 'BLOCKED', id + ': ' + result.error_code);
    assert.equal(result.error_code, 'parent_fixture_unavailable', id);
    assert.deepEqual(calls, [{scenario, caseId: id, phase: 'steady', iteration: 2, am: 'b'}], id);
    assert.equal(result.requests.length, 0);
  }
});

test('channel_delete setup blocks before browser mutation when its parent contract is incomplete', async () => {
  const cleanups = [], calls = [];
  const method = async () => ({});
  const complete = {
    local: true, synthetic: true, replicas: [{ready: true}, {ready: true}],
    channelId: 'owned-channel', channelName: 'owned', baseline: {turns: 0, effects: 0, deliveries: 0},
    expected: {ownerSubject: 'owner', unrelatedChannelId: 'unrelated', unrelatedChannelDigest: 'a'.repeat(64)},
    request: method, counters: method, platformSend: method, quiesce: method,
    close: async () => calls.push('close'),
  };
  const ctx = {row: {case_id: 'C275:EG-031'}, phase: 'steady', iteration: 1, am: 'a',
    defer: (_label, operation) => cleanups.push(operation)};
  const acquire = async () => complete;
  const result = await setups.channel_delete(ctx, {eg: {acquire}});
  assert.equal(result.channelId, 'owned-channel');
  await result.cleanup();
  assert.deepEqual(calls, ['close']);
  for (const field of ['channelId', 'channelName', 'baseline', 'expected']) {
    const incomplete = {...complete}; delete incomplete[field];
    await assert.rejects(parentEgSetup(ctx, {eg: {acquire: async () => incomplete}}, 'channel_delete'),
      new RegExp('eg_fixture_field_' + field));
  }
  for (const methodName of ['request', 'counters', 'platformSend', 'quiesce']) {
    const incomplete = {...complete}; delete incomplete[methodName];
    await assert.rejects(parentEgSetup(ctx, {eg: {acquire: async () => incomplete}}, 'channel_delete'),
      new RegExp('eg_fixture_method_' + methodName.toLowerCase()));
  }
});

test('named cleanup shares one failed release across callers and preserves its receiver', async () => {
  const deferred = [], failure = Error('parent_release_failed'); let releases = 0;
  const lease = {local: true, synthetic: true, replicas: [{ready: true}, {ready: true}],
    channelId: 'channel', channelName: 'channel', expected: {}, request: async () => ({}), counters: async () => ({}),
    cleanup() {assert.equal(this, lease); releases++; throw failure;}};
  const ctx = {row: {case_id: 'C275:EG-019'}, phase: 'steady', iteration: 1, am: 'a',
    defer: (_label, operation) => deferred.push(operation)};
  const result = await parentEgSetup(ctx, {eg: {acquire: async () => lease}}, 'channel_capabilities');
  const outcomes = await Promise.allSettled([result.cleanup(), result.close(), deferred[0]()]);
  assert.equal(releases, 1);
  for (const outcome of outcomes) {assert.equal(outcome.status, 'rejected'); assert.equal(outcome.reason, failure);}
});

test('EG helper forwards expected status and preserves empty transport responses', async t => {
  const f = await fixture(t), transport = await createEgTransport(f.parent);
  const ctx = {...f.ctx, assert: (condition, code) => assert.ok(condition, code)};
  const absent = await egRequest(ctx, transport, {path: '/v1/channel-records/missing',
    replica: 'b', allowed: [404], returnResponse: true});
  assert.equal(absent.status, 404); assert.equal(absent.body.code, 'not_found');
  const removed = await egRequest(ctx, transport, {method: 'DELETE', path: '/v1/channels/missing',
    allowed: [204], returnResponse: true});
  assert.equal(removed.status, 204); assert.equal(removed.body, null);
  assert.deepEqual(f.requests.map(request => request.method), ['GET', 'DELETE']);
});


test('EG transport retains actual Retry-After for the E1 pending contract', async t => {
  const f = await fixture(t), transport = await createEgTransport(f.parent);
  const response = await transport.request({path: '/v1/pending', allowed: [409]});
  assert.equal(response.retryAfterSeconds, 2);
  assert.equal(response.body.retry_after_seconds, response.retryAfterSeconds);
  const ordinary = await transport.request({path: '/v1/channel-capabilities'});
  assert.equal(ordinary.retryAfterSeconds, null);
});
