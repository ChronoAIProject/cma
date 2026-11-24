import assert from 'node:assert/strict';
import { createHash, generateKeyPairSync } from 'node:crypto';
import { chmodSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { cleanupReceiptBytes, validateExternalAccountCleanup } from './external-account-cleanup.mjs';
import { externalCleanupFixture } from './external-account-cleanup.fixture.mjs';
import { campaignFixture } from './public-journey-evidence.fixture.mjs';
import { loadCleanupAuthority, requiredCases, signupCase, validateCampaign } from './staging-authority.mjs';

const validate = (fixture, rows = [fixture.pending, fixture.confirmed]) => validateExternalAccountCleanup(rows, fixture.authority, fixture.now);

test('pending history and identical receipts are retained with deterministic idempotent confirmation', () => {
  const f = externalCleanupFixture();
  const rows = [f.pending, { ...f.pending, at: f.stamp(-500) }, f.confirmed, f.confirmed, { ...f.pending, at: f.stamp(0) }];
  const original = structuredClone(rows);
  validate(f, rows);
  assert.deepEqual(rows, original);
  const reordered = Object.fromEntries(Object.entries(f.payload).reverse());
  assert.deepEqual(cleanupReceiptBytes(reordered), cleanupReceiptBytes(f.payload));
});

test('complete case results require both J0 evidence and its signed terminal cleanup', () => {
  const f = externalCleanupFixture();
  const { results, journal, fixtureOwner } = campaignFixture(requiredCases, f);
  const check = (rows = journal) => validateCampaign(results, rows, f.authority, fixtureOwner);
  check();
  assert.throws(() => check(journal.slice(0, -2)));
  assert.throws(() => check(journal.slice(0, -1)));
  assert.throws(() => check(journal.slice(1)));
  assert.throws(() => validateCampaign(results, journal, undefined, fixtureOwner));
  assert.throws(() => validateCampaign(results, journal, f.authority, 'different-fixture'));
  delete results.cases.find((row) => row.case === signupCase).signupCleanup;
  assert.throws(() => check());
});

for (const key of ['run', 'requestId', 'subject']) {
  test(`a valid terminal receipt for a different signup ${key} does not resolve J0`, () => {
    const f = externalCleanupFixture();
    const { results, journal, fixtureOwner } = campaignFixture(requiredCases, f);
    const pending = { ...f.pending, [key]: 'different-value' };
    const confirmed = { ...f.confirmed, run: pending.run, receipt: f.signed({ ...f.payload, [key]: pending[key] }) };
    validate(f, [pending, confirmed]);
    assert.throws(() => validateCampaign(results, [...journal.slice(0, -2), pending, confirmed], f.authority, fixtureOwner));
  });
}

for (const [name, change] of [
  ['other run', (p) => { p.run = 'other-run'; }],
  ['other subject', (p) => { p.subject = 'other-subject'; }],
  ['other fixture', (p) => { p.fixtureOwner = 'other-fixture'; }],
  ['other origin', (p) => { p.originHash = 'a'.repeat(64); }],
  ['other request', (p) => { p.requestId = 'other-request'; }],
  ['unknown issuer', (p) => { p.issuer = 'untrusted'; }],
  ['unknown key', (p) => { p.keyId = 'untrusted'; }],
  ['delete merely accepted', (p) => { p.state = 'accepted'; }],
  ['cleanup failed', (p) => { p.state = 'failed'; }],
  ['expired receipt', (p, f) => { p.expiresAt = f.stamp(-1); }],
  ['stale issue time', (p, f) => { p.issuedAt = f.stamp(-1000000); }],
  ['future issue time', (p, f) => { p.issuedAt = f.stamp(6000); }],
  ['excessive lifetime', (p, f) => { p.expiresAt = f.stamp(1000000); }],
  ['before signup request', (p, f) => { p.completedAt = f.stamp(-4000); }],
  ['completion after issue', (p, f) => { p.completedAt = f.stamp(0); }],
  ['invalid time', (p) => { p.completedAt = 'yesterday'; }],
]) test(`a valid signature cannot authorize ${name}`, () => {
  const f = externalCleanupFixture();
  change(f.payload, f);
  f.confirmed.receipt = f.signed(f.payload);
  assert.throws(() => validate(f));
});

for (const [name, change] of [
  ['signature tampering', (f) => { f.confirmed.receipt.signature = 'A'.repeat(86); }],
  ['body tampering', (f) => { f.confirmed.receipt.payload.receiptId = 'changed'; }],
  ['rogue signer', (f) => { f.confirmed.receipt = f.signed(f.payload, generateKeyPairSync('ed25519').privateKey); }],
  ['arbitrary completion flag', (f) => { f.confirmed.receipt = { completed: true }; }],
  ['extra diagnostic', (f) => { f.confirmed.receipt.payload.rawLog = 'unreviewed'; }],
  ['legacy pending record', (f) => { delete f.pending.schema; }],
  ['empty subject', (f) => { f.pending.subject = ''; }],
  ['different row run', (f) => { f.confirmed.run = 'other-run'; }],
  ['receipt row predates issue', (f) => { f.confirmed.at = f.stamp(-2000); }],
  ['future pending row', (f) => { f.pending.at = f.stamp(10000); }],
  ['missing issuer', (f) => { f.authority.trust.issuers = []; }],
  ['duplicate issuer', (f) => { f.authority.trust.issuers.push(f.authority.trust.issuers[0]); }],
  ['expired signer', (f) => { f.authority.trust.issuers[0].notAfter = f.stamp(-1); }],
  ['future signer', (f) => { f.authority.trust.issuers[0].notBefore = f.stamp(10000); }],
  ['another trusted target', (f) => { f.authority.origin = 'https://other.example.test'; }],
  ['another trusted fixture', (f) => { f.authority.trust.fixtureOwner = 'other-fixture'; }],
  ['private key in trust', (f) => { f.authority.trust.issuers[0].publicKey = generateKeyPairSync('ed25519').privateKey.export({ type: 'pkcs8', format: 'pem' }); }],
]) test(`cleanup rejects ${name}`, () => {
  const f = externalCleanupFixture(); change(f); assert.throws(() => validate(f));
});

test('orphan confirmations, reversed order, cross-run replays and conflicting duplicates fail', () => {
  const f = externalCleanupFixture();
  for (const rows of [[f.confirmed], [f.confirmed, f.pending], [f.pending], [f.pending, { ...f.pending, subject: 'other-subject' }, f.confirmed], [f.pending, { ...f.pending, requestId: 'other-request' }, f.confirmed]])
    assert.throws(() => validate(f, rows));
  const replay = { ...f.pending, run: 'next-run' };
  assert.throws(() => validate(f, [replay, { ...f.confirmed, run: replay.run }]));
  const conflicting = { ...f.confirmed, receipt: f.signed({ ...f.payload, completedAt: f.stamp(-1500) }) };
  assert.throws(() => validate(f, [f.pending, f.confirmed, conflicting]));
});

test('trust input is optional, pinned to bytes, private, outside the checkout, and never a receipt-carried key', () => {
  const f = externalCleanupFixture();
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'cma-cleanup-trust-')));
  const path = join(directory, 'trust.json');
  const bytes = JSON.stringify(f.authority.trust);
  writeFileSync(path, bytes, { mode: 0o600 });
  const env = { CMA_E2E_CLEANUP_TRUST_FILE: path, CMA_E2E_CLEANUP_TRUST_SHA256: createHash('sha256').update(bytes).digest('hex') };
  try {
    assert.equal(loadCleanupAuthority(f.authority.origin, {}), undefined);
    assert.deepEqual(loadCleanupAuthority(f.authority.origin, env), f.authority);
    assert.throws(() => loadCleanupAuthority(f.authority.origin, { CMA_E2E_CLEANUP_TRUST_FILE: path }));
    assert.throws(() => loadCleanupAuthority(f.authority.origin, { CMA_E2E_CLEANUP_TRUST_SHA256: env.CMA_E2E_CLEANUP_TRUST_SHA256 }));
    writeFileSync(path, bytes + ' ');
    assert.throws(() => loadCleanupAuthority(f.authority.origin, env));
    writeFileSync(path, bytes); chmodSync(path, 0o640);
    assert.throws(() => loadCleanupAuthority(f.authority.origin, env));
    chmodSync(path, 0o600); symlinkSync(path, join(directory, 'alias.json'));
    assert.throws(() => loadCleanupAuthority(f.authority.origin, { ...env, CMA_E2E_CLEANUP_TRUST_FILE: join(directory, 'alias.json') }));
  } finally { rmSync(directory, { recursive: true }); }
});
