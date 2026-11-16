import {randomBytes, randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {digest, segment} from './helpers.mjs';

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
    const started = performance.now();
    let response, parsed, errorCode, witness;
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
      witness = await parent.witness({replica, method, path, status: response.status,
        responseRequestId: response.headers.get('x-request-id')});
      check(witness?.instance === member.instance && typeof witness.requestId === 'string'
        && witness.requestId.trim().length > 0 && witness.slot === replica, 'eg_http_actual_ingress_required');
      if (response.headers.has('x-request-id')) check(witness.requestId === response.headers.get('x-request-id'),
        'eg_http_request_id_mismatch');
      check(allowed.includes(response.status), 'eg_http_status_' + response.status);
      return {status: response.status, body: parsed ?? null, instance: witness.instance,
        requestId: witness.requestId, elapsedMs: performance.now() - started};
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
    && authority.profile?.metadata?.owner && authority.provider?.slug, 'eg_authority_seed_required');
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
    channelName, recordId, expected: {}, updateBody, request: transport.request,
    cleanup, close: cleanup};
}

export const setups = {channel_edit: channelEditSetup};
