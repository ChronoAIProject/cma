import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import { cleanupReceiptBytes } from './external-account-cleanup.mjs';

export function externalCleanupFixture() {
  const now = Date.now();
  const stamp = (offset) => new Date(now + offset).toISOString();
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const origin = 'https://staging.example.test';
  const originHash = createHash('sha256').update(origin).digest('hex');
  const pending = {
    run: 'synthetic-signup-run', at: stamp(-3000), action: 'external_account_cleanup_required',
    schema: 'cma-external-cleanup-request-v1', requestId: 'synthetic-cleanup-request',
    fixtureOwner: 'fixture-owner', subject: 'fresh-subject', originHash,
  };
  const payload = {
    schema: 'cma-external-account-cleanup-v1', receiptId: 'synthetic-terminal-receipt',
    issuer: 'fixture-identity-authority', keyId: 'synthetic-key', run: pending.run,
    requestId: pending.requestId, fixtureOwner: pending.fixtureOwner, subject: pending.subject,
    originHash, state: 'deleted', completedAt: stamp(-2000), issuedAt: stamp(-1000), expiresAt: stamp(60000),
  };
  const signed = (body = payload, key = privateKey) => ({ payload: body, signature: sign(null, cleanupReceiptBytes(body), key).toString('base64url') });
  const confirmed = { run: pending.run, at: stamp(0), action: 'external_account_cleanup_confirmed', receipt: signed() };
  const authority = { origin, trust: {
    schema: 'cma-external-cleanup-trust-v1', originHash, fixtureOwner: pending.fixtureOwner,
    issuers: [{ issuer: payload.issuer, keyId: payload.keyId, publicKey: publicKey.export({ type: 'spki', format: 'pem' }), notBefore: stamp(-86400000), notAfter: stamp(86400000) }],
  } };
  return { now, stamp, pending, payload, signed, confirmed, authority };
}
