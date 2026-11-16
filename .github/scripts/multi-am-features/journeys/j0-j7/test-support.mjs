// Synthetic unit-test data only. Never imported by the live invocation.
import {generateKeyPairSync, sign} from 'node:crypto';
import {canonical, digest} from './provenance.mjs';
import {cookieDigest} from './browser-proof.mjs';

export const source = '1'.repeat(40), image = 'cma-api@sha256:' + '2'.repeat(64);
export const binding = {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true,
  coordination_enabled: true, production_acceptance: false, production_writes: false,
  api_source: source, api_image: image, kind_context: 'kind-unit-test', namespace: 'unit',
  am_origins: {a: 'http://127.0.0.1:18881', b: 'http://127.0.0.1:18882'}, web_origin: 'http://127.0.0.1:18880'};
export const buildBytes = Buffer.from('{"revision":"unit-test"}');
export const release = {source, image, kind_context: binding.kind_context, namespace: binding.namespace,
  artifact_sha256: '3'.repeat(64), web: {source: '4'.repeat(40), manifest: 'sha256:' + '5'.repeat(64),
    build_info_sha256: digest(buildBytes)}};
export const topology = () => ({kind_context: binding.kind_context, namespace: binding.namespace,
  shared_database: true, coordination_enabled: true,
  ams: Object.fromEntries(['a', 'b'].map(am => [am, {source, image, ready: true,
    origin: binding.am_origins[am], pod_uid: 'pod-' + am, instance_id: 'process-' + am}]))});
export const cookies = [{name: 'session', value: 'synthetic-unit-cookie', domain: '127.0.0.1',
  path: '/', httpOnly: true, secure: false}];
export const subject = 'synthetic-unit-subject';
export function signer() {
  const {privateKey, publicKey} = generateKeyPairSync('ed25519');
  const trust = {key_id: 'unit-capture', public_key_pem: publicKey.export({type: 'spki', format: 'pem'}),
    environment: 'nonproduction', issuer: 'https://nyxid.unit.invalid',
    response_schema: Object.fromEntries(['registration', 'verification'].map(role => [role,
      {subject: '/subject', registration_id: '/registration_id', created_at: '/created_at'}]))};
  return {trust, capture(payload) {
    const encoded = canonical(payload);
    return {key_id: trust.key_id, sha256: digest(encoded), payload,
      signature: sign(null, Buffer.from(encoded), privateKey).toString('base64')};
  }};
}
export const providerBody = time => JSON.stringify({subject, registration_id: 'unit-registration', created_at: new Date(time).toISOString()});
export function payload(challenge, time = Date.now()) {
  const created_at = new Date(time).toISOString(), subject_sha256 = digest(subject);
  const cookie_jar_sha256 = cookieDigest(cookies);
  const session = {authentication: 'browser_session', subject_sha256, cookie_jar_sha256,
    origin: binding.web_origin, callback_sha256: digest('synthetic-callback')};
  session.binding_sha256 = digest(canonical({subject_sha256, cookie_jar_sha256, origin: session.origin}));
  return {schema: 'mfunc01-nyxid-registration-v2', challenge,
    registration: {issuer: 'https://nyxid.unit.invalid', method: 'POST', status: 201,
      subject_sha256, registration_id: 'unit-registration', request_id: 'unit-register-request',
      created_at, response_sha256: digest(providerBody(time))},
    verification: {issuer: 'https://nyxid.unit.invalid', method: 'GET', status: 200,
      subject_sha256, registration_id: 'unit-registration', request_id: 'unit-independent-read',
      created_at, observed_at: created_at, response_sha256: digest(providerBody(time))}, session};
}
export function browserSession({authSubject = subject, sessionCookies = cookies} = {}) {
  return {origin: binding.web_origin, close: async () => {}, page: {
    context: () => ({cookies: async () => sessionCookies}),
    request: {get: async url => url.endsWith('/auth/session')
      ? {status: () => 200, json: async () => ({authenticated: true, subject: authSubject})}
      : {status: () => 200, body: async () => buildBytes}},
  }};
}
export const browserEvidence = () => ({schema: 'multi-am-browser-evidence-v1', real_ui: true,
  authentication: 'browser_session', secrets_redacted: true, compiled_source: release.web.source,
  compiled_manifest: release.web.manifest, requests: [{am: 'a', request_id: 'browser-unit-read', method: 'GET'}]});
export function harness() {
  const signed = signer(), counts = {registration: 0, request: 0, cleanup: 0, browser: 0};
  const events = [], cleanups = [];
  let captured; const captures = new Map();
  const environment = {phase: 'before_restart', iteration: 1, am: 'a', source, image,
    campaignId: 'SYNTHETIC-CONTRACT-TEST', caseBudgetMs: 1000, cleanupBudgetMs: 1000,
    record: async event => events.push(event),
    adapter: {topology: async () => topology()},
    transport: {request: async () => {counts.request++; throw Error('unit_request_unexpected');}},
    fixtures: {
      'fresh-identity': {register: async ({challenge}) => {
        counts.registration++; const time = Date.now(); captured = signed.capture(payload(challenge, time));
        captures.set(digest(providerBody(time)), providerBody(time));
        captures.set(digest('synthetic-callback'), 'synthetic-callback');
        return {actor: 'new-user'};
      }, cleanup: async value => {counts.cleanup++; cleanups.push(value);}},
      'registration-evidence': {collect: async () => captured, readCapture: async ({sha256}) => captures.get(sha256)},
      browser: {open: async () => {counts.browser++; return browserSession();}, evidence: async () => browserEvidence()},
      'runtime-controls': {}, 'lifecycle-evidence': {},
    }};
  return {binding: structuredClone(binding), release: structuredClone(release), trust: signed.trust,
    environment, counts, events, cleanups, signed};
}
