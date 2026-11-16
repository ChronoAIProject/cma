import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {createDecipheriv, createHash, randomBytes, randomUUID} from 'node:crypto';
import {createNativeLarkWebhook} from './native-lark-webhook.mjs';
import {digest} from './helpers.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';
import {Blocked} from '../../runner.mjs';

// Independent loopback receiver validates actual transmitted bytes. This is a
// protocol regression, not a claim that the native Rust handler was executed.
async function fixture(t, options = {}) {
  const credentials = {encrypt_key: randomBytes(32).toString('hex'), verification_token: randomBytes(32).toString('hex')};
  const identity = {region: options.region ?? 'lark', app_id: 'cli_synthetic', tenant_key: 'tenant_synthetic', bot_open_id: 'ou_synthetic'};
  const requests = [], observations = [], members = {}, image = 'fixture@sha256:' + 'a'.repeat(64), source = 'b'.repeat(40);
  let authorizations = 0, changed = false;
  for (const replica of ['a', 'b']) {
    const server = createServer(async (req, res) => {
      const parts = []; for await (const part of req) parts.push(part);
      const bytes = Buffer.concat(parts), headers = req.headers;
      const request = {replica, method: req.method, path: req.url, bytes, headers}; requests.push(request);
      if (options.loseResponse) {req.socket.destroy(); return;}
      if (options.holdResponse) {options.received?.(); return;}
      let payload, valid = true;
      try {
        const envelope = JSON.parse(bytes);
        assert.deepEqual(Object.keys(envelope), ['encrypt']);
        const encrypted = Buffer.from(envelope.encrypt, 'base64');
        const key = createHash('sha256').update(credentials.encrypt_key).digest();
        const decipher = createDecipheriv('aes-256-cbc', key, encrypted.subarray(0, 16));
        payload = JSON.parse(Buffer.concat([decipher.update(encrypted.subarray(16)), decipher.final()]));
        const challenge = payload.type === 'url_verification';
        if (!challenge) {
          const stamp = headers['x-lark-request-timestamp'], nonce = headers['x-lark-request-nonce'];
          const h = createHash('sha256'); h.update(stamp ?? ''); h.update(nonce ?? ''); h.update(credentials.encrypt_key); h.update(bytes);
          valid = h.digest('hex') === headers['x-lark-signature']
            && Math.abs(Math.floor(Date.now() / 1000) - Number(stamp)) <= 300
            && String(Number(stamp)) === stamp && /^[A-Za-z0-9_-]{1,128}$/.test(nonce ?? '')
            && payload.schema === '2.0' && payload.header.app_id === identity.app_id && payload.header.tenant_key === identity.tenant_key;
        }
        valid &&= (challenge ? payload.token : payload.header.token) === credentials.verification_token;
      } catch {valid = false;}
      const status = valid ? 200 : 401;
      const body = valid ? (payload.type === 'url_verification' ? {challenge: payload.challenge} : {accepted: true})
        : {status: 401, code: 'not_authenticated', detail: 'native_lark_authentication_failed'};
      request.payload = payload; request.status = status;
      if (options.changeProcess) changed = true;
      res.writeHead(status, {'content-type': valid ? 'application/json' : 'application/problem+json',
        'x-request-id': replica + '-' + requests.length});
      res.end(JSON.stringify(body));
    });
    server.listen(0, '127.0.0.1'); await once(server, 'listening');
    t.after(() => new Promise(resolve => {server.close(resolve); server.closeAllConnections();}));
    members[replica] = {origin: 'http://127.0.0.1:' + server.address().port, instance: 'eg-' + replica,
      instance_id: 'eg-' + replica, pod_uid: 'pod-' + replica, container_id: 'container-' + replica, ready: true, image, source};
  }
  const parent = {scope: 'LOCAL_DISPOSABLE_TWO_AM', synthetic: true, shared_database: true,
    expectedImage: image, expectedSource: source,
    topology: async () => Object.fromEntries(Object.entries(members).map(([slot, value]) => [slot,
      {...value, ...(changed ? {instance: 'changed-' + slot, instance_id: 'changed-' + slot} : {})}])),
    authorize: async () => {authorizations++; return {authorization: 'Bearer ' + randomBytes(32).toString('hex')};},
    witness: async ({replica, responseRequestId}) => ({slot: replica, instance: 'eg-' + replica, requestId: responseRequestId})};
  const configuration = {local: true, synthetic: true, channelId: 'native-channel', identity, credentials};
  const sender = await createNativeLarkWebhook(parent, configuration, value => observations.push(value));
  t.after(() => sender.close());
  return {parent, configuration, sender, observations, requests, members, authorizations: () => authorizations};
}

function event(replica = 'a', extra = {}) {
  return {replica, eventId: randomUUID(), eventType: 'im.message.receive_v1', event: {
    sender: {sender_type: 'user', sender_id: {open_id: 'ou_person'}, tenant_key: 'tenant_synthetic'},
    message: {message_id: 'om_' + randomUUID().replaceAll('-', ''), chat_id: 'oc_private', chat_type: 'p2p',
      create_time: String(Date.now()), message_type: 'text', content: JSON.stringify({text: 'Synthetic Unicode 文本 😀'})}}, ...extra};
}

test('native Lark sender encrypts and signs exact transmitted bytes on both attributed EGs', async t => {
  const f = await fixture(t);
  const sent = await Promise.all(['a', 'b'].map(replica => f.sender.sendEvent(event(replica))));
  assert.equal(f.requests.length, 2); assert.equal(f.authorizations(), 0);
  assert.equal(new Set(f.requests.map(r => r.headers['x-lark-request-nonce'])).size, 2);
  assert.equal(new Set(f.requests.map(r => JSON.parse(r.bytes).encrypt)).size, 2);
  for (const r of sent) {
    assert.deepEqual(r.body, {accepted: true}); assert.ok(validEgReceipt(r.eg_receipt));
    const actual = f.requests.find(v => v.replica === r.eg_receipt.replica);
    assert.equal(actual.method, 'POST'); assert.equal(actual.path, '/v1/native-lark/native-channel');
    assert.equal(actual.headers.authorization, undefined); assert.equal(actual.headers['content-type'], 'application/json');
    assert.equal(r.sent.body_sha256, digest(actual.bytes));
    assert.equal(r.sent.signature_sha256, digest(actual.headers['x-lark-signature']));
    assert.equal(r.sent.provenance, 'sender'); assert.equal(actual.payload.header.app_id, f.configuration.identity.app_id);
    assert.equal(actual.payload.event.message.content, JSON.stringify({text: 'Synthetic Unicode 文本 😀'}));
  }
  const retained = JSON.stringify(f.observations);
  for (const value of [...Object.values(f.configuration.credentials), 'Synthetic Unicode 文本', 'ou_person'])
    assert.equal(retained.includes(value), false);
  for (const r of f.requests) {
    assert.equal(retained.includes(r.headers['x-lark-signature']), false);
    assert.equal(retained.includes(JSON.parse(r.bytes).encrypt), false);
  }
});

test('encrypted URL challenge proves token without inventing signature-header authentication', async t => {
  const f = await fixture(t, {region: 'feishu'});
  const challenge = 'fresh-' + randomUUID();
  const response = await f.sender.sendChallenge({replica: 'b', challenge});
  assert.deepEqual(response.body, {challenge}); assert.equal(response.sent.kind, 'challenge');
  assert.equal(response.sent.signature_sha256, null);
  for (const h of ['x-lark-request-timestamp', 'x-lark-request-nonce', 'x-lark-signature'])
    assert.equal(f.requests[0].headers[h], undefined);
  assert.equal(f.requests[0].payload.type, 'url_verification');
  const invalid = await f.sender.sendChallenge({replica: 'a', challenge, variant: 'invalid_token', allowed: [401]});
  assert.equal(invalid.status, 401); assert.equal(invalid.body.detail, 'native_lark_authentication_failed');
  assert.notEqual(invalid.sent.verification_token_sha256, digest(f.configuration.credentials.verification_token));
});

test('native authentication probes change exact signed facts and retain typed 401 responses', async t => {
  const f = await fixture(t);
  for (const variant of ['invalid_signature', 'invalid_token', 'wrong_app', 'wrong_tenant', 'stale_timestamp']) {
    const response = await f.sender.sendEvent(event('a', {variant, allowed: [401]}));
    assert.equal(response.status, 401); assert.equal(response.body.code, 'not_authenticated');
    assert.equal(response.sent.variant, variant); assert.ok(validEgReceipt(response.eg_receipt));
  }
  assert.equal(f.requests.length, 5); assert.equal(f.authorizations(), 0);
});

test('unsupported custody, foreign topology and malformed payloads block before dispatch', async t => {
  const f = await fixture(t);
  for (const edit of [c => c.synthetic = false, c => c.identity.region = 'foreign',
    c => c.channelId = '../foreign', c => c.credentials.encrypt_key = '', c => c.identity.app_id = 'ou_wrong']) {
    const c = structuredClone(f.configuration); edit(c);
    await assert.rejects(createNativeLarkWebhook(f.parent, c), error => error instanceof Blocked);
  }
  const parent = {...f.parent, topology: async () => ({...f.members, a: {...f.members.a, origin: 'https://example.com'}})};
  await assert.rejects(createNativeLarkWebhook(parent, f.configuration), /loopback_origin_required/);
  for (const input of [{eventId: 'bad/event'}, {eventType: 'bad event'}, {event: []}, {variant: 'unknown'},
    {event: {content: 'a'.repeat(256 * 1024)}}])
    await assert.rejects(f.sender.sendEvent(event('a', input)), error => error instanceof Blocked);
  assert.equal(f.requests.length, 0);
});

test('lost native callback response remains unknown and is never replayed', async t => {
  const f = await fixture(t, {loseResponse: true});
  await assert.rejects(f.sender.sendEvent(event()), error => error.message === 'eg_http_unconfirmed' && error.outcome === 'unknown');
  assert.equal(f.requests.length, 1);
  assert.ok(f.observations.some(v => v.schema === 'multi-eg-native-lark-sender-v1' && v.error_code === 'eg_http_unconfirmed'));
  await f.sender.close(); await f.sender.close();
  await assert.rejects(f.sender.sendEvent(event()), /sender_closed/);
  assert.equal(f.requests.length, 1);
});

test('a changed EG process cannot turn an acknowledged native callback into attributed success', async t => {
  const f = await fixture(t, {changeProcess: true});
  await assert.rejects(f.sender.sendEvent(event()), /eg_http_serving_window_unproved/);
  assert.equal(f.requests.length, 1); assert.equal(f.requests[0].status, 200);
  assert.ok(f.observations.some(v => v.error_code === 'eg_http_serving_window_unproved'));
});

test('parent cancellation aborts one pending callback without admitting a replacement', async t => {
  let received; const arrived = new Promise(resolve => {received = resolve;});
  const f = await fixture(t, {holdResponse: true, received}), controller = new AbortController();
  const request = f.sender.sendEvent(event('b', {signal: controller.signal}));
  const rejected = assert.rejects(request, error => error.message === 'eg_http_unconfirmed' && error.outcome === 'unknown');
  await arrived; controller.abort(); await rejected; await f.sender.close();
  assert.equal(f.requests.length, 1);
});

test('closing the sender joins and cancels in-flight work, while pre-cancelled work never starts', async t => {
  let received; const arrived = new Promise(resolve => {received = resolve;});
  const f = await fixture(t, {holdResponse: true, received});
  const cancelled = new AbortController(); cancelled.abort();
  await assert.rejects(f.sender.sendEvent(event('a', {signal: cancelled.signal})), /cancelled_before_dispatch/);
  assert.equal(f.requests.length, 0);
  const request = f.sender.sendEvent(event('a'));
  const rejected = assert.rejects(request, /eg_http_unconfirmed/);
  await arrived; await f.sender.close(); await rejected;
  assert.equal(f.requests.length, 1);
});
