import {createCipheriv, createHash, randomBytes, randomUUID} from 'node:crypto';
import {Blocked} from '../../runner.mjs';
import {createEgTransport} from './setup.mjs';
import {digest, segment} from './helpers.mjs';

const validId = value => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(value);
const secret = value => typeof value === 'string' && /^[!-~]{16,512}$/.test(value);
const requireValue = (condition, code) => {if (!condition) throw new Blocked(code);};
const timestamp = () => Math.floor(Date.now() / 1000);

// Native Lark uses the Encrypt Key's SHA256 as its AES key, with a fresh IV
// prepended to the PKCS7 ciphertext. The signature covers the exact outer JSON
// bytes, not the decrypted event and not a NyxID relay envelope.
function encrypt(payload, key) {
  const iv = randomBytes(16), cipher = createCipheriv('aes-256-cbc', key, iv);
  const bytes = Buffer.from(JSON.stringify(payload));
  try {
    const encrypted = Buffer.concat([iv, cipher.update(bytes), cipher.final()]);
    const body = {encrypt: encrypted.toString('base64')};
    requireValue(Buffer.byteLength(JSON.stringify(body)) <= 256 * 1024, 'native_lark_webhook_body_bound');
    return body;
  } finally {bytes.fill(0);}
}

/**
 * Bind only the parent's existing two-EG loopback forwards. This is an actual
 * encrypted HTTP sender, not an independent ingress observer or a case verdict.
 * Credentials are supplied from already-owned synthetic custody, never read.
 */
export async function createNativeLarkWebhook(parent, configuration, record = async () => {}) {
  requireValue(configuration?.local === true && configuration.synthetic === true,
    'native_lark_synthetic_credential_custody_required');
  const {identity, channelId, credentials} = configuration;
  requireValue(validId(channelId) && identity && ['lark', 'feishu'].includes(identity.region)
    && validId(identity.app_id) && identity.app_id.startsWith('cli_')
    && validId(identity.tenant_key) && validId(identity.bot_open_id) && identity.bot_open_id.startsWith('ou_'),
  'native_lark_exact_native_identity_required');
  requireValue(secret(credentials?.encrypt_key) && secret(credentials?.verification_token),
    'native_lark_synthetic_webhook_credentials_required');
  // Topology/source/image validation happens before any event or challenge can
  // be sent. Creating this binding starts no listener, browser or application.
  const initial = await createEgTransport(parent);
  const sourceIdentity = Object.freeze({region: identity.region, app_id: identity.app_id,
    tenant_key: identity.tenant_key, bot_open_id: identity.bot_open_id});
  const path = '/v1/native-lark/' + segment(channelId);
  let encryptKey = credentials.encrypt_key, verificationToken = credentials.verification_token;
  const aesKey = createHash('sha256').update(encryptKey).digest();
  const active = new Set(), controllers = new Set();
  let closed = false, closing;

  const send = (kind, input = {}) => {
    const operation = (async () => {
      requireValue(!closed, 'native_lark_webhook_sender_closed');
      const {replica = 'a', variant = 'valid', signal, timeoutMs = 15000, allowed = [200]} = input;
      requireValue(['a', 'b'].includes(replica), 'native_lark_webhook_replica_required');
      requireValue((kind === 'challenge' ? ['valid', 'invalid_token']
        : ['valid', 'invalid_token', 'invalid_signature', 'wrong_app', 'wrong_tenant', 'stale_timestamp']).includes(variant),
      'native_lark_webhook_variant_required');
      requireValue(!signal?.aborted, 'native_lark_webhook_cancelled_before_dispatch');
      let payload, safePayload;
      const token = variant === 'invalid_token' ? randomBytes(32).toString('hex') : verificationToken;
      if (kind === 'challenge') {
        requireValue(typeof input.challenge === 'string' && input.challenge.length > 0 && input.challenge.length <= 4096
          && !/[\u0000-\u001f\u007f-\u009f]/.test(input.challenge), 'native_lark_webhook_challenge_required');
        safePayload = {type: 'url_verification', challenge: input.challenge};
        payload = {...safePayload, token};
      } else {
        requireValue(validId(input.eventId) && typeof input.eventType === 'string' && /^[a-z][a-z0-9_.]{1,127}$/.test(input.eventType)
          && input.event && typeof input.event === 'object' && !Array.isArray(input.event), 'native_lark_webhook_event_required');
        let event; try {event = structuredClone(input.event);} catch {throw new Blocked('native_lark_webhook_event_required');}
        safePayload = {schema: '2.0', header: {event_id: input.eventId, event_type: input.eventType,
          app_id: variant === 'wrong_app' ? 'cli_foreign_' + randomUUID().replaceAll('-', '') : sourceIdentity.app_id,
          tenant_key: variant === 'wrong_tenant' ? 'tenant_foreign_' + randomUUID().replaceAll('-', '') : sourceIdentity.tenant_key}, event};
        payload = {...safePayload, header: {...safePayload.header, token}};
      }
      let body;
      try {body = encrypt(payload, aesKey);} catch (error) {
        if (error instanceof Blocked) throw error;
        throw new Blocked('native_lark_webhook_payload_not_serializable');
      }
      const bytes = Buffer.from(JSON.stringify(body)), headers = {};
      const sentTimestamp = String(timestamp() - (variant === 'stale_timestamp' ? 301 : 0)), nonce = randomUUID();
      if (kind === 'event') {
        let signature = createHash('sha256').update(sentTimestamp).update(nonce).update(encryptKey).update(bytes).digest('hex');
        if (variant === 'invalid_signature') signature = (signature[0] === '0' ? '1' : '0') + signature.slice(1);
        Object.assign(headers, {'x-lark-request-timestamp': sentTimestamp, 'x-lark-request-nonce': nonce, 'x-lark-signature': signature});
      }
      const proof = {schema: 'multi-eg-native-lark-sender-v1', provenance: 'sender', kind, variant,
        path_sha256: digest(path), body_sha256: digest(bytes), payload_sha256: digest(safePayload),
        encrypt_key_sha256: digest(encryptKey), verification_token_sha256: digest(token),
        signature_sha256: kind === 'event' ? digest(headers['x-lark-signature']) : null,
        nonce_sha256: kind === 'event' ? digest(nonce) : null, timestamp: kind === 'event' ? sentTimestamp : null};
      const controller = new AbortController(); controllers.add(controller);
      let response, errorCode;
      try {
        // Each invocation gets its own immutable header closure. Parallel a/b
        // challenges cannot cross credentials, signatures or callback paths.
        const transport = await createEgTransport({...parent, authorize: async request => {
          requireValue(request.actor === 'native_lark_platform' && request.method === 'POST'
            && request.path === path && request.replica === replica, 'native_lark_callback_authority_scope');
          return headers;
        }}, record);
        requireValue(transport.replicas.every(actual => {
          const before = initial.replicas.find(r => r.slot === actual.slot);
          return ['instance', 'instance_id', 'pod_uid', 'container_id', 'source', 'image'].every(k => before?.[k] === actual[k]);
        }), 'native_lark_webhook_topology_changed');
        response = await transport.request({actor: 'native_lark_platform', method: 'POST', path,
          body, replica, allowed, timeoutMs, signal: signal ? AbortSignal.any([signal, controller.signal]) : controller.signal});
        return {...response, sent: proof};
      } catch (error) {
        errorCode = /^(?:eg|native_lark)_[a-z0-9_]+$/.test(error?.message ?? '') ? error.message : 'native_lark_webhook_unknown';
        const safe = Error(errorCode); safe.outcome = error?.outcome ?? 'unknown'; throw safe;
      } finally {
        controllers.delete(controller);
        await record({...proof, replica, status: response?.status ?? null,
          request_id: response?.requestId ?? null, error_code: errorCode ?? null});
      }
    })();
    active.add(operation); operation.then(() => active.delete(operation), () => active.delete(operation));
    return operation;
  };
  return {identity: sourceIdentity, channelId, replicas: structuredClone(initial.replicas),
    sendEvent: input => send('event', input), sendChallenge: input => send('challenge', input),
    close() {
      if (closing) return closing;
      closed = true;
      for (const controller of controllers) controller.abort();
      return closing = (async () => {
        await Promise.allSettled([...active]); aesKey.fill(0); encryptKey = null; verificationToken = null;
      })();
    }};
}
