import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {randomBytes} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
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
  const method = async () => ({})
  const lease = {
    local: true, synthetic: true, replicas: [{ready: true}, {ready: true}],
    channelId: 'channel', channelName: 'channel', expected: {},
    request: method, close: async () => calls.push('close'),
  };
  const ctx = {row: {case_id: 'C275:EG-019'}, phase: 'before_restart', iteration: 1, am: 'a',
    defer: (_label, operation) => cleanups.push(operation)};
  const primitives = {eg: {acquire: async options => {calls.push(options); return lease;}}};
  const incomplete = {...lease}; delete incomplete.channelName;
  primitives.eg.acquire = async options => {calls.push(options); return incomplete;};
  await assert.rejects(parentEgSetup(ctx, primitives, 'channel_capabilities'), /eg_fixture_field_channelName/);
  // The factory was entered before the shape refusal, but cleanup custody is
  // still impossible without a concrete contract and must remain explicit.
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
});
