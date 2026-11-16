import test from 'node:test';
import assert from 'node:assert/strict';
import {randomBytes, randomUUID} from 'node:crypto';
import {channelDraft, assertCreateIntent, assertCreatedChannel, assertRecoveredChannel,
  armChannelCreation, channelCreateCase, channelRecoveryCase} from './channel-creation.mjs';
import {digest} from './helpers.mjs';
import {parentEgSetup} from './setup.mjs';
import {Blocked} from '../../runner.mjs';

const ctx = {assert: (condition, code) => assert.ok(condition, code)};
const lease = {expected: {ownerSubject: 'synthetic-owner'}, draft: {
  name: 'Synthetic channel', profileName: 'Synthetic profile', description: 'Disposable channel test',
  provider: 'synthetic-provider', model: {model: 'synthetic-model', access: 'managed',
    service: 'synthetic-service', api_type: 'responses', thinking_effort: 'high'},
  permission: {action_mode: 'guarded', service_scope: ['synthetic-service']}, systemPrompt: 'Synthetic instructions',
  hmacKeyId: 'synthetic-hmac', promptTemplate: '{{text}}', autoFinal: false, ackAfterMs: 1700,
}};

function values() {
  const desired = channelDraft(lease, {relay_authoring: {}});
  const credentials = {agent_key: randomBytes(32).toString('hex'),
    hmac_secrets: [{key_id: lease.draft.hmacKeyId, secret: randomBytes(32).toString('hex')}]};
  const credentialHashes = {agent: 'sha256:' + digest(credentials.agent_key),
    hmac: 'sha256:' + digest(credentials.hmac_secrets[0].secret)};
  const body = {...desired, record_id: randomUUID(), credentials}, key = randomUUID();
  const intent = {recordId: body.record_id, configuration: desired};
  const record = {record_id: body.record_id, channel_id: 'observed-channel', version: 2,
    definition: {...desired, owner: desired.profile.metadata.owner,
      // ProviderBinding omits its default profile target when serialized.
      provider: {slug: desired.provider.slug, binding_id: 'observed-binding', level: 'L1'},
      creation: {ready: true, channel_id: 'observed-channel', fingerprint: 'sha256:' + digest(body)},
      secrets: {agent_key_hash: credentialHashes.agent}},
    provider: {slug: desired.provider.slug, binding_id: 'observed-binding', level: 'L1',
      honoured: ['/profile/model'], refused: []}};
  return {desired, body, key, credentialHashes, intent, record};
}

test('UI channel intent requires separate generated UUIDs, exact configuration and private credentials', () => {
  const v = values(); assertCreateIntent(ctx, v.body, v.key, v.desired, v.credentialHashes);
  const variants = [b => {b.record_id = 'not-a-uuid';}, b => {b.name += ' changed';},
    b => {b.profile.metadata.owner.subject = 'other-owner';}, b => {b.credentials.hmac_secrets = [];},
    b => {b.credentials.hmac_secrets[0].secret = randomBytes(32).toString('hex');},
    b => {b.credentials.channel_key = randomBytes(32).toString('hex');},
    b => {b.credentials.agent_key = randomBytes(32).toString('hex');}];
  for (const change of variants) {
    const body = structuredClone(v.body); change(body);
    assert.throws(() => assertCreateIntent(ctx, body, v.key, v.desired, v.credentialHashes), /channel_/);
  }
  assert.throws(() => assertCreateIntent(ctx, v.body, v.body.record_id, v.desired, v.credentialHashes), /record_and_key/);
});

test('durable custom execution follows both the actual capability and provider support', () => {
  assert.equal(channelDraft(lease, {}).durable_custom_execution, undefined);
  for (const authoring of [{custom_durable_execution_enabled: true},
    {custom_durable_execution_enabled: false, response_recovery_providers: [lease.draft.provider]},
    {custom_durable_execution_enabled: true, response_recovery_providers: ['other-provider']}])
    assert.equal(channelDraft(lease, {relay_authoring: authoring}).durable_custom_execution, undefined);
  assert.equal(channelDraft(lease, {relay_authoring: {custom_durable_execution_enabled: true,
    response_recovery_providers: [lease.draft.provider]}}).durable_custom_execution, true);
  for (const definition of [channelCreateCase, channelRecoveryCase]) {
    assert.equal(definition.evidence_scope, 'eg_only');
    assert.ok(definition.fixtures.includes('eg_browser'));
  }
});

test('recovery compares retained version and credentials while allowing GET to omit the original bind report', () => {
  const v = values(), recovered = structuredClone(v.record); recovered.provider.honoured = [];
  assertCreatedChannel(ctx, recovered, v.intent, v.credentialHashes);
  assertRecoveredChannel(ctx, recovered, v.record);
  for (const change of [r => {r.version++;}, r => {r.channel_id = 'other-channel';},
    r => {r.provider.binding_id = 'other-binding';}, r => {r.definition.secrets.agent_key_hash = 'sha256:' + 'b'.repeat(64);},
    r => {r.definition.creation.fingerprint = 'sha256:' + 'c'.repeat(64);},
    r => {r.definition.sources[0].auth.key_id = 'other-key';}, r => {r.definition.reply.sinks = [];},
    r => {r.definition.profile.permission.action_mode = 'autonomous';}]) {
    const changed = structuredClone(recovered); change(changed);
    assert.throws(() => assertRecoveredChannel(ctx, changed, v.record), /channel_recovered/);
  }
  const leaked = structuredClone(v.record); leaked.definition.secrets.hmac_secrets = v.body.credentials.hmac_secrets;
  assert.throws(() => assertCreatedChannel(ctx, leaked, v.intent, v.credentialHashes), /private_values/);
  for (const target of [null, {type: 'existing_agent', agent_id: 'unexpected-agent'}]) {
    const changed = structuredClone(v.record); changed.definition.provider.target = target;
    assert.throws(() => assertCreatedChannel(ctx, changed, v.intent, v.credentialHashes), /provider_binding/);
  }
  const existing = structuredClone(v.record), intent = structuredClone(v.intent);
  intent.configuration.provider.target = {type: 'existing_agent', agent_id: 'observed-agent'};
  existing.definition.provider.target = intent.configuration.provider.target;
  assertCreatedChannel(ctx, existing, intent, v.credentialHashes);
  delete existing.definition.provider.target;
  assert.throws(() => assertCreatedChannel(ctx, existing, intent, v.credentialHashes), /provider_binding/);
});

async function interception(options = {}) {
  const v = values(), calls = [], deferred = [], observations = [], controller = new AbortController(); let handler;
  const page = {url: () => 'http://127.0.0.1:1234/event-gateway', async route(matcher, callback) {
    assert.equal(matcher(new URL('http://127.0.0.1:1234/api/event-gateway/v1/channels')), true);
    assert.equal(matcher(new URL('http://127.0.0.1:4321/api/event-gateway/v1/channels')), false);
    assert.equal(matcher(new URL('http://127.0.0.1:1234/api/v1/bots')), false); handler = callback;
  }, async unroute() {calls.push('unroute'); if (options.unrouteFailure) throw Error('unroute_failed');}};
  const parent = {async retainChannelIntent(intent) {
    calls.push('custody');
    assert.ok(intent.key === v.key && intent.recordId === v.body.record_id);
    if (options.custodyFailure) throw Error('custody_failed');
    if (options.cancelAtCustody) controller.abort(Error('cancelled'));
    return {retained: true, record_sha256: digest(intent.recordId),
      key_sha256: options.wrongCustody ? '0'.repeat(64) : digest(intent.key), body_sha256: digest(intent.body)};
  }};
  const operation = await armChannelCreation({...ctx, signal: controller.signal, remainingMs: () => 500,
    defer: (_label, work) => deferred.push(work), observe: async (_label, read) => observations.push(await read())},
  page, parent, v.desired, v.credentialHashes, {loseResponse: options.loseResponse ?? true});
  const dispatch = (method = 'POST', path = '/api/event-gateway/v1/channels') => handler({request: () => ({
    method: () => method, url: () => 'http://127.0.0.1:1234' + path,
    postDataJSON: () => options.wrongBody ? {...v.body, name: 'different'} : v.body,
    allHeaders: async () => ({'idempotency-key': v.key})}),
  async fallback() {calls.push('fallback');}, async fetch(config) {
    assert.deepEqual(config, {maxRetries: 0, maxRedirects: 0, timeout: 500}); calls.push('fetch');
    if (options.waitFetch) await options.waitFetch;
    if (options.fetchFailure) throw Error('upstream_unknown');
    return {status: () => options.status ?? 201, json: async () => v.record,
      headers: () => ({'x-request-id': 'observed-eg-request'}), async dispose() {
        calls.push('dispose'); if (options.disposeFailure) throw Error('dispose_failed');}};
  }, async fulfill({response}) {
    calls.push('fulfill'); assert.equal(await response.json(), v.record);
    if (options.fulfillFailure) throw Error('fulfill_failed');
  }, async abort(reason) {calls.push('abort'); assert.equal(reason, 'failed');
    if (options.abortFailure) throw Error('abort_failed');}});
  return {...v, operation, calls, observations, controller, dispatch, cleanup: () => deferred[0]()};
}

test('one accepted channel POST gets cleanup custody before forwarding and only its browser response is lost', async () => {
  const f = await interception(); await f.dispatch(); const actual = await f.operation.result();
  assert.equal(actual.record, f.record); assert.equal(actual.intent.recordId, f.body.record_id);
  assert.equal(actual.requestId, 'observed-eg-request');
  assert.deepEqual(f.calls, ['custody', 'fetch', 'dispose', 'abort']);
  for (const method of ['GET', 'HEAD', 'OPTIONS']) await f.dispatch(method);
  assert.equal(f.operation.attempts(), 1); await f.cleanup();
  for (const value of [f.body.credentials.agent_key, f.body.credentials.hmac_secrets[0].secret,
    f.key, f.body.record_id]) assert.ok(!JSON.stringify(f.observations).includes(value));
  assert.equal(Object.hasOwn(actual.intent, 'credentials'), false);
});

test('wrong body, absent custody and changed custody hashes refuse the first dispatch', async () => {
  for (const option of ['wrongBody', 'custodyFailure', 'wrongCustody']) {
    const f = await interception({[option]: true}); await f.dispatch();
    await assert.rejects(f.operation.result()); assert.ok(!f.calls.includes('fetch'));
    await assert.rejects(f.cleanup(), /channel_creation_route_failed/);
  }
});

test('ordinary channel creation forwards its original accepted response exactly once', async () => {
  const f = await interception({loseResponse: false}); await f.dispatch();
  assert.equal((await f.operation.result()).record, f.record);
  assert.deepEqual(f.calls, ['custody', 'fetch', 'fulfill', 'dispose']);
  await f.dispatch(); assert.equal(f.calls.filter(c => c === 'fetch').length, 1);
  await assert.rejects(f.cleanup(), /channel_creation_route_failed/);
});

test('failed response delivery preserves unknown outcome; disposal failure cannot abort a fulfilled route', async () => {
  const failed = await interception({loseResponse: false, fulfillFailure: true}); await failed.dispatch();
  await assert.rejects(failed.operation.result(), /fulfill_failed/);
  assert.deepEqual(failed.calls, ['custody', 'fetch', 'fulfill', 'dispose', 'abort']);
  await assert.rejects(failed.cleanup());
  const disposed = await interception({loseResponse: false, disposeFailure: true}); await disposed.dispatch();
  await assert.rejects(disposed.operation.result(), /dispose_failed/);
  assert.ok(!disposed.calls.includes('abort')); await assert.rejects(disposed.cleanup());
});

test('duplicate POST and any changed mutation stay failures even after successful recovery evidence', async () => {
  for (const [method, path] of [['POST', '/api/event-gateway/v1/channels'],
    ['PUT', '/api/event-gateway/v1/channels/observed-channel'], ['DELETE', '/api/event-gateway/v1/channels/observed-channel']]) {
    const f = await interception(); await f.dispatch(); await f.operation.result(); await f.dispatch(method, path);
    assert.equal(f.calls.filter(c => c === 'fetch').length, 1); assert.equal(f.operation.attempts(), 2);
    await assert.rejects(f.cleanup(), e => e.errors.some(error => error.message === 'channel_create_unexpected_mutation'));
  }
});

test('unknown forwarding and non-201 acceptance never become a lost-success claim or a retry', async () => {
  for (const options of [{fetchFailure: true}, {status: 409}, {status: 200}]) {
    const f = await interception(options); await f.dispatch(); await assert.rejects(f.operation.result());
    assert.equal(f.calls.filter(c => c === 'fetch').length, 1); await assert.rejects(f.cleanup());
  }
});

test('cleanup joins an in-flight channel create and retains routing and abort failures', async () => {
  let finish; const waitFetch = new Promise(done => {finish = done;});
  const f = await interception({waitFetch, unrouteFailure: true, abortFailure: true}); const pending = f.dispatch();
  while (!f.calls.includes('fetch')) await new Promise(done => setImmediate(done));
  let finished = false; const cleaned = f.cleanup().catch(error => {finished = true; return error;});
  await new Promise(done => setImmediate(done)); assert.equal(finished, false);
  finish(); await pending; await assert.rejects(f.operation.result(), /abort_failed/);
  assert.deepEqual((await cleaned).errors.map(e => e.message), ['unroute_failed', 'abort_failed']);
});

test('cancellation settles without submission and prevents forwarding after custody', async () => {
  const idle = await interception(); idle.controller.abort(Error('cancelled'));
  await assert.rejects(idle.operation.result(), /cancelled/); await idle.cleanup();
  const active = await interception({cancelAtCustody: true}); await active.dispatch();
  await assert.rejects(active.operation.result(), /channel_create_cancelled/);
  assert.ok(!active.calls.includes('fetch')); await assert.rejects(active.cleanup());
});

test('both named creation setups require low-level intent custody before returning a lease', async () => {
  for (const scenario of ['channel_create', 'channel_create_uncertain']) {
    const deferred = [], calls = [], parent = {...lease, local: true, synthetic: true,
      replicas: [{ready: true}, {ready: true}], fillProviderKey: async () => ({}),
      retainChannelIntent: async () => ({}), channelCreationRecords: async () => ({}), close: async () => calls.push('close')};
    const context = {row: {}, defer: (_label, cleanup) => deferred.push(cleanup)};
    for (const method of ['fillProviderKey', 'retainChannelIntent', 'channelCreationRecords']) {
      const incomplete = {...parent}; delete incomplete[method];
      await assert.rejects(parentEgSetup(context, {eg: {acquire: async () => incomplete}}, scenario),
        e => e instanceof Blocked && e.message === 'eg_fixture_method_' + method.toLowerCase());
      await deferred.at(-1)();
    }
    assert.equal(calls.length, 3);
  }
});
