import {randomBytes, randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {digest, segment} from './helpers.mjs';
import {validEgReceipt} from '../../evidence-scope.mjs';
import {validateNodeTestFixture} from './channel-tests.mjs';
import {validateInventoryFixture} from './channel-inventory.mjs';
import {validateRelayFixture} from './relay-verification.mjs';
import {validateJourneyFixture} from './relay-journey.mjs';
import {validateNativeTelegramFixture} from './native-telegram.mjs';
import {validateNativeWhatsappFixture} from './native-whatsapp.mjs';

const check = (value, code) => {if (!value) throw Error(code);};
const required = (value, code) => {if (!value) throw new Blocked(code); return value;};
const ownOrigin = value => {
  const url = new URL(value);
  check(['http:', 'https:'].includes(url.protocol) && ['127.0.0.1', '[::1]'].includes(url.hostname)
    && !url.username && !url.password && url.pathname === '/' && !url.search && !url.hash,
  'eg_parent_loopback_origin_required');
  return url.origin;
};

// No process creation, credential discovery or inferred replica identity. The
// parent supplies its existing direct forwards and independent ingress witness.
export async function createEgTransport(parent, record = async () => {}) {
  required(parent?.scope === 'LOCAL_DISPOSABLE_TWO_AM' && parent.synthetic === true
    && parent.shared_database === true, 'eg_parent_local_lease_required');
  for (const name of ['topology', 'authorize', 'witness']) required(typeof parent[name] === 'function',
    'eg_parent_' + name + '_required');
  required(/@sha256:[a-f0-9]{64}$/.test(parent.expectedImage ?? '')
    && /^[a-f0-9]{40}$/.test(parent.expectedSource ?? ''), 'eg_parent_image_binding_required');
  const topology = await parent.topology();
  const replicas = ['a', 'b'].map(slot => {
    const member = topology?.[slot];
    required(member?.ready === true && member.image === parent.expectedImage
      && member.source === parent.expectedSource && typeof member.instance === 'string'
      && member.instance.length > 0, 'eg_parent_two_ready_replicas_required');
    return {...member, slot, origin: ownOrigin(member.origin)};
  });
  check(replicas[0].origin !== replicas[1].origin && replicas[0].instance !== replicas[1].instance,
    'eg_parent_distinct_replicas_required');
  const snapshot = async replica => {
    const value = (await parent.topology())?.[replica];
    required(value?.ready === true && value.draining !== true && value.source === parent.expectedSource
      && value.image === parent.expectedImage && ['pod_uid', 'instance_id', 'container_id'].every(key =>
        typeof value[key] === 'string' && value[key].length > 0), 'eg_parent_serving_process_required');
    return {schema: 'c275-eg-serving-v1', scope: 'eg', replica, origin: ownOrigin(value.origin),
      pod_uid: value.pod_uid, instance_id: value.instance_id, container_id: value.container_id,
      ready: true, draining: value.draining === true, source: value.source, image: value.image,
      observed_at: new Date().toISOString()};
  };
  return {replicas: replicas.map(({origin, ...rest}) => rest), async request({method = 'GET', path,
    body, key, actor = 'owner', replica = 'a', allowed = [200], timeoutMs = 30000}) {
    check(['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD'].includes(method), 'eg_http_method');
    check(['a', 'b'].includes(replica) && typeof path === 'string' && /^\/(?:v1\/|mcp$)/.test(path)
      && !path.includes('\\') && !path.includes('#'), 'eg_http_path');
    const member = replicas.find(item => item.slot === replica), url = new URL(path, member.origin);
    check(url.origin === member.origin && (url.pathname.startsWith('/v1/') || url.pathname === '/mcp'), 'eg_http_origin');
    check(Number.isSafeInteger(timeoutMs) && timeoutMs > 0 && timeoutMs <= 125000, 'eg_http_timeout_bound');
    const headers = new Headers(await parent.authorize({actor, method, path, replica}));
    if (body !== undefined) headers.set('content-type', 'application/json');
    if (key !== undefined) {
      check(typeof key === 'string' && /^[\x21-\x7e]{1,256}$/.test(key), 'eg_http_key');
      headers.set('Idempotency-Key', key);
    }
    const serving = await snapshot(replica);
    check(serving.origin === member.origin && serving.instance_id === member.instance, 'eg_http_serving_changed');
    const started = performance.now(), startedAt = new Date().toISOString();
    let response, parsed, errorCode, witness, egReceipt;
    try {
      response = await fetch(url, {method, headers, redirect: 'manual',
        body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(timeoutMs)});
      check(response.status < 300 || response.status >= 400, 'eg_http_redirect_refused');
      const chunks = []; let bytes = 0;
      if (response.body) for await (const chunk of response.body) {
        bytes += chunk.length; check(bytes <= 2 * 1024 * 1024, 'eg_http_body_bound'); chunks.push(chunk);
      }
      const text = Buffer.concat(chunks).toString('utf8');
      if (text) { try {parsed = JSON.parse(text);} catch {throw Error('eg_http_json_required');} }
      const endedAt = new Date().toISOString();
      witness = await parent.witness({replica, method, path, status: response.status,
        responseRequestId: response.headers.get('x-request-id')});
      check(witness?.instance === member.instance && typeof witness.requestId === 'string'
        && witness.requestId.trim().length > 0 && witness.slot === replica, 'eg_http_actual_ingress_required');
      if (response.headers.has('x-request-id')) check(witness.requestId === response.headers.get('x-request-id'),
        'eg_http_request_id_mismatch');
      egReceipt = {scope: 'eg', replica, request_id: witness.requestId, method, status: response.status,
        started_at: startedAt, ended_at: endedAt, serving, serving_after: await snapshot(replica),
        attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
      check(validEgReceipt(egReceipt), 'eg_http_serving_window_unproved');
      check(allowed.includes(response.status), 'eg_http_status_' + response.status);
      return {status: response.status, body: parsed ?? null, instance: witness.instance,
        requestId: witness.requestId, elapsedMs: performance.now() - started, eg_receipt: egReceipt,
        retryAfterSeconds: /^\d+$/.test(response.headers.get('retry-after') ?? '')
          ? Number(response.headers.get('retry-after')) : null};
    } catch (error) {
      // Never expose a credential-bearing body or a library error with its URL.
      errorCode = /^eg_[a-z0-9_]+$/.test(error?.message ?? '') ? error.message : 'eg_http_unconfirmed';
      const safe = Error(errorCode);
      safe.outcome = ['GET', 'HEAD'].includes(method) ? 'unavailable' : 'unknown';
      throw safe;
    } finally {
      await record({schema: 'multi-eg-http-evidence-v1', scope: 'eg', method, replica,
        path_sha256: digest(url.pathname), status: response?.status ?? null,
        request_id: witness?.requestId ?? null,
        instance_sha256: witness?.instance ? digest(witness.instance) : null,
        body_sha256: digest(parsed ?? null), elapsed_ms: performance.now() - started,
        eg_receipt: egReceipt ?? null,
        error_code: errorCode ?? null});
    }
  }};
}

export async function channelEditSetup(ctx, primitives) {
  const parent = required(primitives.eg, 'eg_parent_primitives_required');
  const transport = await createEgTransport(parent, value => ctx.observe('eg_setup_http', async () => value));
  required(typeof parent.leaseAuthority === 'function', 'eg_parent_authority_lease_required');
  const authority = await parent.leaseAuthority({caseId: ctx.row.case_id, phase: ctx.phase, iteration: ctx.iteration});
  check(typeof authority?.close === 'function', 'eg_authority_cleanup_required');
  let released = false;
  const release = async () => {if (!released) {await authority.close(); released = true;}};
  ctx.defer('release_eg_authority', release);
  required(authority.local === true && authority.synthetic === true && authority.agentKey
    && typeof authority.profile?.metadata?.owner?.subject === 'string'
    && authority.profile.metadata.owner.subject.length > 0 && authority.provider?.slug,
  'eg_authority_seed_required');
  const recordId = randomUUID(), channelName = `EG channel edit ${recordId.slice(0, 8)}`;
  const secret = randomBytes(32).toString('base64'), keyId = 'synthetic-hmac';
  const body = {schema_version: 1, record_id: recordId, name: channelName,
    profile: authority.profile, provider: authority.provider,
    credentials: {agent_key: authority.agentKey, channel_key: randomBytes(32).toString('hex'),
      hmac_secrets: [{key_id: keyId, secret}]},
    sources: [{type: 'custom', auth: {type: 'hmac', key_id: keyId}}],
    partition: {version: 1, key: 'conversation_and_sender'},
    reply: {auto_final: true, ack_after_ms: 3000, sinks: [{type: 'source'}]}};
  let cleanupAttempt;
  // Reserve custody before the first POST, including an accepted response that
  // is lost or fails attribution. Cleanup recovers this record; it never creates.
  const cleanup = () => cleanupAttempt ??= (async () => {
    const recovered = await transport.request({path: `/v1/channel-records/${segment(recordId)}`,
      replica: ctx.am === 'a' ? 'b' : 'a', allowed: [200, 404]});
    if (recovered.status === 200) {
      check(recovered.body.record_id === recordId && typeof recovered.body.channel_id === 'string'
        && Number.isSafeInteger(recovered.body.version) && recovered.body.version > 0,
      'eg_cleanup_record_identity');
      await transport.request({method: 'DELETE', path: `/v1/channels/${segment(recovered.body.channel_id)}`
        + `?expected_version=${recovered.body.version}`, replica: ctx.am, allowed: [204, 404]});
      const final = await transport.request({path: `/v1/channel-records/${segment(recordId)}`,
        replica: ctx.am === 'a' ? 'b' : 'a', allowed: [404]});
      check(final.status === 404, 'eg_cleanup_record_retained');
    }
  })();
  ctx.defer('cleanup_reserved_eg_record', cleanup);
  const created = (await transport.request({method: 'POST', path: '/v1/channels', body,
    key: recordId, replica: ctx.am, allowed: [201]})).body;
  check(created.record_id === recordId && created.definition?.creation?.ready === true
    && created.definition.name === channelName && created.version > 0, 'eg_setup_creation_identity');
  const {credentials, record_id, ...updateBody} = body;
  return {local: true, synthetic: true, replicas: transport.replicas, channelId: created.channel_id,
    channelName, recordId, expected: {ownerSubject: authority.profile.metadata.owner.subject}, updateBody, request: transport.request,
    cleanup, close: cleanup};
}

const scenarioRequirements = Object.freeze({
  native_whatsapp: {fields: ['recordId', 'channelId', 'expected'],
    methods: ['request', 'platformConfiguration', 'webhookChallenge', 'platformSend', 'callbackCapture', 'nativeEventCapture',
      'waitForTurn', 'providerContext', 'waitForDelivery', 'captureEffects', 'counters', 'drain']},
  native_telegram: {fields: ['recordId', 'channelId', 'expected'],
    methods: ['request', 'platformConfiguration', 'platformSend', 'callbackCapture', 'nativeEventCapture',
      'waitForTurn', 'providerContext', 'waitForDelivery', 'counters', 'drain']},
  ...Object.fromEntries(['telegram', 'discord', 'whatsapp', 'lark', 'feishu', 'slack'].map(platform => ['relay_' + platform,
    {fields: ['recordId', 'channelId', 'expected'], methods: ['request', 'platformSend', 'callbackCapture',
      'waitForTurn', 'providerContext', 'waitForDelivery', 'counters', 'drain']} ])),
  channel_inventory: {fields: ['expected'],
    methods: ['request', 'counters', 'quiesce', 'scanCredentialValues', 'scanBrowserForCredentialValues']},
  channel_capabilities: {fields: ['channelId', 'channelName', 'expected'], methods: ['request', 'counters']},
  channel_create: {fields: ['draft', 'expected'], methods: ['fillProviderKey', 'retainChannelIntent', 'channelCreationRecords']},
  channel_create_uncertain: {fields: ['draft', 'expected'], methods: ['fillProviderKey', 'retainChannelIntent', 'channelCreationRecords']},
  producer_edit: {fields: ['channelId', 'channelName', 'updateBody', 'expected'], methods: ['request']},
  channel_delete: {fields: ['channelId', 'channelName', 'baseline', 'expected'], methods: ['request', 'counters', 'platformSend', 'quiesce']},
  channel_admission: {fields: ['recordId', 'channelId', 'identities', 'expected'],
    methods: ['request', 'platformSend', 'counters', 'waitForTurn', 'providerContext', 'waitForDelivery',
      'captureEffects', 'releaseCompletion', 'quiesce', 'drain']},
  consumer_profile: {fields: ['draft', 'expected', 'settings', 'existingAgentId'],
    methods: ['fillProviderKey', 'retainChannelIntent', 'channelCreationRecords', 'bindingSnapshot', 'scanBrowserForCredentialValues']},
  multi_consumer_routing: {fields: ['channelId', 'routingBody', 'expected'], methods: ['request', 'platformSend', 'counters']},
  channel_tests: {fields: ['recordId', 'channelId', 'channelName', 'expected'],
    methods: ['request', 'counters', 'nodeTestCaptures', 'quiesce', 'drain']},
  channel_activity: {fields: ['recordId', 'channelId', 'expected'],
    methods: ['request', 'releaseCompletion', 'drain', 'providerContext']},
  relay_verification: {fields: ['recordId', 'channelId', 'expected'],
    methods: ['request', 'platformSend', 'replayEvent', 'callbackCapture', 'waitForTurn',
      'releaseCompletion', 'waitForDelivery', 'counters', 'quiesce', 'drain']},
  outbound_prepare: {fields: ['channelId', 'outboundBody', 'keys', 'expected'], methods: ['request', 'captureEffects']},
  outbound_send: {fields: ['channelId', 'outboundBody', 'keys', 'expected'], methods: ['request', 'dropOutboundSendResponse', 'outboundStatus', 'captureEffects', 'drain']},
  managed_connection_inventory: {fields: ['draft', 'expected'], methods: ['request', 'fillProviderKey',
    'retainChannelIntent', 'channelCreationRecords']},
  connection_browser_setup: {fields: ['recordId', 'event', 'expected', 'keys'], methods: ['request', 'submitConnectionBrowserForm', 'connectionStatus']},
  connection_cancel: {fields: ['recordId', 'event', 'expected', 'keys'], methods: ['request', 'openCancelledConnection', 'connectionRecords']},
  consumer_inventory: {fields: ['botId', 'expected'], methods: []},
  consumer_create: {fields: ['wizard', 'expected'], methods: ['request', 'retainCreationIntent', 'creationRecords']},
  bot_father_profile: {fields: ['draft', 'reviewed', 'connection', 'expected'],
    methods: ['retainProfileIntent', 'profileRecords', 'retainCreationIntent', 'creationRecords']},
  bot_father_actions: {fields: ['event', 'expected', 'keys'], methods: ['botSetupAdvance', 'botSetupAction']},
  console_session: {fields: ['recordId', 'channelName', 'expected'], methods: ['request']},
  console_inspection: {fields: ['recordId', 'channelId', 'channelName', 'expected'], methods: ['request']},
  console_source_lifecycle: {fields: ['recordId', 'channelId', 'channelName', 'expected'], methods: ['request']},
});

function requireLeaseShape(lease, scenario) {
  required(lease?.local === true && lease.synthetic === true, 'eg_parent_fixture_not_local_synthetic');
  required(Array.isArray(lease.replicas) && lease.replicas.length === 2
    && lease.replicas.every(replica => replica.ready === true), 'eg_parent_fixture_replicas_required');
  const specification = scenarioRequirements[scenario];
  required(specification, 'eg_parent_fixture_scenario_unknown');
  for (const field of specification.fields)
    required(lease[field] !== undefined && lease[field] !== null, 'eg_fixture_field_' + field);
  for (const method of specification.methods)
    required(typeof lease[method] === 'function', 'eg_fixture_method_' + method.toLowerCase());
  return lease;
}

// Each named recipe calls the parent's real EG fixture factory, then validates
// the concrete custody and operation surface needed by its case. It never
// performs the feature action and cannot return a case verdict.
export async function parentEgSetup(ctx, primitives, scenario) {
  const parent = required(primitives?.eg, 'eg_parent_primitives_required');
  required(typeof parent.acquire === 'function', 'eg_parent_acquire_required');
  const lease = await parent.acquire({scenario, caseId: ctx.row.case_id,
    phase: ctx.phase, iteration: ctx.iteration, am: ctx.am});
  const release = lease?.cleanup ?? lease?.close;
  if (typeof release !== 'function') throw new Blocked('eg_setup_cleanup_required');
  let cleanupAttempt;
  // Reuse the pending or rejected promise: duplicate cleanup callers must not
  // replay disposal or turn a failed release into a successful no-op.
  const cleanup = () => cleanupAttempt ??= Promise.resolve().then(() => release.call(lease));
  ctx.defer('cleanup_' + scenario.replaceAll('-', '_'), cleanup);
  const shaped = requireLeaseShape(lease, scenario);
  if (scenario === 'managed_connection_inventory') {
    const expected = shaped.expected;
    for (const field of ['ownerSubject', 'connectionRef', 'connectionLabel', 'connectionAdmission',
      'boundConnectionRef', 'boundRecordId', 'notReadyConnectionRef', 'foreignConnectionRef', 'routeId']) {
      required(expected?.[field] !== undefined && expected?.[field] !== null,
        'eg_managed_connection_' + field + '_required');
    }
    required(expected.connectionRef !== expected.boundConnectionRef
      && expected.connectionRef !== expected.notReadyConnectionRef
      && expected.connectionRef !== expected.foreignConnectionRef,
    'eg_managed_connection_refs_must_be_distinct');
  }
  if (scenario === 'channel_inventory') validateInventoryFixture(shaped);
  if (scenario === 'native_telegram') validateNativeTelegramFixture(shaped);
  if (scenario === 'native_whatsapp') validateNativeWhatsappFixture(shaped);
  if (scenario === 'channel_tests') validateNodeTestFixture(shaped);
  if (scenario === 'relay_verification') validateRelayFixture(shaped);
  else if (scenario.startsWith('relay_')) validateJourneyFixture(shaped, scenario.slice(6),
    ['relay_telegram', 'relay_discord', 'relay_whatsapp'].includes(scenario));
  if (scenario === 'channel_delete') {
    const expected = shaped.expected;
    required(typeof expected?.ownerSubject === 'string' && expected.ownerSubject.length > 0,
      'eg_channel_delete_ownerSubject_required');
    required(typeof expected?.unrelatedChannelId === 'string' && expected.unrelatedChannelId.length > 0,
      'eg_channel_delete_unrelatedChannelId_required');
    required(/^[a-f0-9]{64}$/.test(expected?.unrelatedChannelDigest ?? ''),
      'eg_channel_delete_unrelatedChannelDigest_required');
    required(expected.unrelatedChannelId !== shaped.channelId,
      'eg_channel_delete_unrelated_channel_required');
    for (const key of ['turns', 'effects', 'deliveries']) {
      required(Number.isSafeInteger(shaped.baseline?.[key]) && shaped.baseline[key] >= 0,
        'eg_channel_delete_baseline_' + key + '_required');
    }
  }
  return {...shaped, cleanup, close: cleanup};
}

export const setups = {
  channel_edit: channelEditSetup,
  ...Object.fromEntries(Object.keys(scenarioRequirements).map(scenario => [scenario,
    (ctx, primitives) => parentEgSetup(ctx, primitives, scenario)])),
};
