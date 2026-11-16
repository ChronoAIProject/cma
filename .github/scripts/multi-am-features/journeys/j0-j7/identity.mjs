import {randomUUID} from 'node:crypto';
import {contextFor} from './context.mjs';
import {need, SHA256, digest, canonical, verifyCapture} from './provenance.mjs';

export {digest};
const nonempty = value => typeof value === 'string' && value.trim().length > 0;
const timestamp = value => typeof value === 'string' ? Date.parse(value) : NaN;

export function validateRegistrationCapture(envelope, trust, challenge, startedAt, now = Date.now()) {
  const receipt = verifyCapture(envelope, trust, 'mfunc01-nyxid-registration-v2', challenge);
  const {registration: r, verification: v, session: s} = receipt;
  need(trust.environment === 'nonproduction' && typeof trust.issuer === 'string',
    'nyxid_nonproduction_trust_required');
  let issuer;
  try { issuer = new URL(trust.issuer); } catch { need(false, 'nyxid_issuer_invalid'); }
  need(issuer.protocol === 'https:' && !issuer.username && !issuer.password,
    'nyxid_issuer_invalid');
  need(r?.issuer === trust.issuer && v?.issuer === trust.issuer, 'nyxid_issuer_mismatch');
  need(r.method === 'POST' && [200, 201].includes(r.status) && nonempty(r.registration_id)
    && nonempty(r.request_id) && SHA256.test(r.subject_sha256 ?? '')
    && SHA256.test(r.response_sha256 ?? ''), 'nyxid_registration_receipt_incomplete');
  need(v.method === 'GET' && v.status === 200 && nonempty(v.request_id)
    && v.request_id !== r.request_id && v.registration_id === r.registration_id
    && v.subject_sha256 === r.subject_sha256 && v.created_at === r.created_at
    && SHA256.test(v.response_sha256 ?? ''), 'nyxid_independent_read_mismatch');
  const created = timestamp(r.created_at), observed = timestamp(v.observed_at);
  // A registration from before this invocation cannot be relabelled as fresh.
  need(Number.isFinite(created) && created >= startedAt && created <= now
    && observed >= created && observed <= now, 'nyxid_registration_not_fresh');
  need(s?.authentication === 'browser_session' && s.subject_sha256 === r.subject_sha256
    && SHA256.test(s.cookie_jar_sha256 ?? '') && SHA256.test(s.callback_sha256 ?? '')
    && nonempty(s.origin), 'nyxid_cookie_binding_missing');
  const binding = digest(canonical({subject_sha256: s.subject_sha256,
    cookie_jar_sha256: s.cookie_jar_sha256, origin: s.origin}));
  need(s.binding_sha256 === binding, 'nyxid_session_binding_mismatch');
  return receipt;
}

function pointer(document, path) {
  need(typeof path === 'string' && path.startsWith('/') && path.length < 200,
    'nyxid_response_mapping_missing');
  return path.slice(1).split('/').map(part => part.replaceAll('~1', '/').replaceAll('~0', '~'))
    .reduce((value, key) => value && Object.hasOwn(value, key) ? value[key] : undefined, document);
}

export async function verifyProviderArtifacts(witness, receipt, trust, signal) {
  need(typeof witness?.readCapture === 'function' && trust.response_schema,
    'nyxid_provider_capture_reader_missing');
  for (const role of ['registration', 'verification']) {
    const claim = receipt[role];
    const bytes = await witness.readCapture({sha256: claim.response_sha256, signal});
    need((typeof bytes === 'string' || Buffer.isBuffer(bytes)) && Buffer.byteLength(bytes) <= 1024 * 1024
      && digest(bytes) === claim.response_sha256, 'nyxid_provider_artifact_hash_mismatch');
    let body;
    try { body = JSON.parse(bytes.toString()); } catch { need(false, 'nyxid_provider_artifact_not_json'); }
    const mapping = trust.response_schema[role];
    const subject = pointer(body, mapping?.subject), registration = pointer(body, mapping?.registration_id);
    need(typeof subject === 'string' && digest(subject) === claim.subject_sha256
      && registration === claim.registration_id && pointer(body, mapping?.created_at) === claim.created_at,
    'nyxid_provider_artifact_claim_mismatch');
  }
  const callback = await witness.readCapture({sha256: receipt.session.callback_sha256, signal});
  need((typeof callback === 'string' || Buffer.isBuffer(callback)) && Buffer.byteLength(callback) <= 1024 * 1024
    && digest(callback) === receipt.session.callback_sha256, 'nyxid_callback_capture_hash_mismatch');
}

export async function registerFreshIdentity(ctx) {
  const {trust, binding} = contextFor(ctx);
  const registrar = ctx.fixture('fresh-identity'), witness = ctx.fixture('registration-evidence');
  need(typeof registrar?.register === 'function' && typeof registrar?.cleanup === 'function',
    'fresh_identity_registration_unavailable');
  need(witness !== registrar && typeof witness?.collect === 'function',
    'independent_registration_witness_missing');
  need(trust?.key_id && trust.public_key_pem && trust.environment === 'nonproduction',
    'independent_capture_trust_missing');
  const challenge = randomUUID(), startedAt = Date.now();
  // Cleanup custody precedes registration, including unknown outcome/timeouts.
  ctx.defer('fresh_identity_cleanup', async () => {
    const unresolved = contextFor(ctx).resourcesReleased === false;
    await registrar.cleanup({challenge, mode: unresolved ? 'revoke_session_retain_account' : 'delete_account'});
    need(!unresolved, 'identity_retained_for_resource_reconciliation');
  });
  const identity = await registrar.register({challenge, purpose: 'M-FUNC-01',
    issuer: trust.issuer, local_kind: true, signal: ctx.signal});
  need(/^[a-z][a-z0-9_-]{0,63}$/.test(identity?.actor ?? ''), 'fresh_identity_actor_invalid');
  const envelope = await witness.collect({challenge, signal: ctx.signal});
  const verified = validateRegistrationCapture(envelope, trust, challenge, startedAt);
  await verifyProviderArtifacts(witness, verified, trust, ctx.signal);
  need(verified.session.origin === binding.web_origin, 'nyxid_session_origin_mismatch');
  await ctx.observe('fresh_identity_registration', async () => ({
    schema: 'mfunc01-registration-proof-v2', capture: structuredClone(envelope), receipt_sha256: envelope.sha256,
    capture_key_id: envelope.key_id, registration: verified.registration,
    verification: verified.verification, session: verified.session,
  }));
  return Object.freeze({actor: identity.actor, registration: Object.freeze(verified.registration),
    session: Object.freeze(verified.session), receipt_sha256: envelope.sha256});
}
