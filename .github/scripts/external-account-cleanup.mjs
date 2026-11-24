import assert from 'node:assert/strict';
import { createHash, createPublicKey, verify } from 'node:crypto';

const keys = (value, expected) => assert.deepEqual(Object.keys(value).sort(), [...expected].sort());
const id = (value) => assert.match(value, /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$/);
const time = (value) => {
  assert.equal(typeof value, 'string');
  const parsed = Date.parse(value);
  assert.ok(Number.isFinite(parsed) && new Date(parsed).toISOString() === value);
  return parsed;
};
const sha256 = (value) => createHash('sha256').update(value).digest('hex');
const lifetime = 15 * 60 * 1000;
export const externalCleanupActions = Object.freeze(['external_account_cleanup_required', 'external_account_cleanup_confirmed']);
const signupFields = ['run', 'requestId', 'fixtureOwner', 'subject'];
const bindingFields = [...signupFields, 'originHash'];
const pendingFields = ['at', 'action', 'schema', ...bindingFields];
const receiptFields = ['schema', 'receiptId', 'issuer', 'keyId', 'run', 'requestId', 'fixtureOwner', 'subject', 'originHash', 'state', 'completedAt', 'issuedAt', 'expiresAt'];

// Fixed field order is the signature wire contract; raw JSON whitespace is not authority.
export function cleanupReceiptBytes(payload) {
  keys(payload, receiptFields);
  return Buffer.from(JSON.stringify(receiptFields.map((field) => [field, payload[field]])));
}

function trustedKeys(authority, now) {
  keys(authority, ['trust', 'origin']);
  const { trust, origin } = authority;
  keys(trust, ['schema', 'originHash', 'fixtureOwner', 'issuers']);
  assert.equal(trust.schema, 'cma-external-cleanup-trust-v1');
  const url = new URL(origin);
  assert.ok(url.protocol === 'https:' && url.origin === origin);
  assert.equal(trust.originHash, sha256(origin));
  id(trust.fixtureOwner);
  assert.ok(Array.isArray(trust.issuers) && trust.issuers.length > 0 && trust.issuers.length <= 16);
  const result = new Map();
  for (const issuer of trust.issuers) {
    keys(issuer, ['issuer', 'keyId', 'publicKey', 'notBefore', 'notAfter']);
    id(issuer.issuer); id(issuer.keyId);
    const key = JSON.stringify([issuer.issuer, issuer.keyId]);
    assert.ok(!result.has(key));
    assert.ok(time(issuer.notBefore) <= now && time(issuer.notAfter) > now);
    assert.ok(typeof issuer.publicKey === 'string' && issuer.publicKey.length <= 4096);
    assert.match(issuer.publicKey, /^-----BEGIN PUBLIC KEY-----\n[A-Za-z0-9+/=\n]+\n-----END PUBLIC KEY-----\n?$/);
    const publicKey = createPublicKey(issuer.publicKey);
    assert.equal(publicKey.asymmetricKeyType, 'ed25519');
    result.set(key, { ...issuer, publicKey });
  }
  return result;
}

/** Pending history remains append-only. Only a scoped, signed terminal receipt resolves it. */
export function validateExternalAccountCleanup(journal, authority, now = Date.now()) {
  const rows = journal.filter((row) => externalCleanupActions.includes(row.action));

  const pending = new Map();
  const scopes = new Map();
  for (const row of rows) {
    if (row.action !== externalCleanupActions[0]) continue;
    keys(row, pendingFields);
    assert.equal(row.schema, 'cma-external-cleanup-request-v1');
    for (const key of ['run', 'requestId', 'fixtureOwner', 'subject']) id(row[key]);
    assert.match(row.originHash, /^[a-f0-9]{64}$/);
    const requestedAt = time(row.at);
    assert.ok(requestedAt <= now + 5000);
    const binding = bindingFields.map((key) => row[key]);
    const scope = JSON.stringify([row.run, row.fixtureOwner, row.subject, row.originHash]);
    assert.ok(!scopes.has(scope) || scopes.get(scope) === row.requestId);
    scopes.set(scope, row.requestId);
    const previous = pending.get(row.requestId);
    if (previous) assert.deepEqual(previous.binding, binding);
    else pending.set(row.requestId, { binding, requestedAt, confirmed: false });
  }
  assert.ok(pending.size > 0, 'A receipt cannot invent a cleanup request');
  assert.ok(authority, 'External cleanup remains pending without trusted receipt authority');
  const issuers = trustedKeys(authority, now);
  const seenReceipts = new Map();
  const seenPending = new Set();
  for (const row of rows) {
    if (row.action === externalCleanupActions[0]) {
      assert.equal(row.originHash, authority.trust.originHash);
      assert.equal(row.fixtureOwner, authority.trust.fixtureOwner);
      seenPending.add(row.requestId);
      continue;
    }
    keys(row, ['run', 'at', 'action', 'receipt']);
    keys(row.receipt, ['payload', 'signature']);
    const { payload, signature } = row.receipt;
    const bytes = cleanupReceiptBytes(payload);
    assert.equal(payload.schema, 'cma-external-account-cleanup-v1');
    for (const key of ['receiptId', 'issuer', 'keyId', 'run', 'requestId', 'fixtureOwner', 'subject']) id(payload[key]);
    assert.equal(row.run, payload.run);
    assert.ok(seenPending.has(payload.requestId), 'Confirmation must follow the recorded request');
    const request = pending.get(payload.requestId);
    assert.deepEqual(bindingFields.map((key) => payload[key]), request.binding);
    assert.equal(payload.state, 'deleted');
    const completed = time(payload.completedAt), issued = time(payload.issuedAt), expires = time(payload.expiresAt);
    assert.ok(completed >= request.requestedAt && completed <= issued);
    assert.ok(issued <= now + 5000 && now - issued <= lifetime);
    assert.ok(expires > now && expires > issued && expires - issued <= lifetime);
    assert.ok(time(row.at) >= issued && time(row.at) <= now + 5000);
    const issuer = issuers.get(JSON.stringify([payload.issuer, payload.keyId]));
    assert.ok(issuer, 'Untrusted receipt issuer');
    assert.ok(issued >= time(issuer.notBefore) && issued < time(issuer.notAfter));
    assert.match(signature, /^[A-Za-z0-9_-]{86}$/);
    assert.ok(verify(null, bytes, issuer.publicKey, Buffer.from(signature, 'base64url')), 'Invalid terminal receipt signature');
    const receiptIdentity = JSON.stringify([payload.issuer, payload.receiptId]);
    const fingerprint = sha256(bytes);
    assert.ok(!seenReceipts.has(receiptIdentity) || seenReceipts.get(receiptIdentity) === fingerprint, 'Conflicting receipt reuse');
    seenReceipts.set(receiptIdentity, fingerprint);
    request.confirmed = true;
  }
  assert.ok([...pending.values()].every((request) => request.confirmed), 'External account cleanup remains pending');
  return pending;
}

export function validateSignupCleanup(signup, journal, authority, fixtureOwner) {
  keys(signup, signupFields);
  for (const key of signupFields) id(signup[key]);
  id(fixtureOwner);
  assert.equal(signup.fixtureOwner, fixtureOwner);
  const pending = validateExternalAccountCleanup(journal, authority);
  const request = pending.get(signup.requestId);
  assert.ok(request?.confirmed, 'J0 needs a matching terminal cleanup receipt');
  const binding = { ...signup, originHash: authority.trust.originHash };
  assert.deepEqual(request.binding, bindingFields.map((key) => binding[key]));
}
